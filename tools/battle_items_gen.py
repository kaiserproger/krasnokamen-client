#!/usr/bin/env python3
"""Generates the battle-video item models and textures (assets/battle_video/assets/solaris_battle).

Own art, no third-party bytes: shields are the vanilla shield's geometry (plate 12x22x1, handle
2x6x6, read from the local ShieldModel class) built from elements with a painted heraldic texture;
weapons are boxes painted from a 32x32 swatch palette. Run from the client root:

    python3 tools/battle_items_gen.py            # rewrites models/item/*.json and textures/item/*.png

Weapon coordinates are authored in "real" pixels relative to the grip (x right, y along the
weapon toward the tip, z depth), mapped to the model grid (grip at 8,8,8) by the per-item factor
`f`; the display scale is divided by `f` so the in-hand size stays as authored.
"""
import json, math, os, sys
from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PACK = os.path.join(ROOT, "assets/battle_video/assets/solaris_battle")
MODELS = os.path.join(PACK, "models/item")
TEX = os.path.join(PACK, "textures/item")

# ------------------------------------------------------------------ palette (weapons)
SWATCHES = {  # name: colour, 4x4 px cell each, row-major in a 32x32 texture
    "steel_hi": (222, 226, 232), "steel": (178, 184, 194), "steel_lo": (128, 134, 146), "steel_dk": (84, 88, 100),
    "iron_dk": (58, 60, 68), "gold": (214, 176, 70), "gold_dk": (150, 112, 40), "wood": (140, 96, 52),
    "wood_dk": (98, 64, 34), "wood_lt": (176, 128, 74), "leather": (92, 58, 40), "leather_dk": (62, 38, 28),
    "cloth_red": (176, 36, 40), "cloth_blue": (40, 74, 176), "bone": (232, 226, 206), "string": (206, 200, 180),
    "fletch": (236, 236, 228),
}
NAMES = list(SWATCHES)


def swatch_uv(name):
    i = NAMES.index(name)
    cx, cy = (i % 8) * 4, (i // 8) * 4
    # inner 2x2 px of the 4x4 cell, in 0..16 uv units of the 32 px texture
    return [(cx + 1) / 2, (cy + 1) / 2, (cx + 3) / 2, (cy + 3) / 2]


def write_palette():
    im = Image.new("RGBA", (32, 32), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    for i, n in enumerate(NAMES):
        cx, cy = (i % 8) * 4, (i // 8) * 4
        d.rectangle([cx, cy, cx + 3, cy + 3], fill=SWATCHES[n] + (255,))
    im.save(os.path.join(TEX, "battle_parts.png"))


# ------------------------------------------------------------------ element helpers
def r3(v):
    return [round(x, 4) for x in v]


class Weapon:
    def __init__(self, f=1.0):
        self.f = f
        self.elements = []

    def box(self, lo, hi, side, ends=None, front=None, rot=None):
        """lo/hi in authored px relative to the grip. side: swatch for the long faces; ends: swatch for the
        two end faces (default side); front: swatch for the +z/-z faces (default side).
        rot: (axis, angle, origin(authored rel))."""
        f = self.f
        lo = [lo[0], lo[1], lo[2]]
        hi = [hi[0], hi[1], hi[2]]
        el = {"from": r3([8 + lo[i] * f for i in range(3)]), "to": r3([8 + hi[i] * f for i in range(3)])}
        ends = ends or side
        front = front or side
        faces = {}
        for face, sw in (("north", front), ("south", front), ("east", side), ("west", side), ("up", ends), ("down", ends)):
            faces[face] = {"uv": [round(v, 3) for v in swatch_uv(sw)], "texture": "#parts"}
        el["faces"] = faces
        if rot:
            axis, angle, origin = rot
            el["rotation"] = {"origin": r3([8 + origin[i] * f for i in range(3)]), "axis": axis, "angle": angle}
        self.elements.append(el)

    def model(self, display):
        return {"textures": {"parts": "solaris_battle:item/battle_parts", "particle": "solaris_battle:item/battle_parts"},
                "elements": self.elements, "display": display}


def hand_display(f, rot_tp, tr_tp, sc_tp, rot_fp, tr_fp, sc_fp, gui_scale, gui_rot=(30, 45, 0), gui_tr=(0, 0, 0)):
    k = 1.0 / f
    d = {
        "thirdperson_righthand": {"rotation": rot_tp, "translation": tr_tp, "scale": [round(sc_tp * k, 4)] * 3},
        "firstperson_righthand": {"rotation": rot_fp, "translation": tr_fp, "scale": [round(sc_fp * k, 4)] * 3},
        "gui": {"rotation": list(gui_rot), "translation": list(gui_tr), "scale": [round(gui_scale * k, 4)] * 3},
        "ground": {"rotation": [0, 0, 0], "translation": [0, 3, 0], "scale": [round(0.5 * k, 4)] * 3},
        "fixed": {"rotation": [0, 0, 0], "translation": [0, 0, 0], "scale": [round(0.7 * k, 4)] * 3},
    }
    return d


def save_model(name, model):
    os.makedirs(MODELS, exist_ok=True)
    with open(os.path.join(MODELS, name + ".json"), "w") as fh:
        json.dump(model, fh, indent=1)
        fh.write("\n")


def save_item(name, definition):
    d = os.path.join(PACK, "items")
    with open(os.path.join(d, name + ".json"), "w") as fh:
        json.dump(definition, fh, indent=2)
        fh.write("\n")


# ------------------------------------------------------------------ weapons
def longsword():
    w = Weapon(0.75)
    w.box((-0.9, -9, -0.9), (0.9, -4, 0.9), "leather", "leather_dk")        # grip
    for k in range(4):                                                      # grip wrap bands
        w.box((-1.05, -8.6 + k * 1.2, -1.05), (1.05, -8.0 + k * 1.2, 1.05), "leather_dk", "leather_dk")
    w.box((-1.5, -11, -1.5), (1.5, -9, 1.5), "gold", "gold_dk")             # pommel
    w.box((-5.5, -4, -1.2), (5.5, -2.4, 1.2), "steel_dk", "steel_lo")       # crossguard
    w.box((-6.2, -4.2, -0.9), (-5.2, -2.2, 0.9), "gold", "gold_dk")         # guard tips
    w.box((5.2, -4.2, -0.9), (6.2, -2.2, 0.9), "gold", "gold_dk")
    w.box((-1.6, -2.4, -0.55), (1.6, 17, 0.55), "steel", "steel_hi", "steel")  # blade
    w.box((-0.35, -2.2, -0.62), (0.35, 15, 0.62), "steel_lo", "steel_lo")   # fuller
    w.box((-1.2, 17, -0.45), (1.2, 19.5, 0.45), "steel", "steel_hi")        # taper
    w.box((-0.6, 19.5, -0.35), (0.6, 21, 0.35), "steel_hi", "steel_hi")     # tip
    disp = hand_display(0.75, [0, 0, 0], [0, 0, 0], 0.85, [0, -90, -20], [1.13, 3.2, 1.13], 0.68, 0.62, (30, 45, -40))
    return w.model(disp)


def mace():
    w = Weapon(1.0)
    w.box((-0.9, -10, -0.9), (0.9, 6, 0.9), "wood", "wood_dk")              # haft
    for k in range(3):
        w.box((-1.1, -9 + k * 1.4, -1.1), (1.1, -8.2 + k * 1.4, 1.1), "leather_dk", "leather_dk")
    w.box((-1.4, -12, -1.4), (1.4, -10, 1.4), "steel_dk", "iron_dk")        # butt cap
    w.box((-1.6, 6, -1.6), (1.6, 11, 1.6), "steel", "steel_hi")             # head core
    for ang in (0, 60, 120):                                                # flanges: 6 plates round the core
        w.box((-3.4, 6.4, -0.55), (3.4, 10.6, 0.55), "steel_lo", "steel_hi", rot=("y", ang, (0, 8, 0)))
    w.box((-1.0, 11, -1.0), (1.0, 12.6, 1.0), "steel_hi", "steel_hi")       # top spike
    disp = hand_display(1.0, [0, 0, 0], [0, 0, 0], 0.85, [0, -90, -20], [1.13, 3.2, 1.13], 0.68, 0.8, (30, 45, -40))
    return w.model(disp)


def spear():
    w = Weapon(0.5)
    w.box((-0.9, -14, -0.9), (0.9, 24, 0.9), "wood_lt", "wood_dk")          # shaft
    w.box((-1.4, -16, -1.4), (1.4, -14, 1.4), "steel_dk", "iron_dk")        # butt spike cap
    w.box((-1.5, 24, -1.5), (1.5, 27, 1.5), "steel_dk", "iron_dk")          # socket
    w.box((-2.7, 27, -0.6), (2.7, 32, 0.6), "steel", "steel_hi", "steel")   # leaf lower
    w.box((-1.6, 32, -0.55), (1.6, 36, 0.55), "steel", "steel_hi")          # leaf mid
    w.box((-0.7, 36, -0.5), (0.7, 39, 0.5), "steel_hi", "steel_hi")         # tip
    w.box((-0.3, 27, -0.7), (0.3, 37, 0.7), "steel_lo", "steel_lo")         # midrib
    w.box((-1.0, 8, -1.2), (1.0, 11, 1.2), "cloth_red", "cloth_red")        # tassel
    disp = hand_display(0.5, [-15, 0, 0], [0, 0, 0], 0.8, [0, -90, -20], [1.13, 3.2, 1.13], 0.6, 0.62, (30, 45, -40))
    return w.model(disp)


def lance():
    w = Weapon(0.5)
    w.box((-1.1, -22, -1.1), (1.1, 36, 1.1), "wood", "wood_dk")             # shaft
    w.box((-1.5, -26, -1.5), (1.5, -22, 1.5), "steel_dk", "iron_dk")        # butt
    for k, wd in enumerate((2.0, 2.8, 3.6, 4.4)):                           # vamplate: stepped cone/disc
        w.box((-wd, -4 + k * 1.2, -wd), (wd, -2.8 + k * 1.2, wd), "steel_lo", "steel")
    w.box((-1.5, 36, -1.5), (1.5, 41, 1.5), "steel_dk", "iron_dk")          # coronet
    w.box((-1.9, 41, -1.9), (1.9, 46, 1.9), "steel", "steel_hi")            # head base
    w.box((-1.3, 46, -1.3), (1.3, 50, 1.3), "steel", "steel_hi")
    w.box((-0.7, 50, -0.7), (0.7, 54, 0.7), "steel_hi", "steel_hi")         # point
    w.box((0, 31, -0.2), (7, 38, 0.2), "cloth_red", "cloth_red")            # pennon
    disp = hand_display(0.5, [-65, 0, 0], [0, 0, 0], 0.8, [0, -90, -20], [1.13, 3.2, 1.13], 0.6, 0.5, (30, 45, -40))
    return w.model(disp)


def halberd():
    w = Weapon(0.5)
    w.box((-0.9, -16, -0.9), (0.9, 32, 0.9), "wood", "wood_dk")             # shaft
    w.box((-1.4, -18, -1.4), (1.4, -16, 1.4), "steel_dk", "iron_dk")
    w.box((-1.3, 30, -1.3), (1.3, 33, 1.3), "steel_dk", "iron_dk")          # head socket
    w.box((-0.7, 33, -0.6), (0.7, 45, 0.6), "steel", "steel_hi")            # spike
    w.box((-0.35, 45, -0.5), (0.35, 49, 0.5), "steel_hi", "steel_hi")
    w.box((0.9, 31, -0.5), (3.5, 44, 0.5), "steel", "steel_hi")             # axe blade (+x)
    w.box((3.5, 33, -0.45), (6.0, 42, 0.45), "steel", "steel_hi")
    w.box((6.0, 35, -0.4), (7.2, 40, 0.4), "steel_hi", "steel_hi")
    w.box((-0.9, 30, -0.7), (-6.5, 34, 0.7), "steel_lo", "steel_hi")        # back hook (-x)
    w.box((-6.5, 30, -0.6), (-8.0, 34, 0.6), "steel_lo", "steel_hi")
    w.box((-8.0, 29, -0.5), (-9.2, 33, 0.5), "steel_hi", "steel_hi")
    w.box((-1.1, 4, -1.1), (1.1, 6, 1.1), "leather_dk", "leather_dk")       # grip wraps
    w.box((-1.1, 8, -1.1), (1.1, 10, 1.1), "leather_dk", "leather_dk")
    disp = hand_display(0.5, [-10, 0, 0], [0, 0, 0], 0.8, [0, -90, -20], [1.13, 3.2, 1.13], 0.6, 0.5, (30, 45, -40))
    return w.model(disp)


def longbow():
    w = Weapon(0.75)
    # a limb is a chain of angled segments, bulging toward +x (the belly faces -x toward the archer)
    pts = [(0, 0), (1.0, 3), (1.9, 6.5), (2.4, 11), (2.2, 15.5), (1.4, 19), (0.0, 21.5)]
    for sgn in (1, -1):
        for a, b in zip(pts[:-1], pts[1:]):
            ax, ay = a
            bx, by = b
            ay, by = ay * sgn, by * sgn
            length = math.hypot(bx - ax, by - ay)
            ang = math.degrees(math.atan2(bx - ax, by - ay))  # from +y toward +x
            cx, cy = (ax + bx) / 2, (ay + by) / 2
            lo = (cx - 0.9, cy - length / 2 - 0.3, -0.9)
            hi = (cx + 0.9, cy + length / 2 + 0.3, 0.9)
            w.box(lo, hi, "wood", "wood_dk", rot=("z", -ang, (cx, cy, 0)))
    w.box((-0.6, -3, -1.2), (2.2, 3, 1.2), "leather", "leather_dk")           # grip wrap
    w.box((-0.5, 21.0, -0.8), (0.8, 22.5, 0.8), "bone", "bone")                   # nocks
    w.box((-0.5, -22.5, -0.8), (0.8, -21.0, 0.8), "bone", "bone")
    w.box((-0.1, -21.8, -0.15), (0.2, 21.8, 0.15), "string", "string")           # string
    disp = hand_display(0.75, [0, -90, -25], [0, 0, 0], 0.9, [0, -90, -25], [1.13, 3.2, 1.13], 0.68, 0.6, (30, 45, -40))
    return w.model(disp)


def crossbow():
    w = Weapon(0.75)
    w.box((-1.4, -14, -1.6), (1.4, 12, 1.6), "wood", "wood_dk", "wood_lt")  # stock
    w.box((-0.6, -14, -2.0), (0.6, 10, -1.6), "iron_dk", "iron_dk")         # rail
    w.box((-1.6, -17, -1.8), (1.6, -14, 1.8), "wood_dk", "wood_dk")         # butt
    w.box((-1.3, 11, -1.8), (1.3, 15, 1.8), "steel_dk", "iron_dk")          # prod housing
    pts = [(0, 0), (4, -0.8), (8, -2.2), (11, -4.5)]                          # steel prod, swept back
    for sgn in (1, -1):
        for a, b in zip(pts[:-1], pts[1:]):
            ax, ay = a[0] * sgn, a[1]
            bx, by = b[0] * sgn, b[1]
            length = math.hypot(bx - ax, by - ay)
            ang = math.degrees(math.atan2(by - ay, bx - ax))
            cx, cy = (ax + bx) / 2, (ay + by) / 2 + 13
            w.box((cx - length / 2 - 0.2, cy - 0.7, -1.0), (cx + length / 2 + 0.2, cy + 0.7, 1.0), "steel", "steel_hi",
                  rot=("z", ang if sgn > 0 else -ang, (cx, cy, 0)))
    w.box((-11.2, 8.4, -0.15), (11.2, 8.8, 0.15), "string", "string", rot=None)  # string pulled to the lock
    w.box((-0.3, -4, -1.2), (0.3, 8, -2.6), "steel_hi", "steel_hi")         # bolt on the rail
    w.box((-0.5, 8, -3.0), (0.5, 10, -1.9), "steel_hi", "steel_hi")
    w.box((-0.5, -6, -2.5), (0.5, -4, -1.5), "fletch", "fletch")
    w.box((-0.7, -3, -2.2), (0.7, 0, -1.7), "leather", "leather_dk")        # trigger guard stub
    disp = hand_display(0.75, [-80, 180, 0], [0, 0, 0], 0.85, [0, -90, -20], [1.13, 3.2, 1.13], 0.68, 0.5, (30, 45, -40))
    return w.model(disp)


# ------------------------------------------------------------------ shields
SHIELD_W, SHIELD_H = 12, 22          # vanilla ShieldModel plate
PX = 4                               # texture px per model px of the painted face


def heater_rows():
    """[(y_top_above_plate_bottom, y_bottom, half_width)] heater silhouette, 1 px resolution, bottom pointed."""
    rows = []
    # full-width shoulders, then taper to a point at the bottom (22 rows; row 0 is the top)
    half = [6] * 12 + [6, 5.5, 5, 4.5, 4, 3.5, 3, 2.5, 2, 1.5]  # per 1 px row, top to bottom
    return half


def kite_rows():
    return [6] * 6 + [6, 6, 5.5, 5.5, 5, 5, 4.5, 4, 3.5, 3, 2.5, 2.5, 2, 2, 1.5, 1.5]


def tower_rows():
    return [7] * 22


def make_shield(name, rows, crest, field, trim, boss, w_half=None, tex_size=(64, 64)):
    """rows: half width per px row from the top. Returns (model, texture image)."""
    n = len(rows)
    # texture layout (64x64): painted front at (0,0) size (2*wmax*PX/ ...). Use PX=2 px per model px.
    P = 2
    wmax = int(max(rows) * 2)
    fw, fh = wmax * P, n * P
    tw, th = tex_size
    im = Image.new("RGBA", tex_size, (0, 0, 0, 0))
    px = im.load()
    # silhouette mask of the front painting
    mask = [[False] * fw for _ in range(fh)]
    for y in range(fh):
        half = rows[y // P] * P
        for x in range(fw):
            mask[y][x] = abs(x + 0.5 - fw / 2) <= half
    # paint: field, trim border, crest
    def inside(x, y, d):
        return all(0 <= x + dx < fw and 0 <= y + dy < fh and mask[y + dy][x + dx]
                   for dx in (-d, 0, d) for dy in (-d, 0, d))
    for y in range(fh):
        for x in range(fw):
            if not mask[y][x]:
                continue
            c = field
            shade = 1.0 + (((x * 7 + y * 13) % 5) - 2) * 0.012
            c = tuple(min(255, int(v * shade)) for v in c)
            if not inside(x, y, 2):
                c = trim
            elif not inside(x, y, 3):
                c = tuple(int(v * 0.6) for v in trim)
            px[x, y] = c + (255,)
    crest(im, fw, fh, px, mask)
    # swatches: wood back / edge, iron boss, grip
    sw = {"wood_back": (150, 104, 60), "wood_edge": (96, 62, 34), "wood_grain": (128, 86, 48),
          "grip": (88, 56, 36), "iron": boss}
    ox, oy = 0, 48
    for i, (k, col) in enumerate(sw.items()):
        for dx in range(4):
            for dy in range(4):
                px[ox + i * 4 + dx, oy + dy] = col + (255,)
        # light grain row for the back board
    for dx in range(4):
        px[ox + dx, oy + 2] = sw["wood_grain"] + (255,)
    uvr = lambda i: [(ox + i * 4 + 1) * 16 / tw, (oy + 1) * 16 / th, (ox + i * 4 + 3) * 16 / tw, (oy + 3) * 16 / th]
    # model: slabs per row group (merge equal widths). plate z: [9,10]; handle z: [3,9]
    zf0, zf1 = 9.0, 10.0
    elements = []
    top = 8 + n / 2  # plate top y in model space (centred on 8)
    runs = []
    for i, half in enumerate(rows):
        if runs and runs[-1][2] == half:
            runs[-1][1] = i + 1
        else:
            runs.append([i, i + 1, half])
    for a, b, half in runs:
        y_hi, y_lo = top - a, top - b
        x_lo, x_hi = 8 - half, 8 + half
        # painted face uv: continuous painting
        u0 = (fw / 2 - half * P) * 16 / tw
        u1 = (fw / 2 + half * P) * 16 / tw
        v0, v1 = a * P * 16 / th, b * P * 16 / th
        eu = uvr(1)
        face = {"north": {"uv": [round(x, 3) for x in uvr(0)], "texture": "#shield"},  # back (wood)
                "south": {"uv": [round(u0, 3), round(v0, 3), round(u1, 3), round(v1, 3)], "texture": "#shield"},
                "east": {"uv": [round(x, 3) for x in eu], "texture": "#shield"},
                "west": {"uv": [round(x, 3) for x in eu], "texture": "#shield"},
                "up": {"uv": [round(x, 3) for x in eu], "texture": "#shield"},
                "down": {"uv": [round(x, 3) for x in eu], "texture": "#shield"}}
        elements.append({"from": [x_lo, y_lo, zf0], "to": [x_hi, y_hi, zf1], "faces": face})
    # boss on the front
    bf = {k: {"uv": [round(x, 3) for x in uvr(4)], "texture": "#shield"} for k in ("north", "south", "east", "west", "up", "down")}
    ycen = top - n * 0.36
    elements.append({"from": [6.5, ycen - 1.5, zf1], "to": [9.5, ycen + 1.5, zf1 + 1], "faces": bf})
    # handle (vanilla: 2x6x6 behind the plate) with a grip bar
    hf = {k: {"uv": [round(x, 3) for x in uvr(3)], "texture": "#shield"} for k in ("north", "south", "east", "west", "up", "down")}
    elements.append({"from": [7, ycen - 3, 3], "to": [9, ycen + 3, 9], "faces": hf})
    # vanilla display for the shield item (read from the local shield.json / shield_blocking.json)
    disp = {
        "thirdperson_righthand": {"rotation": [0, 90, 0], "translation": [10, 6, -4], "scale": [1, 1, 1]},
        "thirdperson_lefthand": {"rotation": [0, 90, 0], "translation": [10, 6, 12], "scale": [1, 1, 1]},
        "firstperson_righthand": {"rotation": [0, 180, 5], "translation": [-10, 1.75, -10], "scale": [1.25, 1.25, 1.25]},
        "firstperson_lefthand": {"rotation": [0, 180, 5], "translation": [10, 0, -10], "scale": [1.25, 1.25, 1.25]},
        "gui": {"rotation": [15, -25, -5], "translation": [2, 3, 0], "scale": [0.65, 0.65, 0.65]},
        "fixed": {"rotation": [0, 180, 0], "translation": [-4.5, 4.5, -5], "scale": [0.55, 0.55, 0.55]},
        "ground": {"rotation": [0, 0, 0], "translation": [2, 4, 2], "scale": [0.25, 0.25, 0.25]},
    }
    model = {"textures": {"shield": f"solaris_battle:item/{name}", "particle": f"solaris_battle:item/{name}"},
             "elements": elements, "display": disp}
    # shield_blocking.json display (same local read): the board turns to the front
    blocking = json.loads(json.dumps(model))
    blocking["display"].update({
        "thirdperson_righthand": {"rotation": [45, 155, 0], "translation": [-3.49, 11, -2], "scale": [1, 1, 1]},
        "thirdperson_lefthand": {"rotation": [45, 155, 0], "translation": [11.51, 7, 2.5], "scale": [1, 1, 1]},
        "firstperson_righthand": {"rotation": [0, 180, -5], "translation": [-15, 3.25, -11], "scale": [1.25, 1.25, 1.25]},
        "firstperson_lefthand": {"rotation": [0, 180, -5], "translation": [5, 5, -11], "scale": [1.25, 1.25, 1.25]},
    })
    return model, blocking, im


def cross_crest(col, arm=0.34, width=0.2):
    def crest(im, fw, fh, px, mask):
        cx, cy = fw / 2, fh * 0.40
        for y in range(fh):
            for x in range(fw):
                if not mask[y][x]:
                    continue
                dx, dy = abs(x + 0.5 - cx), abs(y + 0.5 - cy)
                if (dx <= fw * width / 2 and dy <= fh * arm) or (dy <= fw * width / 2 and dx <= fw * arm * 0.9 and dx <= fw * 0.30):
                    # inset from the trim
                    px[x, y] = col + (255,)
    return crest


def chevron_crest(col):
    def crest(im, fw, fh, px, mask):
        for y in range(fh):
            for x in range(fw):
                if not mask[y][x]:
                    continue
                dx = abs(x + 0.5 - fw / 2)
                line = fh * 0.22 + dx * 0.9
                if 0 <= y - line <= fh * 0.09 and dx < fw * 0.38:
                    px[x, y] = col + (255,)
    return crest


def plain_crest(im, fw, fh, px, mask):
    return


def banded_crest(col):
    def crest(im, fw, fh, px, mask):
        for y in range(fh):
            for x in range(fw):
                if mask[y][x] and abs(x + 0.5 - fw / 2) <= 2 and 6 <= y <= fh - 8:
                    px[x, y] = col + (255,)
    return crest


SHIELDS = {
    "red_heater_shield": (heater_rows(), cross_crest((236, 212, 150)), (150, 24, 28), (88, 62, 36), (190, 160, 70)),
    "blue_heater_shield": (heater_rows(), cross_crest((234, 232, 224)), (30, 56, 150), (88, 62, 36), (200, 200, 205)),
    "iron_kite_shield": (kite_rows(), chevron_crest((214, 176, 70)), (96, 102, 112), (58, 60, 70), (190, 196, 206)),
    "iron_tower_shield": (tower_rows(), banded_crest((190, 196, 206)), (110, 116, 126), (58, 60, 70), (190, 196, 206)),
}


def main():
    os.makedirs(TEX, exist_ok=True)
    write_palette()
    for name, fn in (("iron_longsword", longsword), ("iron_flanged_mace", mace), ("iron_spear", spear), ("iron_lance", lance),
                     ("iron_halberd", halberd), ("iron_longbow_standby", longbow), ("iron_heavy_crossbow_standby", crossbow)):
        save_model(name, fn())
    for name, (rows, crest, field, trim, boss) in SHIELDS.items():
        model, blocking, im = make_shield(name, rows, crest, field, trim, boss)
        save_model(name, model)
        save_model(name + "_blocking", blocking)
        save_item(name, {"model": {"type": "minecraft:condition", "property": "minecraft:using_item",
                                    "on_false": {"type": "minecraft:model", "model": f"solaris_battle:item/{name}"},
                                    "on_true": {"type": "minecraft:model", "model": f"solaris_battle:item/{name}_blocking"}}})
        im.save(os.path.join(TEX, name + ".png"))
    print("generated")


if __name__ == "__main__":
    main()
