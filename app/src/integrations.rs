//! Connects Scribble2Fix to AI tools: registers this executable (run as
//! `scribble2fix mcp --client <name>`) as an MCP server and, where the tool
//! supports it, installs the `/scribble` command (or a `scribble` skill where
//! skills replaced commands) and lets its tools run without asking.
//!
//! - Claude: Claude Code (CLI, and the Code tab of the Claude desktop app)
//!   through `claude mcp add` when the CLI is on PATH, otherwise in
//!   `~/.claude.json`; Claude Desktop chats in `claude_desktop_config.json`.
//! - Codex (CLI, IDE extension and desktop app all read the same file): in
//!   `~/.codex/config.toml`.
//! - GitHub Copilot (Copilot CLI, and Copilot in VS Code): in
//!   `~/.copilot/mcp-config.json`.
//! - Cursor: in `~/.cursor/mcp.json`.
//! - Gemini: Gemini CLI in `~/.gemini/settings.json`, Antigravity (IDE and
//!   `agy` CLI) in `~/.gemini/config/mcp_config.json`.
//! - OpenCode (CLI and desktop app share the global config): in
//!   `~/.config/opencode/opencode.json(c)`.
//! - Lumo Desktop (no CLI): in `accounts/<UserID>/mcp_servers.json` for every
//!   account signed in to the app. Lumo reads it at startup.
//!
//! JSON configs that may hold comments are edited in place, keeping them.
//! Only tools the user picked (in the onboarding or settings) are connected.
//! The tools only read captures, and a scribble request asks the user anyway,
//! so where the tool allows it they run without asking each time.
//! Each registration is remembered, so it only runs again when the app's path
//! changes (e.g. after moving or reinstalling it) or the setup gets extended.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Serialize;
use serde_json::{json, Value};

use crate::storage;

const SCRIBBLE_COMMAND: &str = include_str!("scribble.md");

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Claude,
    Codex,
    Copilot,
    Cursor,
    Gemini,
    OpenCode,
    Lumo,
}

pub struct Tool {
    /// Stored in the config's `tools` list.
    pub id: &'static str,
    pub name: &'static str,
    /// What connecting sets up, for the settings.
    setup: &'static str,
    kind: Kind,
}

/// Roughly by how widely they're used.
pub const TOOLS: [Tool; 7] = [
    Tool {
        id: "claude",
        name: "Claude",
        setup: "/scribble command and MCP server",
        kind: Kind::Claude,
    },
    Tool {
        id: "codex",
        name: "Codex",
        setup: "/scribble command and MCP server",
        kind: Kind::Codex,
    },
    Tool {
        id: "copilot",
        name: "GitHub Copilot",
        setup: "scribble skill and MCP server",
        kind: Kind::Copilot,
    },
    Tool {
        id: "cursor",
        name: "Cursor",
        setup: "/scribble skill and MCP server",
        kind: Kind::Cursor,
    },
    Tool {
        id: "gemini",
        name: "Gemini & Antigravity",
        setup: "/scribble command and MCP server",
        kind: Kind::Gemini,
    },
    Tool {
        id: "opencode",
        name: "OpenCode",
        setup: "/scribble command and MCP server",
        kind: Kind::OpenCode,
    },
    Tool {
        id: "lumo",
        name: "Lumo",
        setup: "MCP server",
        kind: Kind::Lumo,
    },
];

const CLAUDE_REMOVE: &[&str] = &["mcp", "remove", "--scope", "user", "scribble2fix"];
const CLAUDE_ADD: &[&str] = &["mcp", "add", "--scope", "user", "scribble2fix", "--"];

#[derive(Serialize)]
pub struct ToolStatus {
    pub id: &'static str,
    pub name: &'static str,
    pub installed: bool,
    pub connected: bool,
    /// The apps found, e.g. "Claude Code and Claude Desktop".
    pub found: String,
    pub setup: &'static str,
}

pub fn status(config: &storage::Config) -> Vec<ToolStatus> {
    let search_path = search_path();
    TOOLS
        .iter()
        .map(|t| {
            let parts = found(t, &search_path);
            ToolStatus {
                id: t.id,
                name: t.name,
                installed: !parts.is_empty(),
                connected: config.tools.iter().any(|c| c == t.id),
                found: parts.join(" and "),
                setup: t.setup,
            }
        })
        .collect()
}

fn tool(id: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == id)
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// The parts of a tool that are on this computer (empty: not installed).
fn found(tool: &Tool, search_path: &str) -> Vec<&'static str> {
    let any = |checks: &[bool]| checks.iter().any(|&c| c);
    let mut parts = vec![];
    match tool.kind {
        Kind::Claude => {
            if claude_code_installed(search_path) {
                parts.push("Claude Code");
            }
            if claude_desktop_config().is_some() {
                parts.push("Claude Desktop");
            }
        }
        Kind::Gemini => {
            if any(&[gemini_dir().join("settings.json").is_file(), find_cli("gemini", search_path).is_some()]) {
                parts.push("Gemini CLI");
            }
            if antigravity_installed(search_path) {
                parts.push("Antigravity");
            }
        }
        Kind::Codex => {
            if any(&[home().join(".codex").is_dir(), find_cli("codex", search_path).is_some()]) {
                parts.push("Codex");
            }
        }
        Kind::Copilot => {
            if copilot_installed(search_path) {
                parts.push("GitHub Copilot");
            }
        }
        Kind::Cursor => {
            if any(&[
                home().join(".cursor").is_dir(),
                find_cli("cursor", search_path).is_some(),
                app_installed(&["cursor"], &["Cursor.app"]),
            ]) {
                parts.push("Cursor");
            }
        }
        Kind::OpenCode => {
            if any(&[opencode_dir().is_dir(), find_cli("opencode", search_path).is_some()]) {
                parts.push("OpenCode");
            }
        }
        Kind::Lumo => {
            if lumo_installed(search_path) {
                parts.push("Lumo");
            }
        }
    }
    parts
}

/// A desktop app in its usual install folder: on Windows
/// `%LOCALAPPDATA%\Programs\<name>` (or `%LOCALAPPDATA%\<name>`), on macOS
/// `/Applications/<bundle>` or `~/Applications/<bundle>`.
fn app_installed(windows_dirs: &[&str], mac_bundles: &[&str]) -> bool {
    if cfg!(windows) {
        let Some(local) = dirs::data_local_dir() else { return false };
        windows_dirs
            .iter()
            .any(|d| local.join("Programs").join(d).is_dir() || local.join(d).is_dir())
    } else if cfg!(target_os = "macos") {
        mac_bundles
            .iter()
            .any(|b| Path::new("/Applications").join(b).is_dir() || home().join("Applications").join(b).is_dir())
    } else {
        false
    }
}

fn claude_code_installed(search_path: &str) -> bool {
    home().join(".claude").is_dir() || find_cli("claude", search_path).is_some()
}

/// Copilot CLI and VS Code's Copilot both read `$COPILOT_HOME`, by default `~/.copilot`.
fn copilot_dir() -> PathBuf {
    std::env::var_os("COPILOT_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".copilot"))
}

/// The Copilot CLI, its config folder, or the Copilot Chat extension in VS Code.
fn copilot_installed(search_path: &str) -> bool {
    if copilot_dir().is_dir() || find_cli("copilot", search_path).is_some() {
        return true;
    }
    [".vscode", ".vscode-insiders"].iter().any(|d| {
        fs::read_dir(home().join(d).join("extensions")).is_ok_and(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().to_lowercase().starts_with("github.copilot"))
        })
    })
}

fn gemini_dir() -> PathBuf {
    home().join(".gemini")
}

fn antigravity_installed(search_path: &str) -> bool {
    gemini_dir().join("config").join("mcp_config.json").is_file()
        || gemini_dir().join("antigravity").is_dir()
        || find_cli("agy", search_path).is_some()
        || find_cli("antigravity", search_path).is_some()
        || app_installed(&["Antigravity"], &["Antigravity.app"])
}

/// OpenCode's global config folder: `$XDG_CONFIG_HOME/opencode`, or
/// `~/.config/opencode` on every platform (Windows and macOS included).
fn opencode_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("opencode")
}

const LUMO_ID: &str = "me.proton.lumo-app";

/// Where Lumo Desktop keeps its settings: %APPDATA%\me.proton.lumo-app,
/// ~/Library/Application Support/me.proton.lumo-app, ~/.config/me.proton.lumo-app.
fn lumo_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(LUMO_ID))
}

/// Lumo Desktop's own folders. Its settings folder only appears once it saved
/// something, but its log folder exists from the first start.
fn lumo_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [dirs::config_dir(), dirs::data_local_dir(), dirs::data_dir()]
        .into_iter()
        .flatten()
        .map(|d| d.join(LUMO_ID))
        .collect();
    // macOS keeps logs in ~/Library/Logs/<id>.
    dirs.push(home().join("Library").join("Logs").join(LUMO_ID));
    dirs
}

fn lumo_installed(search_path: &str) -> bool {
    lumo_dirs().iter().any(|d| d.is_dir())
        || app_installed(&["Lumo"], &["Lumo.app"])
        || find_cli("lumo-desktop", search_path).is_some()
}

/// Lumo's account ids are its folder names: letters, digits, `-`, `_`, `=`.
fn valid_lumo_account(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || "-_=".contains(c))
}

/// Accounts signed in to Lumo: its account folders, plus the ones its log
/// mentions ("active account is now <id>"), since a folder only appears once
/// Lumo saved something for that account.
fn lumo_accounts() -> Vec<String> {
    let mut ids = vec![];
    if let Some(dir) = lumo_config_dir() {
        if let Ok(entries) = fs::read_dir(dir.join("accounts")) {
            ids.extend(
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned()),
            );
        }
    }
    const MARKER: &str = "active account is now ";
    for dir in lumo_dirs() {
        for logs in [dir.join("logs"), dir.clone()] {
            let Ok(entries) = fs::read_dir(&logs) else { continue };
            for entry in entries.filter_map(|e| e.ok()) {
                if !entry.file_name().to_string_lossy().ends_with(".log") {
                    continue;
                }
                let Ok(text) = fs::read_to_string(entry.path()) else { continue };
                for line in text.lines() {
                    if let Some(rest) = line.split(MARKER).nth(1) {
                        ids.extend(rest.split_whitespace().next().map(String::from));
                    }
                }
            }
        }
    }
    ids.retain(|id| valid_lumo_account(id));
    ids.sort();
    ids.dedup();
    ids
}

/// `mcp_servers.json` of each account signed in to Lumo Desktop.
fn lumo_configs() -> Vec<PathBuf> {
    let Some(dir) = lumo_config_dir() else { return vec![] };
    lumo_accounts()
        .iter()
        .map(|id| dir.join("accounts").join(id).join("mcp_servers.json"))
        .collect()
}

/// Claude Desktop's config file (it may not exist yet). Its location depends
/// on the platform and on how the app was installed; None if the app's data
/// folder isn't there.
fn claude_desktop_config() -> Option<PathBuf> {
    const FILE: &str = "claude_desktop_config.json";
    let mut candidates: Vec<PathBuf> = vec![];
    if cfg!(windows) {
        // Newer Store builds keep their data in %LOCALAPPDATA%\Claude-Data\<package id>.
        if let Some(local) = dirs::data_local_dir() {
            if let Ok(entries) = fs::read_dir(local.join("Claude-Data")) {
                candidates.extend(entries.filter_map(|e| e.ok()).map(|e| e.path().join(FILE)));
            }
            if let Ok(entries) = fs::read_dir(local.join("Packages")) {
                candidates.extend(
                    entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.file_name().to_string_lossy().starts_with("Claude_"))
                        .map(|e| e.path().join("LocalCache").join("Roaming").join("Claude").join(FILE)),
                );
            }
        }
    }
    // %APPDATA%\Claude, ~/Library/Application Support/Claude, ~/.config/Claude
    if let Some(config) = dirs::config_dir() {
        candidates.push(config.join("Claude").join(FILE));
    }
    candidates
        .iter()
        .find(|p| p.is_file())
        .or_else(|| candidates.iter().find(|p| p.parent().is_some_and(|d| d.is_dir())))
        .cloned()
}

fn stamp(tool: &Tool) -> PathBuf {
    storage::data_dir().join(format!("registered-{}.txt", tool.id))
}

/// Bumped when `connect` sets up more than before (2: tool permissions,
/// 3: the tool's name passed with `--client`, OpenCode timeout), so tools
/// connected with an older version get the new setup once.
const SETUP_VERSION: u32 = 3;

/// What the stamp file holds: the registered path, and the setup version
/// after it (older versions wrote only the path).
fn stamp_content(exe: &str) -> String {
    format!("{exe}\n{SETUP_VERSION}")
}

/// Arguments that start the MCP server. The name is shown on the request card.
fn mcp_args(client: &str) -> [&str; 3] {
    ["mcp", "--client", client]
}

/// The prompt of the `/scribble` command, without frontmatter and arguments.
fn command_body() -> String {
    let body = SCRIBBLE_COMMAND.splitn(3, "---").nth(2).unwrap_or(SCRIBBLE_COMMAND);
    body.replace("$ARGUMENTS", "").trim().to_string()
}

/// `scribble` skill, for tools where skills replaced custom commands.
fn install_skill(skills: &Path) {
    let dir = skills.join("scribble");
    let skill = format!(
        "---\nname: scribble\ndescription: Apply the user's latest Scribble2Fix screen markup (a screenshot \
         they drew on). Use when they mention their scribble, capture, markup or drawing on the screen.\n---\n{}\n",
        command_body()
    );
    if fs::create_dir_all(&dir).is_ok() {
        let _ = fs::write(dir.join("SKILL.md"), skill);
    }
}

fn remove_skill(skills: &Path) {
    let _ = fs::remove_dir_all(skills.join("scribble"));
}

/// Registers the MCP server and installs the slash command.
pub fn connect(id: &str) -> Result<(), String> {
    let tool = tool(id).ok_or("unknown tool")?;
    let exe = mcp_executable().ok_or("Move Scribble2Fix to a fixed location (e.g. Applications) first")?;
    let exe = exe.to_string_lossy().into_owned();
    let search_path = search_path();

    match tool.kind {
        Kind::Claude => {
            let code = claude_code_installed(&search_path);
            let desktop = claude_desktop_config();
            if !code && desktop.is_none() {
                return Err("Neither Claude Code nor Claude Desktop was found".into());
            }
            if code {
                if let Some(cli) = find_cli("claude", &search_path) {
                    let _ = run(&cli, CLAUDE_REMOVE, &search_path);
                    let mut add = CLAUDE_ADD.to_vec();
                    add.push(exe.as_str());
                    add.extend(mcp_args("Claude"));
                    if !run(&cli, &add, &search_path) {
                        return Err("`claude mcp add` failed".into());
                    }
                } else {
                    let entry = json!({ "type": "stdio", "command": exe, "args": mcp_args("Claude"), "env": {} });
                    set_json(&home().join(".claude.json"), Some(entry))?;
                }
                install_command(&home().join(".claude").join("commands"));
                set_claude_permission(&home().join(".claude").join("settings.json"), true)?;
            }
            if let Some(file) = desktop {
                set_json(&file, Some(json!({ "command": exe, "args": mcp_args("Claude") })))?;
            }
        }
        Kind::Codex => {
            let file = home().join(".codex").join("config.toml");
            set_config_toml(&file, Some(&exe)).map_err(|e| format!("Couldn't update {}: {e}", file.display()))?;
            install_command(&home().join(".codex").join("prompts"));
        }
        Kind::Copilot => {
            let entry = json!({ "type": "stdio", "command": exe, "args": mcp_args("Copilot"), "tools": ["*"] });
            set_jsonc_server(&copilot_dir().join("mcp-config.json"), Some(entry))?;
            install_skill(&copilot_dir().join("skills"));
        }
        Kind::Cursor => {
            let entry = json!({ "type": "stdio", "command": exe, "args": mcp_args("Cursor") });
            set_jsonc_server(&home().join(".cursor").join("mcp.json"), Some(entry))?;
            install_skill(&home().join(".cursor").join("skills"));
        }
        Kind::Gemini => {
            // `trust`: Gemini CLI runs its tools without asking.
            let entry = json!({ "command": exe, "args": mcp_args("Gemini"), "trust": true });
            set_jsonc_server(&gemini_dir().join("settings.json"), Some(entry))?;
            install_gemini_command(&gemini_dir().join("commands"));
            let entry = json!({ "command": exe, "args": mcp_args("Antigravity") });
            set_jsonc_server(&gemini_dir().join("config").join("mcp_config.json"), Some(entry.clone()))?;
            // Older Antigravity versions read this one; only touched if it's there.
            let legacy = gemini_dir().join("antigravity").join("mcp_config.json");
            if legacy.is_file() {
                set_jsonc_server(&legacy, Some(entry))?;
            }
        }
        Kind::OpenCode => {
            let dir = opencode_dir();
            // Use the file the user already has; OpenCode reads both.
            let (jsonc, json) = (dir.join("opencode.jsonc"), dir.join("opencode.json"));
            let (target, other) = if jsonc.is_file() || !json.is_file() { (jsonc, json) } else { (json, jsonc) };
            set_opencode(&other, None)?;
            set_opencode(&target, Some(&exe))?;
            // OpenCode's command frontmatter has no `argument-hint`.
            let command: String = SCRIBBLE_COMMAND
                .lines()
                .filter(|l| !l.starts_with("argument-hint:"))
                .map(|l| format!("{l}\n"))
                .collect();
            let commands = dir.join("commands");
            if fs::create_dir_all(&commands).is_ok() {
                let _ = fs::write(commands.join("scribble.md"), command);
            }
            reload_opencode(&search_path);
        }
        Kind::Lumo => {
            let files = lumo_configs();
            if files.is_empty() {
                return Err("Sign in to Lumo Desktop first, then try again".into());
            }
            for file in files {
                set_lumo(&file, Some(&exe))?;
            }
        }
    }
    let _ = fs::create_dir_all(storage::data_dir());
    let _ = fs::write(stamp(tool), stamp_content(&exe));
    Ok(())
}

/// Unregisters the MCP server and removes the slash command (or skill).
pub fn disconnect(id: &str) -> Result<(), String> {
    let tool = tool(id).ok_or("unknown tool")?;
    match tool.kind {
        Kind::Claude => {
            let search_path = search_path();
            if let Some(cli) = find_cli("claude", &search_path) {
                let _ = run(&cli, CLAUDE_REMOVE, &search_path);
            }
            // Also clean up an entry written without the CLI.
            set_json(&home().join(".claude.json"), None)?;
            let _ = fs::remove_file(home().join(".claude").join("commands").join("scribble.md"));
            set_claude_permission(&home().join(".claude").join("settings.json"), false)?;
            if let Some(file) = claude_desktop_config() {
                set_json(&file, None)?;
            }
        }
        Kind::Codex => {
            let file = home().join(".codex").join("config.toml");
            if file.exists() {
                set_config_toml(&file, None).map_err(|e| format!("Couldn't update {}: {e}", file.display()))?;
            }
            let _ = fs::remove_file(home().join(".codex").join("prompts").join("scribble.md"));
        }
        Kind::Copilot => {
            set_jsonc_server(&copilot_dir().join("mcp-config.json"), None)?;
            remove_skill(&copilot_dir().join("skills"));
        }
        Kind::Cursor => {
            set_jsonc_server(&home().join(".cursor").join("mcp.json"), None)?;
            remove_skill(&home().join(".cursor").join("skills"));
        }
        Kind::Gemini => {
            set_jsonc_server(&gemini_dir().join("settings.json"), None)?;
            set_jsonc_server(&gemini_dir().join("config").join("mcp_config.json"), None)?;
            set_jsonc_server(&gemini_dir().join("antigravity").join("mcp_config.json"), None)?;
            let _ = fs::remove_file(gemini_dir().join("commands").join("scribble.toml"));
        }
        Kind::OpenCode => {
            let dir = opencode_dir();
            set_opencode(&dir.join("opencode.jsonc"), None)?;
            set_opencode(&dir.join("opencode.json"), None)?;
            let _ = fs::remove_file(dir.join("commands").join("scribble.md"));
            reload_opencode(&search_path());
        }
        Kind::Lumo => {
            for file in lumo_configs() {
                set_lumo(&file, None)?;
            }
        }
    }
    let _ = fs::remove_file(stamp(tool));
    Ok(())
}

/// Gemini CLI's `/scribble`: a TOML file; `{{args}}` is what's typed after it.
fn install_gemini_command(dir: &Path) {
    let toml = format!(
        "description = \"Apply my latest Scribble2Fix screen markup\"\nprompt = \"\"\"\n{}\n\n{{{{args}}}}\n\"\"\"\n",
        command_body()
    );
    if fs::create_dir_all(dir).is_ok() {
        let _ = fs::write(dir.join("scribble.toml"), toml);
    }
}

/// Edits a JSON config file in place, keeping everything else (and its key
/// order) as it is. A missing file is only created when `create` is set, and
/// the file is only written when something changed. A file that isn't valid
/// JSON is left alone.
fn edit_json(
    file: &Path,
    create: bool,
    edit: impl FnOnce(&mut serde_json::Map<String, Value>) -> Result<(), &'static str>,
) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("Couldn't update {}: {e}", file.display());
    let old = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !create {
                return Ok(());
            }
            "{}".into()
        }
        Err(e) => return Err(fail(&e)),
    };
    let mut root: Value = serde_json::from_str(&old).map_err(|e| fail(&e))?;
    let before = root.clone();
    edit(root.as_object_mut().ok_or_else(|| fail(&"not a JSON object"))?).map_err(|e| fail(&e))?;
    if root == before {
        return Ok(());
    }
    let new = serde_json::to_string_pretty(&root).map_err(|e| fail(&e))? + "\n";
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    }
    fs::write(file, new).map_err(|e| fail(&e))
}

/// Sets (or with `None` removes) `mcpServers.scribble2fix` in a JSON config.
fn set_json(file: &Path, entry: Option<Value>) -> Result<(), String> {
    let create = entry.is_some();
    edit_json(file, create, |root| {
        match entry {
            Some(entry) => {
                root.entry("mcpServers")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or("mcpServers is not an object")?
                    .insert("scribble2fix".into(), entry);
            }
            None => {
                if let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) {
                    servers.remove("scribble2fix");
                }
            }
        }
        Ok(())
    })
}

/// Converts JSON into the input of the comment-keeping JSONC editor.
fn to_cst(value: &Value) -> jsonc_parser::cst::CstInputValue {
    use jsonc_parser::cst::CstInputValue as C;
    match value {
        Value::Null => C::Null,
        Value::Bool(b) => C::Bool(*b),
        Value::Number(n) => C::Number(n.to_string()),
        Value::String(s) => C::String(s.clone()),
        Value::Array(a) => C::Array(a.iter().map(to_cst).collect()),
        Value::Object(o) => C::Object(o.iter().map(|(k, v)| (k.clone(), to_cst(v))).collect()),
    }
}

/// Sets (or with `None` removes) `mcpServers.scribble2fix` in a JSON config
/// that may contain comments (Copilot, Cursor, Gemini), keeping them and the
/// formatting. A file that can't be parsed is left alone.
fn set_jsonc_server(file: &Path, entry: Option<Value>) -> Result<(), String> {
    use jsonc_parser::{cst::CstRootNode, ParseOptions};
    let fail = |e: &dyn std::fmt::Display| format!("Couldn't update {}: {e}", file.display());
    let old = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if entry.is_none() {
                return Ok(());
            }
            "{}\n".into()
        }
        Err(e) => return Err(fail(&e)),
    };
    let root = CstRootNode::parse(&old, &ParseOptions::default()).map_err(|e| fail(&e))?;
    let obj = root.object_value_or_create().ok_or_else(|| fail(&"not a JSON object"))?;
    let servers = obj.object_value("mcpServers");
    let existing = servers.as_ref().and_then(|s| s.get("scribble2fix"));
    match (entry, existing) {
        // Replace in place, so it keeps its position.
        (Some(entry), Some(prop)) => prop.set_value(to_cst(&entry)),
        (Some(entry), None) => {
            let servers = match servers {
                Some(s) => s,
                None => obj.object_value_or_create("mcpServers").ok_or_else(|| fail(&"mcpServers is not an object"))?,
            };
            servers.append("scribble2fix", to_cst(&entry));
        }
        (None, Some(prop)) => prop.remove(),
        (None, None) => {}
    }
    let new = root.to_string();
    if new != old {
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).map_err(|e| fail(&e))?;
        }
        fs::write(file, new).map_err(|e| fail(&e))?;
    }
    Ok(())
}

/// Claude Code permission rule that lets every Scribble2Fix tool run without
/// asking (they only read captures, and a scribble request asks the user anyway).
const CLAUDE_PERMISSION: &str = "mcp__scribble2fix";

/// Adds (or removes) `CLAUDE_PERMISSION` in `permissions.allow` of Claude
/// Code's user `settings.json`.
fn set_claude_permission(file: &Path, allow: bool) -> Result<(), String> {
    edit_json(file, allow, |root| {
        let permissions = root
            .entry("permissions")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or("permissions is not an object")?;
        let list = permissions
            .entry("allow")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or("permissions.allow is not a list")?;
        list.retain(|r| r != CLAUDE_PERMISSION);
        if allow {
            list.push(CLAUDE_PERMISSION.into());
        } else {
            // Don't leave empty containers behind that we created.
            if list.is_empty() {
                permissions.remove("allow");
            }
            if permissions.is_empty() {
                root.remove("permissions");
            }
        }
        Ok(())
    })
}

/// Replaces (or with `None` removes) the `[mcp_servers.scribble2fix]` table in
/// a Codex `config.toml`, leaving everything else as it is.
fn set_config_toml(file: &Path, exe: Option<&str>) -> std::io::Result<()> {
    const TABLE: &str = "[mcp_servers.scribble2fix]";
    let old = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut new = String::new();
    let mut skipping = false;
    for line in old.split_inclusive('\n') {
        let header = line.trim();
        if header.starts_with('[') {
            skipping = header == TABLE || header.starts_with("[mcp_servers.scribble2fix.");
        }
        if !skipping {
            new.push_str(line);
        }
    }
    if let Some(exe) = exe {
        // One blank line before the table.
        let trimmed = new.trim_end().len();
        new.truncate(trimmed);
        if !new.is_empty() {
            new.push_str("\n\n");
        }
        let exe = exe.replace('\\', "\\\\").replace('"', "\\\"");
        // `approve`: its tools run without asking (they only read captures,
        // and a scribble request asks the user anyway).
        new.push_str(&format!(
            "{TABLE}\ncommand = \"{exe}\"\nargs = [\"mcp\", \"--client\", \"Codex\"]\ndefault_tools_approval_mode = \"approve\"\n"
        ));
    }
    if new != old {
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(file, new)?;
    }
    Ok(())
}

/// Sets (or with `None` removes) the `scribble2fix` MCP server in an OpenCode
/// config, keeping comments and formatting. OpenCode 2 lists servers under
/// `mcp.servers`, OpenCode 1 directly under `mcp`; a config that already has
/// 1.x-style entries gets one too. A file that can't be parsed is left alone.
const OPENCODE_PERMISSION: &str = "scribble2fix_*";

fn set_opencode(file: &Path, exe: Option<&str>) -> Result<(), String> {
    use jsonc_parser::{
        cst::{CstInputValue, CstRootNode},
        ParseOptions,
    };
    let fail = |e: &dyn std::fmt::Display| format!("Couldn't update {}: {e}", file.display());
    let old = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if exe.is_none() {
                return Ok(());
            }
            "{\n  \"$schema\": \"https://opencode.ai/config.json\"\n}\n".into()
        }
        Err(e) => return Err(fail(&e)),
    };
    let root = CstRootNode::parse(&old, &ParseOptions::default()).map_err(|e| fail(&e))?;
    let obj = root.object_value_or_create().ok_or_else(|| fail(&"not a JSON object"))?;
    let mcp = obj.object_value("mcp");
    if let Some(mcp) = &mcp {
        if let Some(p) = mcp.object_value("servers").and_then(|s| s.get("scribble2fix")) {
            p.remove();
        }
        if let Some(p) = mcp.get("scribble2fix") {
            p.remove();
        }
    }
    // OpenCode 2 permission rule (`permissions`, last match wins).
    let is_rule = |n: &jsonc_parser::cst::CstNode| {
        n.as_object()
            .and_then(|o| o.get("action"))
            .and_then(|p| p.value())
            .and_then(|v| v.as_string_lit())
            .and_then(|s| s.decoded_value().ok())
            .is_some_and(|a| a == OPENCODE_PERMISSION)
    };
    if let Some(rules) = obj.array_value("permissions") {
        for rule in rules.elements().into_iter().filter(is_rule) {
            rule.remove();
        }
    }
    if let Some(exe) = exe {
        let mcp = match mcp {
            Some(mcp) => mcp,
            None => obj.object_value_or_create("mcp").ok_or_else(|| fail(&"mcp is not an object"))?,
        };
        let v1 = mcp.properties().iter().any(|p| p.object_value().is_some_and(|o| o.get("type").is_some()));
        let servers = if v1 {
            mcp
        } else {
            mcp.object_value_or_create("servers").ok_or_else(|| fail(&"mcp.servers is not an object"))?
        };
        servers.append(
            "scribble2fix",
            CstInputValue::Object(vec![
                ("type".into(), "local".into()),
                (
                    "command".into(),
                    CstInputValue::Array(
                        std::iter::once(exe).chain(mcp_args("OpenCode")).map(Into::into).collect(),
                    ),
                ),
                // OpenCode gives up on a tool call after 5 s by default, but
                // `request_scribble` waits while the user draws.
                ("timeout".into(), CstInputValue::Number("120000".into())),
            ]),
        );
        // Let its tools run without asking (only OpenCode 2 has `permissions`).
        if !v1 {
            obj.array_value_or_create("permissions")
                .ok_or_else(|| fail(&"permissions is not a list"))?
                .append(CstInputValue::Object(vec![
                    ("action".into(), OPENCODE_PERMISSION.into()),
                    ("resource".into(), "*".into()),
                    ("effect".into(), "allow".into()),
                ]));
        }
    } else if obj.array_value("permissions").is_some_and(|r| r.elements().is_empty()) {
        // Don't leave an empty list behind that we created.
        if let Some(p) = obj.get("permissions") {
            p.remove();
        }
    }
    let new = root.to_string();
    if new != old {
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).map_err(|e| fail(&e))?;
        }
        fs::write(file, new).map_err(|e| fail(&e))?;
    }
    Ok(())
}

/// Sets (or with `None` removes) the `scribble2fix` entry in a Lumo Desktop
/// `mcp_servers.json` (`{"servers": [...]}`), keeping the other servers.
fn set_lumo(file: &Path, exe: Option<&str>) -> Result<(), String> {
    edit_json(file, exe.is_some(), |root| {
        let servers = root
            .entry("servers")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or("servers is not a list")?;
        // Keep its place in the list when it's already there.
        let entry = exe.map(|exe| {
            json!({
                "id": "scribble2fix",
                "display_name": "Scribble2Fix",
                "description": "Screenshots the user marked up on their screen",
                "transport": "stdio",
                "command": exe,
                "args": mcp_args("Lumo"),
                "env": {},
            })
        });
        match (servers.iter().position(|s| s["id"] == "scribble2fix"), entry) {
            (Some(i), Some(entry)) => servers[i] = entry,
            (Some(i), None) => {
                servers.remove(i);
            }
            (None, Some(entry)) => servers.push(entry),
            (None, None) => {}
        }
        Ok(())
    })
}

/// At startup: re-register connected tools whose registration points to
/// another path (the app was moved or reinstalled). Lumo always, to cover
/// accounts signed in since the last start.
pub fn refresh(config: &storage::Config) {
    // Version 1.0 registered Claude Desktop separately; it's part of "claude" now.
    let _ = fs::remove_file(storage::data_dir().join("registered-claude-desktop.txt"));
    let Some(exe) = mcp_executable() else { return };
    let exe = exe.to_string_lossy().into_owned();
    for id in &config.tools {
        let Some(tool) = tool(id) else { continue };
        if tool.kind != Kind::Lumo && fs::read_to_string(stamp(tool)).is_ok_and(|s| s == stamp_content(&exe)) {
            // An update or reinstall ends the MCP servers OpenCode's background
            // service started, and it doesn't restart them by itself.
            if tool.kind == Kind::OpenCode {
                reload_opencode(&search_path());
            }
            continue;
        }
        let _ = connect(id);
    }
}

/// OpenCode 2 keeps a background service that holds the config and the MCP
/// servers. Tell it to reload, so a changed config or a Scribble2Fix server
/// that was ended (by an update) gets connected again. Only when the service
/// is running: `reload` shouldn't start one.
fn reload_opencode(search_path: &str) {
    let Some(cli) = find_cli("opencode", search_path) else { return };
    let running = command(&cli, &["service", "status"], search_path)
        .output()
        .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("http"));
    if running {
        let _ = run(&cli, &["reload"], search_path);
    }
}

/// The path other tools should launch. Returns None where that path isn't
/// stable (macOS App Translocation, or a temporary AppImage mount).
pub fn mcp_executable() -> Option<PathBuf> {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return Some(PathBuf::from(appimage));
    }
    let exe = std::env::current_exe().ok()?;
    if exe.to_string_lossy().contains("/AppTranslocation/") {
        return None;
    }
    Some(exe)
}

fn install_command(dir: &Path) {
    if fs::create_dir_all(dir).is_ok() {
        let _ = fs::write(dir.join("scribble.md"), SCRIBBLE_COMMAND);
    }
}

/// Apps started from the Dock/launcher get a minimal PATH, which usually
/// misses where `claude`/`codex` (and `node`) are installed. Use the login
/// shell's PATH plus the common install locations.
#[cfg(not(windows))]
fn search_path() -> String {
    let mut entries: Vec<String> = vec![];
    if let Ok(shell) = std::env::var("SHELL") {
        if let Ok(out) = Command::new(shell).args(["-lc", "printf %s \"$PATH\""]).output() {
            entries.extend(String::from_utf8_lossy(&out.stdout).split(':').map(String::from));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        entries.extend(std::env::split_paths(&path).map(|p| p.to_string_lossy().into_owned()));
    }
    if let Some(home) = dirs::home_dir() {
        for sub in [".local/bin", ".claude/local", ".npm-global/bin", ".bun/bin"] {
            entries.push(home.join(sub).to_string_lossy().into_owned());
        }
    }
    entries.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(String::from));
    entries.retain(|d| !d.is_empty());
    entries.dedup();
    entries.join(":")
}

#[cfg(windows)]
fn search_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

fn find_cli(name: &str, search_path: &str) -> Option<PathBuf> {
    // On Windows npm installs `claude.cmd` / `codex.cmd`, the native installers `.exe`.
    let names: &[String] = if cfg!(windows) {
        &[format!("{name}.exe"), format!("{name}.cmd")]
    } else {
        &[name.to_string()]
    };
    std::env::split_paths(search_path)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| p.is_file())
}

/// A CLI call without a console window popping up on Windows.
fn command(cli: &Path, args: &[&str], search_path: &str) -> Command {
    let mut cmd = Command::new(cli);
    cmd.args(args).env("PATH", search_path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn run(cli: &Path, args: &[&str], search_path: &str) -> bool {
    command(cli, args, search_path).output().is_ok_and(|o| o.status.success())
}

#[cfg(test)]
mod tests {
    use super::{
        command_body, set_claude_permission, set_config_toml, set_json, set_jsonc_server, set_lumo, set_opencode,
        valid_lumo_account,
    };

    #[test]
    fn jsonc_server_keeps_comments_and_position() {
        let file = std::env::temp_dir().join(format!("scribble2fix-test-jsonc-{}.json", std::process::id()));
        let original = "{\n  // theme\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"a\": { \"command\": \"x\" }\n  }\n}\n";
        std::fs::write(&file, original).unwrap();
        set_jsonc_server(&file, Some(serde_json::json!({ "command": r"C:\s.exe", "args": ["mcp"], "trust": true }))).unwrap();
        set_jsonc_server(&file, Some(serde_json::json!({ "command": "/new", "args": ["mcp"] }))).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("// theme"));
        assert_eq!(text.matches("scribble2fix").count(), 1);
        let v = parse_jsonc(&text);
        assert_eq!(v["mcpServers"]["scribble2fix"]["command"], "/new");
        assert_eq!(v["mcpServers"]["a"]["command"], "x");
        set_jsonc_server(&file, None).unwrap();
        assert_eq!(parse_jsonc(&std::fs::read_to_string(&file).unwrap()), parse_jsonc(original));
        // Missing file: created on connect, left missing on disconnect.
        let _ = std::fs::remove_file(&file);
        set_jsonc_server(&file, None).unwrap();
        assert!(!file.exists());
        set_jsonc_server(&file, Some(serde_json::json!({ "command": "x" }))).unwrap();
        assert_eq!(parse_jsonc(&std::fs::read_to_string(&file).unwrap())["mcpServers"]["scribble2fix"]["command"], "x");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn lumo_account_ids_and_command_body() {
        assert!(valid_lumo_account("zqvrSYpNUFhRJOSZ36IgMmsRYD9-zuE8Qm7cepG1b1KP5Qd79P6Zp3J2loGlHu_Fqn_xtKq3PblTLsH6NQAxKA=="));
        assert!(!valid_lumo_account("../evil") && !valid_lumo_account("") && !valid_lumo_account("a b"));
        let body = command_body();
        assert!(body.starts_with("Call the scribble2fix") && !body.contains("$ARGUMENTS") && !body.contains("---"));
    }

    #[test]
    fn claude_permission_add_remove() {
        let file = std::env::temp_dir().join(format!("scribble2fix-test-perm-{}.json", std::process::id()));
        let original = r#"{"model":"x","permissions":{"allow":["Bash(ls)"],"deny":[]}}"#;
        std::fs::write(&file, original).unwrap();
        set_claude_permission(&file, true).unwrap();
        set_claude_permission(&file, true).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["permissions"]["allow"], serde_json::json!(["Bash(ls)", "mcp__scribble2fix"]));
        set_claude_permission(&file, false).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v, serde_json::from_str::<serde_json::Value>(original).unwrap());

        // Created from nothing, and removed again without leftovers.
        let _ = std::fs::remove_file(&file);
        set_claude_permission(&file, false).unwrap();
        assert!(!file.exists());
        std::fs::write(&file, "{}").unwrap();
        set_claude_permission(&file, true).unwrap();
        set_claude_permission(&file, false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "{}");
        let _ = std::fs::remove_file(&file);
    }

    fn parse_jsonc(s: &str) -> serde_json::Value {
        jsonc_parser::parse_to_serde_value(s, &Default::default()).unwrap()
    }

    #[test]
    fn opencode_config_keeps_comments() {
        let file = std::env::temp_dir().join(format!("scribble2fix-test-{}.jsonc", std::process::id()));
        let original = "{\n  // my model\n  \"model\": \"x\",\n  \"mcp\": { \"servers\": { \"other\": { \"type\": \"remote\", \"url\": \"u\" } } },\n}\n";
        std::fs::write(&file, original).unwrap();

        set_opencode(&file, Some(r"C:\a b\s.exe")).unwrap();
        set_opencode(&file, Some(r"C:\a b\s.exe")).unwrap(); // no duplicate
        let added = std::fs::read_to_string(&file).unwrap();
        assert!(added.contains("// my model"));
        let v = parse_jsonc(&added);
        assert_eq!(
            v["mcp"]["servers"]["scribble2fix"]["command"],
            serde_json::json!([r"C:\a b\s.exe", "mcp", "--client", "OpenCode"])
        );
        assert_eq!(v["mcp"]["servers"]["scribble2fix"]["type"], "local");
        assert_eq!(v["mcp"]["servers"]["scribble2fix"]["timeout"], 120000);
        assert_eq!(v["mcp"]["servers"]["other"]["url"], "u");
        assert_eq!(
            v["permissions"],
            serde_json::json!([{ "action": "scribble2fix_*", "resource": "*", "effect": "allow" }])
        );
        assert_eq!(added.matches("\"scribble2fix\"").count(), 1);

        set_opencode(&file, None).unwrap();
        let removed = std::fs::read_to_string(&file).unwrap();
        assert!(removed.contains("// my model"));
        assert_eq!(parse_jsonc(&removed), parse_jsonc(original));

        // OpenCode 1.x layout: servers directly under `mcp`.
        std::fs::write(&file, r#"{"mcp": {"other": {"type": "local", "command": ["a"]}}}"#).unwrap();
        set_opencode(&file, Some("/x")).unwrap();
        let v = parse_jsonc(&std::fs::read_to_string(&file).unwrap());
        assert_eq!(v["mcp"]["scribble2fix"]["command"][0], "/x");
        assert!(v["mcp"].get("servers").is_none());

        // New file, and broken files are left alone.
        let _ = std::fs::remove_file(&file);
        set_opencode(&file, None).unwrap();
        assert!(!file.exists());
        set_opencode(&file, Some("/x")).unwrap();
        let v = parse_jsonc(&std::fs::read_to_string(&file).unwrap());
        assert_eq!(v["mcp"]["servers"]["scribble2fix"]["command"][0], "/x");
        std::fs::write(&file, "{ not json").unwrap();
        assert!(set_opencode(&file, Some("/x")).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn lumo_config_add_remove_keeps_the_rest() {
        let file = std::env::temp_dir().join(format!("scribble2fix-test-lumo-{}.json", std::process::id()));
        let original = r#"{"servers":[{"id":"other","display_name":"Other","transport":"http","url":"u"}]}"#;
        std::fs::write(&file, original).unwrap();

        set_lumo(&file, Some(r"C:\s.exe")).unwrap();
        set_lumo(&file, Some(r"C:\s.exe")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let servers = v["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[1]["transport"], "stdio");
        assert_eq!(servers[1]["command"], r"C:\s.exe");

        set_lumo(&file, None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v, serde_json::from_str::<serde_json::Value>(original).unwrap());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn json_config_add_remove_keeps_the_rest() {
        let file = std::env::temp_dir().join(format!("scribble2fix-test-{}.json", std::process::id()));
        let original = r#"{"zeta":1,"preferences":{"b":true,"a":false},"mcpServers":{"other":{"command":"x"}}}"#;
        std::fs::write(&file, original).unwrap();

        set_json(
            &file,
            Some(serde_json::json!({ "command": "C:\\a b\\s.exe", "args": ["mcp"] })),
        )
        .unwrap();
        let added: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(added["mcpServers"]["scribble2fix"]["command"], "C:\\a b\\s.exe");
        assert_eq!(added["mcpServers"]["other"]["command"], "x");
        // Key order is kept.
        let keys: Vec<_> = added.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["zeta", "preferences", "mcpServers"]);
        let pref_keys: Vec<_> = added["preferences"].as_object().unwrap().keys().cloned().collect();
        assert_eq!(pref_keys, ["b", "a"]);

        set_json(&file, None).unwrap();
        let removed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(removed, serde_json::from_str::<serde_json::Value>(original).unwrap());

        // Broken JSON is never overwritten.
        std::fs::write(&file, "{ not json").unwrap();
        assert!(set_json(&file, Some(serde_json::json!({}))).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");

        // A missing file is created on connect and left missing on disconnect.
        let _ = std::fs::remove_file(&file);
        set_json(&file, None).unwrap();
        assert!(!file.exists());
        set_json(&file, Some(serde_json::json!({ "command": "x" }))).unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains("scribble2fix"));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn codex_config_toml_add_replace_remove() {
        let file = std::env::temp_dir().join(format!("scribble2fix-test-{}.toml", std::process::id()));
        let original =
            "model = 'x'\n\n[mcp_servers.other]\ncommand = 'a'\n\n[projects.'c:\\foo']\ntrust_level = \"trusted\"\n";
        std::fs::write(&file, original).unwrap();

        set_config_toml(&file, Some(r#"C:\Apps\Scribble2Fix\scribble2fix.exe"#)).unwrap();
        let added = std::fs::read_to_string(&file).unwrap();
        assert!(added.starts_with(original.trim_end()));
        assert!(added.ends_with(
            "\n\n[mcp_servers.scribble2fix]\ncommand = \"C:\\\\Apps\\\\Scribble2Fix\\\\scribble2fix.exe\"\nargs = [\"mcp\", \"--client\", \"Codex\"]\ndefault_tools_approval_mode = \"approve\"\n"
        ));

        // Registering again replaces the table (and any sub-table) instead of duplicating it.
        let with_env = added + "\n[mcp_servers.scribble2fix.env]\nX = '1'\n";
        std::fs::write(&file, with_env).unwrap();
        set_config_toml(&file, Some("/new/path")).unwrap();
        let replaced = std::fs::read_to_string(&file).unwrap();
        assert_eq!(replaced.matches("[mcp_servers.scribble2fix").count(), 1);
        assert!(replaced.contains("command = \"/new/path\""));

        set_config_toml(&file, None).unwrap();
        let removed = std::fs::read_to_string(&file).unwrap();
        assert_eq!(removed.trim_end(), original.trim_end());
        assert!(removed.contains("[mcp_servers.other]") && removed.contains("trust_level"));
        let _ = std::fs::remove_file(&file);
    }
}
