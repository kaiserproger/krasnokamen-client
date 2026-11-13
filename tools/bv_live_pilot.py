#!/usr/bin/env python3
"""Live battle-video rank pilot (PovRed): drive a real windowed client through its local MCP endpoint.

  bv_live_pilot.py RUNTIME_DIR [--side red] [--k 8]

Keeps PovRed's join-time place relative to the centroid of the K nearest same-side
soldiers (side read from their shield/surcoat ids or dyed kit), walks with held movement
keys, turns smoothly toward the nearest enemy (else along the rank's motion), and
presses attack when an enemy is in reach. RUNTIME_DIR is the client's --runtime-dir;
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
    """The newest instance descriptor in RUNTIME_DIR (the client writes it at startup)."""
    deadline = time.monotonic() + 120
    while True:
        descriptors = sorted(runtime.glob("*.json"), key=lambda p: p.stat().st_mtime)
        if descriptors:
            break
        if time.monotonic() > deadline:
            raise SystemExit(f"no client descriptor in {runtime}; start the client with --runtime-dir {runtime}")
        time.sleep(0.5)
    client = Client(descriptors[-1], evidence)
    if not transcript:
        client.trace.close()
        client.trace = open(os.devnull, "w")
    client.initialize()
    log("attached", descriptors[-1])
    return client


RECOVERABLE = ("window_not_focused", "controller_busy", "not in play", "client_not_in_play", "dialog_screen",
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
        return self.c.tool("minecraft_observe")["player"]


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
    client.tool("minecraft_wait_for_play", {"timeout_seconds": 120})
    log("in play; holding rank")
    me = pilot.player()
    yaw, pitch = float(me.get("yaw", 0.0)), float(me.get("pitch", 0.0))
    rest_yaw = yaw
    sides: dict[int, str | None] = {}
    offset = None
    target = None
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
            offset, target, last, last_step = None, None, position(me), time.monotonic()
            pilot.sent_look = None
            log("respawned at", [round(v, 1) for v in last], "; waiting for the server to re-slot")
            continue
        # A server teleport (re-slot): farther than sprinting (under 8 blocks/s) can go.
        stepped = time.monotonic()
        if last is not None and math.dist(here, last) > 4.0 + 8.0 * (stepped - last_step):
            log("teleported to", [round(v, 1) for v in here], "; rank offset resets, facing", round(float(me.get("yaw", yaw))))
            offset, target = None, None
            yaw = rest_yaw = float(me.get("yaw", yaw))
            pitch = float(me.get("pitch", pitch))
            interval = attack_interval(client, args.attack_interval)
            log("attack interval", round(interval, 2), "s")
        last, last_step = here, stepped
        listed = client.tool("minecraft_list_entities", {"radius": args.radius, "limit": args.limit})["entities"]
        new = pending = 0
        friends, enemies = [], []
        for entity in listed:
            if entity.get("local_player") or "horse" in entity["entity_type"] or entity.get("health", 1) <= 0:
                continue
            ident = entity["entity_id"]
            if ident not in sides:
                if new >= args.classify_per_step:
                    pending += 1
                    continue
                sides[ident] = side_of(client, entity)
                new += 1
            side = sides[ident]
            if side == args.side:
                friends.append(entity)
            elif side is not None:
                enemies.append(entity)
        near = sorted(friends, key=lambda e: e["distance"])[: args.k]
        if len(near) >= min(args.k, 3):
            centroid = [sum(e[a] for e in near) / len(near) for a in ("x", "z")]
            velocity = [sum(e["velocity"][a] for e in near) / len(near) for a in ("x", "z")]
            flow = [0.8 * flow[i] + 0.2 * velocity[i] for i in range(2)]
            # The join-time offset waits until every listed neighbour has a side, so
            # late classification does not shift the centroid it is measured from.
            if offset is None and pending == 0:
                offset = [here[0] - centroid[0], here[2] - centroid[1]]
                log("rank offset", [round(v, 2) for v in offset], "from", len(near), "same-side soldiers")
            if offset is not None:
                goal = [centroid[0] + offset[0], centroid[1] + offset[1]]
                target = goal if target is None else [0.7 * target[i] + 0.3 * goal[i] for i in range(2)]
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
            dx, dz = target[0] - here[0], target[1] - here[2]
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
        pilot.keys(wanted)
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
        client.tool("minecraft_wait_ticks", {"ticks": 2})
        steps += 1
        if time.monotonic() - last_status >= 5.0:
            elapsed = time.monotonic() - last_status
            log("status at", [round(v, 1) for v in here], "friends", len(friends), "enemies", len(enemies),
                "nearest enemy", None if enemy is None else round(enemy["distance"], 1),
                "keys", sorted(pilot.held), "step", round(elapsed / steps, 2), "s")
            last_status, steps = time.monotonic(), 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("runtime", type=pathlib.Path, help="the client's --runtime-dir")
    parser.add_argument("--evidence", type=pathlib.Path, default=pathlib.Path("pilot-evidence"))
    parser.add_argument("--transcript", action="store_true", help="keep the full MCP transcript (large)")
    parser.add_argument("--side", default="red", choices=("red", "blue"))
    parser.add_argument("--k", type=int, default=8, help="same-side soldiers whose centroid anchors the rank slot")
    parser.add_argument("--radius", type=float, default=24.0)
    parser.add_argument("--reach", type=float, default=2.8, help="feet distance; the client's own limit is 3 blocks eye to box")
    parser.add_argument("--guard", type=float, default=6.0,
                        help="raise the off-hand shield while an enemy is this close (0 = never)")
    parser.add_argument("--face-enemy", type=float, default=16.0, help="face the nearest enemy within this range")
    parser.add_argument("--turn-rate", type=float, default=6.0, help="max degrees of turn per 0.1 s step")
    parser.add_argument("--attack-interval", type=float, default=0.65)
    parser.add_argument("--classify-per-step", type=int, default=8)
    parser.add_argument("--limit", type=int, default=64,
                        help="nearest entities read per step (larger pages can exceed the MCP result limit)")
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGHUP, interrupt)
    client = connect(args.runtime, args.evidence, args.transcript)
    try:
        while True:
            try:
                rank(args, client)
            except RuntimeError as error:
                # Recoverable: the window lost focus, a previous pilot's writer lease is
                # still idle-expiring (up to 120 s), or play is between screens. The
                # walk restarts from the current position with a fresh rank offset.
                text = str(error)
                if not any(code in text for code in RECOVERABLE):
                    raise
                log("paused:", next(code for code in RECOVERABLE if code in text), "- retrying in 2 s")
                time.sleep(2)
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
