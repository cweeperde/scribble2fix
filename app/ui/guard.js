// Loaded first by every window. The app's windows are web pages, so the
// browser's own shortcuts would work in them: F5 reloads the page (the overlay
// then loses its frozen screenshot), Ctrl+F opens a search bar, Ctrl+P prints,
// Alt+Left navigates back... Block those. Only preventDefault: the pages' own
// key handlers (like the hotkey recorder) still see every key.
addEventListener('keydown', (e) => {
  const mod = e.ctrlKey || e.metaKey;
  const key = e.key.toLowerCase();
  const blocked =
    ['F3', 'F5', 'F7', 'BrowserBack', 'BrowserForward', 'BrowserRefresh'].includes(e.key) ||
    (mod && ['r', 'f', 'g', 'p', 'u', 'j', 's', 'o', 'h', 'n', 'w'].includes(key)) ||
    (mod && e.shiftKey && ['i', 'c'].includes(key)) ||
    (e.altKey && ['ArrowLeft', 'ArrowRight', 'Home'].includes(e.key));
  if (blocked) e.preventDefault();
}, true);

// Mouse back/forward buttons navigate too.
addEventListener('mouseup', (e) => { if (e.button === 3 || e.button === 4) e.preventDefault(); }, true);
