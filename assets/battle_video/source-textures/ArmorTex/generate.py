#!/usr/bin/env python3
"""Rebuild the battle pack's 4x textures from the adjacent original PNGs.

Requires Pillow. Original texel positions and alpha are retained exactly; all
high-frequency detail is authored within those texels. Run from any directory.
"""
from math import cos, pi, sin
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent
PACK = ROOT.parent.parent
TEXTURES = PACK / 'assets/krasnokamen_battle/textures'
SCALE = 4


def clamp(v):
    return max(0, min(255, round(v)))


def build(source, destination, kind):
    image = Image.open(source).convert('RGBA')
    w, h = image.size
    src = image.load()
    out = Image.new('RGBA', (w * SCALE, h * SCALE))
    dst = out.load()
    for y in range(h * SCALE):
        for x in range(w * SCALE):
            sx, sy = x // SCALE, y // SCALE
            r, g, b, a = src[sx, sy]
            if not a:
                continue
            # Repeatable sub-texel light, leaving original layout and silhouette intact.
            grey = max(r, g, b) - min(r, g, b) < 22
            steel = grey and max(r, g, b) > 60
            if kind == 'plume':
                shade = 5 * sin(x * 0.47 + y * 0.29) + 3 * cos(x * 0.85)
            elif kind == 'cloth' or (kind == 'shield' and not grey):
                # Multiplicative folds retain saturated dyes and the heraldic
                # mark; a fine diagonal cross-thread breaks the flat colour.
                if max(r, g, b) - min(r, g, b) > 40:
                    factor = 1 + 0.30 * sin(x * 0.075) + 0.07 * sin(y * 0.13)
                    factor += 0.025 if (x + y) % 4 == 0 else -0.01
                    dst[x, y] = (clamp(r*factor), clamp(g*factor), clamp(b*factor), a)
                    continue
                shade = 5 * sin(x * 0.105) + 3 * sin(y * 0.24)
            elif steel:
                # Broad cool reflection, thin dark contour, and sparse hairline nicks.
                shade = 12 * cos((x % 64) * 2 * pi / 64) + 5 * sin(y * 0.12)
                if (x * 13 + y * 29) % 389 == 0:
                    shade -= 23
                if (x * 7 + y * 19) % 557 == 0:
                    shade += 27
                # Detect a true transparent edge, not internal UV-island divisions.
                if any(not (0 <= nx < w and 0 <= ny < h) or src[nx, ny][3] == 0
                       for nx, ny in ((sx - 1, sy), (sx + 1, sy), (sx, sy - 1), (sx, sy + 1))):
                    shade -= 11
            else:
                # Wood grain / leather, without shifting the team-colour palette.
                shade = 5 * sin(x * 0.31 + sin(y * 0.08)) + 2 * cos(y * 0.43)
            dst[x, y] = (clamp(r + shade), clamp(g + shade), clamp(b + shade), a)
    if source.stem == 'battle_parts':
        # The 3-D weapons share tiny swatches: spear blade faces use UV
        # [2.5,0.5,3.5,1.5] (0..16 space). Paint a bevel across that
        # exact 8x8 HD region instead of the nearly uniform source swatch.
        for y in range(4, 12):
            for x in range(20, 28):
                shade = (-42, -26, -8, 25, 38, 12, -12, -32)[x - 20]
                r, g, b, a = dst[x, y]
                dst[x, y] = (clamp(r + shade), clamp(g + shade), clamp(b + shade), a)
        # Shaft UV is [2.5,2.5,3.5,3.5]; long narrow grain runs along
        # the length of each 3-D face. Socket UV is [6.5,0.5,7.5,1.5].
        for y in range(20, 28):
            for x in range(20, 28):
                r, g, b, a = dst[x, y]
                shade = (-30, -12, 9, 18, -13, 5, -21, 8)[x-20]
                dst[x, y] = (clamp(r+shade), clamp(g+shade), clamp(b+shade), a)
        for y in range(4, 12):
            for x in range(52, 60):
                r, g, b, a = dst[x, y]
                shade = (-25, 25, 10, -12, -20, 12, 30, -20)[y-4]
                dst[x, y] = (clamp(r+shade), clamp(g+shade), clamp(b+shade), a)
    if source.parent.name == 'humanoid' and source.stem == 'crested_helm':
        # The shell is a 9x9x9 box at UV (0,0). Its four side faces occupy
        # adjacent 9x9 squares; the original plume UV starts at x=36.
        draw = ImageDraw.Draw(out)
        for face in range(4):
            x = face * 36
            for row in range(36):
                # A bright rolled rim over a cool middle and a dark lower lip.
                t = row / 35
                level = round(198 - 72*t + 46*max(0, 1 - abs(t-.13)/.12))
                draw.line((x, 36+row, x+35, 36+row), fill=(level-9, level, min(255,level+12), 255))
            draw.rectangle((x, 36, x+35, 71), outline=(43, 49, 57, 255), width=2)
            draw.line((x+2, 40, x+33, 40), fill=(236, 244, 248, 255), width=2)
            draw.line((x+2, 65, x+33, 65), fill=(54, 60, 66, 255), width=3)
            draw.line((x+3, 68, x+32, 68), fill=(171, 180, 184, 255), width=1)
            # Rivets belong on the upper rim, not on the face of the visor.
            for cx in (x+3, x+32):
                draw.point((cx, 42), fill=(45, 48, 50, 255))
                draw.point((cx, 41), fill=(252, 245, 221, 255))
            # Continuous eye slit above a row of small breathing perforations.
            draw.line((x+3, 51, x+32, 51), fill=(32, 39, 46, 255), width=3)
            draw.line((x+4, 49, x+31, 49), fill=(225, 234, 236, 255), width=1)
            for cx in range(x+7, x+31, 4):
                draw.point((cx, 59), fill=(35, 39, 43, 255))
                draw.point((cx, 60), fill=(54, 59, 63, 255))
        # Reflective crown face and an axial raised ridge.
        for y in range(0, 36):
            level = round(148 + 52 * sin(pi * y / 35))
            draw.line((36, y, 71, y), fill=(level-7, level, level+8, 255))
        draw.rectangle((36, 0, 71, 35), outline=(53, 62, 70, 255), width=2)
        draw.line((52, 2, 52, 33), fill=(246, 249, 246, 255), width=2)
        draw.line((55, 2, 55, 33), fill=(67, 78, 88, 255), width=2)
    if source.stem in ('iron_plate', 'holy'):
        # Shade the UV-unwrapped armor panels without painting over the
        # source's colored decorations, straps or transparent texels.
        iron_panels = ((71, 28, 9, 9), (80, 28, 9, 9),
                       (89, 28, 9, 9), (98, 28, 9, 9),
                       (38, 13, 10, 10), (48, 13, 10, 10),
                       (28, 23, 10, 3), (38, 23, 10, 3),
                       (48, 23, 10, 3), (58, 23, 10, 3),
                       (0, 17, 5, 13), (5, 17, 9, 13),
                       (14, 17, 5, 13), (19, 17, 9, 13),
                       (0, 67, 5, 5), (5, 67, 6, 5),
                       (11, 67, 5, 5), (16, 67, 6, 5))
        holy_panels = ((0, 33, 5, 13), (5, 33, 9, 13),
                       (14, 33, 5, 13), (19, 33, 9, 13),
                       (25, 28, 7, 5), (32, 28, 6, 5),
                       (38, 28, 7, 5), (45, 28, 6, 5),
                       (32, 21, 6, 7), (38, 21, 6, 7),
                       (46, 33, 5, 12), (51, 33, 5, 12),
                       (56, 33, 5, 12), (61, 33, 5, 12),
                       (37, 49, 4, 5), (41, 49, 4, 5),
                       (45, 49, 4, 5), (49, 49, 4, 5))
        overlay = Image.new('RGBA', out.size)
        draw = ImageDraw.Draw(overlay)
        for ux, uy, width, height in (iron_panels if source.stem == 'iron_plate' else holy_panels):
            x, y, w1, h1 = (round(v * SCALE) for v in (ux, uy, width, height))
            for yy in range(h1):
                for xx in range(w1):
                    across = xx / max(1, w1-1)
                    down = yy / max(1, h1-1)
                    light = round(195 - 80*down + 35*cos((across-.30)*pi))
                    draw.point((x+xx, y+yy), fill=(max(0,light-9), light, min(255,light+12), 135))
            draw.rectangle((x, y, x+w1-1, y+h1-1), outline=(45, 52, 58, 200), width=2)
            draw.line((x+3, y+4, x+w1-4, y+4), fill=(235, 245, 255, 230), width=2)
            draw.line((x+3, y+h1-5, x+w1-4, y+h1-5), fill=(55, 53, 50, 200), width=2)
            draw.line((x+w1//3, y+6, x+w1//3, y+h1-7), fill=(220, 233, 245, 125), width=2)
            # Tiny edge fasteners, not two large dots in the middle of a plate.
            for cx in (x+3, x+w1-4):
                draw.point((cx, y+2), fill=(36, 36, 35, 255))
                draw.point((cx, y+3), fill=(245, 237, 211, 255))
        mask = Image.new('L', out.size)
        m = mask.load()
        for y in range(out.height):
            for x in range(out.width):
                r, g, b, a = dst[x, y]
                if a and max(r, g, b) - min(r, g, b) < 28 and max(r, g, b) > 75:
                    m[x, y] = overlay.getpixel((x, y))[3]
        overlay.putalpha(255)
        out.paste(overlay, (0, 0), mask)
    if source.stem in ('red_surcoat', 'blue_surcoat'):
        # Keep the cross and original seams; darken the lower hem on opaque cloth.
        for y in range(out.height):
            for x in range(out.width):
                r, g, b, a = dst[x, y]
                if a and y >= out.height - 8 and max(r, g, b) - min(r, g, b) > 40:
                    factor = 0.50 if y >= out.height - 4 else 1.20
                    dst[x, y] = (clamp(r*factor), clamp(g*factor), clamp(b*factor), a)
    destination.parent.mkdir(parents=True, exist_ok=True)
    out.save(destination, optimize=True)


for name in ('iron_plate', 'crested_helm', 'holy'):
    build(ROOT / 'humanoid' / (name + '.png'), TEXTURES / 'entity/equipment/humanoid' / (name + '.png'), 'metal')
for name in ('iron_plate_plume', 'crested_helm_plume'):
    build(ROOT / 'humanoid' / (name + '.png'), TEXTURES / 'entity/equipment/humanoid' / (name + '.png'), 'plume')
for name in ('red_surcoat', 'blue_surcoat'):
    build(ROOT / 'humanoid' / (name + '.png'), TEXTURES / 'entity/equipment/humanoid' / (name + '.png'), 'cloth')
build(ROOT / 'humanoid_leggings/iron_plate.png', TEXTURES / 'entity/equipment/humanoid_leggings/iron_plate.png', 'metal')
build(ROOT / 'humanoid_leggings/holy.png', TEXTURES / 'entity/equipment/humanoid_leggings/holy.png', 'metal')
for name in ('battle_parts', 'iron_spear', 'iron_halberd', 'iron_lance', 'iron_longsword'):
    build(ROOT / 'item' / (name + '.png'), TEXTURES / 'item' / (name + '.png'), 'metal')
for name in ('red_heater_shield', 'blue_heater_shield', 'iron_kite_shield', 'iron_tower_shield'):
    build(ROOT / 'item' / (name + '.png'), TEXTURES / 'item' / (name + '.png'), 'shield')
