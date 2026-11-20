"""Renders the README images in docs/images/ with the real overlay.

The fake app (scene.html) is screenshotted and handed to app/ui/overlay.html
as the frozen screen, then the marks are drawn with scripted mouse and
keyboard input, exactly like a user would.

Writes: overlay.png (hero), demo.gif, result.png (the real export),
tools.png, chat.png, request.png (the AI request card), onboarding.png, all
with rounded corners and a hairline border (see SHOWN).

Usage: python docs/mockups/render.py
Needs: pip install playwright pillow && python -m playwright install chromium
"""

import base64
import io
import json
import math
import pathlib
import time

from PIL import Image, ImageChops, ImageDraw
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
    // The real window disappears here: show the screen as it is underneath.
    if (cmd === 'hide_overlay') document.getElementById('mock-screen').style.display = 'block';
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
      -webkit-mask: radial-gradient(circle, transparent 5px, #000 5.5px); }
    #mock-toast.go { animation: mock-pop .55s cubic-bezier(.2, 1.4, .35, 1) both; }
    @keyframes mock-pop { 0% { opacity: 0; transform: translateX(-50%) translateY(14px) scale(.7); } 35% { opacity: 1; }
      100% { opacity: 1; transform: translateX(-50%); } }`;
  document.head.append(style);
  document.body.insertAdjacentHTML('beforeend',
    `<img id="mock-screen" src="${window.__BG}"><div id="mock-cursor"></div><div id="mock-key">F6</div><div id="mock-toast"><i></i>Sent to Claude, also copied</div>`);
  addEventListener('pointermove', (e) => {
    const c = document.getElementById('mock-cursor');
    c.style.left = e.clientX + 'px';
    c.style.top = e.clientY + 'px';
  }, true);
});
"""


# How wide the README shows each image. Corners and border are sized for that,
# so they look the same on every image.
SHOWN = {"overlay.png": 860, "tools.png": 860, "result.png": 700, "chat.png": 800,
         "request.png": 800, "onboarding.png": 290, "demo.gif": 800}
RADIUS = 12          # corner radius as shown in the README, in px
BORDER = (128, 128, 128, 80)  # hairline that works on GitHub's light and dark theme


def frame_mask(size, shown):
    """Rounded-corner alpha mask and hairline border for an image of `size`."""
    w, h = size
    k = w / shown  # image px per README px
    ss = 4         # supersampled for smooth edges
    box = (0, 0, w * ss - 1, h * ss - 1)
    mask = Image.new("L", (w * ss, h * ss), 0)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius=RADIUS * k * ss, fill=255)
    line = Image.new("RGBA", (w * ss, h * ss), (0, 0, 0, 0))
    ImageDraw.Draw(line).rounded_rectangle(box, radius=RADIUS * k * ss, outline=BORDER, width=max(1, round(k * ss)))
    return mask.resize(size, Image.LANCZOS), line.resize(size, Image.LANCZOS)


def save_framed(png, name):
    """Saves a PNG (bytes or path) to docs/images with rounded corners and a border."""
    im = Image.open(io.BytesIO(png) if isinstance(png, bytes) else png).convert("RGBA")
    mask, line = frame_mask(im.size, SHOWN[name])
    im = Image.alpha_composite(im, line)
    im.putalpha(ImageChops.multiply(im.getchannel("A"), mask))
    im.save(OUT / name, optimize=True)


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
            im = Image.open(io.BytesIO(png)).convert("RGBA")
            images.append(im.resize((width, round(im.height * width / im.width)), Image.LANCZOS))
        # Same rounded corners and border as the PNGs. GIF transparency is all
        # or nothing, so the corners are cut at half coverage.
        mask, line = frame_mask(images[0].size, SHOWN[path.name])
        images = [Image.alpha_composite(im, line).convert("RGB") for im in images]
        # One palette for the whole GIF: learned from frames spread over the
        # demo, plus the app's exact colors, so the marks keep their green, red
        # and blue (a palette from one frame turned the green turquoise).
        picks = images[:: max(1, len(images) // 8)]
        sample = Image.new("RGB", (images[0].width, images[0].height * len(picks)))
        for i, im in enumerate(picks):
            sample.paste(im, (0, i * im.height))
        learned = sample.quantize(colors=247, method=Image.Quantize.MEDIANCUT).getpalette()[: 247 * 3]
        exact = [(0x34, 0xC7, 0x59), (0xFF, 0x3B, 0x30), (0x0A, 0x84, 0xFF), (255, 255, 255),
                 (0x18, 0x18, 0x1C), (0x1E, 0x1E, 0x22), (0, 0, 0)]
        palette = Image.new("P", (1, 1))
        clear = 255  # palette entry for the cut-off corners
        colors = learned + [c for rgb in exact for c in rgb]
        palette.putpalette(colors + [255, 0, 255] * (256 - len(colors) // 3))
        corners = mask.point(lambda a: 255 if a < 128 else 0)
        frames = []
        for im in images:
            f = im.quantize(palette=palette, dither=Image.Dither.NONE)
            f.paste(clear, mask=corners)
            frames.append(f)
        durations = [max(20, d) for _, d in self.frames]
        durations[-1] = 2600
        frames[0].save(path, save_all=True, append_images=frames[1:], duration=durations, loop=0,
                       transparency=clear, disposal=1)


def open_overlay(browser, bg_png, size):
    page = browser.new_page(viewport={"width": size[0], "height": size[1]})
    page.add_init_script(f"window.__BG = {json.dumps(data_url(bg_png))};" + STUB)
    page.goto(url(UI / "overlay.html"))
    page.wait_for_function("!!window.__capture")
    return page


def start_capture(page, size, cursor=None):
    cx, cy = cursor or (size[0] / 2, size[1] / 2)
    page.evaluate(f"window.__capture({{ payload: {{ width: {size[0]}, height: {size[1]}, simple: false, request: null, cursor: [{cx}, {cy}] }} }})")


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
    mail = box_of(scene, "alex@example.com")
    bg = scene.screenshot()
    scene.close()

    page = open_overlay(browser, bg, size)
    # The soft white light sweeps turn into hard bands in a GIF's limited palette.
    page.add_style_tag(content="#shine, #lift::after { display: none !important; }")
    rec = Recorder(page)
    show(page, "mock-screen")  # the screen as it is before the overlay opens
    rec.hold(700)
    show(page, "mock-key")
    rec.hold(450)
    show(page, "mock-key", False)
    show(page, "mock-screen", False)
    start_capture(page, size, (export["cx"] + 40, export["cy"] - 60))
    for _ in range(18):  # intro: frame and toolbar come in
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
    keys(page, rec, "Enter")
    # Hide: the email address is private.
    keys(page, rec, "r")
    drag(page, [(mail["l"] - 5 + i * (mail["w"] + 10) / 10, mail["t"] + i * mail["h"] / 10) for i in range(11)], rec)
    keys(page, rec, "p")
    page.mouse.move(size[0] - 140, size[1] - 120)
    show(page, "mock-cursor", False)
    page.wait_for_timeout(250)
    hero = page.screenshot()
    rec.hold(1100)

    # Confirm: the area lifts off and flies to the toast.
    page.keyboard.press("Enter")
    for _ in range(14):
        rec.snap()
    page.wait_for_function("!!window.__export")
    show(page, "mock-screen")
    show(page, "mock-toast")
    page.evaluate("document.getElementById('mock-toast').classList.add('go')")
    for _ in range(6):
        rec.snap()

    save_framed(hero, "overlay.png")
    save_framed(base64.b64decode(page.evaluate("window.__export.annotated")), "result.png")
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
    secret = scene.locator(".secret").bounding_box()
    text_card = scene.locator(".card:nth-child(4)").bounding_box()
    target = scene.locator(".card:nth-child(3) .chip").bounding_box()
    bg = scene.screenshot()
    scene.close()

    page = open_overlay(browser, bg, size)
    start_capture(page, size)
    page.wait_for_timeout(1500)
    page.add_style_tag(content="#toolbar-pos, #hint, #frame, #glow, #bloom, #spark { display: none !important; }")

    keys(page, None, "2", "p")
    drag(page, ellipse(delete["cx"], delete["cy"], delete["w"] / 2 + 16, delete["h"] / 2 + 14))
    keys(page, None, "1", "b")
    drag(page, [(email["l"] - 10 + i * (email["w"] + 20) / 10, email["t"] - 8 + i * (email["h"] + 16) / 10) for i in range(11)])
    keys(page, None, "3", "a")
    drag(page, arc((icon["x"] + icon["width"] / 2, icon["y"] - 10), (target["x"] + target["width"] / 2, target["y"] - 12), -46))
    keys(page, None, "1", "t")
    page.mouse.click(text_card["x"] + 14, upload["b"] + 42)
    page.keyboard.type("Allow PDF too")
    keys(page, None, "Enter", "3", "n")
    for c in chips:
        page.mouse.click(c["x"] + c["width"] + 2, c["y"] + c["height"] / 2)
    keys(page, None, "r")
    drag(page, [(secret["x"] - 4 + i * (secret["width"] + 8) / 10, secret["y"] - 4 + i * (secret["height"] + 8) / 10) for i in range(11)])
    page.wait_for_timeout(200)
    save_framed(page.screenshot(), "tools.png")
    page.close()


def static_pages(browser):
    # Plain pages render at 2x, so text stays sharp on high-DPI screens.
    page = browser.new_page(viewport={"width": 1000, "height": 600}, device_scale_factor=2)
    page.goto(url(HERE / "chat.html"))
    page.wait_for_load_state("networkidle")
    save_framed(page.screenshot(), "chat.png")
    page.close()

    # The real onboarding window, dark mode, with the stubbed Tauri API, on
    # the step where the AI tools get connected.
    page = browser.new_page(viewport={"width": 520, "height": 700}, color_scheme="dark", device_scale_factor=2)
    page.add_init_script(path=str(HERE / "tauri-stub.js"))
    page.goto(url(UI / "onboarding.html") + "?w=520&h=700")
    page.wait_for_timeout(300)
    for _ in range(3):  # welcome, hotkey, toolbar, then the AI tools
        page.click("#next")
    # Everything found starts switched on; show a typical choice instead.
    for tool in ["copilot", "cursor", "gemini", "lumo"]:
        page.click(f"input[data-id={tool}]")
    page.wait_for_timeout(300)
    save_framed(page.screenshot(), "onboarding.png")
    page.close()


# Stand-in for the Tauri API in the request card and glow pages: keeps the
# event handlers so the script can fire the events.
EVENTS_STUB = """
window.__L = {};
window.__TAURI__ = { core: { invoke: async () => null },
  event: { listen: async (name, cb) => { window.__L[name] = cb; } } };
"""


def request_card(browser):
    """The real request card and screen frame over the example app."""
    size = (1200, 720)
    scene = browser.new_page(viewport={"width": size[0], "height": size[1]})
    scene.goto(url(HERE / "scene.html"))
    bg = scene.screenshot()
    scene.close()

    page = browser.new_page(viewport={"width": size[0], "height": size[1]})
    page.add_init_script(EVENTS_STUB)
    build = HERE / "build"
    build.mkdir(exist_ok=True)
    (build / "bg.png").write_bytes(bg)
    scene_file = build / "request-scene.html"
    scene_file.write_text(f"""<!doctype html><html><body style="margin:0;overflow:hidden">
      <img src="bg.png" style="position:fixed;inset:0;width:100%;height:100%">
      <iframe id="glow" src="{url(UI / 'glow.html')}" style="position:fixed;inset:0;width:100%;height:100%;border:0;background:transparent" allowtransparency></iframe>
      <iframe id="card" src="{url(UI / 'request.html')}" style="position:fixed;top:0;left:{(size[0] - 520) // 2}px;width:520px;height:210px;border:0;background:transparent" allowtransparency></iframe>
    </body></html>""", encoding="utf-8")
    page.goto(url(scene_file))
    frames = {f.url.rsplit("/", 1)[-1]: f for f in page.frames}
    glow, card = frames["glow.html"], frames["request.html"]
    glow.wait_for_function("!!window.__L.glow")
    card.wait_for_function("!!window.__L.request")
    glow.evaluate("window.__L.glow({ payload: true })")
    card.evaluate("""window.__L.request({ payload: { id: 'a', client: 'Claude', hotkey: 'F6', sound: false,
      reason: 'Mark the buttons you mean', expiresAt: Date.now() + 9 * 60000 + 41000 } })""")
    page.wait_for_timeout(1200)
    save_framed(page.screenshot(), "request.png")
    page.close()


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch()
        hero_and_demo(browser)
        tools(browser)
        static_pages(browser)
        request_card(browser)
        browser.close()
    for f in sorted(OUT.iterdir()):
        print(f"{f.name:16} {f.stat().st_size // 1024:6} KB")


if __name__ == "__main__":
    main()
