# Changelog

All notable changes to Scribble2Fix. Downloads for every version are on the [releases page](https://github.com/cweeperde/scribble2fix/releases).

## 1.1.5 (2026-11-22)

- macOS: when an AI tool asks for a scribble right after Scribble2Fix starts, the glowing frame goes around the whole screen and the request card shows up at once. Before, the frame covered only about half the screen until you pressed **Draw now**.

## 1.1.4 (2026-11-20)

### Smoother hotkey

Pressing the hotkey and closing the overlay no longer flickers: the frozen screen appears in one go, exactly like the real one, and disappears without a ghost image. The camera flash and zoom are gone.

### Hide private info

New tool **Hide** (`R`, in the folded part of the toolbar): drag over a password, an email or anything else the AI shouldn't see. While you draw it's blurred; in both images that are sent it's solid black, so nothing of the original is left to recover. The AI is told these areas are hidden on purpose and not to go looking for what was there.

### Also

- Taking a screenshot of your scribble with Win+Shift+S or ⌘⇧4 no longer refreezes the screen. Switching desktops still does.
- "Freeze when I start drawing" is gone: switching to another desktop or app before drawing already freezes the new screen.
- F6 is the default hotkey on every system (on a MacBook: Fn + F6). Existing settings stay as they are.
- Settings are split into tabs: General, AI tools, Captures, About. "Show welcome again" is now **Run setup again**.
- The welcome screen lets you pick the toolbar style and whether the hotkey pauses in fullscreen apps.
- The small shortcut letters sit neatly inside the toolbar buttons.

## 1.1.3 (2026-11-20)

- **Freeze when I start drawing** (Settings, off by default): the hotkey shows the frame and toolbar over the live screen, and the first click freezes the monitor you click on. Replaced in 1.1.4.
- Switching to another virtual desktop or Space while the screen is frozen, before drawing anything, freezes the new one instead of keeping the old picture up. Also from and to desktops without windows, and with Alt+Tab / ⌘+Tab.
- Windows: the "Copied" message and the AI request card show up on the desktop you're on, not only on the first one.

## 1.1.2 (2026-11-20)

### macOS

- The frozen screen and the frame cover the menu bar and the Dock instead of sliding under them.
- On MacBooks with a notch, the toolbar sits below it and the frame runs around the notch and the rounded screen corners.
- Scribble2Fix shows up on whichever desktop (Space) you're on, also over fullscreen apps.
- When you finish or cancel, the focus goes back to the app you were in, so ⌘V pastes right there.
- New installs used ⌘⇧2 as the hotkey (back to F6 in 1.1.4).
- Settings and the welcome window open in front of other apps.

### All platforms

- A capture names the app you actually drew on (the window under your marks), not just the app in front.
- The "Copied" message sits above the taskbar or Dock.

## 1.1.1 (2026-11-20)

- New, uniform icons for the AI tools.
- The Windows installer no longer creates a desktop shortcut by default.
- [Connect by hand](docs/manual-setup.md) has a copy-and-paste prompt that lets your AI agent set up Scribble2Fix in any MCP client.

## 1.1.0 (2026-11-20)

- New AI tools: GitHub Copilot, Cursor, Gemini CLI & Antigravity, OpenCode and Lumo. Claude Code and Claude Desktop share one switch.
- Connecting sets up everything a tool needs: the MCP server, the `/scribble` command or skill, and permission to use it without asking where the tool allows that.
- AI requests: when an AI asks for a scribble, a card drops in at the top of the screen with **Draw now** / **Not now**, a countdown, a glowing frame and an optional chime.
- New intro, send animation and "Copied" message.
- Settings: pick the monitor to capture and turn the request sound on or off. The welcome screen can connect all found tools at once.
- Connected tools get more details with each capture (system, monitor, drawing time, the AI's request).
- AI models without image support are told so and work from the marks as text.
- [Connect by hand](docs/manual-setup.md): setup for any MCP client.

## 1.0.0 (2026-11-13)

First release. Press a hotkey, the screen freezes, mark what should change in green (add), red (remove) or blue (change or move), press Enter. The annotated screenshot is copied for any chat, and connected AI tools (Claude Code, Claude Desktop, Codex) can read it through MCP, including every mark as text.
