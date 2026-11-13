// Shared by settings.html and onboarding.html.
const { invoke } = window.__TAURI__.core;

const isMac = navigator.platform.startsWith('Mac');

const prettyHotkey = (s) => s.split('+').map((p) =>
  p.replace(/^Key/, '').replace(/^Digit/, '').replace('Super', isMac ? '⌘' : 'Win')
).join(' + ');

function setStatus(el, text, kind = '') { el.textContent = text; el.className = 'status ' + kind; }

// Click the button, press a key combination: saves it as the capture hotkey.
function hotkeyRecorder(button, status, initial, onSaved = () => {}) {
  let current = initial, listening = false;
  const setListening = (on) => {
    listening = on;
    button.classList.toggle('listening', on);
    button.textContent = on ? 'Press keys…' : prettyHotkey(current);
    if (on) setStatus(status, 'Esc to cancel');
  };
  setListening(false);
  button.onclick = () => setListening(!listening);

  addEventListener('keydown', async (e) => {
    if (!listening) return;
    e.preventDefault();
    if (e.key === 'Escape') { setListening(false); setStatus(status, ''); return; }
    if (['Control', 'Shift', 'Alt', 'Meta', 'OS'].includes(e.key)) return; // wait for the actual key
    const mods = [];
    if (e.ctrlKey) mods.push('Ctrl');
    if (e.altKey) mods.push('Alt');
    if (e.shiftKey) mods.push('Shift');
    if (e.metaKey) mods.push('Super');
    const combo = [...mods, e.code].join('+');
    try {
      await invoke('set_hotkey', { hotkey: combo });
      current = combo;
      setListening(false);
      setStatus(status, 'Saved. Active as soon as you close this window.', 'ok');
      onSaved(combo);
    } catch (err) {
      setListening(false);
      setStatus(status, String(err), 'error');
    }
  });
}

function toolStatusText(t) {
  if (!t.installed) return 'Not found on this computer';
  if (!t.connected) return 'Not connected';
  return t.id === 'claude-desktop' ? 'Connected: MCP server' : 'Connected: /scribble command and MCP server';
}
