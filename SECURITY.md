# Security

Scribble2Fix takes screenshots of your screen, so it should never leak what's on it. If you find a way it does, please tell me privately first.

## Reporting a problem

Use **[Report a vulnerability](https://github.com/cweeperde/scribble2fix/security/advisories/new)** (the repository's Security tab). Only the maintainers see the report. Please don't open a public issue for it.

Helpful to include: the Scribble2Fix version (Settings → About), your operating system, the steps to reproduce, and what you expected instead. I'll answer as soon as I can and keep you posted until it's fixed.

## What counts

For example:

- A hidden area (the `R` tool) that can still be seen or recovered in a saved, copied or sent image, or in the text the AI gets.
- Captures, settings or the clipboard reaching anything other than your own computer, the AI tools you connected, or the GitHub update check.
- The MCP server giving an AI tool more than your saved captures and scribble requests, such as other files on your computer.
- Updates that could be replaced by someone else (the update check, its signature or the installers).

## Supported versions

Fixes go into the latest release. Automatic updates are on by default; you can also update under Settings → General → Check for updates.
