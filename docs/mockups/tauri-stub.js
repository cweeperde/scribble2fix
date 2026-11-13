// Stand-in for the Tauri API so the real settings/onboarding pages render in a
// plain browser for README screenshots (see render.sh).
window.__TAURI__ = {
  core: {
    invoke: async (cmd) => cmd === 'get_settings' ? {
      hotkey: 'F6', autostart: true, keepCaptures: 10, maxKeepCaptures: 100,
      neverSave: false, autoUpdate: true, version: '1.0.0',
      tools: [
        { id: 'claude', name: 'Claude Code', installed: true, connected: true },
        { id: 'claude-desktop', name: 'Claude Desktop', installed: true, connected: true },
        { id: 'codex', name: 'Codex', installed: true, connected: false },
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
