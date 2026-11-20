// MacBook screens with a notch also have rounded top corners, about this
// radius (CSS px).
const SCREEN_CORNER = 10;

// Leads the frame (frame.css) around the camera notch of a MacBook screen.
// `notch` is { left, right, height } in CSS px from the window's top left
// corner, or null for a screen without one.
function setFrameNotch(notch) {
  for (const edge of document.querySelectorAll('.frame-edge')) {
    if (!notch) { edge.style.removeProperty('--notch'); edge.style.removeProperty('--rt'); continue; }
    const w = parseFloat(getComputedStyle(edge).getPropertyValue('--w'));
    // The rounded screen corner cuts off the outer edge there: an inner
    // corner around the same center keeps the band as wide as elsewhere.
    edge.style.setProperty('--rt', Math.max(SCREEN_CORNER - w, 1) + 'px');
    const { left: l, right: r, height: h } = notch;
    const f = 6;                      // the notch flares out a little where it meets the top edge
    const rad = Math.min(10, h / 3);  // its rounded bottom corners
    const x0 = l - f - w, width = r - l + 2 * (f + w), height = h + w;
    // A band twice as wide as the frame, centered on the notch outline: the
    // inner half is hidden behind the notch, the outer half shows.
    const d = `M${l - f} 0Q${l} 0 ${l} ${f}V${h - rad}Q${l} ${h} ${l + rad} ${h}H${r - rad}Q${r} ${h} ${r} ${h - rad}V${f}Q${r} 0 ${r + f} 0`;
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${x0} 0 ${width} ${height}">`
      + `<path d="${d}" fill="none" stroke="#000" stroke-width="${2 * w}" stroke-linejoin="round"/></svg>`;
    edge.style.setProperty('--notch',
      `url("data:image/svg+xml,${encodeURIComponent(svg)}") ${x0}px 0 / ${width}px ${height}px no-repeat`);
  }
}
