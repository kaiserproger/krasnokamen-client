#!/usr/bin/env python3
"""Preview renderer for item models (element models, flat `item/generated|handheld` sprites,
condition/special item definitions are NOT evaluated: pass the model id).

  item_models.py items MODEL_ID... --pack DIR [-o out.png]
      each model alone: front / 3-4 / side / top, side by side (the GUI-less raw geometry).
  item_models.py hand  MODEL_ID... --pack DIR [--hand right|left] [--pose forward|hang|block] [-o out.png]
      each model held by an armoured husk soldier through the client's own chain
      (arm bone * ItemInHandLayer attachment * display transform), front / 3-4 / side.
  item_models.py first RIGHT_MODEL|- LEFT_MODEL|- [--pose block] -o out.png
      first-person frame (camera space, FOV 70) with the right/left hand items at the client's rest anchor.
  item_models.py compare --old DIR --pack DIR MODEL_ID... [--hand ...] -o out.png
      `hand` rows, old pack on top, new pack below.

DIR is a pack root (the directory holding assets/<namespace>/...). Vanilla models and
textures (parents `minecraft:item/handheld`, sprites) are read from $SOLARIS_RESOURCES
(default <qa-armada template-data>/resources), never copied into Git.
Matrices mirror the Rust client: ui/items.rs display_matrix, entities/presentation.rs
held_item_attachment. The native client frame is the final judge; this is the quick check.
"""
import argparse, json, math, os, sys
import numpy as np
from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
VANILLA = os.environ.get("SOLARIS_RESOURCES") or "/run/media/kaiserroman/games/solaris-work/qa-armada/template-data/resources"
ENTITY_MODELS = os.path.join(ROOT, "assets/solaris/models/entity")


# ---------------------------------------------------------------- model loading
def _roots(pack):
    return [os.path.join(pack, "assets"), os.path.join(VANILLA, "assets")]


def _find(roots, ns, kind, name, ext):
    for r in roots:
        p = os.path.join(r, ns, kind, name + ext)
        if os.path.isfile(p):
            return p
    return None


def split_id(i):
    return i.split(":", 1) if ":" in i else ("minecraft", i)


def load_resolved(pack, model_id):
    roots = _roots(pack)
    ns, name = split_id(model_id)
    if name in ("builtin/generated", "item/generated") and ns == "minecraft":
        return {"generated": True, "textures": {}, "display": {}}
    p = _find(roots, ns, "models", name, ".json")
    if p is None:
        raise SystemExit(f"model not found: {model_id}")
    own = json.load(open(p))
    parent = own.get("parent")
    base = {"textures": {}, "display": {}}
    if parent in ("minecraft:builtin/generated", "builtin/generated"):
        base["generated"] = True
    elif parent:
        base = load_resolved(pack, parent)
    out = dict(base)
    out["textures"] = {**base["textures"], **own.get("textures", {})}
    disp = dict(base["display"])
    od = dict(own.get("display", {}))
    for left, right in (("thirdperson_lefthand", "thirdperson_righthand"), ("firstperson_lefthand", "firstperson_righthand")):
        if left not in od and right in od:  # same-file rule of the Rust loader
            od[left] = od[right]
    disp.update(od)
    out["display"] = disp
    if "elements" in own:
        out["elements"] = own["elements"]
        out.pop("generated", None)
    return out


_tex_cache = {}


def load_texture(pack, ref):
    key = (pack, ref)
    if key not in _tex_cache:
        ns, name = split_id(ref)
        p = _find(_roots(pack), ns, "textures", name, ".png")
        if p is None:
            raise SystemExit(f"texture not found: {ref}")
        _tex_cache[key] = np.array(Image.open(p).convert("RGBA"))
    return _tex_cache[key]


def resolve_tex(textures, ref):
    seen = 0
    while ref.startswith("#") and seen < 8:
        ref = textures[ref[1:]]
        seen += 1
    return ref


# ---------------------------------------------------------------- geometry
def rot_axis(axis, deg):
    a = math.radians(deg)
    c, s = math.cos(a), math.sin(a)
    if axis == "x":
        return np.array([[1, 0, 0], [0, c, -s], [0, s, c]])
    if axis == "y":
        return np.array([[c, 0, s], [0, 1, 0], [-s, 0, c]])
    return np.array([[c, -s, 0], [s, c, 0], [0, 0, 1]])


def face_corners(f, t, face):
    (x0, y0, z0), (x1, y1, z1) = f, t
    # corners ordered so that corner k takes uv (u0,v0),(u1,v0),(u1,v1),(u0,v1) as seen from outside
    return {
        "north": [(x1, y1, z0), (x0, y1, z0), (x0, y0, z0), (x1, y0, z0)],
        "south": [(x0, y1, z1), (x1, y1, z1), (x1, y0, z1), (x0, y0, z1)],
        "east": [(x1, y1, z1), (x1, y1, z0), (x1, y0, z0), (x1, y0, z1)],
        "west": [(x0, y1, z0), (x0, y1, z1), (x0, y0, z1), (x0, y0, z0)],
        "up": [(x0, y1, z0), (x1, y1, z0), (x1, y1, z1), (x0, y1, z1)],
        "down": [(x0, y0, z1), (x1, y0, z1), (x1, y0, z0), (x0, y0, z0)],
    }[face]


def default_uv(f, t, face):
    (x0, y0, z0), (x1, y1, z1) = f, t
    return {
        "north": [16 - x1, 16 - y1, 16 - x0, 16 - y0], "south": [x0, 16 - y1, x1, 16 - y0],
        "east": [16 - z1, 16 - y1, 16 - z0, 16 - y0], "west": [z0, 16 - y1, z1, 16 - y0],
        "up": [x0, z0, x1, z1], "down": [x0, 16 - z1, x1, 16 - z0],
    }[face]


NORMALS = {"north": (0, 0, -1), "south": (0, 0, 1), "east": (1, 0, 0), "west": (-1, 0, 0), "up": (0, 1, 0), "down": (0, -1, 0)}


def model_quads(model, pack):
    """[(corners(4,3) in 0..16 item px, uvs(4,2) in texel units, tex array, normal)]"""
    out = []
    tex_refs = model["textures"]
    if model.get("generated"):
        tex = load_texture(pack, resolve_tex(tex_refs, tex_refs["layer0"]))
        h, w = tex.shape[:2]
        z0, z1 = 7.5, 8.5
        # item px: x right, y up; sprite pixel (i,j): column i, row j from the top
        def quad(face, pts, uv):
            out.append((np.array(pts, float), np.array(uv, float), tex, np.array(NORMALS[face], float)))
        quad("north", [(16, 16, z0), (0, 16, z0), (0, 0, z0), (16, 0, z0)], [(0, 0), (w, 0), (w, h), (0, h)])
        quad("south", [(0, 16, z1), (16, 16, z1), (16, 0, z1), (0, 0, z1)], [(0, 0), (w, 0), (w, h), (0, h)])
        solid = tex[:, :, 3] > 8
        for j in range(h):
            for i in range(w):
                if not solid[j, i]:
                    continue
                x0, x1 = i * 16 / w, (i + 1) * 16 / w
                y1, y0 = 16 - j * 16 / h, 16 - (j + 1) * 16 / h
                uv = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)]
                if i == 0 or not solid[j, i - 1]:
                    quad("west", face_corners((x0, y0, z0), (x1, y1, z1), "west"), uv)
                if i == w - 1 or not solid[j, i + 1]:
                    quad("east", face_corners((x0, y0, z0), (x1, y1, z1), "east"), uv)
                if j == 0 or not solid[j - 1, i]:
                    quad("up", face_corners((x0, y0, z0), (x1, y1, z1), "up"), uv)
                if j == h - 1 or not solid[j + 1, i]:
                    quad("down", face_corners((x0, y0, z0), (x1, y1, z1), "down"), uv)
        return out
    for el in model.get("elements", []):
        f, t = el["from"], el["to"]
        rot = el.get("rotation")
        for face, fd in el["faces"].items():
            pts = np.array(face_corners(f, t, face), float)
            n = np.array(NORMALS[face], float)
            if rot:
                o = np.array(rot["origin"], float)
                R = rot_axis(rot["axis"], rot["angle"])
                pts = (pts - o) @ R.T + o
                n = R @ n
            ref = resolve_tex(tex_refs, fd["texture"])
            tex = load_texture(pack, ref)
            h, w = tex.shape[:2]
            u0, v0, u1, v1 = fd.get("uv") or default_uv(f, t, face)
            r = fd.get("rotation", 0)
            uvs = [(u0, v0), (u1, v0), (u1, v1), (u0, v1)]
            uvs = uvs[(r // 90) % 4:] + uvs[:(r // 90) % 4]
            uvs = np.array([(u * w / 16, v * h / 16) for u, v in uvs])
            out.append((pts, uvs, tex, n))
    return out


def display_matrix(model, context):
    t = model["display"].get(context)
    m = np.eye(4)
    if t:
        rot = np.array(t.get("rotation", [0, 0, 0]), float)
        tr = np.array(t.get("translation", [0, 0, 0]), float)
        sc = np.array(t.get("scale", [1, 1, 1]), float)
        if context.endswith("lefthand"):
            tr = tr * [-1, 1, 1]
            rot = rot * [1, -1, -1]
        R = rot_axis("x", rot[0]) @ rot_axis("y", rot[1]) @ rot_axis("z", rot[2])
        m[:3, :3] = R @ np.diag(sc)
        m[:3, 3] = tr / 16
    c = np.eye(4)
    c[:3, 3] = -0.5
    return m @ c


# ---------------------------------------------------------------- rasterizer
LIGHT = np.array([-0.35, -0.55, -0.75])
LIGHT /= np.linalg.norm(LIGHT)


def shade_of(n_view):
    return 0.68 + 0.42 * max(0.0, float(np.dot(n_view, -LIGHT)))


class Scene:
    """Triangles in view space (x right, y DOWN, z away from the viewer)."""

    def __init__(self):
        self.q = []

    def add(self, pts, uvs, tex, n, shade=None):
        self.q.append((pts, uvs, tex, n, shade))


def raster(scene, scale, size, centre, bg=(46, 48, 58), ss=2):
    W, H = size[0] * ss, size[1] * ss
    col = np.zeros((H, W, 3), np.uint8)
    col[:] = bg
    zbuf = np.full((H, W), 1e9)
    for pts, uvs, tex, n, shade in scene.q:
        if n[2] >= -1e-6 and shade is None:
            continue  # back face
        sh = shade if shade is not None else shade_of(n)
        th, tw = tex.shape[:2]
        sx = (pts[:, 0] * scale + centre[0]) * ss
        sy = (pts[:, 1] * scale + centre[1]) * ss
        for a, b, c in ((0, 1, 2), (0, 2, 3)):
            x = np.array([sx[a], sx[b], sx[c]])
            y = np.array([sy[a], sy[b], sy[c]])
            area = (x[1] - x[0]) * (y[2] - y[0]) - (x[2] - x[0]) * (y[1] - y[0])
            if abs(area) < 1e-9:
                continue
            x0, x1 = int(max(math.floor(x.min()), 0)), int(min(math.ceil(x.max()), W - 1))
            y0, y1 = int(max(math.floor(y.min()), 0)), int(min(math.ceil(y.max()), H - 1))
            if x0 > x1 or y0 > y1:
                continue
            gx, gy = np.meshgrid(np.arange(x0, x1 + 1) + 0.5, np.arange(y0, y1 + 1) + 0.5)
            w0 = ((x[1] - gx) * (y[2] - gy) - (x[2] - gx) * (y[1] - gy)) / area
            w1 = ((x[2] - gx) * (y[0] - gy) - (x[0] - gx) * (y[2] - gy)) / area
            w2 = 1 - w0 - w1
            inside = (w0 >= -1e-6) & (w1 >= -1e-6) & (w2 >= -1e-6)
            if not inside.any():
                continue
            z = w0 * pts[a, 2] + w1 * pts[b, 2] + w2 * pts[c, 2]
            u = w0 * uvs[a][0] + w1 * uvs[b][0] + w2 * uvs[c][0]
            v = w0 * uvs[a][1] + w1 * uvs[b][1] + w2 * uvs[c][1]
            tx = np.clip(np.floor(u).astype(int), 0, tw - 1)
            ty = np.clip(np.floor(v).astype(int), 0, th - 1)
            texel = tex[ty, tx]
            ok = inside & (texel[:, :, 3] > 100)
            sub_z = zbuf[y0:y1 + 1, x0:x1 + 1]
            ok &= z < sub_z
            sub_z[ok] = z[ok]
            col[y0:y1 + 1, x0:x1 + 1][ok] = np.clip(texel[ok][:, :3] * sh, 0, 255).astype(np.uint8)
    im = Image.fromarray(col, "RGB")
    return im.resize(size, Image.LANCZOS) if ss > 1 else im


def view_matrix(yaw, pitch):
    return rot_axis("x", pitch) @ rot_axis("y", yaw)


# ---------------------------------------------------------------- views
def add_item_quads(scene, quads, M, V, flip_y_item=False):
    """M: item px (0..16, y up) -> view-source frame (px, same frame as V input)."""
    for pts, uvs, tex, n in quads:
        p = np.c_[pts, np.ones(len(pts))] @ M.T
        p = p[:, :3] @ V.T
        nn = V @ (M[:3, :3] @ n)
        nn = nn / (np.linalg.norm(nn) + 1e-9)
        scene.add(p, uvs, tex, nn)


def item_alone_views(pack, model_id, cell=240):
    model = load_resolved(pack, model_id)
    quads = model_quads(model, pack)
    # item space is y up; screen is y down
    M = np.diag([1.0, -1.0, 1.0, 1.0])
    M[:3, 3] = [-8, 8, -8]  # centre on the block centre
    tiles = []
    for name, yaw, pitch in (("front(-z)", 0, 0), ("back(+z)", 180, 0), ("3/4", 145, -20), ("side", 90, 0), ("top", 0, -80)):
        sc = Scene()
        V = view_matrix(yaw, pitch)
        # view from -z (front = north face) -> mirror x so "front" shows the north face upright
        Mx = np.diag([-1.0, 1, 1, 1]) @ M
        add_item_quads(sc, quads, Mx, V)
        im = raster(sc, cell / 38, (cell, cell), (cell / 2, cell / 2))
        ImageDraw.Draw(im).text((4, 4), name, fill=(255, 255, 0))
        tiles.append(im)
    return tiles


def husk_scene_parts(pack):
    """Husk + iron plate armour geometry: [(model json, texture array, bone-overrides)]"""
    parts = []
    husk = json.load(open(os.path.join(ENTITY_MODELS, "husk.json")))
    t = np.array(Image.open(os.path.join(VANILLA, "assets/minecraft/textures", husk["texture"]["path"])).convert("RGBA"))
    parts.append((husk, t))
    for piece in ("head", "chest"):
        m = json.load(open(os.path.join(ENTITY_MODELS, f"battle_iron_plate_{piece}.json")))
        p = _find(_roots(pack), "solaris_battle", "textures", m["texture"]["path"], "")
        if p:
            parts.append((m, np.array(Image.open(p).convert("RGBA"))))
    return parts


def rotm(rx, ry, rz):
    return rot_axis("z", rz) @ rot_axis("y", ry) @ rot_axis("x", rx)


def bone_mats(model, overrides):
    res = {}

    def rec(b, parent):
        m = np.eye(4)
        rot = overrides.get(b["name"], b.get("rotation", [0, 0, 0]))
        m[:3, :3] = rotm(*rot) @ np.diag(b.get("scale", [1, 1, 1]))
        m[:3, 3] = b.get("pivot", [0, 0, 0])
        w = parent @ m
        res[b["name"]] = w
        yield b, w
        for ch in b.get("bones", []):
            yield from rec(ch, w)

    for b in model["bones"]:
        yield from rec(b, np.eye(4))


FACES = ["up", "down", "right", "front", "left", "back"]


def cube_quads(c, tw, th):
    ox, oy, oz = c["origin"]
    w, h, d = c["size"]
    i = c.get("inflate", 0.0)
    x0, y0, z0, x1, y1, z1 = ox - i, oy - i, oz - i, ox + w + i, oy + h + i, oz + d + i
    u, v = c.get("uv", [0, 0])
    r = {
        "up": [u + d, v, u + d + w, v + d], "down": [u + d + w, v, u + d + 2 * w, v + d],
        "right": [u, v + d, u + d, v + d + h], "front": [u + d, v + d, u + d + w, v + d + h],
        "left": [u + d + w, v + d, u + 2 * d + w, v + d + h], "back": [u + 2 * d + w, v + d, u + 2 * d + 2 * w, v + d + h],
    }
    r.update({k: list(map(float, x)) for k, x in c.get("faces", {}).items()})
    spec = {
        "up": ((0, -1, 0), [(x1, y0, z1, 1, 0), (x0, y0, z1, 0, 0), (x0, y0, z0, 0, 1), (x1, y0, z0, 1, 1)]),
        "down": ((0, 1, 0), [(x1, y1, z0, 1, 1), (x0, y1, z0, 0, 1), (x0, y1, z1, 0, 0), (x1, y1, z1, 1, 0)]),
        "right": ((-1, 0, 0), [(x0, y0, z0, 1, 0), (x0, y0, z1, 0, 0), (x0, y1, z1, 0, 1), (x0, y1, z0, 1, 1)]),
        "front": ((0, 0, -1), [(x1, y0, z0, 1, 0), (x0, y0, z0, 0, 0), (x0, y1, z0, 0, 1), (x1, y1, z0, 1, 1)]),
        "left": ((1, 0, 0), [(x1, y0, z1, 1, 0), (x1, y0, z0, 0, 0), (x1, y1, z0, 0, 1), (x1, y1, z1, 1, 1)]),
        "back": ((0, 0, 1), [(x0, y0, z1, 1, 0), (x1, y0, z1, 0, 0), (x1, y1, z1, 0, 1), (x0, y1, z1, 1, 1)]),
    }
    cx = (x0 + x1) / 2
    for f in FACES:
        n, corners = spec[f]
        ru0, rv0, ru1, rv1 = r[f]
        q = []
        for x, y, z, su, sv in corners:
            if c.get("mirror"):
                x = 2 * cx - x
            q.append((x, y, z, ru1 if su else ru0, rv1 if sv else rv0))
        if c.get("mirror"):
            q.reverse()
            n = (-n[0], n[1], n[2])
        yield np.array([p[:3] for p in q], float), np.array([p[3:] for p in q], float), np.array(n, float)


# Zombie arm pose of the Rust client (presentation.rs): 80 degrees forward, 100 for aggressive.
POSES = {
    "forward": {"right_arm": [-80, 5.7, 0], "left_arm": [-80, -5.7, 0]},
    "hang": {"right_arm": [0, 0, 0], "left_arm": [0, 0, 0]},
    # HumanoidModel.poseBlockingArm(left): xRot = xRot * 0.5 - 0.9424, yRot = +-pi/6
    "block": {"right_arm": [-54, -30, 0], "left_arm": [-54, 30, 0]},
}


def held_attachment(slot_hand):
    side = (-1.0, 1.0)[slot_hand] / 16
    T = np.eye(4)
    T[:3, 3] = [side, 0.625, -0.125]
    R1 = np.eye(4)
    R1[:3, :3] = rot_axis("x", -90)
    R2 = np.eye(4)
    R2[:3, :3] = rot_axis("y", 180)
    return T @ R1 @ R2


def soldier_views(pack, held, cell=300, views=(("front", 0, 0), ("3/4", 145, -15), ("side", 90, 0)), pose="forward", armour=True):
    """held: {hand(0 right,1 left): model_id}. Returns tiles."""
    parts = husk_scene_parts(pack)
    if not armour:
        parts = parts[:1]
    pose_rot = POSES[pose]
    quads_per_hand = {}
    for hand, mid in held.items():
        if pose == "block" and _find(_roots(pack), *split_id(mid + "_blocking")[:1], "models", split_id(mid + "_blocking")[1], ".json"):
            mid += "_blocking"  # using_item branch of the item definition
        model = load_resolved(pack, mid)
        quads_per_hand[hand] = (model, model_quads(model, pack))
    tiles = []
    for name, yaw, pitch in views:
        V = view_matrix(yaw, pitch)
        sc = Scene()
        arm_world = {}
        for model, tex in parts:
            th, tw = tex.shape[:2]
            ov = {k: v for k, v in pose_rot.items()}
            for b, W in bone_mats(model, ov):
                if b["name"] in ("right_arm", "left_arm") and model["id"] == "husk":
                    arm_world[b["name"]] = W
                for c in b.get("cubes", []):
                    for pts, uvs, n in cube_quads(c, tw, th):
                        p = (np.c_[pts, np.ones(4)] @ W.T)[:, :3]
                        sc.add(p @ V.T, uvs, tex, V @ (W[:3, :3] @ n))
        for hand, (model, quads) in quads_per_hand.items():
            arm = arm_world["right_arm" if hand == 0 else "left_arm"]
            ctx = "thirdperson_righthand" if hand == 0 else "thirdperson_lefthand"
            # block frame is in blocks; px = blocks * 16. item px -> blocks first.
            to_blocks = np.diag([1 / 16, 1 / 16, 1 / 16, 1.0])
            A = np.eye(4)
            A[:3, :3] = arm[:3, :3]
            A[:3, 3] = arm[:3, 3] / 16
            # the arm pose bone frame of the Rust rig: pivot-based; same composition
            Mi = A @ held_attachment(hand) @ display_matrix(model, ctx) @ to_blocks
            S = np.diag([16, 16, 16, 1.0])
            Mi = S @ Mi
            add_item_quads(sc, quads, Mi, V)
        pts = np.concatenate([q[0] for q in sc.q])
        cy = (pts[:, 1].min() + pts[:, 1].max()) / 2
        cx = (pts[:, 0].min() + pts[:, 0].max()) / 2
        ext = max(pts[:, 0].max() - pts[:, 0].min(), pts[:, 1].max() - pts[:, 1].min()) + 4
        scale = cell / ext
        im = raster(sc, scale, (cell, cell), (cell / 2 - cx * scale, cell / 2 - cy * scale))
        ImageDraw.Draw(im).text((4, 4), name, fill=(255, 255, 0))
        tiles.append(im)
    return tiles


def firstperson_view(pack, held, size=(640, 360), fov=70.0, blocking=False):
    """Camera-space first-person frame: hand anchor translate(side*0.56, -0.52, -0.72) (player/visual.rs
    append_first_person at rest) then the item's firstperson_* display transform. held {hand(0 right,1 left): id}."""
    W, H = size
    F = (H / 2) / math.tan(math.radians(fov / 2))
    sc = Scene()
    for hand, mid in held.items():
        if blocking and _find(_roots(pack), split_id(mid + "_blocking")[0], "models", split_id(mid + "_blocking")[1], ".json"):
            mid += "_blocking"
        model = load_resolved(pack, mid)
        quads = model_quads(model, pack)
        side = 1.0 if hand == 0 else -1.0
        T = np.eye(4)
        T[:3, 3] = [side * 0.56, -0.52, -0.72]
        Mi = T @ display_matrix(model, "firstperson_righthand" if hand == 0 else "firstperson_lefthand") @ np.diag([1 / 16, 1 / 16, 1 / 16, 1.0])
        for pts, uvs, tex, n in quads:
            c = (np.c_[pts, np.ones(4)] @ Mi.T)[:, :3]      # camera space: x right, y up, -z forward
            depth = -c[:, 2]
            if (depth < 0.05).any():
                continue
            nn = Mi[:3, :3] @ n
            nn = nn / (np.linalg.norm(nn) + 1e-9)
            sx = c[:, 0] / depth * F
            sy = -c[:, 1] / depth * F
            sc.add(np.c_[sx, sy, depth], uvs, tex, np.array([nn[0], -nn[1], -nn[2]]))
    return raster(sc, 1.0, size, (W / 2, H / 2), bg=(120, 160, 200))


def montage(rows, labels, out, cell):
    cols = max(len(r) for r in rows)
    canvas = Image.new("RGB", (cols * cell, len(rows) * cell + 0), (20, 20, 24))
    for i, row in enumerate(rows):
        for j, t in enumerate(row):
            canvas.paste(t, (j * cell, i * cell))
        ImageDraw.Draw(canvas).text((4, i * cell + cell - 14), labels[i], fill=(255, 255, 255))
    canvas.save(out)
    print("wrote", out)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cmd", choices=["items", "hand", "compare", "first"])
    ap.add_argument("models", nargs="+")
    ap.add_argument("--pack", default=os.path.join(ROOT, "assets/battle_video"))
    ap.add_argument("--old")
    ap.add_argument("--hand", default="right", choices=["right", "left"])
    ap.add_argument("--pose", default="forward", choices=list(POSES))
    ap.add_argument("--no-armour", action="store_true")
    ap.add_argument("-o", "--output", required=True)
    a = ap.parse_args()
    hand = 0 if a.hand == "right" else 1
    if a.cmd == "items":
        rows = [item_alone_views(a.pack, m) for m in a.models]
        montage(rows, a.models, a.output, 240)
    elif a.cmd == "first":
        # models: [right-hand model] [left-hand model]; "-" for an empty hand
        held = {i: m for i, m in enumerate(a.models) if m != "-"}
        firstperson_view(a.pack, held, blocking=a.pose == "block").save(a.output)
        print("wrote", a.output)
    elif a.cmd == "hand":
        rows = [soldier_views(a.pack, {hand: m}, pose=a.pose, armour=not a.no_armour) for m in a.models]
        montage(rows, a.models, a.output, 300)
    else:
        rows, labels = [], []
        for m in a.models:
            rows.append(soldier_views(a.old, {hand: m}, pose=a.pose, armour=not a.no_armour))
            labels.append(f"OLD {m}")
            rows.append(soldier_views(a.pack, {hand: m}, pose=a.pose, armour=not a.no_armour))
            labels.append(f"NEW {m}")
        montage(rows, labels, a.output, 300)


if __name__ == "__main__":
    main()
