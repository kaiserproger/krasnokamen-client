#!/usr/bin/env python3
"""Live battle-video rank pilot (PovRed): drive a real windowed client through its local MCP endpoint.

  bv_live_pilot.py RUNTIME_DIR [--side red] [--k 8]

Keeps PovRed's join-time place relative to the centroid of the K same-side soldiers
nearest him at join, respawn or re-slot (side read from their shield/surcoat ids or dyed kit), walks with held movement
keys, turns smoothly toward the nearest enemy (else along the rank's motion), and
presses attack when an enemy is in reach. Route guard: before a movement key is held, the cells it walks into
are read from the client's loaded blocks (minecraft_scan_blocks); a key toward a drop of more than 3 blocks (no ground
within 4 below the feet), water, lava, unread blocks, or into the keep-out box (--keepout, default the mine
x -192..-184, z 21..30) is not pressed and logs `edge refused <key> at X,Y,Z drop=N (why)` or `keepout refused <key> ...`.
RUNTIME_DIR is the client's --runtime-dir;
the client runs with `--automation --mcp-http 127.0.0.1:0 --windowed` and
`--hide-hud-keep-hand`. MCP input needs the client window focused. Every action is an
ordinary client input, never a packet bot; the loop is paced by client ticks.
CamSpec needs no pilot: `--third-person` plus the server's Set Camera.
"""
from __future__ import annotations

import argparse
import json
import math
import os
import pathlib
import re
import signal
import sys
import time
import urllib.request

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from mcp_client import Client  # noqa: E402

# Equipment slots read for side gear: off-hand shield, chest (surcoat or dyed
# padding), legs (dyed). Shields and surcoats carry solaris_battle:red_*/blue_*;
# plain kit is told apart by its dye (red 0x6E1414-like, blue 0x243A7F-like).
SIDE_SLOTS = (1, 4, 3)
SIDE = re.compile(r"solaris_battle:(red|blue)_")


def log(*parts):
    print(time.strftime("%H:%M:%S"), *parts, flush=True)


def connect(runtime: pathlib.Path, evidence: pathlib.Path, transcript: bool) -> Client:
    """The newest instance descriptor in RUNTIME_DIR: the client prints
    `automation descriptor: PATH` once it is written; start the pilot after that line."""
    descriptors = sorted(runtime.glob("*.json"), key=lambda p: p.stat().st_mtime)
    if not descriptors:
        raise SystemExit(f"no client descriptor in {runtime}: start the client with --runtime-dir {runtime} "
                         "and run the pilot after it prints 'automation descriptor:'")
    client = Client(descriptors[-1], evidence)
    if not transcript:
        client.trace.close()
        client.trace = open(os.devnull, "w")
    client.initialize()
    log("attached", descriptors[-1])
    return client


def next_state_event(client: Client):
    """Block until the client's next applied state event (packets, lifecycle, focus)."""
    version = client.tool("minecraft_observe")["state_version"]
    try:
        client.tool("minecraft_wait_for_state_change", {"observed_version": version, "timeout_seconds": 120})
    except RuntimeError:
        pass  # no event within 120 s: the caller re-checks


def wait_until_writable(client: Client):
    """Unfocused window: re-try a no-op input (release sprint) on each state event."""
    while True:
        next_state_event(client)
        try:
            client.tool("minecraft_press_inputs", {"keys": ["sprint"], "ticks": 1, "hold": False})
            return
        except RuntimeError as error:
            if "window_not_focused" not in str(error):
                raise


RECOVERABLE = ("window_not_focused", "not in play", "client_not_in_play", "dialog_screen",
               "player died", "client player is not dead")
MOVE_KEYS = ("forward", "back", "left", "right", "sprint", "jump", "use")


def interrupt(*_):
    raise KeyboardInterrupt


def release(client: Client):
    """End the MCP session so its writer lease frees at once (else it idles out in 120 s)."""
    if not client.session:
        return
    request = urllib.request.Request(client.endpoint, method="DELETE", headers={
        "Authorization": "Bearer " + client.token, "Mcp-Session-Id": client.session,
        "MCP-Protocol-Version": "2025-11-25"})
    try:
        urllib.request.urlopen(request, timeout=10).close()
    except OSError:
        pass


def wrap(angle: float) -> float:
    return (angle + 180.0) % 360.0 - 180.0


def look_angles(src, dst):
    """Vanilla yaw/pitch from src to dst: yaw 0 faces +Z, -90 faces +X; pitch +90 looks down."""
    dx, dy, dz = (dst[i] - src[i] for i in range(3))
    yaw = math.degrees(math.atan2(-dx, dz))
    pitch = -math.degrees(math.atan2(dy, math.hypot(dx, dz)))
    return yaw, pitch


def approach(current: float, target: float, rate: float, limit: float) -> float:
    step = max(-limit, min(limit, wrap(target - current) * rate))
    return current + step


# Route guard. Every horizontal key is checked against the client's own loaded blocks
# (`minecraft_scan_blocks`) before it is held: the cells it walks into over the next step must have
# standing ground within MAX_DROP blocks below the feet, with no water or lava on the way down, and
# must stay out of the keep-out box. A refused key is not pressed (a held one is released).
WALK_KEYS = ("forward", "back", "left", "right")
MAX_DROP = 3
KEEPOUT = (-192.0, -184.0, 21.0, 30.0)  # x0, x1, z0, z1: the mine's box, blocks (cells -192..-184 / 21..30), any height
PLAYER_RADIUS = 0.3
LOOK_STEP = 0.7  # blocks between checked points along a key's direction, from the feet
LOG_AGAIN = 2.0  # seconds before the same refusal is logged again


def key_vector(key: str, yaw: float):
    """Horizontal unit vector of a movement key for view yaw (forward (-sin, cos), right (-cos, -sin))."""
    radians = math.radians(yaw)
    return {"forward": (-math.sin(radians), math.cos(radians)), "back": (math.sin(radians), -math.cos(radians)),
            "right": (-math.cos(radians), -math.sin(radians)), "left": (math.cos(radians), math.sin(radians))}[key]


def in_keepout(x: float, z: float, box) -> bool:
    """Whether (x, z) is in the keep-out box (cells x0..x1, z0..z1 inclusive) with the player's radius."""
    if box is None:
        return False
    x0, x1, z0, z1 = box
    return x0 - PLAYER_RADIUS < x < x1 + 1 + PLAYER_RADIUS and z0 - PLAYER_RADIUS < z < z1 + 1 + PLAYER_RADIUS


def judge_column(rows: dict, feet_y: int):
    """(ok, drop, why) for walking into a column: `rows` maps y to the scan row of that cell, feet_y the cell the
    feet stand in. The ground is the first cell with collision at or below feet_y - 1, found within MAX_DROP + 1
    cells; a drop is the free cells between the feet and it. Water or lava in a cell passed, an unread cell,
    or no ground within reach all refuse. A solid cell at feet level (a step or wall) is not a drop."""
    for y in range(feet_y, feet_y - MAX_DROP - 2, -1):
        row = rows.get(y)
        if row is None:
            return False, feet_y - y, "unloaded"
        if not row["fluid"]["empty"]:
            return False, max(feet_y - y, 0), "lava" if "lava" in row["block_id"] else "water"
        if not row["collision_empty"]:
            return True, max(feet_y - 1 - y, 0), ""
    return False, MAX_DROP + 1, "no ground"


class Guard:
    """Route guard over the client's MCP block reads; see the comment above."""

    def __init__(self, client, keepout=KEEPOUT):
        self.c, self.keepout = client, keepout
        self.cache: dict[tuple, dict] = {}  # (x, z, feet_y) -> {y: row} for fully read columns
        self.logged: dict[tuple, float] = {}

    def cells(self, here, vector, reach):
        """Distinct (x, z) cells along `vector` from the feet, 0.7 blocks apart up to `reach`, not the feet's own."""
        own = (math.floor(here[0]), math.floor(here[2]))
        out, k = [], LOOK_STEP
        while k <= reach + 1e-9:
            cell = (math.floor(here[0] + vector[0] * k), math.floor(here[2] + vector[1] * k))
            if cell != own and cell not in out:
                out.append(cell)
            k += LOOK_STEP
        return out

    def columns(self, cells, feet_y):
        """{cell: {y: row or None}} for columns feet_y - MAX_DROP - 1 .. feet_y; one scan for the cells not cached.
        Only complete columns are cached, so a chunk that loads later is read again."""
        need = [c for c in cells if (c[0], c[1], feet_y) not in self.cache]
        got: dict = {}
        if need:
            low = feet_y - MAX_DROP - 1
            box = {"min_x": min(c[0] for c in need), "max_x": max(c[0] for c in need), "min_z": min(c[1] for c in need),
                   "max_z": max(c[1] for c in need), "min_y": low, "max_y": feet_y}
            offset = 0
            while True:
                page = self.c.tool("minecraft_scan_blocks", {**box, "offset": offset} if offset else box)
                for row in page["blocks"]:
                    got[(row["x"], row["y"], row["z"])] = row
                if not page.get("truncated"):
                    break
                offset = page["next_offset"]
            for cell in need:
                column = {y: got.get((cell[0], y, cell[1])) for y in range(low, feet_y + 1)}
                if None not in column.values():
                    self.cache[(cell[0], cell[1], feet_y)] = column
                else:
                    self.cache.pop((cell[0], cell[1], feet_y), None)
                    got[("partial", cell)] = column
        return {c: self.cache.get((c[0], c[1], feet_y)) or got[("partial", c)] for c in cells}

    def judge(self, cells, feet_y):
        """The first refused cell as (cell, drop, why), else None."""
        try:
            columns = self.columns(cells, feet_y)
        except RuntimeError as error:
            if any(code in str(error) for code in RECOVERABLE):
                raise
            return cells[0], MAX_DROP + 1, "block read failed: " + str(error)[:80]
        for cell in cells:
            ok, drop, why = judge_column(columns[cell], feet_y)
            if not ok:
                return cell, drop, why
        return None

    def refuse(self, line: str, key: tuple):
        now = time.monotonic()
        if now - self.logged.get(key, -1e9) >= LOG_AGAIN:
            self.logged[key] = now
            log(line)

    def filter(self, wanted: set, here, yaw: float, reach: float) -> set:
        """`wanted` without the horizontal keys that walk into the keep-out box, off a drop of more than MAX_DROP
        blocks, or into water or lava; sprint and jump go with their last walking key. `reach` is how far (blocks)
        the key can carry him before the next check. From inside the box only keys that lead out are allowed."""
        feet_y = math.floor(here[1])
        inside = in_keepout(here[0], here[2], self.keepout)
        centre = None if self.keepout is None else ((self.keepout[0] + self.keepout[1] + 1) / 2, (self.keepout[2] + self.keepout[3] + 1) / 2)
        keep = set(wanted)
        for key in WALK_KEYS:
            if key not in wanted:
                continue
            vector = key_vector(key, yaw)
            if inside:
                leaves = vector[0] * (here[0] - centre[0]) + vector[1] * (here[2] - centre[1]) > 0
                if not leaves:
                    keep.discard(key)
                    self.refuse(f"keepout refused {key} at {[round(v, 1) for v in here]} (inside the box, key leads deeper)", ("keepout", key))
                continue
            ahead = [(here[0] + vector[0] * k, here[2] + vector[1] * k)
                     for k in [LOOK_STEP * i for i in range(1, int(reach / LOOK_STEP) + 2)]]
            if any(in_keepout(x, z, self.keepout) for x, z in ahead):
                keep.discard(key)
                self.refuse(f"keepout refused {key} at {[round(v, 1) for v in here]} "
                            f"(box x {self.keepout[0]:g}..{self.keepout[1]:g}, z {self.keepout[2]:g}..{self.keepout[3]:g})", ("keepout", key))
                continue
            bad = self.judge(self.cells(here, vector, reach), feet_y)
            if bad is not None:
                (cx, cz), drop, why = bad
                keep.discard(key)
                self.refuse(f"edge refused {key} at {cx},{feet_y},{cz} drop={drop if drop <= MAX_DROP else str(MAX_DROP + 1) + '+'} ({why})",
                            ("edge", key, cx, cz))
        if not keep & set(WALK_KEYS):
            keep -= {"sprint", "jump"}
        elif "forward" not in keep:
            keep.discard("sprint")
        return keep


class Pilot:
    def __init__(self, client: Client):
        self.c = client
        self.held: set[str] = set()
        self.sent_look = None

    def keys(self, wanted: set[str]):
        release = self.held - wanted
        press = wanted - self.held
        if release:
            self.c.tool("minecraft_press_inputs", {"keys": sorted(release), "ticks": 1, "hold": False})
        if press:
            self.c.tool("minecraft_press_inputs", {"keys": sorted(press), "ticks": 1, "hold": True})
        self.held = set(wanted)

    def look(self, yaw: float, pitch: float):
        target = (int(round(wrap(yaw))), int(round(max(-90, min(90, pitch)))))
        if target == (180, target[1]):
            target = (-180, target[1])
        if target != self.sent_look:
            self.c.tool("minecraft_look", {"yaw_deg": target[0], "pitch_deg": target[1]})
            self.sent_look = target

    def tap(self, key: str):
        self.c.tool("minecraft_press_inputs", {"keys": [key], "ticks": 1})

    def player(self):
        observed = self.c.tool("minecraft_observe")
        if "player" not in observed:
            raise RuntimeError("not in play (respawning or loading)")
        return observed["player"]


def position(record) -> list[float]:
    return [float(record["x"]), float(record["y"]), float(record["z"])]


def attack_interval(client: Client, fallback: float) -> float:
    """Seconds between full-strength hits with the selected weapon: 1 / (4 + attack_speed modifier)."""
    try:
        stack = client.tool("minecraft_read_item_components", {"source": "selected"})
    except RuntimeError:
        return fallback
    for modifier in (stack.get("components") or {}).get("minecraft:attribute_modifiers") or []:
        if isinstance(modifier, dict) and modifier.get("type") == "minecraft:attack_speed":
            speed = 4.0 + float(modifier.get("amount", 0.0))
            if speed > 0:
                return 1.0 / speed + 0.05
    return fallback


def identity(entity):
    return {"entity_id": entity["entity_id"], "entity_uuid": entity["entity_uuid"],
            "entity_type": entity["entity_type"]}


def soldier(entity) -> bool:
    """A living mob or other player near PovRed: not himself, not a horse."""
    return not entity.get("local_player") and "horse" not in entity["entity_type"] and entity.get("health", 1) > 0


def side_of(client: Client, entity) -> str | None:
    for slot in SIDE_SLOTS:
        try:
            stack = client.tool("minecraft_read_item_components", {"source": "entity", "slot": slot, **identity(entity)})
        except RuntimeError:
            continue
        found = SIDE.search(json.dumps(stack))
        if found:
            return found.group(1)
        dye = (stack.get("components") or {}).get("minecraft:dyed_color")
        if isinstance(dye, dict):
            dye = dye.get("rgb")
        if isinstance(dye, int):
            red, blue = (dye >> 16) & 255, dye & 255
            if red > 1.3 * blue + 8:
                return "red"
            if blue > 1.3 * red + 8:
                return "blue"
    return None


def rank(args, client: Client):
    pilot = Pilot(client)
    guard = None if args.no_guard else Guard(client, None if args.no_keepout else tuple(args.keepout))
    client.tool("minecraft_wait_for_play", {"timeout_seconds": 120})
    log("in play; holding rank")
    me = pilot.player()
    yaw, pitch = float(me.get("yaw", 0.0)), float(me.get("pitch", 0.0))
    rest_yaw = yaw
    sides: dict[int, str | None] = {}
    offset = anchor = None
    members: set[int] = set()
    target = last_goal = None
    drift = [0.0, 0.0]  # blocks/s of his place, measured between steps
    last = None
    last_step = time.monotonic()
    flow = [0.0, 0.0]
    moving = False
    last_attack = 0.0
    last_status, steps = time.monotonic(), 0
    interval = attack_interval(client, args.attack_interval)
    log("attack interval", round(interval, 2), "s")
    while True:
        me = pilot.player()
        here = position(me)
        if me.get("health") is not None and float(me["health"]) <= 0:
            # Death screen: the ordinary Respawn action. The server then re-equips and
            # re-slots PovRed; the teleport below re-takes the offset to new neighbours.
            pilot.held.clear()
            log("dead at", [round(v, 1) for v in here], "; pressing Respawn")
            try:
                client.tool("minecraft_respawn", {"timeout_seconds": 30})
            except RuntimeError as error:
                log("respawn not accepted yet:", str(error)[:200])
                client.tool("minecraft_wait_ticks", {"ticks": 10})
                continue
            me = pilot.player()
            yaw = rest_yaw = float(me.get("yaw", yaw))
            pitch = float(me.get("pitch", pitch))
            offset, anchor, target, last, last_step = None, None, None, position(me), time.monotonic()
            last_goal, drift = None, [0.0, 0.0]
            pilot.sent_look = None
            log("respawned at", [round(v, 1) for v in last], "; waiting for the server to re-slot")
            continue
        # A server teleport (re-slot): farther than sprinting (under 8 blocks/s) can go.
        stepped = time.monotonic()
        step_time = stepped - last_step
        if last is not None and math.dist(here, last) > 4.0 + 8.0 * step_time:
            log("teleported to", [round(v, 1) for v in here], "; rank offset resets, facing", round(float(me.get("yaw", yaw))))
            offset, anchor, target, last_goal, drift = None, None, None, None, [0.0, 0.0]
            yaw = rest_yaw = float(me.get("yaw", yaw))
            pitch = float(me.get("pitch", pitch))
            interval = attack_interval(client, args.attack_interval)
            log("attack interval", round(interval, 2), "s")
        last, last_step = here, stepped
        listed = client.tool("minecraft_list_entities", {"radius": args.radius, "limit": args.limit})["entities"]
        soldiers = sorted((e for e in listed if soldier(e)), key=lambda e: e["distance"])
        friends, enemies, unread = [], [], []
        for entity in soldiers:
            side = sides.get(entity["entity_id"], "unread")
            if side == "unread":
                # Read after this step's movement and attack, so combat is not delayed.
                unread.append(entity)
            elif side == args.side:
                friends.append(entity)
            elif side is not None:
                enemies.append(entity)
        if offset is None and anchor is None:
            # The rank place is measured from one instant, right after join, respawn or
            # re-slot: PovRed and his nearest 2K neighbours as listed now, however long
            # reading their sides takes while the rank marches on.
            anchor = (here, soldiers[: 2 * args.k])
        if offset is None:
            placed, ready = [], True
            for entity in anchor[1]:
                side = sides.get(entity["entity_id"], "unread")
                if side == "unread":
                    ready = False
                    break
                if side == args.side:
                    placed.append(entity)
                    if len(placed) == args.k:
                        break
            if ready and len(placed) >= min(args.k, 3):
                # PovRed then follows these same soldiers (by id), not whoever is nearest
                # later: a nearest-K centroid shifts as he moves and walks him out of the
                # rank. The cap keeps a late measurement (pilot started after the march
                # began) inside the formation instead of behind it.
                then = [sum(e[a] for e in placed) / len(placed) for a in ("x", "z")]
                offset = [anchor[0][0] - then[0], anchor[0][2] - then[1]]
                length = math.hypot(*offset)
                if length > args.max_offset:
                    offset = [v * args.max_offset / length for v in offset]
                members = {e["entity_id"] for e in placed}
                log("rank offset", [round(v, 2) for v in offset], "from", len(placed), "same-side soldiers",
                    "(measured", round(length, 2), "blocks)")
            elif ready:
                anchor = None  # too few soldiers of his side near him: measure again next step
            # Their sides are read first, nearest first.
            first = [e for e in anchor[1] if e["entity_id"] not in sides] if anchor else []
            unread = first + [e for e in unread if e["entity_id"] not in {f["entity_id"] for f in first}]
        near = friends[: args.k]
        if near:
            velocity = [sum(e["velocity"][a] for e in near) / len(near) for a in ("x", "z")]
            flow = [0.8 * flow[i] + 0.2 * velocity[i] for i in range(2)]
        if offset is not None:
            tracked = [e for e in friends if e["entity_id"] in members]
            if len(tracked) < min(args.k, 3) and len(near) >= min(args.k, 3):
                # His soldiers left the nearest page (he lags or leads a few blocks, or they
                # fell): hand his place, unchanged, to the soldiers nearest him now. Any
                # soldiers who keep formation can carry it.
                place = target if target is not None else [here[0], here[2]]
                centroid = [sum(e[a] for e in near) / len(near) for a in ("x", "z")]
                offset = [place[0] - centroid[0], place[1] - centroid[1]]
                members = {e["entity_id"] for e in near}
                tracked = near
                log("rank place handed to", len(near), "nearer soldiers; offset", [round(v, 2) for v in offset])
            if len(tracked) >= min(args.k, 3):
                centroid = [sum(e[a] for e in tracked) / len(tracked) for a in ("x", "z")]
                goal = [centroid[0] + offset[0], centroid[1] + offset[1]]
                if last_goal is not None:
                    drift = [(goal[i] - last_goal[i]) / max(step_time, 0.05) for i in range(2)]
                last_goal = goal
                # Eased over ~0.3 s of client time, not per step: a per-step blend lags far
                # behind a marching rank when steps are slow.
                blend = 1.0 - math.exp(-step_time / 0.3)
                target = goal if target is None else [target[i] + blend * (goal[i] - target[i]) for i in range(2)]
        enemy = min(enemies, key=lambda e: e["distance"]) if enemies else None
        # Facing: the nearest enemy, else the rank's own motion, else the join-time facing.
        if enemy is not None and enemy["distance"] <= args.face_enemy:
            face_yaw, face_pitch = look_angles([here[0], here[1] + 1.62, here[2]],
                                               [enemy["x"], enemy["y"] + 1.3, enemy["z"]])
        elif math.hypot(*flow) > 0.03:
            face_yaw, face_pitch = math.degrees(math.atan2(-flow[0], flow[1])), 8.0
        else:
            face_yaw, face_pitch = rest_yaw, 8.0
        yaw = approach(yaw, face_yaw, 0.25, args.turn_rate)
        pitch = pitch + max(-3.0, min(3.0, (max(-20.0, min(30.0, face_pitch)) - pitch) * 0.25))
        pilot.look(yaw, pitch)
        wanted: set[str] = set()
        if target is not None:
            # Aim where his place will be a step from now: the rank keeps marching while
            # the step's other calls run, and the next correction comes a step later.
            aim = [target[i] + drift[i] * step_time for i in range(2)]
            dx, dz = aim[0] - here[0], aim[1] - here[2]
            distance = math.hypot(dx, dz)
            moving = distance > (0.4 if moving else 1.0)
            if moving:
                radians = math.radians(yaw)
                forward = dx * -math.sin(radians) + dz * math.cos(radians)
                right = dx * -math.cos(radians) + dz * -math.sin(radians)
                if forward > 0.35 * distance:
                    wanted.add("forward")
                elif forward < -0.35 * distance:
                    wanted.add("back")
                if right > 0.35 * distance:
                    wanted.add("right")
                elif right < -0.35 * distance:
                    wanted.add("left")
                if distance > 4.0 and "forward" in wanted and "back" not in wanted:
                    wanted.add("sprint")
                # Steps and pit walls: jump while walking into a block.
                if me.get("horizontal_collision"):
                    wanted.add("jump")
        now = time.monotonic()
        strike = enemy is not None and enemy["distance"] <= args.reach and now - last_attack >= interval
        # Shield up (the ordinary use input on the off-hand shield) while an enemy is
        # close, lowered for each swing: vanilla cannot attack while blocking.
        if args.guard > 0 and enemy is not None and enemy["distance"] <= args.guard and not strike:
            wanted.add("use")
        # Route guard: no key that walks into the keep-out box, off a drop or into water or lava.
        if guard is not None:
            speed = 5.6 if "sprint" in wanted else 4.3
            wanted = guard.filter(wanted, here, yaw, min(3.0, max(1.5, speed * step_time + 1.0)))
        # Held keys stay down for the whole step. When the last step was so long that
        # holding would carry him past his place, walk a bounded pulse of ticks instead.
        pulse = wanted & {"forward", "back", "left", "right", "sprint", "jump"}
        speed = 5.6 if "sprint" in pulse else 4.3
        if pulse and step_time * speed > distance + 0.5:
            wanted -= pulse
        else:
            pulse = set()
        pilot.keys(wanted)
        if pulse:
            client.tool("minecraft_press_inputs",
                        {"keys": sorted(pulse), "ticks": max(1, min(40, round(distance / speed * 20)))})
        if strike:
            # The ordinary attack: aim at any visible part of the enemy's box, attack
            # input, then wait briefly for authoritative damage or removal.
            before = enemy.get("health")
            try:
                result = client.tool("minecraft_attack_entity_once", {**identity(enemy), "timeout_seconds": 1.0})
                log("hit", enemy["entity_id"], "health before", before, "->", json.dumps(result)[:240])
            except RuntimeError as error:
                log("swing at", enemy["entity_id"], "no confirmed damage:", str(error)[:200])
            last_attack = time.monotonic()
            aimed = pilot.player()
            yaw, pitch = float(aimed.get("yaw", yaw)), float(aimed.get("pitch", pitch))
            pilot.sent_look = (int(round(wrap(yaw))), int(round(pitch)))
        # Classify unread neighbours, nearest first. While his place is unknown he stands
        # still, so many per step; afterwards two, because movement keys stay held for a
        # whole step and a long step overshoots his place (each read waits a client frame).
        budget = args.classify_per_step if offset is None else 2
        for entity in unread[:budget]:
            sides[entity["entity_id"]] = side_of(client, entity)
        client.tool("minecraft_wait_ticks", {"ticks": 2})
        steps += 1
        if time.monotonic() - last_status >= 5.0:
            elapsed = time.monotonic() - last_status
            log("status at", [round(v, 1) for v in here], "friends", len(friends), "enemies", len(enemies),
                "nearest enemy", None if enemy is None else round(enemy["distance"], 1),
                "keys", sorted(pilot.held), "step", round(elapsed / steps, 2), "s")
            last_status, steps = time.monotonic(), 0


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("runtime", type=pathlib.Path, help="the client's --runtime-dir")
    parser.add_argument("--evidence", type=pathlib.Path, default=pathlib.Path("pilot-evidence"))
    parser.add_argument("--transcript", action="store_true", help="keep the full MCP transcript (large)")
    parser.add_argument("--side", default="red", choices=("red", "blue"))
    parser.add_argument("--k", type=int, default=8, help="same-side soldiers whose centroid anchors the rank slot")
    parser.add_argument("--max-offset", type=float, default=3.0,
                        help="cap (blocks) on PovRed's place relative to that centroid")
    parser.add_argument("--radius", type=float, default=24.0)
    parser.add_argument("--reach", type=float, default=2.8, help="feet distance; the client's own limit is 3 blocks eye to box")
    parser.add_argument("--guard", type=float, default=6.0,
                        help="raise the off-hand shield while an enemy is this close (0 = never)")
    parser.add_argument("--face-enemy", type=float, default=16.0, help="face the nearest enemy within this range")
    parser.add_argument("--turn-rate", type=float, default=6.0, help="max degrees of turn per 0.1 s step")
    parser.add_argument("--attack-interval", type=float, default=0.65)
    parser.add_argument("--classify-per-step", type=int, default=8,
                        help="side reads per step until his place is measured (then 2)")
    parser.add_argument("--keepout", type=float, nargs=4, metavar=("X0", "X1", "Z0", "Z1"), default=list(KEEPOUT),
                        help="keep-out box in block cells, any height (default: the mine, x -192..-184, z 21..30)")
    parser.add_argument("--no-keepout", action="store_true")
    parser.add_argument("--no-guard", action="store_true", help="no block checks before steps (drops, water, lava)")
    parser.add_argument("--limit", type=int, default=64,
                        help="nearest entities read per step (larger pages can exceed the MCP result limit)")
    return parser.parse_args(argv)


def main():
    args = arguments()
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGHUP, interrupt)
    client = connect(args.runtime, args.evidence, args.transcript)
    try:
        while True:
            try:
                rank(args, client)
            except RuntimeError as error:
                # Recoverable: the window lost focus, or play is between screens. The
                # walk restarts from the current position with a fresh rank offset.
                # Waits are client events (state changes, play), never wall-clock sleeps.
                text = str(error)
                if "controller_busy" in text:
                    raise SystemExit("another MCP writer holds this client (a pilot killed with SIGKILL?); "
                                     "its lease frees after 120 idle seconds, then start the pilot again")
                code = next((code for code in RECOVERABLE if code in text), None)
                if code is None:
                    raise
                log("paused:", code, "- resuming on the next client event")
                if code == "window_not_focused":
                    wait_until_writable(client)
                else:
                    client.tool("minecraft_wait_for_play", {"timeout_seconds": 3600})
                    next_state_event(client)
    except KeyboardInterrupt:
        pass
    finally:
        try:
            client.tool("minecraft_press_inputs", {"keys": list(MOVE_KEYS), "ticks": 1, "hold": False})
        except RuntimeError as error:
            log("could not release movement keys:", str(error)[:120])
        release(client)


if __name__ == "__main__":
    main()
