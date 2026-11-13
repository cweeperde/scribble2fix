//! Minimal MCP server (stdio, newline-delimited JSON-RPC 2.0). Runs when the app
//! is started as `scribble2fix mcp`. Tools:
//! - `get_latest_capture`: reads the newest capture in `~/.scribble2fix/captures/`
//! - `request_scribble` / `wait_for_scribble`: asks the running app (started
//!   if it isn't running) to prompt the user for a scribble, then waits for the result.
//!   They talk through files in `~/.scribble2fix/` (see `storage.rs`).

use std::{
    fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::Engine;
use serde_json::{json, Value};

use crate::storage;

const STALE_AFTER_MINUTES: i64 = 10;
/// A tool call returns after this long even if the user is still drawing
/// (Codex cancels tool calls after 60 s by default); the AI then calls
/// `wait_for_scribble`.
const WAIT_PER_CALL: Duration = Duration::from_secs(50);

/// Friendly name of the connected AI tool, from `initialize`.
static CLIENT: Mutex<String> = Mutex::new(String::new());

pub fn run() {
    let mut out = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => match handle(&msg) {
                Some(r) => r,
                None => continue, // notification, no reply
            },
            Err(e) => error(Value::Null, -32700, &format!("Parse error: {e}")),
        };
        if writeln!(out, "{response}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let params = &msg["params"];
    Some(match msg["method"].as_str().unwrap_or("") {
        "initialize" => {
            *CLIENT.lock().unwrap() = client_name(params["clientInfo"]["name"].as_str().unwrap_or(""));
            ok(
                id,
                json!({
                    "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "scribble2fix", "version": env!("CARGO_PKG_VERSION") },
                }),
            )
        }
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let args = &params["arguments"];
            let result = if storage::load_config().never_save {
                text_result(
                    "Scribble2Fix is set to never save images (Settings → Captures & privacy), so its tools are \
                     turned off. Ask the user to paste their capture from the clipboard instead.",
                    true,
                )
            } else {
                match params["name"].as_str() {
                    Some("get_latest_capture") => latest_capture(),
                    Some("get_recent_captures") => recent_captures(args["count"].as_u64().unwrap_or(3)),
                    Some("request_scribble") => request_scribble(args["reason"].as_str().unwrap_or("")),
                    Some("wait_for_scribble") => wait_for_scribble(args["request_id"].as_str().unwrap_or("")),
                    _ => return Some(error(id, -32602, "Unknown tool")),
                }
            };
            ok(id, result)
        }
        _ => error(id, -32601, "Method not found"),
    })
}

fn client_name(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("claude") {
        "Claude".into()
    } else if lower.contains("codex") {
        "Codex".into()
    } else if raw.is_empty() {
        "Your AI tool".into()
    } else {
        raw.into()
    }
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "get_latest_capture",
            "description": "Get the user's latest Scribble2Fix screen markup: a screenshot they drew on \
                (green = add, red = remove, blue = change/move), the same area without the drawings, \
                and the app/window it was taken from. Use it when the user mentions their capture, \
                scribble, drawing, markup, circle, or screenshot.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "title": "Get latest Scribble2Fix capture", "readOnlyHint": true },
        },
        {
            "name": "get_recent_captures",
            "description": "Get the user's last few Scribble2Fix captures (newest first), each with \
                its annotated and clean screenshot. Use it when the user made several captures in a \
                row and wants all of them applied.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "count": { "type": "integer", "minimum": 1, "maximum": 10, "description": "How many captures (default 3)." }
                }
            },
            "annotations": { "title": "Get recent Scribble2Fix captures", "readOnlyHint": true },
        },
        {
            "name": "request_scribble",
            "description": "Ask the user to mark something on their screen with Scribble2Fix and get \
                the result (annotated + clean screenshot). Useful when you need to know which UI \
                element, area or layout the user means. IMPORTANT: ALWAYS ask the user in the chat \
                first whether they want to scribble now, and only call this tool after they agreed. \
                The user then gets a notice and starts the capture with their hotkey when ready. \
                If they are still drawing when this returns, call `wait_for_scribble` with the \
                returned request_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "reason": {
                        "type": "string",
                        "description": "Short instruction shown to the user, e.g. \"Mark where the new button should go\". Max ~80 characters."
                    }
                },
                "required": ["reason"]
            },
            "annotations": { "title": "Request a Scribble2Fix scribble", "readOnlyHint": true },
        },
        {
            "name": "wait_for_scribble",
            "description": "Keep waiting for a scribble requested with `request_scribble`.",
            "inputSchema": {
                "type": "object",
                "properties": { "request_id": { "type": "string" } },
                "required": ["request_id"]
            },
            "annotations": { "title": "Wait for a Scribble2Fix scribble", "readOnlyHint": true },
        },
    ])
}

// ---------------------------------------------------------------- requests

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn request_scribble(reason: &str) -> Value {
    let reason: String = reason.trim().chars().take(120).collect();
    let request = storage::Request {
        id: format!("{:x}", now_ms()),
        client: CLIENT.lock().unwrap().clone(),
        reason: if reason.is_empty() {
            "Mark what you mean on the screen".into()
        } else {
            reason
        },
        created_unix_ms: now_ms(),
    };
    if let Err(e) = storage::write_request(&request) {
        return text_result(&format!("Couldn't create the request: {e}"), true);
    }
    // A running app (single instance) gets the request from a short-lived second
    // launch. Otherwise the app is started through `scribble2fix launch`, so it
    // isn't a child of the AI tool and keeps running when the tool exits.
    let exe = crate::integrations::mcp_executable().or_else(|| std::env::current_exe().ok());
    let launched = exe.is_some_and(|exe| {
        if storage::app_running() {
            spawn(&exe, &["--request"]).is_ok()
        } else {
            launch(&exe) && wait_until_running()
        }
    });
    if !launched {
        storage::clear_request(&request.id);
        return text_result("Couldn't reach the Scribble2Fix app. Ask the user to start it.", true);
    }
    wait_for_scribble(&request.id)
}

fn launch(exe: &Path) -> bool {
    let mut cmd = Command::new(exe);
    cmd.args(["launch", "--request"]);
    detach(&mut cmd);
    cmd.status().is_ok_and(|s| s.success())
}

fn wait_until_running() -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        if storage::app_running() {
            return true;
        }
        thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Starts `exe args` in the background, detached from this process.
pub fn spawn(exe: &Path, args: &[&str]) -> io::Result<()> {
    let mut cmd = Command::new(exe);
    cmd.args(args);
    detach(&mut cmd);
    cmd.spawn().map(drop)
}

fn detach(cmd: &mut Command) {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
}

fn wait_for_scribble(id: &str) -> Value {
    let start = Instant::now();
    loop {
        if let Ok(s) = fs::read_to_string(storage::result_file(id)) {
            storage::clear_request(id);
            let result: Option<storage::RequestResult> = serde_json::from_str(&s).ok();
            return match result.and_then(|r| r.capture) {
                Some(dir) => capture_result(&dir),
                None => text_result(
                    "The user cancelled the scribble. Ask them how they'd like to continue.",
                    false,
                ),
            };
        }
        if storage::read_request().is_none_or(|r| r.id != id) {
            return text_result(
                "This scribble request is no longer open (it expired or was replaced). Ask the user \
                 before requesting a new one.",
                true,
            );
        }
        if start.elapsed() >= WAIT_PER_CALL {
            return text_result(
                &format!(
                    "The user hasn't finished the scribble yet. Call `wait_for_scribble` with \
                     request_id \"{id}\" to keep waiting."
                ),
                false,
            );
        }
        thread::sleep(Duration::from_millis(250));
    }
}

// ---------------------------------------------------------------- captures

/// Complete capture folders (meta.json is written last), newest first.
fn capture_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(storage::captures_dir())
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.join("meta.json").is_file())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort_by(|a, b| b.cmp(a));
    dirs
}

fn recent_captures(count: u64) -> Value {
    let dirs: Vec<PathBuf> = capture_dirs().into_iter().take(count.clamp(1, 10) as usize).collect();
    if dirs.is_empty() {
        return latest_capture();
    }
    let total = dirs.len();
    let mut content = vec![json!({ "type": "text", "text": format!(
        "{total} Scribble2Fix capture(s), newest first. Each has its own description and two images."
    ) })];
    for (i, dir) in dirs.iter().enumerate() {
        let mut part = capture_result(dir);
        if let Some(items) = part["content"].as_array_mut() {
            if let Some(text) = items.first_mut().and_then(|t| t.get_mut("text")) {
                *text = json!(format!(
                    "--- Capture {} of {total} ---\n{}",
                    i + 1,
                    text.as_str().unwrap_or("")
                ));
            }
            content.append(items);
        }
    }
    json!({ "content": content })
}

fn latest_capture() -> Value {
    match capture_dirs().first() {
        Some(dir) => capture_result(dir),
        None => text_result(
            "No Scribble2Fix capture found. Ask the user to press the Scribble2Fix hotkey (F6 by default), \
             draw on the screen and confirm with Enter, then try again.",
            true,
        ),
    }
}

fn capture_result(dir: &Path) -> Value {
    let meta: Value = fs::read_to_string(dir.join("meta.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let (Ok(annotated), Ok(clean)) = (fs::read(dir.join("annotated.png")), fs::read(dir.join("clean.png"))) else {
        return text_result(&format!("The capture in {} is incomplete.", dir.display()), true);
    };

    let age_minutes = meta["captured_at_unix_ms"]
        .as_i64()
        .map(|t| (now_ms() - t).max(0) / 60_000);

    let mut text = String::from("Scribble2Fix capture");
    if let Some(m) = age_minutes {
        text += &format!(", taken {}", describe_age(m));
    }
    if let Some(at) = meta["captured_at"].as_str() {
        text += &format!(" ({at})");
    }
    text += ".\n";
    let app = meta["app_name"].as_str().unwrap_or("");
    let title = meta["window_title"].as_str().unwrap_or("");
    if !app.is_empty() || !title.is_empty() {
        text += &format!("Window under the drawing: {app} - \"{title}\"\n");
    }
    text += "\n\
The user drew on a screenshot of their screen to request changes. Each color has a fixed meaning:\n\
- GREEN: add something new here\n\
- RED: remove this\n\
- BLUE: change or move this (an arrow means: move it from the arrow's start to where it points)\n\
\n\
Image 1 is the annotated screenshot (with a color legend strip at the bottom). Image 2 is the same \
area without the drawings, so you can see what is underneath the marks. The user may refer \
to numbered markers (circles with 1, 2, 3...) and typed text labels.\n\
\n\
Find the code that renders this UI and make the requested changes. If any mark is ambiguous, ask \
the user a clarifying question instead of guessing.\n";
    if let Some(marks) = meta["marks"].as_array().filter(|m| !m.is_empty()) {
        text += "\nThe marks as data (pixels in image 1, origin top left):\n";
        for m in marks {
            text += &format!("- {}\n", describe_mark(m));
        }
    }
    if age_minutes.is_some_and(|m| m >= STALE_AFTER_MINUTES) {
        text += "\nNote: this capture is more than 10 minutes old. Mention this to the user in case \
they meant to take a new one.\n";
    }

    let b64 = base64::engine::general_purpose::STANDARD;
    json!({
        "content": [
            { "type": "text", "text": text },
            { "type": "image", "mimeType": "image/png", "data": b64.encode(annotated) },
            { "type": "image", "mimeType": "image/png", "data": b64.encode(clean) },
        ]
    })
}

fn describe_mark(m: &Value) -> String {
    let meaning = m["meaning"].as_str().unwrap_or("?").to_uppercase();
    let pt = |v: &Value| format!("({}, {})", v[0], v[1]);
    match m["kind"].as_str().unwrap_or("") {
        "arrow" if m["via"].is_array() => format!(
            "{meaning} curved arrow from {} via {} to {}",
            pt(&m["from"]),
            pt(&m["via"]),
            pt(&m["to"])
        ),
        "arrow" => format!("{meaning} arrow from {} to {}", pt(&m["from"]), pt(&m["to"])),
        "text" => format!("{meaning} text label at {}: {}", pt(&m["at"]), m["text"]),
        "marker" => format!("{meaning} marker #{} at {}", m["number"], pt(&m["at"])),
        kind => {
            let b = &m["box"];
            let shape = if kind == "box" { "box" } else { "freehand mark" };
            format!(
                "{meaning} {shape} spanning ({}, {}) to ({}, {})",
                b[0], b[1], b[2], b[3]
            )
        }
    }
}

fn describe_age(minutes: i64) -> String {
    match minutes {
        0 => "just now".into(),
        1 => "1 minute ago".into(),
        m if m < 120 => format!("{m} minutes ago"),
        m if m < 48 * 60 => format!("{} hours ago", m / 60),
        m => format!("{} days ago", m / (24 * 60)),
    }
}

fn text_result(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}
