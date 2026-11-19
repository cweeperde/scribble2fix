// Stand-in for the Tauri API so the real settings/onboarding pages render in a
// plain browser for README screenshots (see render.sh).
window.__TAURI__ = {
  core: {
    invoke: async (cmd) => cmd === 'get_settings' ? {
      hotkey: 'F6', autostart: true, keepCaptures: 10, maxKeepCaptures: 100,
      neverSave: false, autoUpdate: true, version: '1.0.0', requestSound: true, captureMonitor: '',
      monitors: [{ id: 'DISPLAY1', label: 'Display 1 · 2560 × 1440 (main)' }, { id: 'DISPLAY2', label: 'Display 2 · 1920 × 1080' }],
      tools: [
        { id: 'claude', name: 'Claude', installed: true, connected: true, found: 'Claude Code and Claude Desktop', setup: '/scribble command and MCP server' },
        { id: 'codex', name: 'Codex', installed: true, connected: false, found: 'Codex', setup: '/scribble command and MCP server' },
        { id: 'copilot', name: 'GitHub Copilot', installed: true, connected: false, found: 'GitHub Copilot', setup: 'scribble skill and MCP server' },
        { id: 'cursor', name: 'Cursor', installed: true, connected: false, found: 'Cursor', setup: '/scribble skill and MCP server' },
        { id: 'gemini', name: 'Gemini & Antigravity', installed: true, connected: false, found: 'Gemini CLI and Antigravity', setup: '/scribble command and MCP server' },
        { id: 'opencode', name: 'OpenCode', installed: true, connected: false, found: 'OpenCode', setup: '/scribble command and MCP server' },
        { id: 'lumo', name: 'Lumo', installed: false, connected: false, found: '', setup: 'MCP server' },
      ],
    } : null,
  },
  event: { listen: async () => {} },
};

// Fixed page size (?w=460&h=580), matching the real window. Headless Chrome
// has a minimum window width, so the size can't come from the window alone.
const size = new URLSearchParams(location.search);
addEventListener('DOMContentLoaded', () => {
  const html = document.documentElement;
  html.style.width = size.get('w') + 'px';
  html.style.height = size.get('h') + 'px';
  html.style.overflow = 'hidden';
});

// Screenshots show the final state, not a frame of an animation.
addEventListener('DOMContentLoaded', () => {
  const style = document.createElement('style');
  style.textContent = '*, *::before, *::after { animation: none !important; transition: none !important; }';
  document.head.append(style);
});
