#!/usr/bin/env python3
"""Render a `tmux capture-pane -p -e` dump to a PNG.

Text is drawn with a monospace font; box drawing, block elements and braille
are drawn as shapes so neighbouring cells join without gaps. The swarmux UI
uses no private-use glyphs, so a plain monospace font is enough.

usage: render_ansi.py IN.ans OUT.png COLS ROWS

Environment:
  SWX_FONT, SWX_FONT_BOLD  regular and bold TTF files (default: the first
                           JetBrains Mono, then DejaVu Sans Mono, found)
  SWX_PX                   font size in pixels (default 16)
"""
import glob
import os
import re
import sys

from PIL import Image, ImageDraw, ImageFont

SIZE = int(os.environ.get("SWX_PX", "16"))
LW = max(1, round(SIZE / 14))
HW = 2 * LW
BG = (0, 0, 0)
FG = (204, 204, 204)

HOME = os.path.expanduser("~")
FONT_DIRS = [
    f"{HOME}/.local/share/fonts/**",
    f"{HOME}/.fonts/**",
    "/usr/share/fonts/**",
    "/usr/local/share/fonts/**",
    "/Library/Fonts/**",
    "/System/Library/Fonts/**",
    f"{HOME}/Library/Fonts/**",
]


def find(names):
    """The first file named `names[i]` under any font folder, in name order."""
    for name in names:
        for d in FONT_DIRS:
            hits = sorted(glob.glob(f"{d}/{name}", recursive=True))
            if hits:
                return hits[0]
    return None


MONO = ["JetBrainsMono-Regular.ttf", "JetBrainsMonoNerdFontMono-Regular.ttf"]
MONO_B = ["JetBrainsMono-Bold.ttf", "JetBrainsMonoNerdFontMono-Bold.ttf"]
FALL_R = ["DejaVuSansMono.ttf", "Menlo.ttc"]
FALL_B = ["DejaVuSansMono-Bold.ttf"]

REG = os.environ.get("SWX_FONT") or find(MONO) or find(FALL_R)
BOLD = os.environ.get("SWX_FONT_BOLD") or find(MONO_B) or find(FALL_B) or REG
FALL = find(FALL_R) or REG
FALLB = find(FALL_B) or FALL
if REG is None:
    sys.exit("render_ansi.py: no monospace font found; set SWX_FONT and SWX_FONT_BOLD")

fonts = {False: ImageFont.truetype(REG, SIZE), True: ImageFont.truetype(BOLD, SIZE)}
fallback = {False: ImageFont.truetype(FALL, SIZE - 1), True: ImageFont.truetype(FALLB, SIZE - 1)}

try:
    from fontTools.ttLib import TTFont

    CMAP = set(TTFont(REG).getBestCmap().keys())
except Exception:  # fontTools is optional: without it no fallback is used
    CMAP = None

CW = round(fonts[False].getlength("M"))
CH = round(SIZE * 1.32)
ASC, DESC = fonts[False].getmetrics()
BASE = (CH - (ASC + DESC)) // 2

PAL16 = [
    (0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16),
    (36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229),
    (102, 102, 102), (241, 76, 76), (35, 209, 139), (245, 245, 67),
    (59, 142, 234), (214, 112, 214), (41, 184, 219), (255, 255, 255),
]


def x256(n):
    if n < 16:
        return PAL16[n]
    if n < 232:
        n -= 16
        lv = [0, 95, 135, 175, 215, 255]
        return (lv[n // 36], lv[(n // 6) % 6], lv[n % 6])
    v = 8 + (n - 232) * 10
    return (v, v, v)


def sgr(st, params):
    ps = [int(p) if p else 0 for p in re.split("[;:]", params)] if params else [0]
    i = 0
    while i < len(ps):
        p = ps[i]
        if p == 0:
            st.update(fg=None, bg=None, bold=False, dim=False, rev=False)
        elif p == 1:
            st["bold"] = True
        elif p == 2:
            st["dim"] = True
        elif p == 22:
            st["bold"] = st["dim"] = False
        elif p == 7:
            st["rev"] = True
        elif p == 27:
            st["rev"] = False
        elif 30 <= p <= 37:
            st["fg"] = PAL16[p - 30]
        elif 90 <= p <= 97:
            st["fg"] = PAL16[p - 90 + 8]
        elif 40 <= p <= 47:
            st["bg"] = PAL16[p - 40]
        elif 100 <= p <= 107:
            st["bg"] = PAL16[p - 100 + 8]
        elif p == 39:
            st["fg"] = None
        elif p == 49:
            st["bg"] = None
        elif p in (38, 48):
            key = "fg" if p == 38 else "bg"
            if i + 1 < len(ps) and ps[i + 1] == 2 and i + 4 < len(ps):
                st[key] = tuple(ps[i + 2:i + 5])
                i += 4
            elif i + 1 < len(ps) and ps[i + 1] == 5 and i + 2 < len(ps):
                st[key] = x256(ps[i + 2])
                i += 2
        i += 1


def parse(path):
    text = open(path, encoding="utf-8", errors="replace").read()
    rows = []
    st = {"fg": None, "bg": None, "bold": False, "dim": False, "rev": False}
    for line in text.split("\n"):
        cells = []
        i = 0
        while i < len(line):
            ch = line[i]
            if ch == "\x1b":
                m = re.match(r"\x1b\[([0-9;:]*)([A-Za-z])", line[i:])
                if m:
                    if m.group(2) == "m":
                        sgr(st, m.group(1))
                    i += m.end()
                    continue
                i += 1
                continue
            cells.append((ch, dict(st)))
            i += 1
        rows.append(cells)
    return rows


def blend(a, b, k):
    return tuple(int(a[j] + (b[j] - a[j]) * k) for j in range(3))


LIGHT = {  # which sides a box-drawing cell reaches: left, right, up, down
    "─": "lr", "│": "ud", "┌": "rd", "┐": "ld", "└": "ru", "┘": "lu",
    "├": "udr", "┤": "udl", "┬": "lrd", "┴": "lru", "┼": "lrud",
    "╭": "rd", "╮": "ld", "╰": "ru", "╯": "lu",
}
HEAVY = {
    "━": "lr", "┃": "ud", "╸": "l", "╺": "r", "╹": "u", "╻": "d",
    "┏": "rd", "┓": "ld", "┗": "ru", "┛": "lu", "┣": "udr", "┫": "udl",
    "┳": "lrd", "┻": "lru", "╋": "lrud",
}
QUAD = {"▖": "c", "▗": "d", "▘": "a", "▝": "b", "▙": "acd", "▚": "ad",
        "▛": "abc", "▜": "abd", "▞": "bc", "▟": "bcd"}


def geometric(d, ch, x, y, fg, bg):
    """Draw cell-joining glyphs as shapes. Returns True if handled."""
    cx, cy = x + CW // 2, y + CH // 2
    if ch in LIGHT or ch in HEAVY:
        segs = LIGHT.get(ch) or HEAVY[ch]
        w = LW if ch in LIGHT else HW
        h0 = w // 2
        for s in segs:
            if s == "l":
                d.rectangle([x, cy - h0, cx + w - 1 - h0, cy - h0 + w - 1], fill=fg)
            if s == "r":
                d.rectangle([cx - h0, cy - h0, x + CW - 1, cy - h0 + w - 1], fill=fg)
            if s == "u":
                d.rectangle([cx - h0, y, cx - h0 + w - 1, cy + w - 1 - h0], fill=fg)
            if s == "d":
                d.rectangle([cx - h0, cy - h0, cx - h0 + w - 1, y + CH - 1], fill=fg)
        return True
    if ch in QUAD:
        hw, hh = CW // 2, CH // 2
        boxes = {"a": (0, 0), "b": (1, 0), "c": (0, 1), "d": (1, 1)}
        for q in QUAD[ch]:
            qx, qy = boxes[q]
            x1 = x + CW - 1 if qx else x + hw - 1
            y1 = y + CH - 1 if qy else y + hh - 1
            d.rectangle([x + qx * hw, y + qy * hh, x1, y1], fill=fg)
        return True
    if ch in "┄┈╌":
        n = 3 if ch == "┄" else 4
        seg = CW / n
        for k in range(n):
            x0 = x + int(k * seg)
            d.rectangle([x0, cy, x0 + max(1, int(seg / 2)) - 1, cy + LW - 1], fill=fg)
        return True
    if ch == "┊":
        for k in range(4):
            y0 = y + int(k * CH / 4)
            d.rectangle([cx, y0, cx + LW - 1, y0 + max(1, CH // 8)], fill=fg)
        return True
    o = ord(ch)
    if 0x2581 <= o <= 0x2588:  # lower eighths .. full block
        h = round(CH * (o - 0x2580) / 8)
        d.rectangle([x, y + CH - h, x + CW - 1, y + CH - 1], fill=fg)
        return True
    if 0x2589 <= o <= 0x258F:  # left seven-eighths .. one-eighth
        w = round(CW * (0x2590 - o) / 8)
        d.rectangle([x, y, x + w - 1, y + CH - 1], fill=fg)
        return True
    if ch == "▀":
        d.rectangle([x, y, x + CW - 1, y + CH // 2 - 1], fill=fg)
        return True
    if ch == "▐":
        d.rectangle([x + CW // 2, y, x + CW - 1, y + CH - 1], fill=fg)
        return True
    if ch in "░▒▓":
        k = {"░": 0.25, "▒": 0.5, "▓": 0.75}[ch]
        d.rectangle([x, y, x + CW - 1, y + CH - 1], fill=blend(bg, fg, k))
        return True
    if 0x2800 <= o <= 0x28FF:  # braille
        bits = o - 0x2800
        dots = [(0, 0, 0x01), (0, 1, 0x02), (0, 2, 0x04), (1, 0, 0x08),
                (1, 1, 0x10), (1, 2, 0x20), (0, 3, 0x40), (1, 3, 0x80)]
        r = max(1, CW // 5)
        for col, row, b in dots:
            if bits & b:
                px = x + (CW * (1 + 2 * col)) // 4
                py = y + (CH * (1 + 2 * row)) // 8
                d.ellipse([px - r, py - r, px + r - 1, py + r - 1], fill=fg)
        return True
    return False


def main():
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    src, out, ncols, nrows = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
    rows = parse(src)[:nrows]
    rows += [[] for _ in range(nrows - len(rows))]
    pad = SIZE * 3 // 4
    img = Image.new("RGB", (ncols * CW + pad * 2, nrows * CH + pad * 2), BG)
    d = ImageDraw.Draw(img)
    missing = set()
    for r, cells in enumerate(rows):
        for c, (ch, st) in enumerate(cells[:ncols]):
            x, y = pad + c * CW, pad + r * CH
            fg = st["fg"] or FG
            bg = st["bg"] or BG
            if st["rev"]:
                fg, bg = bg, fg
            if st["dim"]:
                fg = blend(fg, bg, 0.5)
            if bg != BG:
                d.rectangle([x, y, x + CW - 1, y + CH - 1], fill=bg)
            if ch == " ":
                continue
            if geometric(d, ch, x, y, fg, bg):
                continue
            f = fonts[st["bold"]]
            if CMAP is not None and ord(ch) not in CMAP:
                f = fallback[st["bold"]]
                missing.add(ch)
            d.text((x, y + BASE), ch, font=f, fill=fg)
    # A palette PNG is a fraction of the size of a truecolor one and, for
    # flat terminal colours plus antialiased text, looks the same.
    img = img.quantize(colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
    img.save(out, optimize=True)
    if missing:
        print("glyphs drawn from the fallback font:", "".join(sorted(missing)), file=sys.stderr)
    print(f"{out}: {img.size[0]}x{img.size[1]} ({ncols}x{nrows} cells, font {os.path.basename(REG)})")


if __name__ == "__main__":
    main()
