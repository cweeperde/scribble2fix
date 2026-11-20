//! Everything Scribble2Fix keeps on disk lives in `~/.scribble2fix/`:
//! - `config.json`: user settings
//! - `captures/<timestamp>/`: `annotated.png`, `clean.png`, `meta.json`
//! - `request.json` / `result-<id>.json`: an AI tool asking for a scribble
//!   (see `mcp.rs`)
//! - `app.lock`: locked while the app runs, so the MCP server knows whether
//!   it has to start it
//!
//! The MCP server reads the newest capture folder that has a `meta.json`,
//! so `meta.json` is always written last.

use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

/// One key on every system. On MacBook keyboards it's Fn+F6 (unless the F
/// keys are set to work as standard function keys); the welcome screen says so.
pub const DEFAULT_HOTKEY: &str = "F6";
pub const DEFAULT_KEEP_CAPTURES: usize = 10;
pub const MAX_KEEP_CAPTURES: usize = 100;

pub fn data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".scribble2fix")
}

pub fn captures_dir() -> PathBuf {
    data_dir().join("captures")
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub hotkey: String,
    pub keep_captures: usize,
    /// Only copy to the clipboard: nothing is written to disk and the MCP
    /// tools report that they are turned off.
    pub never_save: bool,
    /// Put the saved image's file path on the clipboard instead of the image.
    pub copy_path: bool,
    /// Only the basic tools (colors, freehand, box, arrow) in the toolbar.
    pub simple_toolbar: bool,
    pub auto_update: bool,
    /// Release the hotkey while a fullscreen app (game, video) is in front.
    pub pause_in_fullscreen: bool,
    /// The first-run onboarding was completed.
    pub onboarded: bool,
    /// Play a short sound when an AI tool asks for a scribble.
    pub request_sound: bool,
    /// Which monitor to capture: "" = the one under the mouse, "primary", or
    /// a monitor's name (falls back to the mouse when it isn't connected).
    pub capture_monitor: String,
    /// AI tools the user chose to connect (ids from `integrations::TOOLS`).
    pub tools: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hotkey: DEFAULT_HOTKEY.into(),
            keep_captures: DEFAULT_KEEP_CAPTURES,
            never_save: false,
            copy_path: false,
            simple_toolbar: false,
            auto_update: true,
            pause_in_fullscreen: false,
            onboarded: false,
            request_sound: true,
            capture_monitor: String::new(),
            tools: vec![],
        }
    }
}

pub fn load_config() -> Config {
    let mut config: Config = fs::read_to_string(data_dir().join("config.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // Version 1.0 had Claude Desktop as its own tool; it's part of "claude" now.
    if config.tools.iter().any(|t| t == "claude-desktop") {
        config.tools.retain(|t| t != "claude-desktop" && t != "claude");
        config.tools.push("claude".into());
    }
    config
}

pub fn save_config(config: &Config) -> io::Result<()> {
    fs::create_dir_all(data_dir())?;
    fs::write(
        data_dir().join("config.json"),
        serde_json::to_string_pretty(config)?,
    )
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The monitor a capture was taken on (physical pixels).
#[derive(Serialize, Clone, Default)]
pub struct MonitorInfo {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Display scaling, e.g. 1.5 for 150 %.
    pub scale: f64,
}

/// The AI request this capture answered.
#[derive(Serialize)]
pub struct MetaRequest {
    pub client: String,
    pub reason: String,
}

/// `meta.json` next to the two images of a capture.
#[derive(Serialize)]
pub struct Meta {
    pub app_version: &'static str,
    /// "windows", "macos" or "linux".
    pub os: &'static str,
    /// Local time, RFC 3339.
    pub captured_at: String,
    pub captured_at_unix_ms: i64,
    /// From freezing the screen to confirming.
    pub drawing_seconds: f64,
    pub app_name: String,
    pub window_title: String,
    pub monitor: MonitorInfo,
    /// Crop rectangle in physical pixels of the captured monitor.
    pub crop: Crop,
    /// Exported image pixels per screen pixel (below 1 when a large area was
    /// scaled down). Image pixel = (screen pixel - crop origin) * image_scale.
    pub image_scale: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<MetaRequest>,
    /// The marks as data, in pixels of the exported image (see overlay.html).
    pub marks: serde_json::Value,
}

pub fn save_capture(
    annotated_png: &[u8],
    clean_png: &[u8],
    meta: &Meta,
    keep: usize,
) -> io::Result<PathBuf> {
    let name = chrono::DateTime::from_timestamp_millis(meta.captured_at_unix_ms)
        .unwrap_or_default()
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d_%H-%M-%S-%3f")
        .to_string();
    let dir = captures_dir().join(name);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("annotated.png"), annotated_png)?;
    fs::write(dir.join("clean.png"), clean_png)?;
    fs::write(dir.join("meta.json"), serde_json::to_string_pretty(meta)?)?;
    prune(keep);
    Ok(dir)
}

/// Keep only the newest `keep` captures. Folder names sort chronologically.
pub fn prune(keep: usize) {
    let Ok(entries) = fs::read_dir(captures_dir()) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let excess = dirs.len().saturating_sub(keep);
    for old in &dirs[..excess] {
        let _ = fs::remove_dir_all(old);
    }
}

// ---------------------------------------------------------------- AI requests

/// An AI tool (through the MCP server) asking the user for a scribble.
#[derive(Serialize, Deserialize, Clone)]
pub struct Request {
    pub id: String,
    /// "Claude", "Codex", ...
    pub client: String,
    /// What the AI wants the user to mark.
    pub reason: String,
    pub created_unix_ms: i64,
}

/// How long a request stays open before the app ignores it.
pub const REQUEST_TTL_MS: i64 = 10 * 60 * 1000;

#[derive(Serialize, Deserialize)]
pub struct RequestResult {
    /// The capture folder, or None if the user cancelled.
    pub capture: Option<PathBuf>,
}

fn request_file() -> PathBuf {
    data_dir().join("request.json")
}

pub fn result_file(id: &str) -> PathBuf {
    data_dir().join(format!("result-{id}.json"))
}

pub fn write_request(request: &Request) -> io::Result<()> {
    fs::create_dir_all(data_dir())?;
    fs::write(request_file(), serde_json::to_string(request)?)
}

/// The open request, if there is one and it hasn't expired.
pub fn read_request() -> Option<Request> {
    let request: Request = serde_json::from_str(&fs::read_to_string(request_file()).ok()?).ok()?;
    let age = chrono::Utc::now().timestamp_millis() - request.created_unix_ms;
    (age < REQUEST_TTL_MS).then_some(request)
}

/// Answers the open request (if any) and closes it.
pub fn answer_request(capture: Option<PathBuf>) -> Option<Request> {
    let request = read_request();
    let _ = fs::remove_file(request_file());
    let request = request?;
    let json = serde_json::to_string(&RequestResult { capture }).ok()?;
    fs::write(result_file(&request.id), json).ok()?;
    Some(request)
}

fn lock_file() -> io::Result<fs::File> {
    fs::create_dir_all(data_dir())?;
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir().join("app.lock"))
}

/// Held by the running app; released by the OS when it exits (even on a crash).
pub fn hold_app_lock() {
    if let Ok(file) = lock_file() {
        if file.try_lock().is_ok() {
            std::mem::forget(file);
        }
    }
}

pub fn app_running() -> bool {
    lock_file().is_ok_and(|file| matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock)))
}

pub fn clear_request(id: &str) {
    if read_request().is_some_and(|r| r.id == id) {
        let _ = fs::remove_file(request_file());
    }
    let _ = fs::remove_file(result_file(id));
}
