// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod integrations;
mod mcp;
mod storage;

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

use base64::Engine;
use serde::Serialize;
use storage::{Config, Crop, Meta};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, RunEvent, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::UpdaterExt;

const REPO_URL: &str = "https://github.com/cweeperde/scribble2fix";

/// The monitor a capture was taken on, in physical pixels.
#[derive(Clone, Copy)]
struct Area {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
}

/// What we know about the screen at the moment the hotkey was pressed.
#[derive(Clone, Default)]
struct Source {
    unix_ms: i64,
    app_name: String,
    window_title: String,
    monitor: storage::MonitorInfo,
}

#[derive(Default)]
struct AppState {
    /// True while the overlay is up (or being prepared). Ignores repeated hotkeys.
    active: AtomicBool,
    /// Raw RGBA of the frozen screen, handed to the overlay once.
    pixels: Mutex<Option<Vec<u8>>>,
    source: Mutex<Source>,
    area: Mutex<Option<Area>>,
    config: Mutex<Config>,
    /// Kept alive for the app's lifetime: on Linux the clipboard content
    /// disappears when its owner is dropped.
    clipboard: Mutex<Option<arboard::Clipboard>>,
    toast_generation: AtomicU64,
    /// Bumped whenever the request card is shown or hidden, so a stale
    /// watcher thread knows to stop.
    request_generation: AtomicU64,
    /// The hotkey is released because a fullscreen app is in front.
    hotkey_paused: AtomicBool,
    /// The windows on screen when it froze, front to back.
    windows: Mutex<Vec<ScreenWindow>>,
    /// Bumped with every freeze, so an older desktop watcher stops.
    freeze_generation: AtomicU64,
    /// The desktop that's frozen (`desktop_marker`).
    frozen_desktop: Mutex<Option<DesktopMarker>>,
}

/// A window on screen, in physical pixels like `Area`.
#[derive(Clone)]
struct ScreenWindow {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    app_name: String,
    title: String,
}

/// The windows on screen (not ours), front to back.
fn screen_windows(area: &Area) -> Vec<ScreenWindow> {
    #[cfg(target_os = "macos")]
    {
        // Points; on the captured monitor physical pixels are points × scale.
        let k = area.scale;
        macos::app_windows()
            .into_iter()
            .map(|w| ScreenWindow {
                x: w.x * k,
                y: w.y * k,
                width: w.width * k,
                height: w.height * k,
                ..w
            })
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = area;
        let own = std::process::id();
        xcap::Window::all()
            .unwrap_or_default()
            .into_iter()
            .filter(|w| w.pid().is_ok_and(|p| p != own) && !w.is_minimized().unwrap_or(false))
            .filter_map(|w| {
                Some(ScreenWindow {
                    x: w.x().ok()? as f64,
                    y: w.y().ok()? as f64,
                    width: w.width().ok()? as f64,
                    height: w.height().ok()? as f64,
                    app_name: w.app_name().unwrap_or_default(),
                    title: w.title().unwrap_or_default(),
                })
            })
            .collect()
    }
}

/// "Super+Shift+Digit2" as "⌘ + Shift + 2" (Mac) or "Win + Shift + 2", like
/// prettyHotkey in common.js.
fn pretty_hotkey(hotkey: &str) -> String {
    let win = if cfg!(target_os = "macos") { "⌘" } else { "Win" };
    hotkey
        .split('+')
        .map(|p| {
            p.trim_start_matches("Key")
                .trim_start_matches("Digit")
                .replace("Super", win)
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn config(app: &AppHandle) -> Config {
    app.state::<AppState>().config.lock().unwrap().clone()
}

/// Changes the config and saves it.
fn update_config(app: &AppHandle, change: impl FnOnce(&mut Config)) -> Result<Config, String> {
    let state = app.state::<AppState>();
    let mut config = state.config.lock().unwrap();
    change(&mut config);
    storage::save_config(&config).map_err(err)?;
    Ok(config.clone())
}

// ---------------------------------------------------------------- macOS

/// The top of a screen: the camera notch (in CSS px from the screen's top
/// left corner) and how far the toolbar has to move down to stay clear of it.
#[derive(Serialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
struct ScreenTop {
    safe_top: f64,
    notch: Option<Notch>,
}

#[derive(Serialize, Clone, Copy)]
struct Notch {
    left: f64,
    right: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{Notch, ScreenTop, ScreenWindow};
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSApplicationActivationOptions, NSPopUpMenuWindowLevel, NSRunningApplication, NSScreen, NSWindow,
        NSWindowAnimationBehavior, NSWindowCollectionBehavior,
    };
    use objc2_app_kit::{NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification};
    use objc2_core_foundation::{CFDictionary, CFNumber, CFNumberType, CFString, CGRect};
    use objc2_core_graphics::{CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption};
    use objc2_foundation::NSNotification;
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
    use tauri::{AppHandle, Manager, WebviewWindow};

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    static ASKED: AtomicBool = AtomicBool::new(false);

    /// Without the Screen Recording permission macOS still returns a
    /// screenshot, but with every window removed (only wallpaper and menu
    /// bar). If it looks missing, ask for it and open the settings page, but
    /// only once per run: the check can stay negative until the app restarts,
    /// and the capture itself is still attempted. Returns false only the
    /// first time access looks missing in this run.
    pub fn check_screen_access() -> bool {
        if unsafe { CGPreflightScreenCaptureAccess() } || ASKED.swap(true, Ordering::SeqCst) {
            return true;
        }
        // Adds Scribble2Fix to the list in System Settings (and prompts once).
        unsafe { CGRequestScreenCaptureAccess() };
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
            .spawn();
        false
    }

    /// Runs `f` on the main thread (AppKit requires it) and returns its result.
    fn on_main<R: Send + 'static>(app: &AppHandle, f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
        if MainThreadMarker::new().is_some() {
            return Some(f());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(f());
        })
        .ok()?;
        rx.recv().ok()
    }

    fn with_ns_window(window: &WebviewWindow, f: impl FnOnce(&NSWindow) + Send + 'static) {
        let Ok(ptr) = window.ns_window() else { return };
        let ptr = ptr as usize;
        on_main(window.app_handle(), move || {
            if let Some(w) = unsafe { (ptr as *const NSWindow).as_ref() } {
                f(w)
            }
        });
    }

    /// Shows the window on every desktop (Space) and over fullscreen apps, so
    /// it's on the one the user is looking at and stays there when they switch.
    pub fn join_all_spaces(window: &WebviewWindow) {
        with_ns_window(window, |w| {
            w.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::IgnoresCycle,
            )
        });
    }

    /// Bundle id of the app in front.
    pub fn front_app_id() -> Option<String> {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()?
            .bundleIdentifier()
            .map(|id| id.to_string())
    }

    pub fn set_alpha(window: &WebviewWindow, alpha: f64) {
        with_ns_window(window, move |w| w.setAlphaValue(alpha));
    }

    pub fn no_animations(window: &WebviewWindow) {
        with_ns_window(window, |w| w.setAnimationBehavior(NSWindowAnimationBehavior::None));
    }

    /// "Always on top" windows still stay below the menu bar and the Dock.
    /// The frozen screen and the frame have to cover them too.
    pub fn above_menu_bar(window: &WebviewWindow) {
        with_ns_window(window, |w| w.setLevel(NSPopUpMenuWindowLevel));
    }

    /// Notch and safe area of the screen at `x`, `width` (points), from the
    /// areas macOS reports left and right of the camera housing.
    pub fn screen_top(app: &AppHandle, x: f64, width: f64) -> ScreenTop {
        on_main(app, move || {
            let mtm = MainThreadMarker::new()?;
            let screen = NSScreen::screens(mtm).iter().find(|s| {
                let f = s.frame();
                (f.origin.x - x).abs() < 1.0 && (f.size.width - width).abs() < 1.0
            })?;
            let frame = screen.frame();
            let safe_top = screen.safeAreaInsets().top;
            let (l, r) = (screen.auxiliaryTopLeftArea(), screen.auxiliaryTopRightArea());
            let notch = (safe_top > 0.0 && l.size.width > 0.0 && r.size.width > 0.0).then(|| Notch {
                left: l.origin.x + l.size.width - frame.origin.x,
                right: r.origin.x - frame.origin.x,
                height: safe_top,
            });
            Some(ScreenTop { safe_top, notch })
        })
        .flatten()
        .unwrap_or_default()
    }

    /// Normal app windows (layer 0, not ours) from front to back, in points.
    pub fn app_windows() -> Vec<ScreenWindow> {
        let Some(list) = CGWindowListCopyWindowInfo(
            CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
            0,
        ) else {
            return Vec::new();
        };
        let own = std::process::id() as i32;
        (0..list.count())
            .filter_map(|i| unsafe {
                let info = (list.value_at_index(i) as *const CFDictionary).as_ref()?;
                let get = |key: &str| {
                    let key = CFString::from_str(key);
                    let value = info.value((&*key as *const CFString).cast());
                    (!value.is_null()).then_some(value)
                };
                let number = |key: &str| {
                    let n = &*(get(key)? as *const CFNumber);
                    let mut v = 0i32;
                    n.value(CFNumberType::IntType, &mut v as *mut i32 as *mut c_void)
                        .then_some(v)
                };
                let string = |key: &str| {
                    get(key)
                        .map(|v| (*(v as *const CFString)).to_string())
                        .unwrap_or_default()
                };
                if number("kCGWindowLayer")? != 0 || number("kCGWindowOwnerPID")? == own {
                    return None;
                }
                let mut r = CGRect::default();
                let bounds = &*(get("kCGWindowBounds")? as *const CFDictionary);
                if !CGRectMakeWithDictionaryRepresentation(Some(bounds), &mut r) {
                    return None;
                }
                Some(ScreenWindow {
                    x: r.origin.x,
                    y: r.origin.y,
                    width: r.size.width,
                    height: r.size.height,
                    app_name: string("kCGWindowOwnerName"),
                    title: string("kCGWindowName"),
                })
            })
            .collect()
    }

    static SPACE_CHANGES: AtomicU64 = AtomicU64::new(0);

    /// Counts Space switches (macOS tells every app), also to and from
    /// Spaces without windows. Call once, on the main thread.
    pub fn watch_spaces() {
        let block = RcBlock::new(|_: NonNull<NSNotification>| {
            SPACE_CHANGES.fetch_add(1, Ordering::SeqCst);
        });
        let observer = unsafe {
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .addObserverForName_object_queue_usingBlock(
                    Some(NSWorkspaceActiveSpaceDidChangeNotification),
                    None,
                    None,
                    &block,
                )
        };
        // Observes for the app's lifetime.
        std::mem::forget(observer);
    }

    pub fn space_changes() -> u64 {
        SPACE_CHANGES.load(Ordering::SeqCst)
    }

    static FRONT_PID: AtomicI32 = AtomicI32::new(0);

    /// The app the user was in when the screen froze.
    pub fn remember_front_app(pid: u64) {
        let pid = i32::try_from(pid).unwrap_or(0);
        let own = std::process::id() as i32;
        FRONT_PID.store(if pid == own { 0 } else { pid }, Ordering::SeqCst);
    }

    /// Hands the focus back to that app when the overlay closes. Otherwise
    /// Scribble2Fix stays the active app: ⌘V wouldn't paste where the user
    /// was, and the next capture would name Scribble2Fix as the app.
    pub fn restore_front_app() {
        let pid = FRONT_PID.swap(0, Ordering::SeqCst);
        if pid == 0 {
            return;
        }
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            #[allow(deprecated)]
            let _ = app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod macos {
    use super::ScreenTop;
    use tauri::{AppHandle, WebviewWindow};

    pub fn check_screen_access() -> bool {
        true
    }
    pub fn join_all_spaces(_window: &WebviewWindow) {}
    pub fn above_menu_bar(_window: &WebviewWindow) {}
    pub fn screen_top(_app: &AppHandle, _x: f64, _width: f64) -> ScreenTop {
        ScreenTop::default()
    }
    pub fn remember_front_app(_pid: u64) {}
    pub fn restore_front_app() {}
    pub fn watch_spaces() {}
}

// ---------------------------------------------------------------- capture

fn start_capture(app: &AppHandle) {
    if app.state::<AppState>().active.swap(true, Ordering::SeqCst) {
        return;
    }
    if !macos::check_screen_access() {
        show_toast(app, "Allow Screen Recording for Scribble2Fix, then restart it");
    }
    if storage::read_request().is_some() {
        hide_request(app);
    }
    let app = app.clone();
    thread::spawn(move || {
        if let Err(e) = prepare(&app, target_monitor(&app)).and_then(|_| capture(&app)) {
            app.state::<AppState>().active.store(false, Ordering::SeqCst);
            show_toast(&app, &format!("Capture failed: {e}"));
        }
    });
}

/// Remembers the app in front (to name it and to hand the focus back) and
/// the monitor to capture.
fn prepare(app: &AppHandle, monitor: Option<tauri::Monitor>) -> Result<Area, String> {
    let (app_name, window_title) = match active_win_pos_rs::get_active_window() {
        Ok(w) => {
            macos::remember_front_app(w.process_id);
            (w.app_name, w.title)
        }
        Err(_) => Default::default(),
    };
    {
        let state = app.state::<AppState>();
        let mut source = state.source.lock().unwrap();
        source.app_name = app_name;
        source.window_title = window_title;
    }
    Ok(set_area(app, &monitor.ok_or("no monitor found")?))
}

/// Makes `monitor` the one that gets frozen.
fn set_area(app: &AppHandle, monitor: &tauri::Monitor) -> Area {
    let state = app.state::<AppState>();
    let area = Area {
        x: monitor.position().x,
        y: monitor.position().y,
        width: monitor.size().width,
        height: monitor.size().height,
        scale: monitor.scale_factor(),
    };
    *state.area.lock().unwrap() = Some(area);
    state.source.lock().unwrap().monitor = storage::MonitorInfo {
        name: monitor.name().cloned().unwrap_or_default(),
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height,
        scale: area.scale,
    };
    area
}

/// The monitor under the mouse.
fn cursor_monitor(app: &AppHandle) -> Option<tauri::Monitor> {
    let cursor = app.cursor_position().ok()?;
    app.monitor_from_point(cursor.x, cursor.y).ok().flatten()
}

/// The monitor to capture on (Settings → Monitor). Falls back to the one
/// under the mouse when the chosen monitor isn't connected.
fn target_monitor(app: &AppHandle) -> Option<tauri::Monitor> {
    let chosen = match config(app).capture_monitor.as_str() {
        "" => None,
        "primary" => app.primary_monitor().ok().flatten(),
        name => app
            .available_monitors()
            .unwrap_or_default()
            .into_iter()
            .find(|m| m.name().is_some_and(|n| n == name)),
    };
    chosen
        .or_else(|| cursor_monitor(app))
        .or_else(|| app.primary_monitor().ok().flatten())
}

/// Freezes the chosen monitor (`set_area`) and hands the screenshot to the
/// (hidden, pre-warmed) overlay, which shows itself once it's painted.
fn capture(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let unix_ms = chrono::Utc::now().timestamp_millis();
    let area = (*state.area.lock().unwrap()).ok_or("no capture area")?;
    // A point on that monitor, for the fallback lookup in `find_screen`.
    let (px, py) = (
        area.x as f64 + area.width as f64 / 2.0,
        area.y as f64 + area.height as f64 / 2.0,
    );

    let shot = find_screen(&area, px, py)?.capture_image().map_err(err)?;
    let (width, height) = (shot.width(), shot.height());

    *state.pixels.lock().unwrap() = Some(shot.into_raw());
    // For naming the app the marks end up on. Not needed until the user is
    // done, so it doesn't hold up the overlay.
    state.windows.lock().unwrap().clear();
    let handle = app.clone();
    thread::spawn(move || {
        *handle.state::<AppState>().windows.lock().unwrap() = screen_windows(&area);
    });
    state.source.lock().unwrap().unix_ms = unix_ms;

    let overlay = app.get_webview_window("overlay").ok_or("overlay missing")?;
    overlay
        .set_position(PhysicalPosition::new(area.x, area.y))
        .map_err(err)?;
    overlay
        .set_size(PhysicalSize::new(area.width, area.height))
        .map_err(err)?;

    #[derive(Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    struct Payload {
        width: u32,
        height: u32,
        /// Shown at the top while an AI tool waits for this scribble.
        request: Option<RequestInfo>,
        /// Only the basic tools in the toolbar.
        simple: bool,
        /// The camera notch (macOS), which the toolbar and frame avoid.
        top: ScreenTop,
    }
    let request = storage::read_request().map(|r| RequestInfo {
        client: r.client,
        reason: r.reason,
    });
    // Picked before the overlay covers the screen.
    let marker = desktop_marker();
    overlay
        .emit(
            "capture",
            Payload {
                width,
                height,
                request,
                simple: config(app).simple_toolbar,
                top: macos::screen_top(app, area.x as f64 / area.scale, area.width as f64 / area.scale),
            },
        )
        .map_err(err)?;
    watch_desktop(app, marker);
    Ok(())
}

/// The frozen overlay shows on every desktop (Windows) or Space (macOS) and
/// keeps the focus when the user switches, so it never notices by itself.
/// Instead the current desktop is checked a few times a second: once it's
/// another one, the overlay is told (`left`), and it freezes the new desktop
/// unless something was drawn. (On Linux the overlay sits on one workspace
/// and loses the focus when the user leaves it, which it notices itself.)
fn watch_desktop(app: &AppHandle, marker: Option<DesktopMarker>) {
    let state = app.state::<AppState>();
    let generation = state.freeze_generation.fetch_add(1, Ordering::SeqCst) + 1;
    *state.frozen_desktop.lock().unwrap() = marker.clone();
    let Some(marker) = marker else { return };
    let app = app.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(250));
        let state = app.state::<AppState>();
        if state.freeze_generation.load(Ordering::SeqCst) != generation || !state.active.load(Ordering::SeqCst) {
            return;
        }
        if desktop_left(&marker) {
            if let Some(overlay) = app.get_webview_window("overlay") {
                let _ = overlay.emit("left", ());
            }
            return;
        }
    });
}

/// Windows: which virtual desktop is current. Explorer keeps its id in the
/// registry (Windows 11 under VirtualDesktops, Windows 10 under the session),
/// also when the desktop has no windows. If that's missing, a window of the
/// desktop stands in for it.
#[cfg(windows)]
#[derive(Clone)]
struct DesktopMarker {
    desktop: Option<Vec<u8>>,
    window: Option<isize>,
}

#[cfg(windows)]
fn current_virtual_desktop() -> Option<Vec<u8>> {
    use windows::core::{w, HSTRING};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY};
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    let mut session = 0u32;
    let _ = unsafe { ProcessIdToSessionId(std::process::id(), &mut session) };
    let keys = [
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\VirtualDesktops".to_string(),
        format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\SessionInfo\{session}\VirtualDesktops"),
    ];
    keys.iter().find_map(|key| {
        let mut id = [0u8; 16];
        let mut size = id.len() as u32;
        let found = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(key.as_str()),
                w!("CurrentVirtualDesktop"),
                RRF_RT_REG_BINARY,
                None,
                Some(id.as_mut_ptr().cast()),
                Some(&mut size),
            )
        }
        .is_ok();
        (found && size == 16).then(|| id.to_vec())
    })
}

#[cfg(windows)]
fn desktop_marker() -> Option<DesktopMarker> {
    use windows::core::{BOOL, GUID};
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible};
    if let Some(desktop) = current_virtual_desktop() {
        return Some(DesktopMarker {
            desktop: Some(desktop),
            window: None,
        });
    }
    unsafe extern "system" fn collect(hwnd: HWND, list: LPARAM) -> BOOL {
        unsafe { (*(list.0 as *mut Vec<HWND>)).push(hwnd) };
        BOOL(1)
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let manager: IVirtualDesktopManager = CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_ALL).ok()?;
        let mut all: Vec<HWND> = Vec::new();
        // Front to back.
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
        let own = std::process::id();
        let window = all.into_iter().find(|&hwnd| {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            pid != own
                && IsWindowVisible(hwnd).as_bool()
                // Windows shown on every desktop have no desktop id.
                && manager.GetWindowDesktopId(hwnd).is_ok_and(|id| id != GUID::zeroed())
                && manager.IsWindowOnCurrentVirtualDesktop(hwnd).is_ok_and(|on| on.as_bool())
        })?;
        Some(DesktopMarker {
            desktop: None,
            window: Some(window.0 as isize),
        })
    }
}

#[cfg(windows)]
fn desktop_left(marker: &DesktopMarker) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;
    if let Some(desktop) = &marker.desktop {
        return current_virtual_desktop().is_some_and(|now| &now != desktop);
    }
    let Some(window) = marker.window else { return false };
    let hwnd = HWND(window as _);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(manager) = CoCreateInstance::<_, IVirtualDesktopManager>(&VirtualDesktopManager, None, CLSCTX_ALL)
        else {
            return false;
        };
        IsWindow(Some(hwnd)).as_bool()
            && manager
                .IsWindowOnCurrentVirtualDesktop(hwnd)
                .is_ok_and(|on| !on.as_bool())
    }
}

/// macOS: how many Space switches there were so far (`macos::watch_spaces`).
#[cfg(target_os = "macos")]
type DesktopMarker = u64;

#[cfg(target_os = "macos")]
fn desktop_marker() -> Option<DesktopMarker> {
    Some(macos::space_changes())
}

#[cfg(target_os = "macos")]
fn desktop_left(marker: &DesktopMarker) -> bool {
    macos::space_changes() != *marker
}

#[cfg(not(any(windows, target_os = "macos")))]
type DesktopMarker = ();

#[cfg(not(any(windows, target_os = "macos")))]
fn desktop_marker() -> Option<DesktopMarker> {
    None
}

#[cfg(not(any(windows, target_os = "macos")))]
fn desktop_left(_marker: &DesktopMarker) -> bool {
    false
}

#[derive(Serialize, Clone)]
struct RequestInfo {
    client: String,
    reason: String,
}

/// Find the xcap screen matching a Tauri monitor. Platforms disagree on
/// whether monitor origins are physical or logical, so accept either.
fn find_screen(area: &Area, cursor_x: f64, cursor_y: f64) -> Result<xcap::Monitor, String> {
    let lx = (area.x as f64 / area.scale).round() as i32;
    let ly = (area.y as f64 / area.scale).round() as i32;
    let found = xcap::Monitor::all()
        .map_err(err)?
        .into_iter()
        .find(|m| matches!((m.x(), m.y()), (Ok(x), Ok(y)) if (x == area.x && y == area.y) || (x == lx && y == ly)));
    match found {
        Some(m) => Ok(m),
        None => xcap::Monitor::from_point(cursor_x as i32, cursor_y as i32).map_err(err),
    }
}

#[tauri::command]
fn frozen_pixels(state: State<AppState>) -> Result<tauri::ipc::Response, String> {
    state
        .pixels
        .lock()
        .unwrap()
        .take()
        .map(tauri::ipc::Response::new)
        .ok_or_else(|| "no capture pending".into())
}

/// Shows the overlay and gives it the focus, but keeps it see-through until
/// the page has drawn the frozen screen (`reveal_overlay`): a window that was
/// hidden comes up blank for a frame or two, which flashes black.
#[tauri::command]
fn show_overlay(window: WebviewWindow) -> Result<(), String> {
    set_window_shown(&window, false);
    move_to_current_desktop(&window);
    // Linux: shown on all workspaces for a moment and then pinned again, so
    // the window manager puts it on the current one instead of switching
    // back to the workspace it was on before.
    #[cfg(target_os = "linux")]
    let _ = window.set_visible_on_all_workspaces(true);
    window.show().map_err(err)?;
    #[cfg(target_os = "linux")]
    {
        let window = window.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let _ = window.set_visible_on_all_workspaces(false);
        });
    }
    if cfg!(target_os = "macos") {
        macos::above_menu_bar(&window);
    } else {
        window.set_always_on_top(true).map_err(err)?;
    }
    window.set_focus().map_err(err)
}

/// The user left the frozen screen before drawing anything: switched to
/// another desktop (Space), app or monitor. Freeze what they look at now
/// instead of keeping the old picture up.
///
/// `desktop`: the desktop watcher saw a switch. Otherwise it's a lost focus,
/// which can also be a screenshot tool (Win+Shift+S, Cmd+Shift+4) taking the
/// focus to capture the frozen screen with its marks: then it stays up and
/// this returns false.
#[tauri::command]
async fn refreeze(app: AppHandle, window: WebviewWindow, desktop: bool) -> Result<bool, String> {
    let state = app.state::<AppState>();
    if !state.active.load(Ordering::SeqCst) {
        return Ok(false);
    }
    let desktop = desktop || state.frozen_desktop.lock().unwrap().as_ref().is_some_and(desktop_left);
    if !desktop {
        let tool = tauri::async_runtime::spawn_blocking(|| {
            thread::sleep(Duration::from_millis(150));
            screenshot_tool_in_front()
        })
        .await
        .unwrap_or(false);
        if tool {
            return Ok(false);
        }
    }
    // Out of the way, so the new screen gets captured. Without handing the
    // focus back: on macOS that would switch back to the old Space.
    let _ = window.set_always_on_top(false);
    window.hide().map_err(err)?;
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        // Let the desktop switch animation finish.
        thread::sleep(Duration::from_millis(450));
        prepare(&handle, target_monitor(&handle)).and_then(|_| capture(&handle))
    })
    .await
    .unwrap_or_else(|e| Err(err(e)));
    if let Err(e) = result {
        app.state::<AppState>().active.store(false, Ordering::SeqCst);
        show_toast(&app, &format!("Capture failed: {e}"));
        return Err(e);
    }
    Ok(true)
}

/// Windows: another app's window in front that covers the whole monitor and
/// stays on top of everything, like the layer of the Snipping Tool, ShareX
/// or Greenshot. The taskbar or a normal window isn't that.
#[cfg(windows)]
fn screenshot_tool_in_front() -> bool {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, GWL_EXSTYLE, WS_EX_TOPMOST,
    };
    unsafe {
        let front = GetForegroundWindow();
        if front.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(front, &mut pid);
        if pid == std::process::id() || GetWindowLongW(front, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST == 0 {
            return false;
        }
        let mut window: RECT = std::mem::zeroed();
        let mut monitor: MONITORINFO = std::mem::zeroed();
        monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetWindowRect(front, &mut window) != 0
            && GetMonitorInfoW(MonitorFromWindow(front, MONITOR_DEFAULTTONEAREST), &mut monitor) != 0
            && window.left <= monitor.rcMonitor.left
            && window.top <= monitor.rcMonitor.top
            && window.right >= monitor.rcMonitor.right
            && window.bottom >= monitor.rcMonitor.bottom
    }
}

/// macOS: its screenshot tool (Cmd+Shift+3/4/5).
#[cfg(target_os = "macos")]
fn screenshot_tool_in_front() -> bool {
    macos::front_app_id().is_some_and(|id| id == "com.apple.screencaptureui" || id == "com.apple.Screenshot")
}

#[cfg(target_os = "linux")]
fn screenshot_tool_in_front() -> bool {
    false
}

#[tauri::command]
fn reveal_overlay(window: WebviewWindow) {
    set_window_shown(&window, true);
}

/// Windows: cloaks the window (shown for the system, not drawn on screen).
/// macOS and Linux: fully transparent.
#[cfg(windows)]
fn set_window_shown(window: &WebviewWindow, shown: bool) {
    use windows::core::BOOL;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
    let Ok(hwnd) = window.hwnd() else { return };
    let cloak = BOOL(!shown as i32);
    let _ = unsafe {
        DwmSetWindowAttribute(
            HWND(hwnd.0 as _),
            DWMWA_CLOAK,
            &cloak as *const BOOL as *const _,
            std::mem::size_of::<BOOL>() as u32,
        )
    };
}

#[cfg(target_os = "macos")]
fn set_window_shown(window: &WebviewWindow, shown: bool) {
    macos::set_alpha(window, if shown { 1.0 } else { 0.0 });
}

#[cfg(target_os = "linux")]
fn set_window_shown(window: &WebviewWindow, shown: bool) {
    use gtk::prelude::WidgetExt;
    let handle = window.clone();
    let _ = window.run_on_main_thread(move || {
        if let Ok(gtk) = handle.gtk_window() {
            gtk.set_opacity(if shown { 1.0 } else { 0.0 });
        }
    });
}

/// The system's own fade and zoom when a window shows or hides: over a frozen
/// screen it reads as a glitch (the picture shifts against the real screen),
/// and the pages animate themselves anyway.
#[cfg(windows)]
fn no_window_animations(window: &WebviewWindow) {
    use windows::core::BOOL;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
    let Ok(hwnd) = window.hwnd() else { return };
    let off = BOOL(1);
    let _ = unsafe {
        DwmSetWindowAttribute(
            HWND(hwnd.0 as _),
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &off as *const BOOL as *const _,
            std::mem::size_of::<BOOL>() as u32,
        )
    };
}

#[cfg(target_os = "macos")]
fn no_window_animations(window: &WebviewWindow) {
    macos::no_animations(window);
}

#[cfg(target_os = "linux")]
fn no_window_animations(_window: &WebviewWindow) {}

#[tauri::command]
fn hide_overlay(window: WebviewWindow) -> Result<(), String> {
    hide_overlay_window(&window)
}

/// The overlay covers the whole monitor, taskbar included, and stays on top,
/// so Windows treats it like a fullscreen app. Hiding it that way could leave
/// taskbar buttons drawn with the wrong colors until hovered (seen with
/// several monitors at different scaling). Drop "always on top" first, and
/// have the taskbars repaint once it's gone.
fn hide_overlay_window(window: &WebviewWindow) -> Result<(), String> {
    let _ = window.set_always_on_top(false);
    window.hide().map_err(err)?;
    refresh_taskbars();
    macos::restore_front_app();
    Ok(())
}

#[cfg(windows)]
fn refresh_taskbars() {
    thread::spawn(|| {
        use windows_sys::Win32::Graphics::Gdi::{
            RedrawWindow, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RDW_UPDATENOW,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowExW;
        thread::sleep(Duration::from_millis(120));
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        for class in ["Shell_TrayWnd", "Shell_SecondaryTrayWnd"] {
            let class = wide(class);
            let mut hwnd = std::ptr::null_mut();
            loop {
                hwnd = unsafe { FindWindowExW(std::ptr::null_mut(), hwnd, class.as_ptr(), std::ptr::null()) };
                if hwnd.is_null() {
                    break;
                }
                unsafe {
                    RedrawWindow(
                        hwnd,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN | RDW_UPDATENOW,
                    )
                };
            }
        }
    });
}

#[cfg(not(windows))]
fn refresh_taskbars() {}

/// The overlay page (re)loaded. At startup nothing is going on; after an
/// unexpected reload during a capture the frozen screenshot is gone, so hide
/// the then-see-through window and allow a new capture. An open AI request
/// stays open, so the user can simply press the hotkey again.
#[tauri::command]
fn overlay_loaded(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    app.state::<AppState>().active.store(false, Ordering::SeqCst);
    window.hide().map_err(err)
}

#[tauri::command]
fn cancel_capture(app: AppHandle, _window: WebviewWindow) -> Result<(), String> {
    do_cancel(&app);
    Ok(())
}

/// Backs out of a capture: hides the overlay, tells a waiting AI tool the
/// user cancelled and hands the focus back.
fn do_cancel(app: &AppHandle) {
    app.state::<AppState>().active.store(false, Ordering::SeqCst);
    if let Some(request) = storage::answer_request(None) {
        show_toast(app, &format!("Cancelled. {} was told", request.client));
    }
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = hide_overlay_window(&window);
    }
}

/// Receives the cropped images (base64 PNG) from the overlay, saves them for
/// the MCP server and puts the annotated one on the clipboard.
#[tauri::command]
fn finish_capture(
    app: AppHandle,
    window: WebviewWindow,
    annotated: String,
    clean: String,
    crop: Crop,
    image_scale: f64,
    marks: serde_json::Value,
) -> Result<(), String> {
    hide_overlay_window(&window)?;
    let state = app.state::<AppState>();
    let mut source = state.source.lock().unwrap().clone();
    state.active.store(false, Ordering::SeqCst);
    // The app is the topmost window under the middle of the marks. The one
    // in front when the screen froze may be on another monitor.
    if let Some(area) = *state.area.lock().unwrap() {
        let x = area.x as f64 + crop.x as f64 + crop.width as f64 / 2.0;
        let y = area.y as f64 + crop.y as f64 + crop.height as f64 / 2.0;
        let windows = state.windows.lock().unwrap();
        let hit = windows
            .iter()
            .find(|w| x >= w.x && x < w.x + w.width && y >= w.y && y < w.y + w.height);
        if let Some(w) = hit.filter(|w| !w.app_name.is_empty()) {
            source.app_name = w.app_name.clone();
            source.window_title = w.title.clone();
        }
    }

    let app = app.clone();
    thread::spawn(move || {
        let message = match deliver(&app, &annotated, &clean, crop, image_scale, marks, source) {
            Ok(dir) => match dir.and_then(|d| storage::answer_request(Some(d))) {
                Some(request) => format!("Sent to {}, also copied", request.client),
                None if config(&app).copy_path && !config(&app).never_save => "File path copied".to_string(),
                None => paste_hint().to_string(),
            },
            Err(e) => {
                storage::answer_request(None);
                format!("Couldn't copy: {e}")
            }
        };
        show_toast(&app, &message);
    });
    Ok(())
}

/// Unless "never save" is on, saves both images and the metadata. Then puts
/// the annotated image (or with "copy file path", its path) on the clipboard.
/// Returns the capture folder if it was saved.
fn deliver(
    app: &AppHandle,
    annotated: &str,
    clean: &str,
    crop: Crop,
    image_scale: f64,
    marks: serde_json::Value,
    source: Source,
) -> Result<Option<std::path::PathBuf>, String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let annotated = b64.decode(annotated).map_err(err)?;
    let config = config(app);

    let dir = if config.never_save {
        None
    } else {
        let clean = b64.decode(clean).map_err(err)?;
        let captured_at = chrono::DateTime::from_timestamp_millis(source.unix_ms)
            .unwrap_or_default()
            .with_timezone(&chrono::Local)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let meta = Meta {
            app_version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            captured_at,
            captured_at_unix_ms: source.unix_ms,
            drawing_seconds: ((chrono::Utc::now().timestamp_millis() - source.unix_ms).max(0) as f64 / 1000.0).round(),
            app_name: source.app_name,
            window_title: source.window_title,
            monitor: source.monitor,
            crop,
            image_scale,
            request: storage::read_request().map(|r| storage::MetaRequest {
                client: r.client,
                reason: r.reason,
            }),
            marks,
        };
        Some(storage::save_capture(&annotated, &clean, &meta, config.keep_captures).map_err(err)?)
    };

    let state = app.state::<AppState>();
    let mut clipboard = state.clipboard.lock().unwrap();
    if clipboard.is_none() {
        *clipboard = Some(arboard::Clipboard::new().map_err(err)?);
    }
    let clipboard = clipboard.as_mut().unwrap();
    if let (true, Some(dir)) = (config.copy_path, &dir) {
        let path = dir.join("annotated.png");
        clipboard.set_text(path.to_string_lossy().into_owned()).map_err(err)?;
        return Ok(Some(dir.clone()));
    }
    let img = image::load_from_memory(&annotated).map_err(err)?.to_rgba8();
    clipboard
        .set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: img.into_raw().into(),
        })
        .map_err(err)?;
    Ok(dir)
}

fn paste_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "Copied, paste with ⌘V"
    } else {
        "Copied, paste with Ctrl+V"
    }
}

// ---------------------------------------------------------------- toast

const TOAST_MS: u64 = 2600;
/// Wide enough for an AI request; the pill inside is only as wide as its text.
const TOAST_WIDTH: f64 = 720.0;

/// Small non-focusable pill at the bottom of the screen the user worked on.
fn show_toast(app: &AppHandle, text: &str) {
    show_toast_for(app, text, TOAST_MS);
}

fn show_toast_for(app: &AppHandle, text: &str, ms: u64) {
    let Some(toast) = app.get_webview_window("toast") else {
        return;
    };
    let state = app.state::<AppState>();
    let area = (*state.area.lock().unwrap()).or_else(|| {
        app.primary_monitor().ok().flatten().map(|m| Area {
            x: m.position().x,
            y: m.position().y,
            width: m.size().width,
            height: m.size().height,
            scale: m.scale_factor(),
        })
    });
    if let Some(a) = area {
        let w = (TOAST_WIDTH * a.scale) as u32;
        let h = (64.0 * a.scale) as u32;
        let x = a.x + (a.width.saturating_sub(w) / 2) as i32;
        // Above the taskbar / Dock: the bottom of that monitor's work area.
        let bottom = app
            .available_monitors()
            .unwrap_or_default()
            .iter()
            .find(|m| m.position().x == a.x && m.position().y == a.y)
            .map(|m| m.work_area().position.y + m.work_area().size.height as i32)
            .unwrap_or(a.y + a.height as i32);
        let y = bottom - h as i32 - (40.0 * a.scale) as i32;
        let _ = toast.set_position(PhysicalPosition::new(x, y));
        let _ = toast.set_size(PhysicalSize::new(w, h));
    }
    let _ = toast.emit(
        "toast",
        serde_json::json!({ "text": text, "ms": ms.saturating_sub(400) }),
    );
    set_visible_no_activate(&toast, true);

    let generation = state.toast_generation.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(ms));
        if app.state::<AppState>().toast_generation.load(Ordering::SeqCst) == generation {
            set_visible_no_activate(&toast, false);
        }
    });
}

/// The toast, request card and glow must never steal keyboard focus when they
/// appear (the user is typing or about to paste somewhere). On Windows,
/// `show()` activates the window whenever the process is allowed to (e.g.
/// right after launch), so show it without activation.
/// Windows: a window belongs to the virtual desktop it was created on. Shown
/// on another one it stays invisible there, and focusing it switches back to
/// its own desktop. So before showing one, move it to the desktop of the
/// window in front.
#[cfg(windows)]
fn move_to_current_desktop(window: &WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let Ok(hwnd) = window.hwnd() else { return };
    let hwnd = HWND(hwnd.0 as _);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(manager) = CoCreateInstance::<_, IVirtualDesktopManager>(&VirtualDesktopManager, None, CLSCTX_ALL)
        else {
            return;
        };
        if manager
            .IsWindowOnCurrentVirtualDesktop(hwnd)
            .is_ok_and(|on| on.as_bool())
        {
            return;
        }
        if let Ok(desktop) = manager.GetWindowDesktopId(GetForegroundWindow()) {
            let _ = manager.MoveWindowToDesktop(hwnd, &desktop);
        }
    }
}

#[cfg(not(windows))]
fn move_to_current_desktop(_window: &WebviewWindow) {}

#[cfg(windows)]
fn set_visible_no_activate(window: &WebviewWindow, visible: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE};
    if visible {
        move_to_current_desktop(window);
    }
    if let Ok(hwnd) = window.hwnd() {
        unsafe { ShowWindow(hwnd.0 as _, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
    }
}

#[cfg(not(windows))]
fn set_visible_no_activate(window: &WebviewWindow, visible: bool) {
    let _ = if visible { window.show() } else { window.hide() };
}

// ---------------------------------------------------------------- AI requests

/// Request card window size in logical px (the card inside leaves room for
/// its shadow and entrance animation).
const REQUEST_SIZE: (f64, f64) = (520.0, 210.0);

/// Where the request card and the glow frame go on the target monitor, in
/// physical pixels: (glow frame, request card, scale factor). The frame goes
/// around the whole screen on macOS, above the menu bar and around the camera
/// notch (it's click-through). Elsewhere it leaves out the taskbar, so
/// Windows doesn't treat it as a fullscreen window.
fn request_layout(app: &AppHandle) -> Option<(tauri::PhysicalRect<i32, u32>, tauri::PhysicalRect<i32, u32>, f64)> {
    let monitor = target_monitor(app)?;
    // The work area leaves out the taskbar / dock / menu bar.
    let work = *monitor.work_area();
    let frame = if cfg!(target_os = "macos") {
        tauri::PhysicalRect {
            position: *monitor.position(),
            size: *monitor.size(),
        }
    } else {
        work
    };
    let scale = monitor.scale_factor();
    let (w, h) = ((REQUEST_SIZE.0 * scale) as u32, (REQUEST_SIZE.1 * scale) as u32);
    let x = work.position.x + (work.size.width.saturating_sub(w) / 2) as i32;
    let card = tauri::PhysicalRect {
        position: PhysicalPosition::new(x, work.position.y),
        size: PhysicalSize::new(w, h),
    };
    Some((frame, card, scale))
}

/// An AI tool asked for a scribble (`scribble2fix --request`, sent by the MCP
/// server): a card at the top of the screen and a glowing frame around it
/// stay up until the user answers, the request expires or it's answered or
/// cancelled elsewhere.
fn show_request(app: &AppHandle) {
    let Some(request) = storage::read_request() else {
        return;
    };
    let (Some(card), Some(glow)) = (app.get_webview_window("request"), app.get_webview_window("glow")) else {
        return;
    };
    let Some((frame, card_rect, scale)) = request_layout(app) else {
        return;
    };
    let _ = card.set_position(card_rect.position);
    let _ = card.set_size(card_rect.size);
    let _ = glow.set_position(frame.position);
    let _ = glow.set_size(frame.size);
    let top = macos::screen_top(app, frame.position.x as f64 / scale, frame.size.width as f64 / scale);
    let _ = glow.emit("notch", top.notch);

    let config = config(app);
    let _ = card.emit(
        "request",
        serde_json::json!({
            "id": request.id,
            "client": request.client,
            "reason": request.reason,
            "hotkey": config.hotkey,
            "expiresAt": request.created_unix_ms + storage::REQUEST_TTL_MS,
            "sound": config.request_sound,
        }),
    );
    if can_place_windows() {
        let _ = glow.emit("glow", true);
        set_visible_no_activate(&glow, true);
    }
    set_visible_no_activate(&card, true);
    let _ = card.set_always_on_top(true);

    // Close the card once the request is gone (answered, expired, or the AI
    // tool gave up), unless it was shown again or hidden in the meantime.
    let state = app.state::<AppState>();
    let generation = state.request_generation.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(500));
        let state = app.state::<AppState>();
        if state.request_generation.load(Ordering::SeqCst) != generation {
            return;
        }
        if storage::read_request().is_none_or(|r| r.id != request.id) {
            hide_request(&app);
            return;
        }
    });
}

/// On Wayland apps can't place their windows, so a frame meant to hug the
/// screen edges would end up somewhere in the middle: it's skipped there.
fn can_place_windows() -> bool {
    !cfg!(target_os = "linux")
        || std::env::var_os("WAYLAND_DISPLAY").is_none()
        || std::env::var("GDK_BACKEND").is_ok_and(|b| b.contains("x11"))
}

fn hide_request(app: &AppHandle) {
    app.state::<AppState>()
        .request_generation
        .fetch_add(1, Ordering::SeqCst);
    for label in ["request", "glow"] {
        if let Some(window) = app.get_webview_window(label) {
            set_visible_no_activate(&window, false);
        }
    }
    let _ = app.emit("glow", false);
}

/// "Draw now" on the request card: same as pressing the hotkey. Waits a
/// moment so the card and glow are gone before the screen is frozen.
#[tauri::command]
fn request_draw(app: AppHandle) {
    hide_request(&app);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(180));
        start_capture(&app);
    });
}

/// "Not now" on the request card: tells the AI tool the user declined.
#[tauri::command]
fn request_decline(app: AppHandle) {
    hide_request(&app);
    if let Some(request) = storage::answer_request(None) {
        show_toast(&app, &format!("Declined. {} was told", request.client));
    }
}

/// The request card page (re)loaded: replays what `show_request` already
/// emitted, for a page that loads after the event (a first request that
/// arrives right at launch). Nothing to show → `request: null`, the card
/// stays hidden.
#[tauri::command]
fn request_ready(window: WebviewWindow, app: AppHandle) -> serde_json::Value {
    let request = (window.is_visible().unwrap_or(false))
        .then(|| storage::read_request())
        .flatten();
    match request {
        Some(request) => {
            let config = config(&app);
            serde_json::json!({
                "request": {
                    "id": request.id,
                    "client": request.client,
                    "reason": request.reason,
                    "hotkey": config.hotkey,
                    "expiresAt": request.created_unix_ms + storage::REQUEST_TTL_MS,
                    "sound": config.request_sound,
                }
            })
        }
        None => serde_json::json!({ "request": null }),
    }
}

/// The glow page (re)loaded: is a request up, and where is the camera notch?
/// A page that loads after `show_request` misses the `glow` and `notch`
/// events. The notch is read from the glow window's own frame, so it's
/// correct wherever the frame currently is.
#[tauri::command]
fn glow_ready(window: WebviewWindow, app: AppHandle) -> serde_json::Value {
    let on = window.is_visible().unwrap_or(false);
    let notch = match (window.outer_position(), window.outer_size(), window.scale_factor()) {
        (Ok(pos), Ok(size), Ok(scale)) => {
            macos::screen_top(&app, pos.x as f64 / scale, size.width as f64 / scale).notch
        }
        _ => None,
    };
    serde_json::json!({ "on": on, "notch": notch })
}

// ---------------------------------------------------------------- hotkey & settings

fn register_hotkey(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    shortcuts.unregister_all().map_err(err)?;
    shortcuts.register(hotkey).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    hotkey: String,
    autostart: bool,
    keep_captures: usize,
    max_keep_captures: usize,
    never_save: bool,
    copy_path: bool,
    simple_toolbar: bool,
    auto_update: bool,
    pause_in_fullscreen: bool,
    request_sound: bool,
    capture_monitor: String,
    monitors: Vec<MonitorChoice>,
    version: &'static str,
    tools: Vec<integrations::ToolStatus>,
}

#[derive(Serialize)]
struct MonitorChoice {
    /// The monitor's name, stored in `capture_monitor`.
    id: String,
    label: String,
}

/// The connected monitors for Settings → Monitor, e.g. "Display 2 · 2560 × 1440 (main)".
fn monitor_choices(app: &AppHandle) -> Vec<MonitorChoice> {
    let primary = app.primary_monitor().ok().flatten().and_then(|m| m.name().cloned());
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let id = m.name()?.clone();
            let main = if primary.as_ref() == Some(&id) { " (main)" } else { "" };
            let label = format!("Display {} · {} × {}{main}", i + 1, m.size().width, m.size().height);
            Some(MonitorChoice { id, label })
        })
        .collect()
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Settings {
    let config = config(&app);
    Settings {
        hotkey: config.hotkey.clone(),
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        keep_captures: config.keep_captures,
        max_keep_captures: storage::MAX_KEEP_CAPTURES,
        never_save: config.never_save,
        copy_path: config.copy_path,
        simple_toolbar: config.simple_toolbar,
        auto_update: config.auto_update,
        pause_in_fullscreen: config.pause_in_fullscreen,
        request_sound: config.request_sound,
        capture_monitor: config.capture_monitor.clone(),
        monitors: monitor_choices(&app),
        version: env!("CARGO_PKG_VERSION"),
        tools: integrations::status(&config),
    }
}

/// Validates and stores a new hotkey. While the settings or onboarding window
/// is open no hotkey is active (so recording it doesn't trigger a capture);
/// it is re-registered when the window closes.
#[tauri::command]
fn set_hotkey(app: AppHandle, hotkey: String) -> Result<(), String> {
    hotkey
        .parse::<Shortcut>()
        .map_err(|e| format!("Not a valid shortcut: {e}"))?;
    register_hotkey(&app, &hotkey).map_err(|_| format!("{} is already used by another app", pretty_hotkey(&hotkey)))?;
    let _ = app.global_shortcut().unregister_all();
    update_config(&app, |c| c.hotkey = hotkey)?;
    Ok(())
}

#[tauri::command]
fn set_autostart(app: AppHandle, on: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    if on { launcher.enable() } else { launcher.disable() }.map_err(err)
}

#[tauri::command]
fn set_keep_captures(app: AppHandle, count: usize) -> Result<(), String> {
    let count = count.clamp(1, storage::MAX_KEEP_CAPTURES);
    update_config(&app, |c| c.keep_captures = count)?;
    storage::prune(count);
    Ok(())
}

/// Turning it on also deletes all saved captures and cancels an open AI request.
#[tauri::command]
fn set_never_save(app: AppHandle, on: bool) -> Result<(), String> {
    update_config(&app, |c| c.never_save = on)?;
    if on {
        storage::answer_request(None);
        hide_request(&app);
        let dir = storage::captures_dir();
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(err)?;
        }
    }
    Ok(())
}

#[tauri::command]
fn set_copy_path(app: AppHandle, on: bool) -> Result<(), String> {
    update_config(&app, |c| c.copy_path = on)?;
    Ok(())
}

#[tauri::command]
fn set_simple_toolbar(app: AppHandle, on: bool) -> Result<(), String> {
    update_config(&app, |c| c.simple_toolbar = on)?;
    Ok(())
}

#[tauri::command]
fn set_pause_in_fullscreen(app: AppHandle, on: bool) -> Result<(), String> {
    update_config(&app, |c| c.pause_in_fullscreen = on)?;
    Ok(())
}

#[tauri::command]
fn set_request_sound(app: AppHandle, on: bool) -> Result<(), String> {
    update_config(&app, |c| c.request_sound = on)?;
    Ok(())
}

/// "" = the monitor under the mouse, "primary", or a monitor's name.
#[tauri::command]
fn set_capture_monitor(app: AppHandle, id: String) -> Result<(), String> {
    update_config(&app, |c| c.capture_monitor = id)?;
    Ok(())
}

#[tauri::command]
fn set_auto_update(app: AppHandle, on: bool) -> Result<(), String> {
    update_config(&app, |c| c.auto_update = on)?;
    Ok(())
}

/// Connects or disconnects an AI tool. Runs the tool's CLI, so it's async to
/// keep the window responsive.
#[tauri::command]
async fn set_tool(app: AppHandle, id: String, connected: bool) -> Result<(), String> {
    let name = id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if connected {
            integrations::connect(&name)
        } else {
            integrations::disconnect(&name)
        }
    })
    .await
    .map_err(err)??;
    update_config(&app, |c| {
        c.tools.retain(|t| *t != id);
        if connected {
            c.tools.push(id);
        }
    })?;
    Ok(())
}

#[tauri::command]
fn close_window(window: WebviewWindow) -> Result<(), String> {
    window.close().map_err(err)
}

/// Settings → "Run setup again". Async: a sync command runs on the main
/// thread, and creating a window there deadlocks on Windows.
#[tauri::command]
async fn show_onboarding(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    open_onboarding(&app);
    window.close().map_err(err)
}

/// Opens one of the app's own links in the browser.
#[tauri::command]
fn open_link(app: AppHandle, which: String) -> Result<(), String> {
    let url = match which.as_str() {
        "repo" => REPO_URL.to_string(),
        "license" => format!("{REPO_URL}/blob/main/LICENSE"),
        "issues" => format!("{REPO_URL}/issues"),
        "manual" => format!("{REPO_URL}/blob/main/docs/manual-setup.md"),
        "author" => "https://github.com/cweeperde".to_string(),
        "keinplan0" => "https://github.com/KeinPlan0".to_string(),
        _ => return Err("unknown link".into()),
    };
    app.opener().open_url(url, None::<&str>).map_err(err)
}

#[tauri::command]
fn open_captures(app: AppHandle) -> Result<(), String> {
    let dir = storage::captures_dir();
    let _ = std::fs::create_dir_all(&dir);
    app.opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(err)
}

/// Settings and onboarding: small fixed windows. While one is open the hotkey
/// is off (so recording a new one doesn't start a capture).
fn open_window(app: &AppHandle, label: &str, title: &str, width: f64, height: f64) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.set_focus();
        return;
    }
    let Ok(window) = WebviewWindowBuilder::new(app, label, WebviewUrl::App(format!("{label}.html").into()))
        .title(title)
        .inner_size(width, height)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .center()
        .build()
    else {
        return;
    };
    // On macOS a menu bar app's new window can open behind the active app.
    let _ = window.set_focus();
    let _ = app.global_shortcut().unregister_all();
    let app = app.clone();
    let label = label.to_string();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            // Closing the onboarding in any way counts as done; it's all in Settings too.
            if label == "onboarding" {
                let _ = update_config(&app, |c| c.onboarded = true);
            }
            // Settings → "Run setup again" swaps one window for the other;
            // the hotkey stays off until both are closed.
            let other = if label == "settings" { "onboarding" } else { "settings" };
            if app.get_webview_window(other).is_some() {
                return;
            }
            let hotkey = config(&app).hotkey;
            let pretty = pretty_hotkey(&hotkey);
            if register_hotkey(&app, &hotkey).is_err() {
                show_toast(&app, &format!("{pretty} is in use by another app"));
            } else if label == "onboarding" {
                show_toast(
                    &app,
                    &format!("Scribble2Fix is ready. Press {pretty} to mark up your screen"),
                );
            }
        }
    });
}

fn open_settings(app: &AppHandle) {
    open_window(app, "settings", "Scribble2Fix Settings", 460.0, 700.0);
}

fn open_onboarding(app: &AppHandle) {
    open_window(app, "onboarding", "Welcome to Scribble2Fix", 520.0, 700.0);
}

// ---------------------------------------------------------------- fullscreen apps

/// With "Pause hotkey in fullscreen apps" on, releases the hotkey while a
/// fullscreen app is in front, so games and videos get the key, and takes it
/// back afterwards. Checks once a second.
fn watch_fullscreen(app: &AppHandle) {
    loop {
        thread::sleep(Duration::from_secs(1));
        let state = app.state::<AppState>();
        // Settings / onboarding manage the hotkey themselves while open.
        if ["settings", "onboarding"]
            .iter()
            .any(|w| app.get_webview_window(w).is_some())
        {
            continue;
        }
        let config = config(app);
        let pause = config.pause_in_fullscreen && !state.active.load(Ordering::SeqCst) && fullscreen_app_in_front(app);
        if pause == state.hotkey_paused.load(Ordering::SeqCst) {
            continue;
        }
        if pause {
            let _ = app.global_shortcut().unregister_all();
        } else if register_hotkey(app, &config.hotkey).is_err() {
            continue; // try again next round
        }
        state.hotkey_paused.store(pause, Ordering::SeqCst);
    }
}

/// Windows tracks this itself: "busy" means a fullscreen app (including
/// borderless fullscreen), the other state means an exclusive Direct3D game.
#[cfg(windows)]
fn fullscreen_app_in_front(_app: &AppHandle) -> bool {
    use windows_sys::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN};
    let mut state = 0;
    let ok = unsafe { SHQueryUserNotificationState(&mut state) } == 0;
    ok && (state == QUNS_BUSY || state == QUNS_RUNNING_D3D_FULL_SCREEN)
}

/// Elsewhere: the active window (not ours) covers its whole monitor. Window
/// positions may be in physical or logical pixels, so accept either.
#[cfg(not(windows))]
fn fullscreen_app_in_front(app: &AppHandle) -> bool {
    let Ok(w) = active_win_pos_rs::get_active_window() else {
        return false;
    };
    if w.process_id == std::process::id() as u64 {
        return false;
    }
    let p = w.position;
    let near = |a: f64, b: f64| (a - b).abs() <= 2.0;
    app.available_monitors().unwrap_or_default().iter().any(|m| {
        let s = m.scale_factor();
        let (mx, my) = (m.position().x as f64, m.position().y as f64);
        let (mw, mh) = (m.size().width as f64, m.size().height as f64);
        [1.0, s]
            .iter()
            .any(|k| near(p.x, mx / k) && near(p.y, my / k) && near(p.width, mw / k) && near(p.height, mh / k))
    })
}

// ---------------------------------------------------------------- updates

/// Checks GitHub for a newer release. If there is one, downloads it, waits
/// until no capture is in progress, installs it and restarts.
async fn check_update(app: &AppHandle) -> Result<(), String> {
    let Some(update) = app.updater().map_err(err)?.check().await.map_err(err)? else {
        return Ok(());
    };
    show_toast(app, &format!("Updating Scribble2Fix to {}…", update.version));
    let bytes = update.download(|_, _| {}, || {}).await.map_err(err)?;
    while app.state::<AppState>().active.load(Ordering::SeqCst) {
        let _ = tauri::async_runtime::spawn_blocking(|| thread::sleep(Duration::from_secs(2))).await;
    }
    update.install(bytes).map_err(err)?;
    app.restart();
}

/// "Check for updates" in settings: installs right away if there is one.
#[tauri::command]
async fn update_now(app: AppHandle) -> Result<String, String> {
    check_update(&app).await?;
    Ok("You have the latest version".into())
}

// ---------------------------------------------------------------- setup

fn create_windows(app: &AppHandle) -> tauri::Result<()> {
    // Pre-warmed: created hidden at startup so the hotkey only has to show it.
    WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
        .title("Scribble2Fix")
        .visible(false)
        .decorations(false)
        .resizable(false)
        .shadow(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .build()?;

    let toast = WebviewWindowBuilder::new(app, "toast", WebviewUrl::App("toast.html".into()))
        .title("Scribble2Fix")
        .inner_size(TOAST_WIDTH, 64.0)
        .visible(false)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .shadow(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .focused(false)
        .focusable(false)
        .build()?;
    toast.set_ignore_cursor_events(true)?;

    // AI request: a clickable card, and a click-through glowing frame around
    // the work area (it leaves the taskbar out, so Windows doesn't treat it
    // as a fullscreen window).
    // Never takes keyboard focus (the user may be typing), but its buttons
    // take the first click.
    // They start on the monitor a request would go to. A window that is moved
    // to its place only after creation may keep its initial size and place
    // (issue #5: on the first request after launch, the frame page drew its
    // border around half of the screen).
    let layout = can_place_windows().then(|| request_layout(app)).flatten();
    let mut request = WebviewWindowBuilder::new(app, "request", WebviewUrl::App("request.html".into()))
        .title("Scribble2Fix")
        .inner_size(REQUEST_SIZE.0, REQUEST_SIZE.1)
        .visible(false)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .shadow(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .focused(false)
        .focusable(false)
        .accept_first_mouse(true);
    let mut glow_builder = WebviewWindowBuilder::new(app, "glow", WebviewUrl::App("glow.html".into()))
        .title("Scribble2Fix")
        .visible(false)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .shadow(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .focused(false)
        .focusable(false);
    if let Some((frame, card, scale)) = layout {
        request = request.position(card.position.x as f64 / scale, card.position.y as f64 / scale);
        glow_builder = glow_builder
            .position(frame.position.x as f64 / scale, frame.position.y as f64 / scale)
            .inner_size(frame.size.width as f64 / scale, frame.size.height as f64 / scale);
    }
    request.build()?;
    let glow = glow_builder.build()?;
    glow.set_ignore_cursor_events(true)?;
    for label in ["overlay", "toast", "request", "glow"] {
        if let Some(window) = app.get_webview_window(label) {
            macos::join_all_spaces(&window);
            no_window_animations(&window);
        }
    }
    macos::above_menu_bar(&glow);
    Ok(())
}

fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let folder = MenuItem::with_id(app, "folder", "Open captures folder", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Scribble2Fix", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&settings, &folder, &PredefinedMenuItem::separator(app)?, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("app icon"))
        .tooltip("Scribble2Fix")
        .menu(&menu)
        // Left click starts a scribble, right click opens the menu. Linux
        // trays only report menu clicks, so there the menu opens either way.
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                start_capture(tray.app_handle());
            }
        })
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "settings" => open_settings(app),
            "folder" => {
                let _ = open_captures(app.clone());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn main() {
    // `scribble2fix mcp [--client <name>]`: act as the MCP server for an AI tool (stdio).
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        let args: Vec<String> = std::env::args().collect();
        let client = args
            .iter()
            .position(|a| a == "--client")
            .and_then(|i| args.get(i + 1).cloned());
        mcp::run(client);
        return;
    }
    // `scribble2fix launch <args>`: starts the app with <args> and exits right
    // away, so the app outlives the MCP server that asked for it.
    if std::env::args().nth(1).as_deref() == Some("launch") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let ok = std::env::current_exe().is_ok_and(|exe| mcp::spawn(&exe, &args).is_ok());
        std::process::exit(if ok { 0 } else { 1 });
    }

    tauri::Builder::default()
        // Must be first: a second launch (`scribble2fix --capture` bound to a
        // desktop shortcut on Wayland, or `--request` from the MCP server) is
        // forwarded to the running instance.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if argv.iter().any(|a| a == "--capture") {
                start_capture(app);
            } else if argv.iter().any(|a| a == "--request") {
                show_request(app);
            } else {
                show_toast(app, "Scribble2Fix is already running");
            }
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        start_capture(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            frozen_pixels,
            show_overlay,
            reveal_overlay,
            refreeze,
            hide_overlay,
            overlay_loaded,
            cancel_capture,
            finish_capture,
            get_settings,
            set_hotkey,
            set_autostart,
            set_keep_captures,
            set_never_save,
            set_auto_update,
            set_pause_in_fullscreen,
            set_copy_path,
            set_simple_toolbar,
            set_tool,
            close_window,
            show_onboarding,
            open_link,
            open_captures,
            update_now,
            request_draw,
            request_decline,
            request_ready,
            glow_ready,
            set_request_sound,
            set_capture_monitor,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            storage::hold_app_lock();
            let handle = app.handle();

            // Before the windows: `create_windows` places the request card and
            // the glow frame on the monitor from this config.
            let config = storage::load_config();
            *app.state::<AppState>().config.lock().unwrap() = config.clone();

            create_windows(handle)?;
            macos::watch_spaces();
            create_tray(handle)?;

            let request = std::env::args().any(|a| a == "--request");

            if !config.onboarded {
                open_onboarding(handle);
            } else if register_hotkey(handle, &config.hotkey).is_err() {
                show_toast(
                    handle,
                    &format!(
                        "{} is in use. Pick another hotkey in Settings",
                        pretty_hotkey(&config.hotkey)
                    ),
                );
            } else if !request {
                show_toast(
                    handle,
                    &format!(
                        "Scribble2Fix is running. Press {} to mark up your screen",
                        pretty_hotkey(&config.hotkey)
                    ),
                );
            }

            let refresh = config.clone();
            thread::spawn(move || integrations::refresh(&refresh));

            let app_handle = handle.clone();
            thread::spawn(move || watch_fullscreen(&app_handle));

            // Debug builds have no published release to compare against.
            if config.auto_update && !cfg!(debug_assertions) {
                let app = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = check_update(&app).await;
                });
            }

            if std::env::args().any(|a| a == "--capture") {
                start_capture(handle);
            } else if request {
                show_request(handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start Scribble2Fix")
        .run(|_app, event| {
            // Closing windows must not quit the tray app; only "Quit" does.
            if let RunEvent::ExitRequested { code: None, api, .. } = event {
                api.prevent_exit();
            }
        });
}
