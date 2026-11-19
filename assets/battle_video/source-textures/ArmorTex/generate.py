#!/usr/bin/env python3
"""Rebuild the battle pack's 4x textures from the adjacent original PNGs.

Requires Pillow. Original texel positions and alpha are retained exactly; all
high-frequency detail is authored within those texels. Run from any directory.
"""
from math import cos, pi, sin
from pathlib import Path
from PIL import Image

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
                shade = 9 * sin(x * 0.105) + 3 * sin(y * 0.24) + (2 if (x + y) % 4 == 0 else -1)
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
    destination.parent.mkdir(parents=True, exist_ok=True)
    out.save(destination, optimize=True)


for name in ('iron_plate', 'crested_helm'):
    build(ROOT / 'humanoid' / (name + '.png'), TEXTURES / 'entity/equipment/humanoid' / (name + '.png'), 'metal')
for name in ('iron_plate_plume', 'crested_helm_plume'):
    build(ROOT / 'humanoid' / (name + '.png'), TEXTURES / 'entity/equipment/humanoid' / (name + '.png'), 'plume')
for name in ('red_surcoat', 'blue_surcoat'):
    build(ROOT / 'humanoid' / (name + '.png'), TEXTURES / 'entity/equipment/humanoid' / (name + '.png'), 'cloth')
build(ROOT / 'humanoid_leggings/iron_plate.png', TEXTURES / 'entity/equipment/humanoid_leggings/iron_plate.png', 'metal')
for name in ('battle_parts', 'iron_spear', 'iron_halberd', 'iron_lance', 'iron_longsword'):
    build(ROOT / 'item' / (name + '.png'), TEXTURES / 'item' / (name + '.png'), 'metal')
for name in ('red_heater_shield', 'blue_heater_shield', 'iron_kite_shield', 'iron_tower_shield'):
    build(ROOT / 'item' / (name + '.png'), TEXTURES / 'item' / (name + '.png'), 'shield')
