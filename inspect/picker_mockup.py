"""Render the new music picker mockup at the game's native 480x320.

The art comes straight out of YOUR copy of the IPA at run time and the
output goes to debug/picker-mockup/ (gitignored). Nothing from the game
is stored in the repo.

Every piece of game art is drawn 1:1 (no resampling). The only scaling is
the final 2x nearest-neighbour upscale of the whole frame for viewing.

usage (from the repo root):
    python inspect/picker_mockup.py ["path/to/game.ipa"]
Needs Pillow. Uses touchHLE_fonts/LiberationSans-*.ttf for text, the same
fonts touchHLE draws UIKit text with.
"""
import importlib.util
import io
import os
import re
import struct
import sys
import zipfile
import zlib

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IPA = (sys.argv[1] if len(sys.argv) > 1 else
       os.path.join(ROOT, "apps", "Song Summoner The Unsung Heroes Encore.ipa"))
OUT = os.path.join(ROOT, "debug", "picker-mockup")
FONTS = os.path.join(ROOT, "touchHLE_fonts")

# ---- layout (keep in sync with dev-docs/song-summoner-media-plan.md) ----
W, H = 480, 320
NAV_H = 44      # navigation bar
TAB_H = 56      # tab bar: fits the native 40x40 tab icons + label
LIST_H = H - NAV_H - TAB_H  # 220 = exactly 4 rows
ROW_H = 55      # cellbg.png / touchBG.png / list_fighter*.png height
HDR_H = 22      # section header
TEXT_X = 15     # MusicLibraryCell draws title/artist at x=15
FIGHTER_X = W - 200 - 18  # 200-wide portrait, clear of the index strip
CYAN = (90, 200, 255, 255)

# ---- art from the IPA ----
_spec = importlib.util.spec_from_file_location(
    "cgbi", os.path.join(ROOT, "dev-scripts", "cgbi_to_png.py"))
cgbi = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cgbi)


def decode_png(data):
    """Decode an iOS CgBI ("crushed") PNG, or a normal one."""
    chunks = cgbi.read_chunks(data)
    if not any(c[0] == b"CgBI" for c in chunks):
        return Image.open(io.BytesIO(data)).convert("RGBA")
    ihdr = next(c[1] for c in chunks if c[0] == b"IHDR")
    w, h = struct.unpack(">II", ihdr[:8])
    idat = b"".join(c[1] for c in chunks if c[0] == b"IDAT")
    try:
        raw = zlib.decompress(idat, -15)
    except zlib.error:
        raw = zlib.decompress(idat)
    rgba = cgbi.unfilter_and_swap(raw, w, h)
    return Image.frombytes("RGBA", (w, h), bytes(rgba))


_zip = zipfile.ZipFile(IPA)
_names = {n.rsplit("/", 1)[-1]: n for n in _zip.namelist()
          if re.match(r"Payload/[^/]+\.app/[^/]+\.png$", n)}
_cache = {}


def art(name):
    if name not in _cache:
        _cache[name] = decode_png(_zip.read(_names[name]))
    return _cache[name]


def font(bold, size):
    f = "LiberationSans-Bold.ttf" if bold else "LiberationSans-Regular.ttf"
    return ImageFont.truetype(os.path.join(FONTS, f), size)


# ---- drawing helpers ----
def vgrad(w, h, top, bottom):
    g = Image.new("RGBA", (w, h))
    d = ImageDraw.Draw(g)
    for y in range(h):
        t = y / max(1, h - 1)
        d.line([(0, y), (w, y)], fill=tuple(
            int(top[i] + (bottom[i] - top[i]) * t) for i in range(3)) + (255,))
    return g


def ellipsize(d, text, f, maxw):
    if d.textlength(text, font=f) <= maxw:
        return text
    while text and d.textlength(text + "…", font=f) > maxw:
        text = text[:-1]
    return text + "…"


def nav_bar(im, title, back=None):
    im.alpha_composite(vgrad(W, NAV_H, (70, 70, 74), (8, 8, 10)), (0, 0))
    d = ImageDraw.Draw(im)
    d.line([(0, NAV_H - 2), (W, NAV_H - 2)], fill=(40, 150, 220, 255))
    d.line([(0, NAV_H - 1), (W, NAV_H - 1)], fill=(0, 0, 0, 255))
    ft = font(True, 19)
    t = ellipsize(d, title, ft, 250)
    d.text(((W - d.textlength(t, font=ft)) / 2, 11), t, font=ft,
           fill=(255, 255, 255, 255))
    fb = font(True, 12)

    def button(x, label):
        bw = int(d.textlength(label, font=fb)) + 20
        d.rounded_rectangle([x, 8, x + bw, 35], 5, fill=(28, 28, 32, 255),
                            outline=(95, 95, 105, 255))
        d.text((x + 10, 14), label, font=fb, fill=(255, 255, 255, 255))
        return bw
    cancel_w = int(d.textlength("Cancel", font=fb)) + 20
    button(W - cancel_w - 6, "Cancel")
    if back:
        button(6, "‹ " + back)


def tab_bar(im, selected):
    y0 = H - TAB_H
    im.alpha_composite(vgrad(W, TAB_H, (38, 38, 42), (0, 0, 0)), (0, y0))
    d = ImageDraw.Draw(im)
    d.line([(0, y0), (W, y0)], fill=(20, 20, 20, 255))
    tabs = [("song.png", "Song"), ("artist.png", "Artist"),
            ("album.png", "Album"), ("playlist.png", "Playlist")]
    tw = W // len(tabs)
    f = font(True, 10)
    for i, (icon, label) in enumerate(tabs):
        x, sel = i * tw, i == selected
        if sel:
            d.rounded_rectangle([x + 20, y0 + 3, x + tw - 20, y0 + TAB_H - 3],
                                5, fill=(60, 60, 66, 255))
        ic = art(icon)  # native 40x40, never resized
        alpha = ic.getchannel("A")
        if sel:
            ic = Image.new("RGBA", ic.size, CYAN)
            ic.putalpha(alpha)
        else:
            ic = ic.copy()
            ic.putalpha(alpha.point(lambda v: v * 45 // 100))
        im.alpha_composite(ic, (x + (tw - 40) // 2, y0 + 2))
        lw = d.textlength(label, font=f)
        d.text((x + (tw - lw) / 2, y0 + 42), label, font=f,
               fill=CYAN if sel else (150, 150, 150, 255))


def section_header(layer, y, letter):
    layer.alpha_composite(vgrad(W, HDR_H, (58, 64, 74), (36, 40, 48)), (0, y))
    ImageDraw.Draw(layer).text((12, y + 3), letter, font=font(True, 14),
                               fill=(255, 255, 255, 255))
    return y + HDR_H


def row_bg(layer, y, selected):
    layer.alpha_composite(art("touchBG.png" if selected else "cellbg.png"),
                          (0, y))


def song_row(layer, y, title, sub, fighter, selected=False, artwork=False):
    row_bg(layer, y, selected)
    if fighter is not None:
        layer.alpha_composite(art("list_fighter%03d.png" % fighter),
                              (FIGHTER_X, y))
    d = ImageDraw.Draw(layer)
    x = TEXT_X
    if artwork:
        layer.alpha_composite(art("noartwork.png"), (4, y + 2))  # native 50
        x = 62
    ft, fs = font(True, 16), font(False, 13)
    d.text((x, y + 9), ellipsize(d, title, ft, 292 - x), font=ft,
           fill=(255, 255, 255, 255))
    d.text((x, y + 31), ellipsize(d, sub, fs, 292 - x), font=fs,
           fill=(150, 150, 150, 255))
    return y + ROW_H


def group_row(layer, y, name, count, selected=False, artwork=False):
    row_bg(layer, y, selected)
    d = ImageDraw.Draw(layer)
    x = TEXT_X
    if artwork:
        layer.alpha_composite(art("noartwork.png"), (4, y + 2))
        x = 62
    d.text((x, y + 17), name, font=font(True, 16), fill=(255, 255, 255, 255))
    s = "%d songs  ›" % count
    fs = font(False, 13)
    d.text((W - 30 - d.textlength(s, font=fs), y + 20), s, font=fs,
           fill=(150, 150, 150, 255))
    return y + ROW_H


def index_strip(im, active):
    top, bottom = NAV_H, H - TAB_H
    d = ImageDraw.Draw(im)
    d.rounded_rectangle([W - 16, top + 2, W - 2, bottom - 2], 7,
                        fill=(0, 0, 0, 130))
    letters = list("ABCDEFGHIJKLMNOPQRSTUVWXYZ#")
    f = font(True, 7)
    step = (bottom - top - 8) / len(letters)
    for i, ch in enumerate(letters):
        d.text((W - 9 - d.textlength(ch, font=f) / 2, top + 4 + i * step), ch,
               font=f, fill=CYAN if ch == active else (200, 200, 200, 255))


def screen(rows, title, tab, back=None, index=None):
    im = Image.new("RGBA", (W, H), (22, 22, 24, 255))
    layer = Image.new("RGBA", (W, LIST_H + 2 * ROW_H), (22, 22, 24, 255))
    y = 0
    for kind, *a in rows:
        if kind == "hdr":
            y = section_header(layer, y, *a)
        elif kind == "song":
            y = song_row(layer, y, *a[:3], **(a[3] if len(a) > 3 else {}))
        else:
            y = group_row(layer, y, *a[:2], **(a[2] if len(a) > 2 else {}))
    im.alpha_composite(layer.crop((0, 0, W, LIST_H)), (0, NAV_H))
    if index:
        index_strip(im, index)
    nav_bar(im, title, back)
    tab_bar(im, tab)
    return im


# Portraits with a real fighter (003/033/054 are blank placeholders).
SCREENS = {
    "1_songs": lambda: screen([
        ("hdr", "A"),
        ("song", "Afterglow Anthem", "The Lanterns", 6),
        ("song", "Across the Static", "Neon Harbor", 21, {"selected": True}),
        ("hdr", "B"),
        ("song", "Blue Hour Parade", "Marigold Club", 45),
    ], "Song", 0, index="A"),
    "2_artists": lambda: screen([
        ("hdr", "M"),
        ("group", "Marigold Club", 12),
        ("group", "Midnight Relay", 4, {"selected": True}),
        ("group", "Moss & Mirror", 9),
        ("hdr", "N"),
    ], "Artist", 1, index="M"),
    "3_artist_songs": lambda: screen([
        ("song", "Blue Hour Parade", "Paper Suns", 45, {"artwork": True}),
        ("song", "Coastline Radio", "Paper Suns", 12,
         {"artwork": True, "selected": True}),
        ("song", "Dandelion Circuit", "Paper Suns", 15, {"artwork": True}),
        ("song", "Evening Machine", "Paper Suns", 27, {"artwork": True}),
    ], "Marigold Club", 1, back="Artist"),
    "4_albums": lambda: screen([
        ("hdr", "P"),
        ("group", "Paper Suns", 11, {"artwork": True}),
        ("group", "Porchlight Static", 8, {"artwork": True, "selected": True}),
        ("hdr", "R"),
        ("group", "Riverglass", 10, {"artwork": True}),
    ], "Album", 2, index="P"),
}


def loading():
    im = SCREENS["1_songs"]()
    im.alpha_composite(Image.new("RGBA", (W, H), (0, 0, 0, 170)))
    cube = art("cube_anm_03.png")  # native 48x48, animated in the real thing
    cx = (W - 48) // 2
    im.alpha_composite(cube.filter(ImageFilter.GaussianBlur(5)), (cx, 118))
    im.alpha_composite(cube, (cx, 118))
    d = ImageDraw.Draw(im)
    for text, f, y, col in [("Reading your music library…", font(True, 14),
                             178, CYAN),
                            ("1,317 songs", font(False, 12), 198,
                             (170, 170, 170, 255))]:
        d.text(((W - d.textlength(text, font=f)) / 2, y), text, font=f,
               fill=col)
    return im


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    shots = [(k, f()) for k, f in SCREENS.items()] + [("5_loading", loading())]
    for name, im in shots:
        im.resize((W * 2, H * 2), Image.NEAREST).convert("RGB").save(
            os.path.join(OUT, "%s.png" % name))
    g, cols = 16, 2
    rows_n = (len(shots) + cols - 1) // cols
    sheet = Image.new("RGB", (cols * (W + g) + g, rows_n * (H + g) + g),
                      (12, 12, 14))
    for i, (_, im) in enumerate(shots):
        sheet.paste(im.convert("RGB"),
                    (g + (i % cols) * (W + g), g + (i // cols) * (H + g)))
    sheet.resize((sheet.width * 2, sheet.height * 2), Image.NEAREST).save(
        os.path.join(OUT, "sheet.png"))
    print("wrote", OUT)
