"""Render ANSI (truecolor / 256 / 16) terminal text to a PNG so it can be eyeballed."""
import re, sys
from PIL import Image, ImageDraw, ImageFont

FONT = ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf', 16)
CW, CH = 10, 20
BG = (24, 24, 32)
FG = (220, 220, 220)
BASE16 = [(0,0,0),(205,49,49),(13,188,121),(229,229,16),(36,114,200),(188,63,188),(17,168,205),(229,229,229),
          (102,102,102),(241,76,76),(35,209,139),(245,245,67),(59,142,234),(214,112,214),(41,184,219),(255,255,255)]

def c256(n):
    if n < 16: return BASE16[n]
    if n < 232:
        n -= 16; r, g, b = n // 36, (n // 6) % 6, n % 6
        f = lambda v: 0 if v == 0 else 55 + v * 40
        return (f(r), f(g), f(b))
    v = 8 + (n - 232) * 10
    return (v, v, v)

def parse(text):
    rows = []
    fg, bg, bold = None, None, False
    for line in text.split('\n'):
        row = []
        i = 0
        while i < len(line):
            m = re.match(r'\x1b\[([0-9;?]*)([A-Za-z])', line[i:])
            if m:
                i += m.end()
                if m.group(2) == 'C':
                    row.extend([(' ', fg, bg)] * int(m.group(1) or 1)); continue
                if m.group(2) != 'm': continue
                ps = [int(p) if p else 0 for p in m.group(1).split(';')] if m.group(1) else [0]
                j = 0
                while j < len(ps):
                    p = ps[j]
                    if p == 0: fg, bg, bold = None, None, False
                    elif p == 1: bold = True
                    elif p == 22: bold = False
                    elif p == 39: fg = None
                    elif p == 49: bg = None
                    elif 30 <= p <= 37: fg = BASE16[p - 30 + (8 if bold else 0)]
                    elif 90 <= p <= 97: fg = BASE16[p - 90 + 8]
                    elif 40 <= p <= 47: bg = BASE16[p - 40]
                    elif 100 <= p <= 107: bg = BASE16[p - 100 + 8]
                    elif p in (38, 48):
                        if ps[j+1] == 2: col = tuple(ps[j+2:j+5]); j += 4
                        else: col = c256(ps[j+2]); j += 2
                        if p == 38: fg = col
                        else: bg = col
                    j += 1
                continue
            row.append((line[i], fg, bg)); i += 1
        rows.append(row)
    return rows

def render(text, out):
    rows = parse(text)
    w = max((len(r) for r in rows), default=1)
    im = Image.new('RGB', (w * CW + 20, len(rows) * CH + 20), BG)
    d = ImageDraw.Draw(im)
    for y, row in enumerate(rows):
        for x, (ch, fg, bg) in enumerate(row):
            px, py = 10 + x * CW, 10 + y * CH
            if bg: d.rectangle([px, py, px + CW - 1, py + CH - 1], fill=bg)
            if ch == '▀':
                d.rectangle([px, py, px + CW - 1, py + CH // 2 - 1], fill=fg or FG)
            elif ch == '▄':
                d.rectangle([px, py + CH // 2, px + CW - 1, py + CH - 1], fill=fg or FG)
            elif ch == '█':
                d.rectangle([px, py, px + CW - 1, py + CH - 1], fill=fg or FG)
            elif ch != ' ':
                d.text((px, py + 1), ch, font=FONT, fill=fg or FG)
    im.save(out)

if __name__ == '__main__':
    render(sys.stdin.read(), sys.argv[1])
