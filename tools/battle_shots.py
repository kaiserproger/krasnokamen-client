#!/usr/bin/env python3
"""Battle shot planner: `solaris-client scan-replay` timeline -> kit cameras + shots.txt.

  battle_shots.py plan TIMELINE.jsonl --out DIR --name NAME [--seconds 75] [--radius 20]
      DIR/cameras/NAME/*.json, DIR/shots.txt ("NAME CAMERA START END RADIUS", camera paths
      relative to the kit root), DIR/plan.json (why each shot was chosen, preview times).
  battle_shots.py preview DIR --kit KIT --take TAKE.solrec [--size 640x360]
      renders one frame per preview time with KIT/bv-render.sh (BV_* env passes through)
      into KIT/shots and extracts DIR/previews/SHOT-N.png.

Every camera sits at a height above the ground of its own column (take chunk
heightmaps), looks at fighters' chest height, and is rejected when terrain or trees
(WORLD_SURFACE heightmap) block the line of sight or the camera is outside the take's
recorded terrain. Times are tick seconds, the unit of render-replay --timebase tick.
"""
import argparse, hashlib, json, math, os, subprocess, sys
from pathlib import Path

FOV = 70.0


def load(path):
    samples, entities, chunks, header, summary = [], {}, {}, None, None
    with open(path) as f:
        for line in f:
            row = json.loads(line)
            kind = row["kind"]
            if kind == "sample":
                samples.append(row)
            elif kind == "entity":
                entities[row["id"]] = row
            elif kind == "chunk":
                chunks[(row["x"], row["z"])] = (row["surface"], row["ground"])
            elif kind == "header":
                header = row
            elif kind == "summary":
                summary = row
    if summary is None:
        raise SystemExit(f"{path}: no summary row; the scan did not finish")
    return header, samples, entities, Terrain(chunks), summary


class Terrain:
    def __init__(self, chunks):
        self.chunks = chunks

    def _column(self, x, z, which):
        x, z = math.floor(x), math.floor(z)
        maps = self.chunks.get((x >> 4, z >> 4))
        return None if maps is None else maps[which][(x & 15) + (z & 15) * 16]

    def surface(self, x, z):
        """First free y above every non-air block (trees, leaves, tufts)."""
        return self._column(x, z, 0)

    def ground(self, x, z):
        """First free y above the motion-blocking ground (no leaves)."""
        return self._column(x, z, 1)

    def clear(self, eye, target, skip_end=2.0):
        """Line of sight eye -> target clear of terrain; None when leaving the recorded terrain."""
        d = [target[i] - eye[i] for i in range(3)]
        length = math.sqrt(sum(v * v for v in d))
        steps = max(1, int((length - skip_end) / 0.4))
        for k in range(steps + 1):
            s = min(k * 0.4, max(0.0, length - skip_end)) / length if length else 0
            p = [eye[i] + d[i] * s for i in range(3)]
            top, ground = self.surface(p[0], p[2]), self.ground(p[0], p[2])
            if top is None:
                return None
            if top - ground <= 1:  # one block of grass/flowers over the floor hides nothing
                top = ground
            if p[1] < top:
                return False
        return True


def add(a, b, s=1.0):
    return [a[i] + b[i] * s for i in range(len(a))]


def unit(v):
    n = math.hypot(v[0], v[1])
    return [v[0] / n, v[1] / n] if n > 1e-9 else [0.0, 1.0]


def flat(v2, y=0.0):
    return [v2[0], y, v2[1]]


class Timeline:
    def __init__(self, samples):
        self.samples = samples
        self.times = [s["t"] for s in samples]
        self.step = (self.times[1] - self.times[0]) if len(self.times) > 1 else 0.5
        self.end = self.times[-1]

    def at(self, t):
        best = min(range(len(self.times)), key=lambda i: abs(self.times[i] - t))
        return self.samples[best]

    def track(self, t0, t1, seed, key="contacts", side=None, reach=12.0):
        """Same cluster over [t0, t1]: the nearest one to the previous centroid each sample."""
        out, point = [], seed
        for s in self.samples:
            if s["t"] < t0 - 1e-9 or s["t"] > t1 + 1e-9:
                continue
            groups = [g for g in s[key] if side is None or g.get("side") == side]
            near = min(groups, key=lambda g: dist2(g["centroid"], point), default=None)
            if near is not None and dist2(near["centroid"], point) <= reach * reach:
                point = near["centroid"]
                out.append((s["t"], near))
            else:
                out.append((s["t"], None))
        return out


def dist2(a, b):
    return (a[0] - b[0]) ** 2 + (a[2] - b[2]) ** 2


def smooth(track):
    """Centroids of a tracked cluster, gaps filled, 3-sample moving average."""
    known = [(t, g) for t, g in track if g is not None]
    if not known:
        return []
    points, last = [], known[0][1]
    for t, g in track:
        last = g or last
        points.append((t, last))
    out = []
    for i, (t, g) in enumerate(points):
        window = [p[1]["centroid"] for p in points[max(0, i - 1):i + 2]]
        out.append((t, [sum(c[a] for c in window) / len(window) for a in range(3)], g))
    return out


class Shot:
    def __init__(self, kind, start, end, keys, why, score, attach=None, where=None):
        self.kind, self.start, self.end, self.keys = kind, round(start, 2), round(end, 2), keys
        self.why, self.score, self.attach, self.where = why, score, attach, where

    def camera(self):
        doc = {"keyframes": [{"time": round(t, 3), "position": [round(v, 3) for v in p],
                              "look_target": [round(v, 3) for v in l], "fov": fov, "ease": True}
                             for t, p, l, fov in self.keys]}
        if self.attach is not None:
            doc["attach_entity"] = self.attach
        return doc

    def previews(self):
        mid = (self.start + self.end) / 2
        return [round(self.start + 0.3, 2), round(mid, 2), round(self.end - 0.3, 2)]


def place(terrain, xz, height):
    """Camera at `height` above its own column's ground; None outside terrain or inside a tree."""
    ground, top = terrain.ground(xz[0], xz[2]), terrain.surface(xz[0], xz[2])
    if ground is None:
        return None
    y = ground + height
    if top is not None and top > y - 0.3 and top - ground > 1:  # canopy or overhang above the lens
        return None
    return [xz[0], y, xz[2]]


def keys_clear(terrain, keys, attach_base=None):
    for _, p, look, _ in keys:
        eye, target = (p, look) if attach_base is None else (add(attach_base, p), add(attach_base, look))
        if terrain.clear(eye, target) is not True:
            return False
    return True


def keyed(times, positions, looks, fovs):
    return [(t, p, l, f) for t, p, l, f in zip(times, positions, looks, fovs)]


def chest(c, height=1.2):
    """A point on the fighters: `height` above their mean feet."""
    return c[1] + height


def dolly_shots(tl, terrain):
    """Low tracking dolly along the contact line, 1.5-3 high, 4-8 off the line."""
    out = []
    for s in tl.samples:
        for c in s["contacts"][:3]:
            if c["count"] < 8 or s["t"] < 4.5:
                continue
            t0, t1 = s["t"] - 1.0, min(tl.end - 0.1, s["t"] + 4.0)
            if t1 - t0 < 3.0:
                continue
            path = smooth(tl.track(t0, t1, c["centroid"]))
            if len(path) < 4:
                continue
            line, normal = c["line"], c["normal"]
            travel = max(6.0, min(14.0, 0.6 * c["length"]))
            order = (("red", -1.0), ("blue", 1.0)) if int(s["t"] * 2) % 2 == 0 else (("blue", 1.0), ("red", -1.0))
            for side, sign in order:
                for off in (6.0, 5.0, 7.0, 4.0, 8.0):
                    times, pos, look = [], [], []
                    ok = True
                    for i, (t, centroid, _) in enumerate(path[::2] + ([path[-1]] if len(path) % 2 == 0 else [])):
                        u = -travel / 2 + travel * (t - path[0][0]) / (path[-1][0] - path[0][0])
                        base = add(centroid, flat(line), u)
                        cam = place(terrain, add(base, flat(normal), sign * off), 2.8)
                        if cam is None:
                            ok = False
                            break
                        times.append(t)
                        pos.append(cam)
                        look.append([base[0] + 2.0 * line[0], chest(centroid, 0.5), base[2] + 2.0 * line[1]])
                    if not ok or len(times) < 2:
                        continue
                    keys = keyed(times, pos, look, [FOV] * len(times))
                    if keys_clear(terrain, keys):
                        swings = sum(g["swings"] for _, _, g in path if g)
                        out.append(Shot("dolly", times[0], times[-1], keys,
                                        f"contact line of {c['count']} fighters at t {s['t']}, camera {off:g} blocks behind {side}, {swings} swings",
                                        c["count"] + swings, where=c["centroid"]))
                        break
                else:
                    continue
                break
    return out


def push_shots(tl, terrain):
    """Push-in from 18 to 5 blocks into the densest melee."""
    out = []
    for s in tl.samples:
        if not s["contacts"] or s["t"] < 4.5:
            continue
        c = s["contacts"][0]
        if c["count"] < 10:
            continue
        t0, t1 = s["t"] - 0.5, min(tl.end - 0.1, s["t"] + 3.5)
        if t1 - t0 < 3.0:
            continue
        path = smooth(tl.track(t0, t1, c["centroid"]))
        if len(path) < 3:
            continue
        for sign in (-1.0, 1.0):
            first, last = path[0], path[-1]
            n = flat(c["normal"])
            far = place(terrain, add(first[1], n, sign * 18.0), 5.5)
            near = place(terrain, add(last[1], n, sign * 5.0), 2.6)
            if far is None or near is None:
                continue
            mid_t = (first[0] + last[0]) / 2
            mid_c = min(path, key=lambda p: abs(p[0] - mid_t))[1]
            mid = place(terrain, add(mid_c, n, sign * 11.0), 4.0)
            if mid is None:
                continue
            keys = keyed([first[0], mid_t, last[0]], [far, mid, near],
                         [[p[0], chest(p, 0.3), p[2]] for p in (first[1], mid_c, last[1])], [FOV, 66.0, 60.0])
            if keys_clear(terrain, keys):
                out.append(Shot("push-in", first[0], last[0], keys,
                                f"densest melee ({c['count']} in contact, {c['swings']} swings) at t {s['t']}",
                                c["count"] * 1.5 + c["swings"], where=c["centroid"]))
                break
    return out


def shoulder_shots(tl, terrain, entities):
    """Over the shoulder of a front-line fighter walking into contact (attach_entity)."""
    out = []
    for s in tl.samples:
        tc = s["t"]
        if tc < 5.0 or tc + 2.5 > tl.end:
            continue
        for c in s["contacts"][:4]:
            for side, sign in (("red", 1.0), ("blue", -1.0)):
                for fid in c["ids"].get(side, []):
                    e = entities.get(fid)
                    if not e or e["role"] != "fighter" or e.get("vehicle") is not None:
                        continue
                    if e["first_contact_t"] is None or abs(e["first_contact_t"] - tc) > 0.51:
                        continue
                    if e["spawn_t"] > tc - 5.0:
                        continue
                    end = tc + 2.5
                    if (e["death_t"] is not None and e["death_t"] < end) or (e["removed_t"] is not None and e["removed_t"] < end + 0.5):
                        continue
                    d = [c["normal"][0] * sign, c["normal"][1] * sign]
                    right = [-d[1], d[0]]
                    back = lambda k: [-k * d[0] + 0.9 * right[0], 2.9, -k * d[1] + 0.9 * right[1]]
                    look = [6.0 * d[0], -0.2, 6.0 * d[1]]  # about 25 degrees down: the clash, not the sky
                    keys = keyed([tc - 2.5, tc, end], [back(3.0), back(2.4), back(2.0)], [look] * 3, [FOV, 68.0, 64.0])
                    base = c["centroid"]
                    if not keys_clear(terrain, keys, attach_base=base):
                        continue
                    out.append(Shot("shoulder", tc - 2.5, end, keys,
                                    f"over the {side} fighter {fid}'s shoulder as it reaches contact at t {tc} (cluster of {c['count']})",
                                    c["count"] + 10, attach=fid, where=base))
                    break
    return out


def volley_shots(tl, terrain):
    """Rise with one side's volley from behind its archers, ride behind it, end on the impact.

    Volleys fly about 200 blocks, beyond the 128-block render disc, so the camera follows
    the arrows' centroid and ends 40 blocks short of the impact instead of staying put."""
    out = []
    for s in tl.samples:
        for side in ("red", "blue"):
            a = s["arrows"][side]
            if a["launched"] < 20 or a["launch"] is None or s["t"] < 4.5:
                continue
            launch = a["launch"]["centroid"]
            later = [x for x in tl.samples if s["t"] < x["t"] <= s["t"] + 6.0 and x["arrows"][side]["impact"]]
            if not later:
                continue
            hit = max(later, key=lambda x: x["arrows"][side]["impacts"])
            impact = hit["arrows"][side]["impact"]["centroid"]
            d = unit([impact[0] - launch[0], impact[2] - launch[2]])
            # The sample at s["t"] holds the launches of the half second before it: open on the release.
            t0, t1, t3 = s["t"] - 0.3, s["t"] + 0.5, min(tl.end - 0.1, hit["t"] + 0.5)
            t2 = (t1 + t3) / 2
            if t3 - t0 < 3.0 or t3 - t0 > 6.5 or t2 - t1 < 0.5:
                continue
            flights = [tl.at(t)["arrows"][side]["flight"] for t in (t1, t2)]
            if None in flights:
                continue
            f1, f2 = flights[0]["centroid"], flights[1]["centroid"]
            # Behind the archers' rear rank (launch area depth along the volley), not among them.
            lo, hi = a["launch"]["min"], a["launch"]["max"]
            depth = abs(d[0]) * (hi[0] - lo[0]) / 2 + abs(d[1]) * (hi[2] - lo[2]) / 2
            p0 = place(terrain, add(launch, flat(d), -(depth + 5.0)), 2.5)
            p1 = place(terrain, add(launch, flat(d), -4.0), 7.0)
            g2 = terrain.ground(f2[0] - 20 * d[0], f2[2] - 20 * d[1])
            p2 = None if g2 is None else place(terrain, add(f2, flat(d), -20.0), max(6.0, f2[1] - 4.0 - g2))
            p3 = place(terrain, add(impact, flat(d), -40.0), 15.0)
            if None in (p0, p1, p2, p3):
                continue
            keys = keyed([t0, t1, t2, t3], [p0, p1, p2, p3],
                         [add(launch, [d[0] * 14, 6.0, d[1] * 14]), f1, f2, [impact[0], impact[1] + 0.8, impact[2]]],
                         [FOV, 72.0, 72.0, 62.0])
            if keys_clear(terrain, keys):
                out.append(Shot("volley", t0, t3, keys,
                                f"{side} volley of {a['launched']} arrows at t {s['t']} from {[round(v) for v in launch]}, "
                                f"{hit['arrows'][side]['impacts']} impacts at t {hit['t']} around {[round(v) for v in impact]}",
                                a["launched"] / 4.0, where=launch))
    return out


def engaged(tl, t0, t1, g, margin=10.0):
    """A contact cluster within `margin` blocks of the cavalry group's extent during [t0, t1]."""
    lo = [g.get("min", g["centroid"])[a] - margin for a in (0, 2)]
    hi = [g.get("max", g["centroid"])[a] + margin for a in (0, 2)]
    return any(lo[0] <= c["centroid"][0] <= hi[0] and lo[1] <= c["centroid"][2] <= hi[1]
               for s in tl.samples if t0 <= s["t"] <= t1 for c in s["contacts"])


def cavalry_shots(tl, terrain, front):
    """Track a moving cavalry group from its outer flank while it runs into melee
    (a cavalry shot without contact or arrows in frame is a parade, not battle)."""
    out = []
    for s in tl.samples:
        for g in s["cavalry"]:
            speed = math.hypot(*g["velocity"])
            if speed < 0.8 or s["t"] < 4.5:
                continue
            t0, t1 = s["t"] - 1.0, min(tl.end - 0.1, s["t"] + 3.0)
            if t1 - t0 < 3.0 or not engaged(tl, t0, t1, g):
                continue
            path = smooth(tl.track(t0, t1, g["centroid"], key="cavalry", side=g["side"], reach=10.0))
            if len(path) < 4:
                continue
            v = unit(g["velocity"])
            perp = [-v[1], v[0]]
            if front and (g["centroid"][0] + perp[0] - front[0]) ** 2 + (g["centroid"][2] + perp[1] - front[2]) ** 2 < dist2(g["centroid"], front):
                perp = [-perp[0], -perp[1]]  # stay on the side away from the battle
            # Outside the group's near edge (its extent projected on the flank direction).
            edge = 0.0
            if g.get("min") and g.get("max"):
                edge = abs(perp[0]) * (g["max"][0] - g["min"][0]) / 2 + abs(perp[1]) * (g["max"][2] - g["min"][2]) / 2
            times, pos, look, ok = [], [], [], True
            for t, centroid, _ in path[::2]:
                cam = place(terrain, add(centroid, flat(perp), edge + 7.0), 2.6)
                if cam is None:
                    ok = False
                    break
                times.append(t)
                pos.append(cam)
                rim = add(centroid, flat(perp), max(0.0, edge - 2.0))
                look.append([rim[0] + 3 * v[0], centroid[1] + 1.4, rim[2] + 3 * v[1]])
            if not ok or len(times) < 2:
                continue
            keys = keyed(times, pos, look, [FOV] * len(times))
            if keys_clear(terrain, keys):
                out.append(Shot("cavalry", times[0], times[-1], keys,
                                f"{g['side']} cavalry group of {g['count']} at {speed:.1f} blocks/s, t {s['t']}",
                                g["count"] * speed / 2.0, where=g["centroid"]))
    return out


def establishing_shot(tl, terrain):
    """One high oblique (not top-down) over the whole front, horizon kept out of frame."""
    best = max(tl.samples, key=lambda s: s["contact_fighters"])
    if not best["contacts"]:
        return []
    weights = sum(c["count"] for c in best["contacts"])
    front = [sum(c["centroid"][a] * c["count"] for c in best["contacts"]) / weights for a in range(3)]
    n, line = best["contacts"][0]["normal"], best["contacts"][0]["line"]
    out = []
    for sign in (-1.0, 1.0):
        t0, t1 = max(4.5, best["t"] - 2.5), min(tl.end - 0.1, best["t"] + 2.5)
        p0 = place(terrain, add(add(front, flat(n), sign * 34.0), flat(line), -6.0), 24.0)
        p1 = place(terrain, add(add(front, flat(n), sign * 30.0), flat(line), 6.0), 22.0)
        if p0 is None or p1 is None or t1 - t0 < 3.0:
            continue
        target = [front[0], front[1] + 1.0, front[2]]
        keys = keyed([t0, t1], [p0, p1], [target, target], [60.0, 58.0])
        if keys_clear(terrain, keys):
            out.append(Shot("establishing", t0, t1, keys,
                            f"front of {best['contact_fighters']} fighters in contact at t {best['t']}, 30-34 blocks back, 22-24 up (pitch about -35)",
                            1.0, where=front))
            break
    return out


QUOTA = {"push-in": 4, "dolly": 5, "shoulder": 4, "volley": 2, "cavalry": 2, "establishing": 1}


def choose(candidates, seconds):
    picked = []
    for kind, quota in QUOTA.items():
        pool = sorted((c for c in candidates if c.kind == kind), key=lambda c: -c.score)
        mine = []
        for c in pool:
            if len(mine) == quota:
                break
            # Same kind: 8 s apart within 30 blocks, 3 s within 60; farther ones are other places.
            if any((abs(c.start - m.start) < 3.0 and dist2(c.where, m.where) < 3600)
                   or (abs(c.start - m.start) < 8.0 and dist2(c.where, m.where) < 900) for m in mine):
                continue
            mine.append(c)
        picked.extend(mine)
    picked.sort(key=lambda c: c.start)
    total = sum(c.end - c.start for c in picked)
    while total > seconds + 3 and len(picked) > 6:
        drop = min((c for c in picked if c.kind != "establishing"), key=lambda c: c.score)
        picked.remove(drop)
        total -= drop.end - drop.start
    return picked


def plan(args):
    header, samples, entities, terrain, summary = load(args.timeline)
    if len(samples) < 8:
        raise SystemExit("timeline too short")
    if not terrain.chunks:
        raise SystemExit("timeline has no chunk heightmaps; rescan with a current scan-replay")
    tl = Timeline(samples)
    contact_samples = [s for s in samples if s["contacts"]]
    front = None
    if contact_samples:
        best = max(contact_samples, key=lambda s: s["contact_fighters"])
        front = best["contacts"][0]["centroid"]
    candidates = (dolly_shots(tl, terrain) + push_shots(tl, terrain) + shoulder_shots(tl, terrain, entities)
                  + volley_shots(tl, terrain) + cavalry_shots(tl, terrain, front) + establishing_shot(tl, terrain))
    shots = choose(candidates, args.seconds)
    out = Path(args.out)
    cams = out / "cameras" / args.name
    cams.mkdir(parents=True, exist_ok=False)
    lines = [f"# NAME CAMERA START END [RADIUS]   (tick seconds of this take; first_tick {summary['first_tick']})",
             f"# planned by tools/battle_shots.py from {Path(args.timeline).name}"]
    report = []
    for i, shot in enumerate(shots, 1):
        name = f"{args.name}-{i:02d}-{shot.kind}"
        (cams / f"{name}.json").write_text(json.dumps(shot.camera(), indent=1) + "\n")
        lines.append(f"{name} cameras/{args.name}/{name}.json {shot.start:.2f} {shot.end:.2f} {args.radius}")
        report.append({"name": name, "kind": shot.kind, "start": shot.start, "end": shot.end,
                       "attach_entity": shot.attach, "why": shot.why, "previews": shot.previews()})
    (out / "shots.txt").write_text("\n".join(lines) + "\n")
    counts = {k: sum(1 for c in candidates if c.kind == k) for k in QUOTA}
    windows = []
    for s in contact_samples:
        if windows and s["t"] - windows[-1]["end"] <= 1.01:
            w = windows[-1]
        else:
            w = {"start": s["t"], "end": s["t"], "peak": 0}
            windows.append(w)
        w["end"] = s["t"]
        if s["contact_fighters"] > w["peak"]:
            w.update(peak=s["contact_fighters"], peak_t=s["t"], clusters=len(s["contacts"]),
                     where=[[round(v, 1) for v in c["centroid"]] for c in s["contacts"][:4]])
    volleys = [{"t": s["t"], "side": side, "launched": s["arrows"][side]["launched"],
                "from": s["arrows"][side]["launch"]["centroid"]}
               for s in samples for side in ("red", "blue") if s["arrows"][side]["launched"] >= 20]
    (out / "plan.json").write_text(json.dumps({"timeline": str(args.timeline), "summary": summary, "candidates": counts,
                                               "cut_seconds": round(sum(s.end - s.start for s in shots), 2),
                                               "shots": report, "melee_windows": windows, "volleys": volleys}, indent=1) + "\n")
    print(f"melee windows (contact = opposite sides within 2.5 blocks): {len(windows)}")
    for w in windows:
        print(f"  t {w['start']:.2f}-{w['end']:.2f}  peak {w['peak']} fighters at t {w['peak_t']:.2f} in {w['clusters']} clusters, top at {w['where']}")
    print(f"volleys (>= 20 arrows per 0.5 s): " + ", ".join(f"{v['side']} t {v['t']:.2f} x{v['launched']} from {[round(c) for c in v['from']]}" for v in volleys))
    print(f"planned {len(shots)} shots, {sum(s.end - s.start for s in shots):.1f} s; candidates {counts}")
    for r in report:
        print(f"  {r['name']}  {r['start']:.2f}-{r['end']:.2f}  {r['why']}")


def preview(args):
    out = Path(args.dir)
    report = json.loads((out / "plan.json").read_text())
    kit = Path(args.kit).resolve()
    shots_dir = out / "previews"
    shots_dir.mkdir(exist_ok=True)
    env = dict(os.environ, BV_SIZE=args.size)
    status = 0
    for shot in report["shots"]:
        if args.only and shot["name"] not in args.only:
            continue
        cam = next((out / "cameras").glob(f"*/{shot['name']}.json"))
        # Kit render names are content-addressed (camera bytes + time): an unchanged
        # shot reuses its earlier render, a changed one never collides with it.
        digest = hashlib.sha256(cam.read_bytes()).hexdigest()[:10]
        for k, t in enumerate(shot["previews"]):
            if k not in args.index:
                continue
            png = shots_dir / f"{shot['name']}-p{k}.png"
            render = f"preview-{digest}-{t:.2f}"
            mp4 = kit / "shots" / f"{render}.mp4"
            if not mp4.exists():
                result = subprocess.run([str(kit / "bv-render.sh"), "shot", str(Path(args.take).resolve()), str(cam.resolve()),
                                         f"{t:.2f}", f"{t + 0.02:.2f}", render, str(args.radius)], env=env, cwd=kit)
                if result.returncode:
                    print(f"preview {shot['name']} p{k} failed; see {kit / 'shots' / (render + '.log')}", file=sys.stderr)
                    status = 1
                    continue
            png.unlink(missing_ok=True)
            subprocess.run(["ffmpeg", "-loglevel", "error", "-i", str(mp4), "-frames:v", "1", str(png)], check=True)
    return status


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("plan")
    p.add_argument("timeline")
    p.add_argument("--out", required=True)
    p.add_argument("--name", required=True)
    p.add_argument("--seconds", type=float, default=75.0)
    p.add_argument("--radius", type=int, default=20)
    v = sub.add_parser("preview")
    v.add_argument("dir")
    v.add_argument("--kit", required=True)
    v.add_argument("--take", required=True)
    v.add_argument("--size", default="640x360")
    v.add_argument("--radius", type=int, default=20)
    v.add_argument("--only", nargs="*")
    v.add_argument("--index", type=int, nargs="*", default=[0, 1, 2], help="which of the 3 preview times (start, mid, end)")
    args = parser.parse_args()
    if args.command == "plan":
        plan(args)
    else:
        sys.exit(preview(args))


if __name__ == "__main__":
    main()
