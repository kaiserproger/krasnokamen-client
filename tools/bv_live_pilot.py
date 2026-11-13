#!/usr/bin/env python3
"""Live battle-video pilots: drive a real windowed client through its local MCP endpoint.

  bv_live_pilot.py rank   RUNTIME_DIR [--side red] [--k 8]   PovRed: keep his place in rank, face the enemy, swing in reach
  bv_live_pilot.py follow RUNTIME_DIR                        CamSpec: hide the HUD, F5 once (third person on the Set Camera target)

RUNTIME_DIR is the client's --runtime-dir; the client must run with
`--automation --mcp-http 127.0.0.1:0 --windowed`. Every action is an ordinary client
input (held movement keys, absolute look, the attack key, F1/F5), never a packet bot.
The loop is paced by client ticks (minecraft_wait_ticks), not by wall-clock sleeps.
"""
from __future__ import annotations

import argparse
import json
import math
import os
import pathlib
import re
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from mcp_client import Client  # noqa: E402

SIDE = re.compile(r"solaris_battle:(red|blue)_")
MOVE_KEYS = ("forward", "back", "left", "right", "sprint")
# Equipment slots read for side gear: off-hand shield, chest surcoat, main hand.
SIDE_SLOTS = (1, 4, 0)


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
    return None


def rank(args, client: Client):
    pilot = Pilot(client)
    client.tool("minecraft_wait_for_play", {"timeout_seconds": 120})
    log("in play; F1 hides the HUD (the hand stays with --hidden-hud-keeps-hand)")
    pilot.tap("key.keyboard.f1")
    me = pilot.player()
    yaw, pitch = float(me.get("yaw", 0.0)), float(me.get("pitch", 0.0))
    rest_yaw = yaw
    sides: dict[int, str | None] = {}
    offset = None
    target = None
    last = None
    flow = [0.0, 0.0]
    moving = False
    last_attack = 0.0
    while True:
        me = pilot.player()
        here = position(me)
        if me.get("health") is not None and float(me["health"]) <= 0:
            pilot.keys(set())
            client.tool("minecraft_wait_ticks", {"ticks": 10})
            continue
        if last is not None and math.dist(here, last) > 6.0:
            log("teleported to", [round(v, 1) for v in here], "; rank offset resets, facing", round(float(me.get("yaw", yaw))))
            offset, target = None, None
            yaw = rest_yaw = float(me.get("yaw", yaw))
            pitch = float(me.get("pitch", pitch))
        last = here
        listed = client.tool("minecraft_list_entities", {"radius": args.radius, "limit": 160})["entities"]
        new = 0
        friends, enemies = [], []
        for entity in listed:
            if entity.get("local_player") or "horse" in entity["entity_type"] or entity.get("health", 1) <= 0:
                continue
            ident = entity["entity_id"]
            if ident not in sides:
                if new >= args.classify_per_step:
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
            if offset is None:
                offset = [here[0] - centroid[0], here[2] - centroid[1]]
                log("rank offset", [round(v, 2) for v in offset], "from", len(near), "same-side soldiers")
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
        pilot.keys(wanted)
        now = time.monotonic()
        if (enemy is not None and enemy["distance"] <= args.reach and now - last_attack >= args.attack_interval
                and abs(wrap(face_yaw - yaw)) < 25.0):
            pilot.tap("attack")
            last_attack = now
        client.tool("minecraft_wait_ticks", {"ticks": 2})


def follow(args, client: Client):
    pilot = Pilot(client)
    client.tool("minecraft_wait_for_play", {"timeout_seconds": 120})
    log("in play; F1 hides the HUD, F5 once: third person behind the Set Camera target")
    pilot.tap("key.keyboard.f1")
    pilot.tap("key.keyboard.f5")
    log("done; the camera now follows whatever entity the server's Set Camera names")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("mode", choices=("rank", "follow"))
    parser.add_argument("runtime", type=pathlib.Path, help="the client's --runtime-dir")
    parser.add_argument("--evidence", type=pathlib.Path, default=pathlib.Path("pilot-evidence"))
    parser.add_argument("--transcript", action="store_true", help="keep the full MCP transcript (large)")
    parser.add_argument("--side", default="red", choices=("red", "blue"))
    parser.add_argument("--k", type=int, default=8, help="same-side soldiers whose centroid anchors the rank slot")
    parser.add_argument("--radius", type=float, default=24.0)
    parser.add_argument("--reach", type=float, default=3.0)
    parser.add_argument("--face-enemy", type=float, default=16.0, help="face the nearest enemy within this range")
    parser.add_argument("--turn-rate", type=float, default=6.0, help="max degrees of turn per 0.1 s step")
    parser.add_argument("--attack-interval", type=float, default=0.65)
    parser.add_argument("--classify-per-step", type=int, default=24)
    args = parser.parse_args()
    client = connect(args.runtime, args.evidence, args.transcript)
    try:
        (rank if args.mode == "rank" else follow)(args, client)
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
