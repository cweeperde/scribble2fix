"""Renders the README images in docs/images/ with the real overlay.

The fake app (scene.html) is screenshotted and handed to app/ui/overlay.html
as the frozen screen, then the marks are drawn with scripted mouse and
keyboard input, exactly like a user would.

Writes: overlay.png (hero), demo.gif, result.png (the real export),
tools.png, chat.png, onboarding.png.

Usage: python docs/mockups/render.py
Needs: pip install playwright pillow && python -m playwright install chromium
"""

import base64
import io
import json
import math
import pathlib
import time

from PIL import Image
from playwright.sync_api import sync_playwright

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
UI = ROOT / "app" / "ui"
OUT = ROOT / "docs" / "images"

# Stand-in for the Tauri API: serves the frozen screen and keeps the export.
STUB = """
window.__TAURI__ = {
  core: { invoke: async (cmd, args) => {
    if (cmd === 'frozen_pixels') {
      const img = new Image();
      img.src = window.__BG;
      await img.decode();
      const c = new OffscreenCanvas(img.width, img.height), ctx = c.getContext('2d');
      ctx.drawImage(img, 0, 0);
      return ctx.getImageData(0, 0, img.width, img.height).data;
    }
    if (cmd === 'finish_capture') window.__export = args;
    return null;
  } },
  event: { listen: async (name, cb) => { if (name === 'capture') window.__capture = cb; } },
};
addEventListener('DOMContentLoaded', () => {
  const style = document.createElement('style');
  style.textContent = `
    #mock-screen { position: fixed; inset: 0; z-index: 98; width: 100%; height: 100%; display: none; }
    #mock-cursor { position: fixed; z-index: 99; width: 18px; height: 18px; margin: -9px 0 0 -9px; border-radius: 50%;
      border: 2px solid #fff; box-shadow: 0 0 0 1.5px rgba(0,0,0,.6), inset 0 0 0 1.5px rgba(0,0,0,.6); pointer-events: none; display: none; }
    #mock-key { position: fixed; z-index: 99; left: 50%; bottom: 60px; transform: translateX(-50%); display: none;
      padding: 14px 26px; border-radius: 14px; background: #f3f4f7; color: #16181d; font: 700 30px "Segoe UI", system-ui, sans-serif;
      box-shadow: 0 5px 0 #b9bdc7, 0 14px 30px rgba(0,0,0,.45); }
    #mock-toast { position: fixed; z-index: 99; left: 50%; bottom: 48px; transform: translateX(-50%); display: none; align-items: center; gap: 10px;
      padding: 11px 18px 11px 14px; border-radius: 999px; color: #fff; background: rgba(30,30,34,.9); border: 1px solid rgba(255,255,255,.12);
      box-shadow: 0 6px 20px rgba(0,0,0,.3); font: 14px/1.2 system-ui, "Segoe UI", sans-serif; white-space: nowrap; }
    #mock-toast i { width: 20px; height: 20px; border-radius: 50%; background: conic-gradient(#34c759 0 33%, #ff3b30 0 66%, #0a84ff 0);
      -webkit-mask: radial-gradient(circle, transparent 5px, #000 5.5px); }`;
  document.head.append(style);
  document.body.insertAdjacentHTML('beforeend',
    `<img id="mock-screen" src="${window.__BG}"><div id="mock-cursor"></div><div id="mock-key">F6</div><div id="mock-toast"><i></i>Sent to Claude Code, also copied</div>`);
  addEventListener('pointermove', (e) => {
    const c = document.getElementById('mock-cursor');
    c.style.left = e.clientX + 'px';
    c.style.top = e.clientY + 'px';
  }, true);
});
"""


def url(path):
    return path.resolve().as_uri()


def data_url(png_bytes):
    return "data:image/png;base64," + base64.b64encode(png_bytes).decode()


class Recorder:
    """Collects GIF frames; each frame lasts until the next one is taken."""

    def __init__(self, page):
        self.page, self.frames, self.last = page, [], None

    def snap(self):
        now = time.monotonic()
        if self.frames:
            self.frames[-1][1] = round((now - self.last) * 1000)
        self.last = now
        self.frames.append([self.page.screenshot(), 60])

    def hold(self, ms):
        self.snap()
        self.page.wait_for_timeout(ms)
        self.snap()

    def save(self, path, width):
        images = []
        for png, _ in self.frames:
            im = Image.open(io.BytesIO(png)).convert("RGB")
            images.append(im.resize((width, round(im.height * width / im.width)), Image.LANCZOS))
        palette = images[len(images) // 2].quantize(colors=160, method=Image.Quantize.MEDIANCUT)
        frames = [im.quantize(palette=palette, dither=Image.Dither.NONE) for im in images]
        durations = [max(20, d) for _, d in self.frames]
        durations[-1] = 2600
        frames[0].save(path, save_all=True, append_images=frames[1:], duration=durations, loop=0, optimize=True)


def open_overlay(browser, bg_png, size):
    page = browser.new_page(viewport={"width": size[0], "height": size[1]})
    page.add_init_script(f"window.__BG = {json.dumps(data_url(bg_png))};" + STUB)
    page.goto(url(UI / "overlay.html"))
    page.wait_for_function("!!window.__capture")
    return page


def start_capture(page, size):
    page.evaluate(f"window.__capture({{ payload: {{ width: {size[0]}, height: {size[1]}, simple: false, request: null }} }})")


def show(page, el, on=True):
    page.evaluate(f"document.getElementById('{el}').style.display = '{'flex' if on else 'none'}'")


def drag(page, pts, rec=None, every=3):
    page.mouse.move(*pts[0])
    page.mouse.down()
    for i, p in enumerate(pts[1:]):
        page.mouse.move(*p)
        if rec and i % every == 0:
            rec.snap()
    page.mouse.up()
    if rec:
        rec.snap()


def ellipse(cx, cy, rx, ry, start=-2.2, turns=1.08, n=46):
    """A hand-drawn loop: slightly more than one turn, a little wobbly."""
    return [
        (cx + rx * (1 + 0.04 * math.sin(i * 0.7)) * math.cos(start + turns * 2 * math.pi * i / n),
         cy + ry * (1 + 0.05 * math.cos(i * 0.9)) * math.sin(start + turns * 2 * math.pi * i / n))
        for i in range(n + 1)
    ]


def arc(a, b, bend, n=30):
    """Points along a curve from a to b, bent sideways by `bend` px."""
    (ax, ay), (bx, by) = a, b
    length = math.hypot(bx - ax, by - ay)
    nx, ny = -(by - ay) / length, (bx - ax) / length
    return [(ax + (bx - ax) * t + nx * bend * 4 * t * (1 - t), ay + (by - ay) * t + ny * bend * 4 * t * (1 - t))
            for t in (i / n for i in range(n + 1))]


def box_of(page, text):
    b = page.get_by_text(text, exact=True).first.bounding_box()
    return {"l": b["x"], "t": b["y"], "r": b["x"] + b["width"], "b": b["y"] + b["height"],
            "cx": b["x"] + b["width"] / 2, "cy": b["y"] + b["height"] / 2, "w": b["width"], "h": b["height"]}


def keys(page, rec, *names):
    for k in names:
        page.keyboard.press(k)
        page.wait_for_timeout(90)
        if rec:
            rec.snap()


def hero_and_demo(browser):
    size = (1200, 720)
    scene = browser.new_page(viewport={"width": size[0], "height": size[1]})
    scene.goto(url(HERE / "scene.html"))
    export, photo, name = box_of(scene, "Export legacy CSV"), box_of(scene, "Change photo"), box_of(scene, "Alex Morgan")
    bg = scene.screenshot()
    scene.close()

    page = open_overlay(browser, bg, size)
    rec = Recorder(page)
    show(page, "mock-screen")  # the screen as it is before the overlay opens
    rec.hold(700)
    show(page, "mock-key")
    rec.hold(450)
    show(page, "mock-key", False)
    show(page, "mock-screen", False)
    start_capture(page, size)
    for _ in range(14):  # intro: frame, glow, toolbar glide-in
        rec.snap()
    page.wait_for_timeout(300)
    show(page, "mock-cursor")

    # Remove: circle the CSV button.
    keys(page, rec, "2", "p")
    drag(page, ellipse(export["cx"], export["cy"], export["w"] / 2 + 22, export["h"] / 2 + 16), rec)
    # Change: circle "Change photo" and bend an arrow to next to the name.
    keys(page, rec, "3")
    drag(page, ellipse(photo["cx"], photo["cy"], photo["w"] / 2 + 20, photo["h"] / 2 + 14, start=0.4), rec)
    keys(page, rec, "a")
    drag(page, arc((photo["l"] - 30, photo["t"] - 10), (name["r"] + 34, name["t"] - 6), 70), rec)
    # Add: a box where Save goes, plus a note.
    keys(page, rec, "1", "b")
    x0, y0 = export["r"] + 34, export["t"] - 8
    drag(page, [(x0 + (130 * i / 12), y0 + (50 * i / 12)) for i in range(13)], rec)
    keys(page, rec, "t")
    page.mouse.click(x0 - 4, y0 + 82)
    rec.snap()
    for chunk in ("Add a ", "Save ", "button"):
        page.keyboard.type(chunk, delay=25)
        rec.snap()
    keys(page, rec, "Enter", "p")
    page.mouse.move(size[0] - 140, size[1] - 120)
    show(page, "mock-cursor", False)
    page.wait_for_timeout(250)
    hero = page.screenshot()
    rec.hold(1100)

    # Confirm: outro, then the toast.
    page.keyboard.press("Enter")
    for _ in range(6):
        rec.snap()
    page.wait_for_function("!!window.__export")
    show(page, "mock-screen")
    show(page, "mock-toast")
    rec.snap()

    (OUT / "overlay.png").write_bytes(hero)
    export_png = base64.b64decode(page.evaluate("window.__export.annotated"))
    (OUT / "result.png").write_bytes(export_png)
    rec.save(OUT / "demo.gif", 800)
    page.close()


def tools(browser):
    size = (1200, 300)
    scene = browser.new_page(viewport={"width": size[0], "height": size[1]})
    scene.goto(url(HERE / "tiles.html"))
    delete, email = box_of(scene, "Delete account"), box_of(scene, "Email")
    upload = box_of(scene, "Upload")
    icon = scene.locator(".icon").bounding_box()
    chips = [c.bounding_box() for c in scene.locator(".card:nth-child(5) .chip").all()]
    target = scene.locator(".card:nth-child(3) .chip").bounding_box()
    bg = scene.screenshot()
    scene.close()

    page = open_overlay(browser, bg, size)
    start_capture(page, size)
    page.wait_for_timeout(1500)
    page.add_style_tag(content="#toolbar-pos, #hint, #frame, #glow, #shine { display: none !important; }")

    keys(page, None, "2", "p")
    drag(page, ellipse(delete["cx"], delete["cy"], delete["w"] / 2 + 16, delete["h"] / 2 + 14))
    keys(page, None, "1", "b")
    drag(page, [(email["l"] - 10 + i * (email["w"] + 20) / 10, email["t"] - 8 + i * (email["h"] + 16) / 10) for i in range(11)])
    keys(page, None, "3", "a")
    drag(page, arc((icon["x"] + icon["width"] / 2, icon["y"] - 10), (target["x"] + target["width"] / 2, target["y"] - 12), -46))
    keys(page, None, "1", "t")
    page.mouse.click(upload["l"] - 6, upload["b"] + 42)
    page.keyboard.type("Allow PDF too")
    keys(page, None, "Enter", "3", "n")
    for c in chips:
        page.mouse.click(c["x"] + c["width"] + 2, c["y"] + c["height"] / 2)
    page.wait_for_timeout(200)
    page.screenshot(path=str(OUT / "tools.png"))
    page.close()


def static_pages(browser):
    page = browser.new_page(viewport={"width": 1000, "height": 600})
    page.goto(url(HERE / "chat.html"))
    page.wait_for_load_state("networkidle")
    page.screenshot(path=str(OUT / "chat.png"))
    page.close()

    # The real onboarding window, dark mode, with the stubbed Tauri API.
    page = browser.new_page(viewport={"width": 520, "height": 660}, color_scheme="dark")
    page.add_init_script(path=str(HERE / "tauri-stub.js"))
    page.goto(url(UI / "onboarding.html") + "?w=520&h=660")
    page.wait_for_timeout(400)
    page.screenshot(path=str(OUT / "onboarding.png"))
    page.close()


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch()
        hero_and_demo(browser)
        tools(browser)
        static_pages(browser)
        browser.close()
    for f in sorted(OUT.iterdir()):
        print(f"{f.name:16} {f.stat().st_size // 1024:6} KB")


if __name__ == "__main__":
    main()
