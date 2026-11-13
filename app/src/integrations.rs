//! Connects Scribble2Fix to AI tools: registers this executable (run as
//! `scribble2fix mcp`) as an MCP server and, where the tool supports it,
//! installs the `/scribble` command.
//!
//! - Claude Code (CLI, and the Code tab of the Claude desktop app): through
//!   `claude mcp add` when the CLI is on PATH, otherwise in `~/.claude.json`.
//! - Claude Desktop (chat): in its `claude_desktop_config.json`.
//! - Codex (CLI, IDE extension and desktop app all read the same file): in
//!   `~/.codex/config.toml`.
//!
//! Only tools the user picked (in the onboarding or settings) are connected.
//! Each registration is remembered, so it only runs again when the app's path
//! changes (e.g. after moving or reinstalling it).

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
    ClaudeCode,
    ClaudeDesktop,
    Codex,
}

pub struct Tool {
    /// Stored in the config's `tools` list.
    pub id: &'static str,
    pub name: &'static str,
    kind: Kind,
}

pub const TOOLS: [Tool; 3] = [
    Tool {
        id: "claude",
        name: "Claude Code",
        kind: Kind::ClaudeCode,
    },
    Tool {
        id: "claude-desktop",
        name: "Claude Desktop",
        kind: Kind::ClaudeDesktop,
    },
    Tool {
        id: "codex",
        name: "Codex",
        kind: Kind::Codex,
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
}

pub fn status(config: &storage::Config) -> Vec<ToolStatus> {
    let search_path = search_path();
    TOOLS
        .iter()
        .map(|t| ToolStatus {
            id: t.id,
            name: t.name,
            installed: is_installed(t, &search_path),
            connected: config.tools.iter().any(|c| c == t.id),
        })
        .collect()
}

fn tool(id: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == id)
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

fn is_installed(tool: &Tool, search_path: &str) -> bool {
    match tool.kind {
        Kind::ClaudeCode => home().join(".claude").is_dir() || find_cli("claude", search_path).is_some(),
        Kind::ClaudeDesktop => claude_desktop_config().is_some(),
        Kind::Codex => home().join(".codex").is_dir() || find_cli("codex", search_path).is_some(),
    }
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

/// Registers the MCP server and installs the slash command.
pub fn connect(id: &str) -> Result<(), String> {
    let tool = tool(id).ok_or("unknown tool")?;
    let exe = mcp_executable().ok_or("Move Scribble2Fix to a fixed location (e.g. Applications) first")?;
    let exe = exe.to_string_lossy().into_owned();

    match tool.kind {
        Kind::ClaudeCode => {
            let search_path = search_path();
            if let Some(cli) = find_cli("claude", &search_path) {
                let _ = run(&cli, CLAUDE_REMOVE, &search_path);
                let mut add = CLAUDE_ADD.to_vec();
                add.extend([exe.as_str(), "mcp"]);
                if !run(&cli, &add, &search_path) {
                    return Err("`claude mcp add` failed".into());
                }
            } else {
                let entry = json!({ "type": "stdio", "command": exe, "args": ["mcp"], "env": {} });
                set_json(&home().join(".claude.json"), Some(entry))?;
            }
            install_command(&home().join(".claude").join("commands"));
        }
        Kind::ClaudeDesktop => {
            let file = claude_desktop_config().ok_or("Claude Desktop's settings folder was not found")?;
            set_json(&file, Some(json!({ "command": exe, "args": ["mcp"] })))?;
        }
        Kind::Codex => {
            let file = home().join(".codex").join("config.toml");
            set_config_toml(&file, Some(&exe)).map_err(|e| format!("Couldn't update {}: {e}", file.display()))?;
            install_command(&home().join(".codex").join("prompts"));
        }
    }
    let _ = fs::create_dir_all(storage::data_dir());
    let _ = fs::write(stamp(tool), &exe);
    Ok(())
}

/// Unregisters the MCP server and removes the slash command.
pub fn disconnect(id: &str) -> Result<(), String> {
    let tool = tool(id).ok_or("unknown tool")?;
    match tool.kind {
        Kind::ClaudeCode => {
            let search_path = search_path();
            if let Some(cli) = find_cli("claude", &search_path) {
                let _ = run(&cli, CLAUDE_REMOVE, &search_path);
            }
            // Also clean up an entry written without the CLI.
            set_json(&home().join(".claude.json"), None)?;
            let _ = fs::remove_file(home().join(".claude").join("commands").join("scribble.md"));
        }
        Kind::ClaudeDesktop => {
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
    }
    let _ = fs::remove_file(stamp(tool));
    Ok(())
}

/// Sets (or with `None` removes) `mcpServers.scribble2fix` in a JSON config
/// file, keeping everything else (and its key order) as it is. A file that
/// isn't valid JSON is left alone.
fn set_json(file: &Path, entry: Option<Value>) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("Couldn't update {}: {e}", file.display());
    let old = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if entry.is_none() {
                return Ok(());
            }
            "{}".into()
        }
        Err(e) => return Err(fail(&e)),
    };
    let mut root: Value = serde_json::from_str(&old).map_err(|e| fail(&e))?;
    let root_map = root.as_object_mut().ok_or_else(|| fail(&"not a JSON object"))?;
    match entry {
        Some(entry) => {
            let servers = root_map.entry("mcpServers").or_insert_with(|| json!({}));
            servers
                .as_object_mut()
                .ok_or_else(|| fail(&"mcpServers is not an object"))?
                .insert("scribble2fix".into(), entry);
        }
        None => {
            let removed = root_map
                .get_mut("mcpServers")
                .and_then(Value::as_object_mut)
                .and_then(|s| s.remove("scribble2fix"));
            if removed.is_none() {
                return Ok(());
            }
        }
    }
    let new = serde_json::to_string_pretty(&root).map_err(|e| fail(&e))? + "\n";
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    }
    fs::write(file, new).map_err(|e| fail(&e))
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
        new.push_str(&format!("{TABLE}\ncommand = \"{exe}\"\nargs = [\"mcp\"]\n"));
    }
    if new != old {
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(file, new)?;
    }
    Ok(())
}

/// At startup: re-register connected tools whose registration points to
/// another path (the app was moved or reinstalled).
pub fn refresh(config: &storage::Config) {
    let Some(exe) = mcp_executable() else { return };
    let exe = exe.to_string_lossy().into_owned();
    for id in &config.tools {
        let Some(tool) = tool(id) else { continue };
        if fs::read_to_string(stamp(tool)).is_ok_and(|s| s == exe) {
            continue;
        }
        let _ = connect(id);
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

fn run(cli: &Path, args: &[&str], search_path: &str) -> bool {
    let mut cmd = Command::new(cli);
    cmd.args(args).env("PATH", search_path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output().is_ok_and(|o| o.status.success())
}

#[cfg(test)]
mod tests {
    use super::{set_config_toml, set_json};

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
            "\n\n[mcp_servers.scribble2fix]\ncommand = \"C:\\\\Apps\\\\Scribble2Fix\\\\scribble2fix.exe\"\nargs = [\"mcp\"]\n"
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
