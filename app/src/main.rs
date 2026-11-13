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
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, RunEvent, State, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
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
    /// The hotkey is released because a fullscreen app is in front.
    hotkey_paused: AtomicBool,
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

// ---------------------------------------------------------------- macOS permission

/// Without the Screen Recording permission macOS still returns a screenshot,
/// but with every window removed (only wallpaper and menu bar). If it looks
/// missing, ask for it and open the settings page, but only once per run:
/// the check can stay negative until the app restarts, and the capture
/// itself is still attempted.
#[cfg(target_os = "macos")]
mod macos {
    use std::sync::atomic::{AtomicBool, Ordering};

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    static ASKED: AtomicBool = AtomicBool::new(false);

    /// Returns false only the first time access looks missing in this run.
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
}

#[cfg(not(target_os = "macos"))]
mod macos {
    pub fn check_screen_access() -> bool {
        true
    }
}

// ---------------------------------------------------------------- capture

fn start_capture(app: &AppHandle) {
    if !macos::check_screen_access() {
        show_toast(
            app,
            "Allow Screen Recording for Scribble2Fix, then restart it",
        );
    }
    if app.state::<AppState>().active.swap(true, Ordering::SeqCst) {
        return;
    }
    if storage::read_request().is_some() {
        hide_toast(app);
    }
    let app = app.clone();
    thread::spawn(move || {
        if let Err(e) = capture(&app) {
            app.state::<AppState>()
                .active
                .store(false, Ordering::SeqCst);
            show_toast(&app, &format!("Capture failed: {e}"));
        }
    });
}

/// Freeze the monitor under the cursor and hand it to the (hidden, pre-warmed)
/// overlay. The overlay shows itself once the screenshot is painted.
fn capture(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let unix_ms = chrono::Utc::now().timestamp_millis();
    let (app_name, window_title) = active_win_pos_rs::get_active_window()
        .map(|w| (w.app_name, w.title))
        .unwrap_or_default();

    let cursor = app.cursor_position().map_err(err)?;
    let monitor = app
        .monitor_from_point(cursor.x, cursor.y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten())
        .ok_or("no monitor found")?;
    let area = Area {
        x: monitor.position().x,
        y: monitor.position().y,
        width: monitor.size().width,
        height: monitor.size().height,
        scale: monitor.scale_factor(),
    };

    let shot = find_screen(&area, cursor.x, cursor.y)?
        .capture_image()
        .map_err(err)?;
    let (width, height) = (shot.width(), shot.height());

    *state.pixels.lock().unwrap() = Some(shot.into_raw());
    *state.area.lock().unwrap() = Some(area);
    *state.source.lock().unwrap() = Source {
        unix_ms,
        app_name,
        window_title,
    };

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
        /// Shown in the hint while an AI tool waits for this scribble.
        request: Option<String>,
        /// Only the basic tools in the toolbar.
        simple: bool,
    }
    let request = storage::read_request().map(|r| format!("{}: {}", r.client, r.reason));
    overlay
        .emit(
            "capture",
            Payload {
                width,
                height,
                request,
                simple: config(app).simple_toolbar,
            },
        )
        .map_err(err)
}

/// Find the xcap screen matching a Tauri monitor. Platforms disagree on
/// whether monitor origins are physical or logical, so accept either.
fn find_screen(area: &Area, cursor_x: f64, cursor_y: f64) -> Result<xcap::Monitor, String> {
    let lx = (area.x as f64 / area.scale).round() as i32;
    let ly = (area.y as f64 / area.scale).round() as i32;
    let found = xcap::Monitor::all().map_err(err)?.into_iter().find(|m| {
        matches!((m.x(), m.y()), (Ok(x), Ok(y)) if (x == area.x && y == area.y) || (x == lx && y == ly))
    });
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

#[tauri::command]
fn show_overlay(window: WebviewWindow) -> Result<(), String> {
    window.show().map_err(err)?;
    window.set_always_on_top(true).map_err(err)?;
    window.set_focus().map_err(err)
}

#[tauri::command]
fn hide_overlay(window: WebviewWindow) -> Result<(), String> {
    window.hide().map_err(err)
}

/// The overlay page (re)loaded. At startup nothing is going on; after an
/// unexpected reload during a capture the frozen screenshot is gone, so hide
/// the then-black window and allow a new capture. An open AI request stays
/// open, so the user can simply press the hotkey again.
#[tauri::command]
fn overlay_loaded(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    app.state::<AppState>()
        .active
        .store(false, Ordering::SeqCst);
    window.hide().map_err(err)
}

#[tauri::command]
fn cancel_capture(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    app.state::<AppState>()
        .active
        .store(false, Ordering::SeqCst);
    if let Some(request) = storage::answer_request(None) {
        show_toast(&app, &format!("Cancelled. {} was told", request.client));
    }
    window.hide().map_err(err)
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
    marks: serde_json::Value,
) -> Result<(), String> {
    window.hide().map_err(err)?;
    let state = app.state::<AppState>();
    let source = state.source.lock().unwrap().clone();
    state.active.store(false, Ordering::SeqCst);

    let app = app.clone();
    thread::spawn(move || {
        let message = match deliver(&app, &annotated, &clean, crop, marks, source) {
            Ok(dir) => match dir.and_then(|d| storage::answer_request(Some(d))) {
                Some(request) => format!("Sent to {}, also copied", request.client),
                None if config(&app).copy_path && !config(&app).never_save => {
                    "File path copied".to_string()
                }
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
            captured_at,
            captured_at_unix_ms: source.unix_ms,
            app_name: source.app_name,
            window_title: source.window_title,
            crop,
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
        clipboard
            .set_text(path.to_string_lossy().into_owned())
            .map_err(err)?;
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

fn hide_toast(app: &AppHandle) {
    app.state::<AppState>()
        .toast_generation
        .fetch_add(1, Ordering::SeqCst);
    if let Some(toast) = app.get_webview_window("toast") {
        let _ = toast.emit("toast", serde_json::json!({ "text": "", "ms": 0 }));
        set_toast_visible(&toast, false);
    }
}

/// An AI tool asked for a scribble (`scribble2fix --request`, sent by the MCP
/// server): keep a notice up until the user starts the capture.
fn show_request(app: &AppHandle) {
    if let Some(request) = storage::read_request() {
        let hotkey = config(app).hotkey;
        let ms = (storage::REQUEST_TTL_MS
            - (chrono::Utc::now().timestamp_millis() - request.created_unix_ms))
            .max(0);
        show_toast_for(
            app,
            &format!(
                "{} asks: {}. Press {hotkey} when ready",
                request.client, request.reason
            ),
            ms as u64,
        );
    }
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
        let y = a.y + a.height as i32 - h as i32 - (90.0 * a.scale) as i32;
        let _ = toast.set_position(PhysicalPosition::new(x, y));
        let _ = toast.set_size(PhysicalSize::new(w, h));
    }
    let _ = toast.emit(
        "toast",
        serde_json::json!({ "text": text, "ms": ms.saturating_sub(400) }),
    );
    set_toast_visible(&toast, true);

    let generation = state.toast_generation.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(ms));
        if app
            .state::<AppState>()
            .toast_generation
            .load(Ordering::SeqCst)
            == generation
        {
            set_toast_visible(&toast, false);
        }
    });
}

/// The toast must never take keyboard focus (the user is about to paste
/// somewhere). On Windows, `show()` activates the window whenever the process
/// is allowed to (e.g. right after launch), so show it without activation.
#[cfg(windows)]
fn set_toast_visible(toast: &WebviewWindow, visible: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE};
    if let Ok(hwnd) = toast.hwnd() {
        unsafe {
            ShowWindow(
                hwnd.0 as _,
                if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
            )
        };
    }
}

#[cfg(not(windows))]
fn set_toast_visible(toast: &WebviewWindow, visible: bool) {
    let _ = if visible { toast.show() } else { toast.hide() };
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
    version: &'static str,
    tools: Vec<integrations::ToolStatus>,
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
    register_hotkey(&app, &hotkey)
        .map_err(|_| format!("{hotkey} is already used by another app"))?;
    let _ = app.global_shortcut().unregister_all();
    update_config(&app, |c| c.hotkey = hotkey)?;
    Ok(())
}

#[tauri::command]
fn set_autostart(app: AppHandle, on: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    if on {
        launcher.enable()
    } else {
        launcher.disable()
    }
    .map_err(err)
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

/// Settings → "Show welcome again". Async: a sync command runs on the main
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
        "author" => "https://github.com/cweeperde".to_string(),
        _ => return Err("unknown link".into()),
    };
    app.opener().open_url(url, None::<&str>).map_err(err)
}

#[tauri::command]
fn open_captures(app: AppHandle) -> Result<(), String> {
    let dir = storage::captures_dir();
    let _ = std::fs::create_dir_all(&dir);
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(err)
}

/// Settings and onboarding: small fixed windows. While one is open the hotkey
/// is off (so recording a new one doesn't start a capture).
fn open_window(app: &AppHandle, label: &str, title: &str, width: f64, height: f64) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.set_focus();
        return;
    }
    let Ok(window) =
        WebviewWindowBuilder::new(app, label, WebviewUrl::App(format!("{label}.html").into()))
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
    let _ = app.global_shortcut().unregister_all();
    let app = app.clone();
    let label = label.to_string();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            // Closing the onboarding in any way counts as done; it's all in Settings too.
            if label == "onboarding" {
                let _ = update_config(&app, |c| c.onboarded = true);
            }
            // Settings → "Show welcome again" swaps one window for the other;
            // the hotkey stays off until both are closed.
            let other = if label == "settings" {
                "onboarding"
            } else {
                "settings"
            };
            if app.get_webview_window(other).is_some() {
                return;
            }
            let hotkey = config(&app).hotkey;
            if register_hotkey(&app, &hotkey).is_err() {
                show_toast(&app, &format!("{hotkey} is in use by another app"));
            } else if label == "onboarding" {
                show_toast(
                    &app,
                    &format!("Scribble2Fix is ready. Press {hotkey} to mark up your screen"),
                );
            }
        }
    });
}

fn open_settings(app: &AppHandle) {
    open_window(app, "settings", "Scribble2Fix Settings", 460.0, 660.0);
}

fn open_onboarding(app: &AppHandle) {
    open_window(app, "onboarding", "Welcome to Scribble2Fix", 520.0, 660.0);
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
        let pause = config.pause_in_fullscreen
            && !state.active.load(Ordering::SeqCst)
            && fullscreen_app_in_front(app);
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
    use windows_sys::Win32::UI::Shell::{
        SHQueryUserNotificationState, QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN,
    };
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
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .any(|m| {
            let s = m.scale_factor();
            let (mx, my) = (m.position().x as f64, m.position().y as f64);
            let (mw, mh) = (m.size().width as f64, m.size().height as f64);
            [1.0, s].iter().any(|k| {
                near(p.x, mx / k)
                    && near(p.y, my / k)
                    && near(p.width, mw / k)
                    && near(p.height, mh / k)
            })
        })
}

// ---------------------------------------------------------------- updates

/// Checks GitHub for a newer release. If there is one, downloads it, waits
/// until no capture is in progress, installs it and restarts.
async fn check_update(app: &AppHandle) -> Result<(), String> {
    let Some(update) = app.updater().map_err(err)?.check().await.map_err(err)? else {
        return Ok(());
    };
    show_toast(
        app,
        &format!("Updating Scribble2Fix to {}…", update.version),
    );
    let bytes = update.download(|_, _| {}, || {}).await.map_err(err)?;
    while app.state::<AppState>().active.load(Ordering::SeqCst) {
        let _ =
            tauri::async_runtime::spawn_blocking(|| thread::sleep(Duration::from_secs(2))).await;
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
    Ok(())
}

fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let folder = MenuItem::with_id(app, "folder", "Open captures folder", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Scribble2Fix", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &settings,
            &folder,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

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
    // `scribble2fix mcp`: act as the MCP server for Claude Code / Codex (stdio).
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        mcp::run();
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
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            frozen_pixels,
            show_overlay,
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
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            storage::hold_app_lock();
            let handle = app.handle();
            create_windows(handle)?;
            create_tray(handle)?;

            let config = storage::load_config();
            *app.state::<AppState>().config.lock().unwrap() = config.clone();
            let request = std::env::args().any(|a| a == "--request");

            if !config.onboarded {
                open_onboarding(handle);
            } else if register_hotkey(handle, &config.hotkey).is_err() {
                show_toast(
                    handle,
                    &format!(
                        "{} is in use. Pick another hotkey in Settings",
                        config.hotkey
                    ),
                );
            } else if !request {
                show_toast(
                    handle,
                    &format!(
                        "Scribble2Fix is running. Press {} to mark up your screen",
                        config.hotkey
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
            if let RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                api.prevent_exit();
            }
        });
}
