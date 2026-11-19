# Connect Scribble2Fix by hand

Scribble2Fix sets itself up in one click (welcome screen or **Settings → AI tools**). Use this page if your tool isn't in that list, or if the automatic setup can't find it.

Scribble2Fix is an [MCP](https://modelcontextprotocol.io) server. Any AI tool that can run MCP servers can use it: the AI gets your latest scribble and can ask you for a new one.

- [1. Find the path](#1-find-the-path)
- [2. Any MCP client](#2-any-mcp-client)
- [3. Step by step for popular tools](#3-step-by-step-for-popular-tools)
- [4. Remove it again](#4-remove-it-again)

## 1. Find the path

Use the path where Scribble2Fix is installed:

| Installed with | Path |
|---|---|
| Windows | `%LOCALAPPDATA%\Scribble2Fix\scribble2fix.exe` |
| Windows (.msi) | `C:\Program Files\Scribble2Fix\scribble2fix.exe` |
| macOS | `/Applications/Scribble2Fix.app/Contents/MacOS/scribble2fix` |
| Linux (.deb / .rpm) | `/usr/bin/scribble2fix` |
| Linux (AppImage) | the full path of the `.AppImage` file |

In JSON and TOML files, write every `\` as `\\` (`C:\\Users\\you\\AppData\\Local\\Scribble2Fix\\scribble2fix.exe`). The examples below use `<path>` for it.

## 2. Any MCP client

Add a **local (stdio)** MCP server:

| Setting | Value |
|---|---|
| Name | `scribble2fix` |
| Command | `<path>` |
| Arguments | `mcp`, `--client`, `<your tool's name>` |
| Timeout | at least 60 seconds, if your tool has this setting |

- `--client` is optional. It's the name shown on the request card ("Claude asks you to show it"). Without it Scribble2Fix uses the name your tool reports.
- The timeout matters for `request_scribble`, which waits while you draw. It returns after 50 seconds at the latest and the AI then calls `wait_for_scribble`, but some tools stop a call much earlier (OpenCode after 5 seconds by default).
- If your tool asks before every tool call, you can allow Scribble2Fix permanently: its tools only read your captures, and a scribble request always shows you a card first.

Most tools that use an `mcpServers` JSON file take this:

```json
{
  "mcpServers": {
    "scribble2fix": {
      "command": "<path>",
      "args": ["mcp", "--client", "My AI"]
    }
  }
}
```

**The tools it offers**

| Tool | What it does |
|---|---|
| `get_latest_capture` | The newest scribble: annotated and clean screenshot, the app it was taken in, and every mark as text |
| `get_recent_captures` | The last few scribbles (default 3, up to 10) |
| `request_scribble` | Asks you to mark something on your screen and waits for it |
| `wait_for_scribble` | Keeps waiting for a scribble that's still being drawn |

**A `/scribble` command**: if your tool supports custom commands or prompts, create one with the text of [scribble.md](../app/src/scribble.md). Otherwise just ask *"look at my latest scribble"*.

**Models that can't see images**: the scribble is a screenshot. Scribble2Fix also sends every mark as text (kind, color, position, typed labels), and tells the AI to say so if it can't view images, so you know to describe the marks instead.

## 3. Step by step for popular tools

<details>
<summary><b>Claude Code</b> (CLI and the Code tab of the Claude desktop app)</summary>

Windows (PowerShell):

```powershell
claude mcp add --scope user scribble2fix -- "$env:LOCALAPPDATA\Scribble2Fix\scribble2fix.exe" mcp --client Claude
New-Item -ItemType Directory -Force "$HOME\.claude\commands" | Out-Null
Invoke-WebRequest https://raw.githubusercontent.com/cweeperde/scribble2fix/main/app/src/scribble.md -OutFile "$HOME\.claude\commands\scribble.md"
```

macOS (on Linux use `/usr/bin/scribble2fix`):

```sh
claude mcp add --scope user scribble2fix -- /Applications/Scribble2Fix.app/Contents/MacOS/scribble2fix mcp --client Claude
mkdir -p ~/.claude/commands
curl -fsSL https://raw.githubusercontent.com/cweeperde/scribble2fix/main/app/src/scribble.md -o ~/.claude/commands/scribble.md
```

To skip the permission prompt, add `"mcp__scribble2fix"` to `permissions.allow` in `~/.claude/settings.json`.
</details>

<details>
<summary><b>Claude Desktop</b> (chats)</summary>

Open **Settings → Developer → Edit Config**, add this under `mcpServers` and restart Claude:

```json
"scribble2fix": { "command": "<path>", "args": ["mcp", "--client", "Claude"] }
```
</details>

<details>
<summary><b>Codex</b> (CLI, desktop app, IDE extension)</summary>

```sh
codex mcp add scribble2fix -- <path> mcp --client Codex
```

Or add this to `~/.codex/config.toml` (`default_tools_approval_mode` skips the permission prompt):

```toml
[mcp_servers.scribble2fix]
command = "<path>"
args = ["mcp", "--client", "Codex"]
default_tools_approval_mode = "approve"
```

For the command, save [scribble.md](../app/src/scribble.md) as `~/.codex/prompts/scribble.md` and type `/prompts:scribble`.
</details>

<details>
<summary><b>GitHub Copilot</b> (Copilot CLI and Copilot in VS Code)</summary>

Add this to `~/.copilot/mcp-config.json`:

```json
{
  "mcpServers": {
    "scribble2fix": { "type": "stdio", "command": "<path>", "args": ["mcp", "--client", "Copilot"], "tools": ["*"] }
  }
}
```

For `/scribble`, save [scribble.md](../app/src/scribble.md) as `~/.copilot/skills/scribble/SKILL.md` and change its header to `name: scribble` and a `description:`.
</details>

<details>
<summary><b>Cursor</b></summary>

Add this to `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "scribble2fix": { "type": "stdio", "command": "<path>", "args": ["mcp", "--client", "Cursor"] }
  }
}
```

For `/scribble`, save [scribble.md](../app/src/scribble.md) as `~/.cursor/skills/scribble/SKILL.md` with `name: scribble` in its header.
</details>

<details>
<summary><b>Gemini CLI</b> and <b>Antigravity</b></summary>

Gemini CLI, in `~/.gemini/settings.json` (`trust` skips the permission prompt):

```json
{
  "mcpServers": {
    "scribble2fix": { "command": "<path>", "args": ["mcp", "--client", "Gemini"], "trust": true }
  }
}
```

Antigravity (IDE and `agy` CLI), in `~/.gemini/config/mcp_config.json`:

```json
{
  "mcpServers": {
    "scribble2fix": { "command": "<path>", "args": ["mcp", "--client", "Antigravity"] }
  }
}
```

For Gemini CLI's `/scribble`, create `~/.gemini/commands/scribble.toml`:

```toml
description = "Apply my latest Scribble2Fix screen markup"
prompt = """
Call the scribble2fix `get_latest_capture` tool and look at my latest capture. Green marks mean add, red means remove, blue means change or move. Apply the requested changes in this codebase. If a mark is ambiguous, ask me before guessing.

{{args}}
"""
```
</details>

<details>
<summary><b>OpenCode</b> (CLI and desktop app)</summary>

Add this to `~/.config/opencode/opencode.json` (on Windows `%USERPROFILE%\.config\opencode\opencode.json`). The `timeout` is important: OpenCode stops a tool call after 5 seconds by default.

```json
{
  "mcp": {
    "servers": {
      "scribble2fix": {
        "type": "local",
        "command": ["<path>", "mcp", "--client", "OpenCode"],
        "timeout": 120000
      }
    }
  },
  "permissions": [{ "action": "scribble2fix_*", "resource": "*", "effect": "allow" }]
}
```

For `/scribble`, save [scribble.md](../app/src/scribble.md) as `~/.config/opencode/commands/scribble.md` (remove its `argument-hint` line).
</details>

<details>
<summary><b>Lumo</b> (Proton's Lumo Desktop)</summary>

Sign in to Lumo Desktop, open **Customize → Connectors → Add MCP server**, choose a local (stdio) server with `<path>` as the command and `mcp --client Lumo` as the arguments.
</details>

<details>
<summary><b>Other tools</b> (Windsurf, Zed, LM Studio, Jan, ...)</summary>

Look for "MCP" in the tool's settings and add a local (stdio) server as described in [Any MCP client](#2-any-mcp-client). Where they usually live:

| Tool | Where |
|---|---|
| Windsurf | `~/.codeium/windsurf/mcp_config.json` (`mcpServers`) |
| Zed | its `settings.json`, under `context_servers` |
| LM Studio | **Program → Install → Edit mcp.json** (`mcpServers`) |
| Jan, Cherry Studio, Msty | their MCP settings page |
</details>

## 4. Remove it again

Switch the tool off in **Settings → AI tools** (that undoes everything the automatic setup did), or remove the `scribble2fix` entry you added: `claude mcp remove --scope user scribble2fix`, `codex mcp remove scribble2fix`, or delete it from the tool's config file.
