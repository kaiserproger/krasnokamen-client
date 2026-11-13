"""PovRed's live pilot (`tools/bv_live_pilot.py`) against a simulated client: a red block
marching +Z, PovRed placed in its front rank, every MCP call costing client time. He must keep
his place: never walk far past it (held keys on a slow step) and never fall far behind it (a
place measured late, or eased per step instead of per second)."""
import math
import time
import types
import unittest
from unittest import mock

from tools import bv_live_pilot as pilot

TICK = 1 / 20


class Stop(Exception):
    """End of the simulated run (not a RuntimeError, so the pilot does not swallow it)."""


def client_reach(me, entity) -> float:
    """The client's reach measure, independent of the pilot's: eye (1.62 up) to the nearest point of the entity's box."""
    box = entity["bounding_box"]
    eye = (me[0], 64.0 + 1.62, me[1])
    near = (min(max(eye[0], box["min_x"]), box["max_x"]), min(max(eye[1], box["min_y"]), box["max_y"]),
            min(max(eye[2], box["min_z"]), box["max_z"]))
    return math.dist(eye, near)


class MarchingRank:
    """The client side of the pilot's MCP calls. Soldiers: 3 ranks x 13 files, 1 block apart in a
    rank, 1.5 between ranks; PovRed's slot is rank 0, file 0 (left empty). `call` seconds of
    client time pass on every MCP call, as with a slow frame rate."""

    def __init__(self, call: float, march: float = 1.2, duration: float = 60.0, floor=lambda x, z: 63, spike=None):
        self.now, self.call, self.march, self.duration = 0.0, call, march, duration
        self.spike = spike  # (time, seconds): from that client time on, every call takes that long
        self.held_during = []  # tools called while a movement key was held down
        self.floor = floor  # y of the solid block under column (x, z), or None for a void
        self.me, self.yaw, self.pitch = [0.0, 0.0], 0.0, 0.0
        self.held: set[str] = set()
        self.ys: dict[int, float] = {}  # feet y of soldiers that are not at 64
        self.using_ticks = 0  # ticks the client still reports an item in use after the use key is let go
        self.always_using = False
        self.waits, self.entity_reads, self.press_times = [], [], []  # wait_ticks counts; ids whose side was read; client time of each attack press
        self.spawn = []  # (time, id, [x, z]): a soldier that appears
        self.list_times = []  # client time of each entity list: one per pilot step
        self.vanish = None  # (time, ids): from that time those soldiers are far away
        self.selected_free = True  # reads of the selected item (the weapon interval) also take no march time
        self.since_attack, self.charge_full, self.hud = 99, 13, True  # ticks since the last attack; ticks to full strength; HUD in observe
        self.reach_limit = 3.0  # the client's raycast reach for entities
        self.press_distances = []  # eye distance to the entity under the crosshair at each attack press
        self.press_strengths = []  # attack strength at each attack press
        self.other = {}  # slot -> stack outside the hotbar (armour 36-39, offhand 40)
        self.hotbar, self.selected = {}, 0  # slot -> stack; selected slot
        self.mode, self.once_calls = "input", []  # the pilot's --attack-mode; targets of minecraft_attack_entity_once
        self.attacks, self.hits = [], []  # attack inputs pressed / entities they struck
        self.soldiers = {}
        ident = 100
        for rank in range(3):
            for file in range(-6, 7):
                if rank == 0 and file == 0:
                    continue  # PovRed's slot
                self.soldiers[ident] = [float(file), -1.5 * rank]
                ident += 1
        self.samples = []  # (time, along-march error, distance) to the slot, every tick

    def advance(self, seconds: float, keys=None):
        keys = self.held if keys is None else keys
        for _ in range(max(1, round(seconds / TICK))):
            radians = math.radians(self.yaw)
            ahead = ("forward" in keys) - ("back" in keys)
            side = ("right" in keys) - ("left" in keys)
            # The pilot's frame: forward (-sin, cos), right (-cos, -sin).
            vx = ahead * -math.sin(radians) + side * -math.cos(radians)
            vz = ahead * math.cos(radians) + side * -math.sin(radians)
            norm = math.hypot(vx, vz)
            if norm:
                speed = 5.6 if "sprint" in keys else 4.3
                self.me[0] += speed * vx / norm * TICK
                self.me[1] += speed * vz / norm * TICK
            for appearing in [w for w in self.spawn if self.now >= w[0]]:
                self.spawn.remove(appearing)
                self.soldiers[appearing[1]] = list(appearing[2])
            if self.vanish and self.now >= self.vanish[0]:
                for ident in self.vanish[1]:
                    self.soldiers[ident] = [500.0, 500.0]
            for position in self.soldiers.values():
                position[1] += self.march * TICK
            self.now += TICK
            self.using_ticks = max(0, self.using_ticks - 1)
            self.since_attack += 1
            slot = [0.0, self.march * self.now]
            self.samples.append((self.now, self.me[1] - slot[1], math.dist(self.me, slot)))
        if self.now >= self.duration:
            raise Stop

    def strength(self):
        return min(1.0, self.since_attack / self.charge_full)

    def ui_nodes(self):
        """The HUD as the client observes it: a 30-pixel crosshair, and the attack bar below full strength."""
        nodes = [{"id": "hud/crosshair", "role": "image", "bounds": [100.0, 100.0, 30.0, 30.0]}]
        if self.strength() < 1.0:
            nodes.append({"id": "hud/attack", "role": "progressbar", "bounds": [100.0, 140.0, 30.0 * self.strength(), 4.0]})
        return nodes

    def crosshair(self, look=None):
        """The client's raycast from the eye along the look: the nearest soldier box within 3 blocks of reach."""
        yaw, pitch = (math.radians(self.yaw), math.radians(self.pitch)) if look is None else map(math.radians, look)
        direction = (-math.sin(yaw) * math.cos(pitch), -math.sin(pitch), math.cos(yaw) * math.cos(pitch))
        eye = (self.me[0], 64.0 + 1.62, self.me[1])
        best = None
        for ident, position in self.soldiers.items():
            box = self.entity(ident, position)["bounding_box"]
            near, far = 0.0, self.reach_limit
            for axis, name in enumerate("xyz"):
                lo, hi = box[f"min_{name}"], box[f"max_{name}"]
                if abs(direction[axis]) < 1e-9:
                    if not lo <= eye[axis] <= hi:
                        near, far = 1.0, 0.0
                    continue
                t1, t2 = (lo - eye[axis]) / direction[axis], (hi - eye[axis]) / direction[axis]
                near, far = max(near, min(t1, t2)), min(far, max(t1, t2))
            if near <= far and (best is None or near < best[0]):
                best = (near, ident)
        if best is None:
            return {"type": "none"}
        point = [eye[axis] + best[0] * direction[axis] for axis in range(3)]
        return {"type": "entity", "location": dict(zip("xyz", point)), "entity_id": best[1],
                "entity_uuid": f"uuid-{best[1]}", "entity_type": "minecraft:husk"}

    def entity(self, ident, position):
        dx, dz = position[0] - self.me[0], position[1] - self.me[1]
        y = self.ys.get(ident, 64.0)
        box = {"min_x": position[0] - 0.3, "max_x": position[0] + 0.3, "min_y": y, "max_y": y + 1.95,
               "min_z": position[1] - 0.3, "max_z": position[1] + 0.3}
        return {"entity_id": ident, "entity_uuid": f"uuid-{ident}", "entity_type": "minecraft:husk", "bounding_box": box,
                "x": position[0], "y": y, "z": position[1], "distance": math.hypot(dx, dz),
                "velocity": {"x": 0.0, "y": 0.0, "z": self.march * TICK}, "health": 20.0}

    def tool(self, name, arguments=None):
        arguments = arguments or {}
        if name != "minecraft_press_inputs" and self.held & {"forward", "back", "left", "right", "sprint", "jump"}:
            self.held_during.append(name)
        # The hotbar reads and the slot choice happen at join, before the march begins on the real server: they take no march time here.
        startup = name == "minecraft_set_hotbar_slot" or (name == "minecraft_read_item_components" and arguments["source"] in ("player", "selected") and self.selected_free)
        if not startup:
            self.advance(self.spike[1] if self.spike and self.now >= self.spike[0] else self.call)
        if name == "minecraft_wait_for_play":
            return {}
        if name == "minecraft_observe":
            return {"state_version": 1, "player": {"x": self.me[0], "y": 64.0, "z": self.me[1], "yaw": self.yaw,
                                                  "pitch": 0.0, "health": 20.0, "horizontal_collision": False},
                    "inputs": {"held": sorted(self.held)},
                    "using": {"item": "minecraft:shield"} if self.always_using or "use" in self.held or self.using_ticks else None,
                    "target": self.crosshair(), **({"ui_nodes": self.ui_nodes()} if self.hud else {})}
        if name == "minecraft_list_entities":
            self.list_times.append(self.now)
            listed = sorted((self.entity(i, p) for i, p in self.soldiers.items()), key=lambda e: e["distance"])
            return {"entities": [e for e in listed if e["distance"] <= arguments["radius"]][: arguments["limit"]]}
        if name == "minecraft_read_item_components":
            if arguments["source"] == "selected":
                if self.selected not in self.hotbar:
                    raise RuntimeError("no held item data")
                return self.hotbar[self.selected]
            if arguments["source"] == "player":
                # The client's Inventory numbering: 0-8 hotbar; 36-40 armour and offhand (self.other); size 43.
                if arguments["slot"] < 9 and arguments["slot"] in self.hotbar:
                    return self.hotbar[arguments["slot"]]
                if arguments["slot"] in self.other:
                    return self.other[arguments["slot"]]
                raise RuntimeError("player inventory slot not received")
            self.entity_reads.append(arguments.get("entity_id"))
            side = "blue" if arguments.get("entity_id", 0) >= 1000 else "red"
            return {"components": {"minecraft:item_model": f"solaris_battle:{side}_shield"}}
        if name == "minecraft_set_hotbar_slot":
            self.selected = arguments["slot"]
            return {"status": "ok"}
        if name == "minecraft_attack_entity_once":
            if self.mode != "once":
                raise AssertionError("--attack-mode input must not use minecraft_attack_entity_once")
            target = arguments["entity_id"]
            self.once_calls.append(target)
            if self.always_using or "use" in self.held or self.using_ticks:
                raise RuntimeError("ordinary entity attack was not dispatched")
            where = self.entity(target, self.soldiers[target])
            if client_reach(self.me, where) > 3.0:
                raise RuntimeError("target entity is out of interaction reach: 4.1 blocks; approach first")
            box = where["bounding_box"]
            centre = [(box["min_x"] + box["max_x"]) / 2, (box["min_y"] + box["max_y"]) / 2, (box["min_z"] + box["max_z"]) / 2]
            eye = (self.me[0], 64.0 + 1.62, self.me[1])
            yaw, pitch = pilot.look_angles(eye, [centre[0], centre[1], centre[2]])
            # the client's aim: the centre ray; a different entity there is "occluded by entity N"
            seen = self.crosshair((yaw, pitch))
            if seen["type"] == "entity" and seen["entity_id"] != target:
                raise RuntimeError(f"target entity is occluded by entity {seen['entity_id']}; reposition for a clear line")
            self.hits.append(target)
            return {"dispatched": True, "confirmed": False}
        if name == "minecraft_scan_blocks":
            return scan(lambda x, y, z: block(solid=self.floor(x, z) is not None and y <= self.floor(x, z)), arguments)
        if name == "minecraft_look":
            assert float(arguments["yaw_deg"]).is_integer() and float(arguments["pitch_deg"]).is_integer(), "the client takes whole degrees"
            self.yaw, self.pitch = float(arguments["yaw_deg"]), float(arguments["pitch_deg"])
            return {}
        if name == "minecraft_press_inputs":
            keys = set(arguments["keys"])
            if "hold" in arguments:
                if not arguments["hold"] and "use" in keys and "use" in self.held:
                    self.using_ticks = 2  # the client reports the item in use until two ticks after the release
                self.held = self.held | keys if arguments["hold"] else self.held - keys
            else:
                if "attack" in keys:
                    self.press_strengths.append(self.strength())
                    self.since_attack = 0
                    self.press_times.append(self.now)
                    seen = self.crosshair()
                    if seen["type"] == "entity":
                        spot = seen["location"]
                        self.press_distances.append(math.dist((self.me[0], 65.62, self.me[1]), (spot["x"], spot["y"], spot["z"])))
                    # The client's ordinary attack edge: dropped while an item is in use, else it strikes the crosshair entity.
                    self.attacks.append(self.crosshair().get("entity_id"))
                    if not (self.always_using or "use" in self.held or self.using_ticks):
                        kind = self.crosshair()
                        if kind["type"] == "entity":
                            self.hits.append(kind["entity_id"])
                self.advance(arguments["ticks"] * TICK, self.held | keys)
            return {}
        if name == "minecraft_wait_ticks":
            self.waits.append((self.now, arguments["ticks"]))
            self.advance(arguments["ticks"] * TICK)
            return {}
        raise AssertionError(f"unexpected tool {name}")


def block(solid=False, water=False, lava=False):
    """The scan row of a cell: stone, water, lava or air."""
    name = "minecraft:lava" if lava else "minecraft:water" if water else "minecraft:stone" if solid else "minecraft:air"
    return {"block_id": name, "is_air": not (solid or water or lava), "collision_empty": not solid,
            "fluid": {"empty": not (water or lava)}}


def scan(cell, arguments):
    """`minecraft_scan_blocks` over a function (x, y, z) -> row; the rows in the client's y/z/x order."""
    rows = [{"x": x, "y": y, "z": z, **cell(x, y, z)}
            for y in range(arguments["min_y"], arguments["max_y"] + 1)
            for z in range(arguments["min_z"], arguments["max_z"] + 1)
            for x in range(arguments["min_x"], arguments["max_x"] + 1)]
    return {"count": len(rows), "total_count": len(rows), "truncated": False, "blocks": rows}


def march(call: float, floor=lambda x, z: 63, logs=None, spike=None, **options) -> MarchingRank:
    sim = MarchingRank(call, floor=floor, spike=spike)
    saved = pilot.time, pilot.log
    pilot.time = types.SimpleNamespace(monotonic=lambda: sim.now, strftime=time.strftime)
    pilot.log = (lambda *parts: logs.append(" ".join(str(p) for p in parts))) if logs is not None else (lambda *parts: None)
    try:
        pilot.rank(pilot.arguments(["runtime", *options.get("argv", [])]), sim)
    except Stop:
        pass
    finally:
        pilot.time, pilot.log = saved
    return sim


class RankPilot(unittest.TestCase):
    def settled(self, sim):
        return [s for s in sim.samples if s[0] >= 15.0]

    def test_slow_client_keeps_near_his_place(self):
        # 0.4 s per MCP call: ~3.3 s steps, as on the host's software renderer in the march,
        # while the rank marches 1.2 blocks/s (about 4 blocks per step).
        sim = march(call=0.4)
        settled = self.settled(sim)
        self.assertLess(max(s[1] for s in settled), 3.0, "walked far past his place (held keys on a slow step)")
        self.assertLess(max(s[2] for s in settled), 6.5, "fell far behind his place")
        self.assertLess(sum(s[2] for s in settled) / len(settled), 3.5, "not near his place on average")

    def test_fast_client_holds_his_place(self):
        sim = march(call=0.03)
        # Bounded pulses instead of held keys lag a little more than held keys did (max 1.5 -> 1.6, mean the same).
        settled = self.settled(sim)
        self.assertLess(max(s[2] for s in settled), 1.75)
        self.assertLess(sum(s[2] for s in settled) / len(settled), 0.8)


class FakeWorld:
    """The block side of the client's MCP: `cell(x, y, z)` rows from a dict of cells; counts the scans."""

    def __init__(self, floor_y=72, cells=None, fail=None):
        self.floor_y, self.cells, self.fail, self.scans = floor_y, cells or {}, fail, []

    def tool(self, name, arguments=None):
        assert name == "minecraft_scan_blocks", name
        self.scans.append(arguments)
        if self.fail:
            raise RuntimeError(self.fail)
        return scan(lambda x, y, z: self.cells.get((x, y, z), block(solid=y <= self.floor_y)), arguments)


class RouteGuard(unittest.TestCase):
    """The guard's choices on a static world: a key toward a drop, water, lava, unread blocks or the keep-out
    box is dropped (and logged); others stay. Feet at y 73, stone floor at y 72, walking +X (yaw -90) unless
    stated."""

    HERE = [100.5, 73.0, 100.5]

    def run_filter(self, world, wanted, here=None, yaw=-90.0, keepout=pilot.KEEPOUT, reach=3.0):
        logs = []
        saved = pilot.log
        pilot.log = lambda *parts: logs.append(" ".join(str(p) for p in parts))
        try:
            guard = pilot.Guard(world, keepout)
            kept = guard.filter(set(wanted), here or self.HERE, yaw, reach)
        finally:
            pilot.log = saved
        return kept, logs

    def test_flat_ground_keeps_every_key(self):
        kept, logs = self.run_filter(FakeWorld(), {"forward", "right", "sprint", "jump"})
        self.assertEqual(kept, {"forward", "right", "sprint", "jump"})
        self.assertEqual(logs, [])

    def test_a_drop_of_three_blocks_is_walkable_and_four_is_refused(self):
        three = FakeWorld(cells={(x, y, 100): block() for x in (101, 102, 103) for y in (70, 71, 72)})  # ground at y 69: standing 4 -> 3 blocks lower
        kept, logs = self.run_filter(three, {"forward"})
        self.assertEqual((kept, logs), ({"forward"}, []), "ground at y 69: a drop of 3 blocks")
        four = FakeWorld(cells={(x, y, 100): block() for x in (101, 102, 103) for y in (69, 70, 71, 72)})  # ground at y 68: a drop of 4
        kept, logs = self.run_filter(four, {"forward"})
        self.assertEqual(kept, set())
        self.assertEqual(len(logs), 1)
        self.assertRegex(logs[0], r"^edge refused forward at 101,73,100 drop=4\+")

    def test_the_log_gives_the_first_refused_cell_and_the_drop(self):
        world = FakeWorld(cells={(102, y, 100): block() for y in (70, 71, 72)})  # a 2-deep pit only at x 102: allowed
        kept, logs = self.run_filter(world, {"forward"})
        self.assertEqual((kept, logs), ({"forward"}, []))

    def test_water_and_lava_cells_are_refused(self):
        for liquid in ("water", "lava"):
            world = FakeWorld(cells={(102, 72, 100): block(**{liquid: True})})
            kept, logs = self.run_filter(world, {"forward"})
            self.assertEqual(kept, set(), liquid)
            self.assertRegex(logs[0], rf"^edge refused forward at 102,73,100 drop=\d.*{liquid}")

    def test_unread_blocks_and_block_read_errors_refuse_the_key(self):
        kept, logs = self.run_filter(FakeWorld(fail="block x,y,z is not loaded"), {"forward"})
        self.assertEqual(kept, set())
        self.assertIn("edge refused forward", logs[0])
        self.assertIn("not loaded", logs[0])
        world = FakeWorld()
        real = world.tool
        world.tool = lambda name, arguments=None: {**real(name, arguments), "blocks": []}  # a page with no rows
        kept, _ = self.run_filter(world, {"forward"})
        self.assertEqual(kept, set())

    def test_a_recoverable_client_error_is_not_swallowed(self):
        with self.assertRaises(RuntimeError):
            self.run_filter(FakeWorld(fail="window_not_focused"), {"forward"})

    def test_only_the_key_toward_the_edge_is_dropped_and_sprint_follows_forward(self):
        # yaw -90: forward is +X, right (-cos, -sin) is +Z, left is -Z: the pit is on the right.
        right_pit = FakeWorld(cells={(100, y, 102): block() for y in range(60, 73)})
        kept, logs = self.run_filter(right_pit, {"forward", "right", "sprint"})
        self.assertEqual(kept, {"forward", "sprint"})
        self.assertIn("edge refused right", logs[0])
        forward_pit = FakeWorld(cells={(x, y, 100): block() for x in range(101, 110) for y in range(60, 73)})
        kept, _ = self.run_filter(forward_pit, {"forward", "left", "sprint", "jump"})
        self.assertEqual(kept, {"left", "jump"}, "sprint needs forward")
        kept, _ = self.run_filter(forward_pit, {"forward", "sprint", "jump"})
        self.assertEqual(kept, set(), "nothing walks, so no jump or sprint either")

    def test_back_and_left_are_checked_in_their_own_directions(self):
        pit_behind = FakeWorld(cells={(x, y, 100): block() for x in range(90, 100) for y in range(60, 73)})
        kept, logs = self.run_filter(pit_behind, {"back", "forward"})
        self.assertEqual(kept, {"forward"})
        self.assertIn("edge refused back", logs[0])
        pit_left = FakeWorld(cells={(100, y, z): block() for z in range(95, 100) for y in range(60, 73)})  # left is -Z
        kept, logs = self.run_filter(pit_left, {"left", "forward"})
        self.assertEqual(kept, {"forward"})
        self.assertIn("edge refused left", logs[0])

    def test_keepout_box_refuses_steps_into_it_without_reading_blocks(self):
        world = FakeWorld()
        here = [-194.5, 73.0, 25.5]  # west of the box (x -192..-184), forward is +X
        kept, logs = self.run_filter(world, {"forward", "back", "sprint"}, here=here)
        self.assertEqual(kept, {"back"})
        self.assertRegex(logs[0], r"^keepout refused forward at \[-194.5, 73.0, 25.5\]")
        self.assertEqual([s["min_x"] for s in world.scans], [-198], "only the back key read blocks")
        kept, _ = self.run_filter(FakeWorld(), {"forward"}, here=[-194.5, 73.0, 31.8])
        self.assertEqual(kept, {"forward"}, "z 31.8 passes beside the box (z 21..30)")
        kept, _ = self.run_filter(FakeWorld(), {"forward"}, here=[-194.5, 5.0, 25.5], reach=3.0)
        self.assertEqual(kept, set(), "any height: the box has no ceiling or floor")

    def test_the_box_edge_counts_the_players_width(self):
        kept, _ = self.run_filter(FakeWorld(), {"forward"}, here=[-192.9, 73.0, 25.5], reach=0.7)
        self.assertEqual(kept, set(), "0.9 blocks from the box at x -192, the player's 0.3 radius and the 0.7 look step reach it")

    def test_from_inside_the_box_only_the_way_out_is_allowed(self):
        here = [-188.5, 73.0, 24.5]  # inside; the box centre is (-187.5, 26)
        kept, logs = self.run_filter(FakeWorld(), {"forward", "back"}, here=here)  # forward +X is deeper, back -X leaves
        self.assertEqual(kept, {"back"})
        self.assertIn("keepout refused forward", logs[0])

    def test_the_diagonal_of_two_keys_is_checked_too(self):
        # Forward (+X) and right (+Z) are each safe cell by cell; the diagonal cell (101, 101) is a pit.
        pit = FakeWorld(cells={(101, y, 101): block() for y in range(60, 73)})
        kept, logs = self.run_filter(pit, {"forward", "right", "sprint"})
        self.assertEqual(kept, {"forward", "sprint"}, "the sideways key goes")
        self.assertEqual(len(logs), 1)
        self.assertRegex(logs[0], r"^edge refused forward\+right at 101,73,101 drop=4\+")
        kept, logs = self.run_filter(FakeWorld(), {"forward", "right", "sprint"})
        self.assertEqual((kept, logs), ({"forward", "right", "sprint"}, []))
        kept, _ = self.run_filter(pit, {"forward", "back"})
        self.assertEqual(kept, {"forward", "back"}, "opposite keys cancel: no diagonal")

    def test_the_diagonal_into_the_keepout_box_is_refused(self):
        # South-west of the box (x -192..-184, z 21..30): forward (+X) runs below z 21 and right (+Z) west of
        # x -192, both outside, but their diagonal runs into the box.
        here = [-193.2, 73.0, 20.0]
        kept, logs = self.run_filter(FakeWorld(), {"forward", "right"}, here=here, reach=3.0)
        self.assertEqual(kept, {"forward"})
        self.assertRegex(logs[0], r"^keepout refused forward\+right")

    def test_the_way_out_of_the_box_is_checked_for_drops_and_water(self):
        here = [-188.5, 73.0, 24.5]  # inside; back is -X and leaves
        kept, _ = self.run_filter(FakeWorld(), {"back"}, here=here)
        self.assertEqual(kept, {"back"})
        pit = FakeWorld(cells={(-190, y, 24): block() for y in range(60, 73)})
        kept, logs = self.run_filter(pit, {"back"}, here=here)
        self.assertEqual(kept, set())
        self.assertIn("edge refused back at -190,73,24", logs[0])
        lake = FakeWorld(cells={(-190, 72, 24): block(water=True)})
        kept, logs = self.run_filter(lake, {"back"}, here=here)
        self.assertEqual(kept, set())
        self.assertIn("water", logs[0])

    def test_no_keepout_box_walks_in(self):
        kept, _ = self.run_filter(FakeWorld(), {"forward"}, here=[-194.5, 73.0, 25.5], keepout=None)
        self.assertEqual(kept, {"forward"})

    def test_every_step_reads_the_blocks_again(self):
        world = FakeWorld()
        guard = pilot.Guard(world)
        self.assertEqual(guard.filter({"forward"}, self.HERE, -90.0, 3.0), {"forward"})
        self.assertEqual(len(world.scans), 1)
        # The ground under the next cell turns to water (or is dug away) between two steps.
        world.cells[(101, 72, 100)] = block(water=True)
        with mock.patch.object(pilot, "log") as log:
            self.assertEqual(guard.filter({"forward"}, self.HERE, -90.0, 3.0), set())
        self.assertEqual(len(world.scans), 2, "the same cell was asked again")
        self.assertIn("water", log.call_args[0][0])
        world.cells = {(101, y, 100): block() for y in range(60, 73)}
        with mock.patch.object(pilot, "log"):
            self.assertEqual(guard.filter({"forward"}, self.HERE, -90.0, 3.0), set())
        self.assertEqual(len(world.scans), 3)

    def test_a_truncated_scan_is_continued_from_next_offset(self):
        world = FakeWorld()
        real = world.tool

        def paged(name, arguments=None):
            page = real(name, arguments)
            offset = arguments.get("offset", 0)
            rows = page["blocks"][offset:]
            return {**page, "blocks": rows[:3], "truncated": len(rows) > 3, "next_offset": offset + 3}

        world.tool = paged
        kept, logs = self.run_filter(world, {"forward"})
        self.assertEqual((kept, logs), ({"forward"}, []))


class RouteGuardMarch(unittest.TestCase):
    """The whole pilot behind a marching rank that walks on across a cliff: he stops at the edge."""

    def test_he_stays_behind_a_cliff_the_rank_walks_over(self):
        logs = []
        sim = march(call=0.05, floor=lambda x, z: 63 if z < 40 else None, logs=logs)
        self.assertLess(max(s[1] + 1.2 * s[0] for s in sim.samples), 40.0, "he walked off the cliff")  # his z
        self.assertTrue(any(line.startswith("edge refused forward at") for line in logs), logs[-5:])
        self.assertTrue(any(f"drop=4+" in line for line in logs))

    def test_a_slow_client_does_not_walk_past_the_ground_it_checked(self):
        # 0.4 s per MCP call: 3-4 s steps, so a held or pulsed key carries him 10+ blocks between checks, far
        # beyond a 3-block look. The cliff is 6+ blocks past where he starts to lag behind the marching rank.
        logs = []
        sim = march(call=0.4, floor=lambda x, z: 63 if z < 40 else None, logs=logs)
        self.assertLess(max(s[1] + 1.2 * s[0] for s in sim.samples), 40.0, "walked off the cliff between checks")
        self.assertTrue(any(line.startswith("edge refused forward at") for line in logs), logs[-5:])

    def test_movement_is_never_held_down_during_another_call(self):
        for call in (0.03, 0.4):
            sim = march(call=call)
            self.assertEqual(sim.held_during, [], f"call {call}: a held key walks through the next MCP call")

    def test_a_sudden_stall_after_the_step_began_does_not_carry_him_over_the_edge(self):
        # Fast calls, then from t 20 s every call takes 5 s (a stalled frame or window): a key held for the
        # step would walk 20+ blocks through the stalled calls. The cliff is a few blocks ahead of his place.
        sim = march(call=0.03, floor=lambda x, z: 63 if z < 30 else None, spike=(20.0, 5.0))
        self.assertLess(max(s[1] + 1.2 * s[0] for s in sim.samples), 30.0, "walked off the cliff in a stalled call")
        self.assertEqual(sim.held_during, [])

    def test_the_stall_walks_him_off_when_keys_are_held(self):
        # The same run with the guard off holds keys (the older behaviour) and does go over: the test above bites.
        sim = march(call=0.03, floor=lambda x, z: 63 if z < 30 else None, spike=(20.0, 5.0), argv=["--no-guard"])
        self.assertGreater(max(s[1] + 1.2 * s[0] for s in sim.samples), 30.0)

    def test_the_cliff_is_crossed_when_the_guard_is_off(self):
        sim = march(call=0.05, floor=lambda x, z: 63 if z < 40 else None, argv=["--no-guard"])
        self.assertGreater(max(s[1] + 1.2 * s[0] for s in sim.samples), 45.0)

    def test_he_does_not_enter_the_keepout_box(self):
        sim = march(call=0.05, argv=["--keepout", "-3", "3", "30", "40"])
        z = max(s[1] + 1.2 * s[0] for s in sim.samples)
        self.assertLess(z, 30.0, "his place is inside the box from z 30; he stays at its edge")


def run_probe(argv, floor=lambda x, z: 63):
    """`--probe` against the simulated client (no soldiers matter): (sim, log lines)."""
    sim = MarchingRank(0.05, floor=floor)
    logs = []
    saved = pilot.time, pilot.log
    pilot.time = types.SimpleNamespace(monotonic=lambda: sim.now, strftime=time.strftime)
    pilot.log = lambda *parts: logs.append(" ".join(str(p) for p in parts))
    try:
        pilot.probe(pilot.arguments(["runtime", "--probe", *argv]), sim)
    except Stop:
        pass
    finally:
        pilot.time, pilot.log = saved
    return sim, logs


class Probe(unittest.TestCase):
    """`--probe X Z`: the same guard and pulses on a chosen spot, for the real-client check at the ledge or the box."""

    def test_it_walks_to_the_goal_on_flat_ground(self):
        sim, logs = run_probe(["0", "12"])
        self.assertLess(math.dist(sim.me, [0.0, 12.0]), 0.7)
        self.assertTrue(logs[-1].startswith("probe done: arrived"), logs[-1])
        self.assertEqual(sim.held_during, [])

    def test_it_stops_at_a_ledge_and_logs_the_refusal(self):
        sim, logs = run_probe(["0", "50"], floor=lambda x, z: 63 if z < 20 else None)
        self.assertLess(sim.me[1], 20.0, "walked off the ledge")
        self.assertGreater(sim.me[1], 15.0, "did not come up to it")
        self.assertTrue(any(line.startswith("edge refused forward at") and "drop=4+" in line for line in logs), logs)
        self.assertTrue(any("REFUSED" in line for line in logs))
        self.assertRegex(logs[-1], r"^probe done: refused 3 steps in a row at")

    def test_it_stops_at_the_keepout_box(self):
        sim, logs = run_probe(["0", "50", "--keepout", "-3", "3", "20", "30"])
        self.assertLess(sim.me[1], 20.0)
        self.assertTrue(any(line.startswith("keepout refused forward at") for line in logs), logs)

    def test_without_the_guard_it_walks_over(self):
        sim, _ = run_probe(["0", "30", "--no-guard"], floor=lambda x, z: 63 if z < 20 else None)
        self.assertGreater(sim.me[1], 25.0)

    def test_the_step_limit_ends_it(self):
        sim, logs = run_probe(["0", "500", "--probe-steps", "3"])
        self.assertRegex(logs[-1], r"^probe done: step limit 3")


# The trial-7 stairs into the mine, walking -X from the valley at (-187.5, 73, 24.5) (sim x = world x + 188, start
# sim 0.5). Columns: the top of the solid run from the bottom of the world at z 24, read from the native checkpoint
# probe receipts/BvLiveBugs/probe-trial7.json (probe-native of world-trial7, blocks x -199..-184, z 21..35, y 0..100;
# the live probe's own MCP transcripts probe-3000/probe-edge/mcp-transcript.jsonl are empty). It reproduces the
# live log: from feet y 73 cell -190 (top 70) is a drop of 2; from feet y 72 the first refused cell is -191 (top 67,
# a drop of 4), from feet y 71 it is -192 (top 64, a drop of 6).
STAIRS = {-189: 72, -190: 70, -191: 67, -192: 64, -193: 19, -194: 13, -195: 10, -196: 7}
DEEP = 7


def stairs_floor(sim_x: float, sim_z: float):
    """The sim's floor (feet y 64 stands for the world's 73: shifted by 9) under sim cell (x, z); world x = sim x - 188."""
    cell = math.floor(sim_x - 188.0)
    ground = 72 if cell >= -189 else STAIRS.get(cell, DEEP)  # (the valley floor beyond -189 is 72-74; 72 is enough)
    return ground - 9


class StairsEdge(unittest.TestCase):
    """PovRed walking -X off the valley toward the stairs: he must stop short of the first refused cell (-191, a
    drop of 5 from y 73; sim cell -3) with the margin, not walk down onto the 2-block step beside it (the live probe did: y 72.8
    -> 71.0, stopped 0.1 blocks from the cell)."""

    EDGE = -190.0  # east edge of the refused cell -191

    def probe(self, *extra):
        sim = MarchingRank(0.05, floor=lambda x, z: stairs_floor(x, z))
        logs, ticks = [], []
        real = sim.tool

        def tool(name, arguments=None):
            if name == "minecraft_press_inputs" and "hold" not in (arguments or {}):
                ticks.append(arguments["ticks"])
            return real(name, arguments)

        sim.tool = tool
        saved = pilot.time, pilot.log
        pilot.time = types.SimpleNamespace(monotonic=lambda: sim.now, strftime=time.strftime)
        pilot.log = lambda *parts: logs.append(" ".join(str(p) for p in parts))
        sim.me = [0.5, 0.5]
        try:
            # world goal x -197 is sim x -9: the probe takes its goal in the sim's own frame
            pilot.probe(pilot.arguments(["runtime", "--probe", "-9", "0.5", *extra]), sim)
        except Stop:
            pass
        finally:
            pilot.time, pilot.log = saved
        return sim, logs, ticks

    def test_he_stops_a_margin_before_the_refused_cell_and_never_steps_down(self):
        sim, logs, ticks = self.probe()
        world_x = sim.me[0] - 188.0
        self.assertGreaterEqual(world_x, self.EDGE + 1.5 - 1e-6, f"stopped at x {world_x:.2f}, closer than 1.5 to the refused cell")
        self.assertTrue(any(line.startswith("edge refused forward at -3,64,0") and "drop=4+" in line for line in logs), logs)
        self.assertRegex(logs[-1], r"^probe done: refused 3 steps in a row")
        self.assertEqual(sim.held_during, [])

    def test_no_pulse_is_longer_than_the_ground_read_beyond_it(self):
        sim, logs, ticks = self.probe()
        speed = 4.3
        self.assertTrue(ticks)
        for t in ticks:
            self.assertLessEqual(t / 20 * speed, pilot.GUARD_REACH - pilot.GUARD_MARGIN + speed / 20 + 1e-6)

    def test_a_pulse_never_ends_nearer_the_refused_cell_than_the_margin(self):
        # Drive it from several starts: whatever the step lengths, the position after every step is >= 1.5 away.
        for start in (12.5, 7.3, 3.1, 0.5):  # sim x; world = sim - 188: -175.5 .. -187.5
            sim = MarchingRank(0.05, floor=lambda x, z: stairs_floor(x, z))
            sim.me = [start, 0.5]
            saved = pilot.time, pilot.log
            pilot.time = types.SimpleNamespace(monotonic=lambda: sim.now, strftime=time.strftime)
            pilot.log = lambda *parts: None
            try:
                pilot.probe(pilot.arguments(["runtime", "--probe", "-9", "0.5"]), sim)
            except Stop:
                pass
            finally:
                pilot.time, pilot.log = saved
            self.assertGreaterEqual(sim.me[0] - 188.0, self.EDGE + 1.5 - 1e-6, f"start {start}")


def duel(enemies, friends=None, duration=10.0, call=0.05, always_using=False, argv=(), bare=False, floor=None):
    """The rank standing still (march 0) with PovRed in his slot, enemies and extra friends at {id: (x, z, y)}: (sim, log lines)."""
    sim = MarchingRank(call, march=0.0, duration=duration, **({"floor": floor} if floor else {}))
    if bare:
        sim.soldiers = {}  # no rank around him: only the soldiers given
    for ident, (x, z, y) in {**enemies, **(friends or {})}.items():
        sim.soldiers[ident] = [x, z]
        sim.ys[ident] = y
    sim.always_using = always_using
    sim.mode = argv[argv.index("--attack-mode") + 1] if "--attack-mode" in argv else "input"
    logs = []
    saved = pilot.time, pilot.log
    pilot.time = types.SimpleNamespace(monotonic=lambda: sim.now, strftime=time.strftime)
    pilot.log = lambda *parts: logs.append(" ".join(str(p) for p in parts))
    try:
        pilot.rank(pilot.arguments(["runtime", *argv]), sim)
    except Stop:
        pass
    finally:
        pilot.time, pilot.log = saved
    return sim, logs


class Fighting(unittest.TestCase):
    """Swings go through the ordinary inputs: shield let go and nothing in use, the look on the enemy, the client's
    crosshair target an enemy within reach, then the attack input; every skip says why."""

    def test_a_swing_after_the_shield_strikes_the_enemy(self):
        sim, logs = duel({1000: (0.0, 2.0, 64.0)})
        self.assertGreaterEqual(sim.hits.count(1000), 5, logs[-6:])
        self.assertTrue(any(line.startswith("attack sent (crosshair enemy 1000") for line in logs))
        self.assertEqual([h for h in sim.attacks if h is None], [], "no attack input is pressed on an empty crosshair")

    def test_no_attack_is_pressed_while_the_client_still_reports_the_item_in_use(self):
        sim, logs = duel({1000: (0.0, 2.0, 64.0)}, always_using=True)
        self.assertEqual(sim.attacks, [], "an attack in use is dropped silently by the client: none is pressed")
        self.assertTrue(any("attack skipped (item still in use after" in line for line in logs), logs[-4:])

    def test_a_friend_under_the_crosshair_is_never_struck(self):
        # a friend fills the whole line to the enemy: no aim point passes him
        sim, logs = duel({1000: (0.0, 2.3, 64.0)}, friends={500: (0.0, 1.2, 64.0)}, duration=30.0)
        self.assertNotIn(500, sim.hits, "the friend is never struck")
        self.assertTrue(any(line.startswith("attack skipped (crosshair friend 500") for line in logs), logs[-4:])
        self.assertLessEqual(sum(1 for line in logs if line.startswith("attack skipped (crosshair friend")), 11,
                        "the target is left alone for a while after every point failed")

    def test_an_entity_of_unknown_side_under_the_crosshair_is_never_struck(self):
        with mock.patch.object(pilot, "side_of", lambda client, entity: None if entity["entity_id"] == 500 else ("blue" if entity["entity_id"] >= 1000 else "red")):
            sim, logs = duel({1000: (0.0, 2.3, 64.0)}, friends={500: (0.0, 1.2, 64.0)}, duration=8.0)
        self.assertNotIn(500, sim.hits, "an entity whose side is unknown is never struck")
        self.assertTrue(any(line.startswith("attack skipped (crosshair unknown-side 500") for line in logs), logs[-4:])

    def test_the_aim_moves_past_a_friends_shoulder(self):
        sim, logs = duel({1000: (0.0, 2.0, 64.0)}, friends={500: (0.25, 1.0, 64.0)})
        self.assertIn(1000, sim.hits, logs[-6:])
        self.assertNotIn(500, sim.hits)

    def test_an_enemy_in_the_way_is_struck_instead(self):
        sim, logs = duel({1000: (0.0, 2.0, 64.0), 1001: (0.0, 1.2, 64.0)})
        self.assertIn(1001, sim.hits)
        self.assertTrue(any("attack sent (crosshair enemy 1001 instead of 1000" in line or "attack sent (crosshair enemy 1001" in line for line in logs), logs[-4:])

    def test_an_enemy_far_below_is_out_of_reach_from_the_eye_though_close_on_the_ground(self):
        sim, logs = duel({1000: (0.0, 2.5, 60.5)})
        self.assertEqual(sim.attacks, [], "no attack beyond the eye reach")
        self.assertEqual([line for line in logs if line.startswith("attack")][1:], [], "no aiming at him either (only the interval line)")
        sim, logs = duel({1000: (0.0, 2.5, 63.0)})
        self.assertTrue(sim.hits, "the same distance at his height is struck")

    def test_every_attack_press_is_framed_by_a_diagnostic_before_and_after(self):
        sim, logs = duel({1000: (0.0, 2.0, 64.0)})
        sent = [i for i, line in enumerate(logs) if line.startswith("attack sent")]
        self.assertTrue(sent)
        for i in sent:
            before, after = logs[i - 2], logs[i - 1]
            self.assertTrue(before.startswith("attack diag before: using=null held=[] game_mode=\"absent\" target={"), before)
            self.assertIn('"entity_id":1000', before)
            self.assertIn("main_hand=unreadable: no held item data", before)
            self.assertEqual(after, 'attack diag after: {}')
        self.assertEqual(len(sim.attacks), len(sent))

    def test_the_diagnostic_line_format_is_never_cut(self):
        line = pilot.diag_before({"using": {"hand": 1, "item_id": 7, "remaining": 40}, "inputs": {"held": ["use", "forward"]},
                                  "player": {"game_mode": 3}, "target": {"type": "none"}}, {"components": {"x": 1}})
        self.assertEqual(line, 'attack diag before: using={"hand":1,"item_id":7,"remaining":40} held=["use", "forward"] '
                               'game_mode=3 target={"type":"none"} main_hand={"components":{"x":1}}')
        absent = pilot.diag_before({"using": None, "inputs": {"held": []}, "target": {"type": "none"}}, "unreadable: x")
        self.assertIn(" game_mode=\"absent\" ", absent)
        self.assertIn(" game_mode=1 ", pilot.diag_before({"game_mode": 1}, {}))
        # nothing is cut: a 1000-character item description and a 1000-character answer stay whole
        big = {"components": {"k": "v" * 1000}}
        self.assertIn("v" * 1000, pilot.diag_before({"using": None}, big))
        self.assertGreater(len(pilot.diag_before({"using": None}, big)), 400)

    def test_a_long_answer_and_a_long_error_are_logged_whole_in_both_modes(self):
        answer = {"dispatched": True, "detail": "x" * 1000}
        error = "ordinary entity attack was not dispatched: " + "y" * 1000
        for mode in ("input", "once"):
            sim = MarchingRank(0.05, march=0.0, duration=60.0)
            sim.soldiers = {1000: [0.0, 2.0]}
            sim.mode = mode
            real = sim.tool
            def tool(name, arguments=None, real=real, mode=mode):
                if name == "minecraft_attack_entity_once":
                    real(name, arguments)
                    return answer
                if name == "minecraft_press_inputs" and arguments and "attack" in arguments.get("keys", []):
                    real(name, arguments)
                    return answer
                return real(name, arguments)
            sim.tool = tool
            logs = []
            saved = pilot.log
            pilot.log = lambda *parts: logs.append(" ".join(str(p) for p in parts))
            try:
                target = next(e for e in real("minecraft_list_entities", {"radius": 9, "limit": 9})["entities"] if e["entity_id"] == 1000)
                swing = pilot.attack_once if mode == "once" else pilot.attack_input
                swing(sim, pilot.Pilot(sim), target, pilot.arguments(["runtime"]), {1000: "blue"}, {}, True)
                self.assertIn("attack diag after: " + pilot.compact(answer), logs, mode)
                if mode == "once":
                    def refuse(name, arguments=None):
                        if name == "minecraft_attack_entity_once":
                            raise RuntimeError(error)
                        return real(name, arguments)
                    sim.tool = refuse
                    swing(sim, pilot.Pilot(sim), target, pilot.arguments(["runtime"]), {1000: "blue"}, {}, True)
                    self.assertIn("attack diag after: error " + error, logs)
            finally:
                pilot.log = saved

    def test_once_mode_strikes_with_the_tool_and_frames_every_call_with_the_diagnostic(self):
        sim, logs = duel({1000: (0.0, 2.0, 64.0)}, argv=("--attack-mode", "once"))
        self.assertGreaterEqual(sim.hits.count(1000), 5, logs[-6:])
        self.assertEqual(sim.attacks, [], "once mode never presses the attack input")
        self.assertEqual(len(sim.once_calls), len([line for line in logs if line.startswith("attack dispatched")]))
        for i, line in enumerate(logs):
            if line.startswith("attack dispatched"):
                self.assertTrue(logs[i - 2].startswith("attack diag before: using=null held=[] game_mode=\"absent\" target="), logs[i - 2])
                self.assertEqual(logs[i - 1], 'attack diag after: {"confirmed":false,"dispatched":true}')

    def test_once_mode_logs_the_error_text_after_a_refused_call(self):
        sim, logs = duel({1000: (0.0, 2.0, 64.0)}, always_using=True, argv=("--attack-mode", "once"))
        self.assertEqual(sim.once_calls, [], "no call while the client reports an item in use")
        self.assertTrue(any("attack not dispatched (item still in use" in line for line in logs))
        sim, logs = duel({1000: (0.0, 2.3, 64.0)}, friends={500: (0.0, 1.2, 64.0)}, duration=6.0, argv=("--attack-mode", "once"))
        self.assertNotIn(500, sim.hits, "a friend in the way is not struck in once mode either")
        self.assertTrue(any(line.startswith("attack diag after: error target entity is occluded by entity 500") for line in logs), logs[-6:])
        self.assertTrue(any(line.startswith("attack occluded by friend 500") for line in logs))

    def test_once_mode_strikes_an_enemy_in_the_way(self):
        sim = MarchingRank(0.05, march=0.0, duration=60.0)
        sim.soldiers = {1000: [0.0, 2.0], 1001: [0.0, 1.2]}
        sim.mode = "once"
        logs = []
        saved = pilot.log
        pilot.log = lambda *parts: logs.append(" ".join(str(p) for p in parts))
        try:
            target = next(e for e in sim.tool("minecraft_list_entities", {"radius": 9, "limit": 9})["entities"] if e["entity_id"] == 1000)
            pilot.attack_once(sim, pilot.Pilot(sim), target, pilot.arguments(["runtime", "--attack-mode", "once"]),
                              {1000: "blue", 1001: "blue"}, {}, True)
        finally:
            pilot.log = saved
        self.assertEqual(sim.hits, [1001])
        self.assertTrue(any(line.startswith("attack occluded by enemy 1001") for line in logs), logs)

    def test_input_mode_is_the_default_and_never_calls_the_tool(self):
        self.assertEqual(pilot.arguments(["runtime"]).attack_mode, "input")
        self.assertEqual(pilot.arguments(["runtime", "--attack-mode", "once"]).attack_mode, "once")
        sim, logs = duel({1000: (0.0, 2.0, 64.0)})  # the sim raises if the tool is called in input mode
        self.assertEqual(sim.once_calls, [])
        self.assertTrue(sim.hits)

    def test_the_look_is_in_whole_degrees_and_the_old_swing_tool_is_not_used(self):
        sim, logs = duel({1000: (0.7, 2.1, 64.0)})  # the sim raises on attack_entity_once and on fractional looks
        self.assertTrue(sim.hits)


SWORD = {"components": {"minecraft:attribute_modifiers": [{"type": "minecraft:attack_damage", "amount": 5.0},
                                                          {"type": "minecraft:attack_speed", "amount": -2.4}]}}
SPEAR = {"components": {"minecraft:attribute_modifiers": [{"type": "minecraft:attack_damage", "amount": 4.0}],
                        "minecraft:piercing_weapon": {}}}
IRON_SPEAR = {"components": {"minecraft:piercing_weapon": {}, "minecraft:minimum_attack_charge": 1.0,
                              "minecraft:attack_range": {"min_reach": 2.0, "max_reach": 4.5, "hitbox_margin": 0.125},
                              "minecraft:attribute_modifiers": [{"type": "minecraft:attack_damage", "amount": 4.0},
                                                                {"type": "minecraft:attack_speed", "amount": -2.95}]}}


def pilot_interval(sim, args):
    result = []
    logged(sim, lambda: result.append(pilot.weapon_profile(sim, args)))
    return result[0]


def logged(sim, call):
    logs = []
    saved = pilot.log
    pilot.log = lambda *parts: logs.append(" ".join(str(p) for p in parts))
    try:
        call()
    finally:
        pilot.log = saved
    return logs


class Fight(unittest.TestCase):
    """The pace of a fight: short steps, a walk up to the enemy, swings by attack strength, the place kept, the
    right weapon in hand."""

    CROWD = {600 + i: (8.0 + 0.3 * i, 3.0 + 0.4 * i, 64.0) for i in range(30)}  # friends beyond the fight, not yet read

    def test_in_a_fight_the_crowd_beyond_it_is_not_read_and_no_step_waits_two_ticks(self):
        with mock.patch.object(pilot, "USE_TICKS", 1):  # the shield release has its own wait
            sim, logs = duel({1000: (0.0, 4.5, 64.0)}, friends=self.CROWD, duration=20.0)
        self.assertTrue(sim.hits)
        self.assertEqual([r for r in sim.entity_reads if r in self.CROWD], [], "soldiers beyond the fight are read after it")
        self.assertEqual([w for w in sim.waits if w[1] > 1 and w[0] > 8.0], [], "a fight step does not wait two ticks")
        steps = [b - a for a, b in zip(sim.list_times, sim.list_times[1:])]
        self.assertLessEqual(sum(steps) / len(steps), 0.5, "mean step in a fight, client seconds")

    def test_he_walks_up_to_the_nearest_enemy(self):
        sim, logs = duel({1000: (0.0, 5.0, 64.0)}, duration=20.0)
        self.assertLessEqual(math.dist(sim.me, [0.0, 5.0]), 2.8, "he stopped far from the enemy")
        self.assertGreaterEqual(math.dist(sim.me, [0.0, 5.0]), 2.0, "he walked into the enemy")
        self.assertGreaterEqual(sim.hits.count(1000), 5, logs[-5:])

    def test_a_swing_waits_for_the_attack_strength_not_for_a_fixed_interval(self):
        sim = MarchingRank(0.05, march=0.0, duration=30.0)
        sim.charge_full = 60  # 3 s to full strength, far slower than the 0.65 s fallback interval and the pilot's step
        sim.soldiers[1000] = [0.0, 2.0]
        logs = logged(sim, lambda: self.run_rank(sim))
        self.assertGreaterEqual(len(sim.press_strengths), 5, logs[-4:])
        self.assertGreaterEqual(min(sim.press_strengths), 0.9, "a swing at low charge")

    def test_without_the_hud_in_the_observation_the_weapon_interval_spaces_the_swings(self):
        sim = MarchingRank(0.05, march=0.0, duration=20.0)
        sim.hud, sim.charge_full = False, 1
        sim.soldiers[1000] = [0.0, 2.0]
        logged(sim, lambda: self.run_rank(sim))
        gaps = [b - a for a, b in zip(sim.press_times, sim.press_times[1:])]
        self.assertTrue(gaps)
        self.assertGreaterEqual(min(gaps), 0.6, "swings closer than the 0.65 s interval")

    def test_the_attack_strength_is_read_from_the_hud(self):
        nodes = lambda *bar: {"ui_nodes": [{"id": "hud/crosshair", "bounds": [0, 0, 30.0, 30.0]},
                                           *([{"id": "hud/attack", "bounds": [0, 0, bar[0], 4.0]}] if bar else [])]}
        self.assertEqual(pilot.attack_strength(nodes()), 1.0)
        self.assertAlmostEqual(pilot.attack_strength(nodes(15.0)), 0.5)
        self.assertIsNone(pilot.attack_strength({}))
        self.assertIsNone(pilot.attack_strength({"ui_nodes": [{"id": "hud/hotbar", "bounds": [0, 0, 1, 1]}]}))
        self.assertTrue(pilot.attack_ready(nodes(), 0.0, 0.0, 9.0))
        self.assertFalse(pilot.attack_ready(nodes(24.0), 100.0, 0.0, 0.0))
        self.assertTrue(pilot.attack_ready(nodes(27.5), 0.0, 0.0, 9.0))
        self.assertFalse(pilot.attack_ready(nodes(29.0), 0.0, 0.0, 9.0, 1.0), "a spear needs full charge, 97% is rejected by the server")
        self.assertTrue(pilot.attack_ready(nodes(), 0.0, 0.0, 9.0, 1.0))

    def test_the_place_is_not_handed_away_and_he_does_not_leave_while_an_enemy_is_near(self):
        def fight():
            sim = MarchingRank(0.05, march=0.0, duration=30.0)
            sim.spawn = [(6.0, 1000, [0.0, 5.0])]
            sim.vanish = (9.0, set(range(100, 124)))  # his rank's soldiers leave the page
            logs = logged(sim, lambda: self.run_rank(sim))
            return sim, logs
        sim, logs = fight()
        self.assertEqual([line for line in logs if "handed" in line], [], logs[-6:])
        self.assertLessEqual(math.dist(sim.me, [0.0, 5.0]), 3.0, "he left the fight")
        self.assertTrue(sim.hits)

    def spear_run(self, enemy_z, duration=30.0, hud=True, charge_full=21):
        sim = MarchingRank(0.05, march=0.0, duration=duration)
        sim.hotbar, sim.selected = {0: IRON_SPEAR}, 0
        sim.soldiers[1000] = [0.0, enemy_z]
        sim.hud, sim.charge_full = hud, charge_full
        sim.reach_limit = 4.5  # the server side takes the spear's reach
        logs = logged(sim, lambda: self.run_rank(sim))
        return sim, logs

    def test_the_weapon_profile_of_a_spear_and_of_a_sword(self):
        sim = MarchingRank(0.05)
        sim.hotbar, sim.selected = {0: IRON_SPEAR}, 0
        args = pilot.arguments(["runtime"])
        interval = pilot_interval(sim, args)
        self.assertEqual((args.charge, args.min_reach, args.reach, args.engage), (1.0, 2.0, 4.5, 3.0))
        self.assertAlmostEqual(interval, (math.ceil(20 / 1.05) + 1) / 20)  # 22 ticks
        sim.hotbar = {0: SWORD}
        args = pilot.arguments(["runtime"])
        interval = pilot_interval(sim, args)
        self.assertEqual((args.charge, args.min_reach, args.reach, args.engage), (pilot.STRENGTH_READY, 0.0, 3.0, pilot.ENGAGE))
        self.assertAlmostEqual(interval, (math.ceil(20 / 1.6) + 1) / 20)  # 14 ticks
        sim.hotbar = {}
        args = pilot.arguments(["runtime"])
        self.assertEqual(pilot_interval(sim, args), 0.65, "unreadable weapon: the --attack-interval fallback")

    def test_a_spear_waits_for_full_charge_not_for_ninety_percent(self):
        sim, logs = self.spear_run(3.2, duration=60.0, charge_full=60)
        self.assertGreaterEqual(len(sim.press_strengths), 3, logs[-5:])
        self.assertEqual(min(sim.press_strengths), 1.0, "a swing below the spear's minimum_attack_charge")

    def test_a_spear_is_not_swung_inside_its_minimum_reach_and_he_backs_off(self):
        sim, logs = self.spear_run(1.9, duration=15.0)
        near = [h for h in sim.press_distances if h < 2.0]
        self.assertEqual(near, [], "a swing at an enemy inside the spear's 2.0 minimum reach")
        self.assertGreaterEqual(math.dist(sim.me, [0.0, 1.9]), 2.4, "he did not back off")

    def test_a_spear_engages_at_three_blocks_and_swings_out_to_four_and_a_half(self):
        sim, logs = self.spear_run(5.5)
        self.assertLessEqual(math.dist(sim.me, [0.0, 5.5]), 3.4)
        self.assertGreaterEqual(math.dist(sim.me, [0.0, 5.5]), 2.6)
        self.assertGreaterEqual(len(sim.hits), 3, logs[-5:])
        self.assertGreater(max(sim.press_distances), 2.5, "he swung only at sword range")

    def test_without_the_hud_a_spear_swings_by_its_twenty_two_tick_interval(self):
        sim, logs = self.spear_run(3.2, hud=False, charge_full=1)
        gaps = [b - a for a, b in zip(sim.press_times, sim.press_times[1:])]
        self.assertTrue(gaps)
        self.assertGreaterEqual(min(gaps), 1.1 - 0.05, "swings closer than 22 ticks")

    def jammed(self, enemy, friend, argv=(), floor=None, duration=25.0):
        return duel({1000: enemy}, friends={500: friend}, duration=duration, argv=argv, bare=True, floor=floor)

    def test_a_friend_in_the_line_makes_him_step_aside_and_strike(self):
        sim, logs = self.jammed((0.0, 2.3, 64.0), (0.0, 1.2, 64.0))
        self.assertIn(1000, sim.hits, logs[-6:])
        self.assertNotIn(500, sim.hits, "the friend is never struck")
        self.assertTrue(any("attack sent (crosshair enemy 1000" in line for line in logs))

    def test_he_does_not_stand_without_input_behind_his_own_man(self):
        sim, logs = self.jammed((0.0, 2.3, 64.0), (0.0, 1.2, 64.0), duration=12.0)
        self.assertGreater(abs(sim.me[0]), 0.3, "he did not leave the line behind the friend")

    def test_the_other_side_is_tried_when_the_guard_refuses_one(self):
        for void in (lambda x, z: None if x > 0.3 else 63, lambda x, z: None if x < -0.3 else 63):
            sim, logs = self.jammed((0.0, 2.3, 64.0), (0.0, 1.2, 64.0), floor=void)
            self.assertIn(1000, sim.hits, logs[-6:])

    def test_a_spear_user_too_close_backs_off_and_then_steps_around_the_friend(self):
        sim = MarchingRank(0.05, march=0.0, duration=30.0)
        sim.soldiers = {500: [0.0, 0.9], 1000: [0.0, 1.6]}
        sim.hotbar, sim.selected = {0: IRON_SPEAR}, 0
        sim.hud, sim.charge_full, sim.reach_limit = True, 21, 4.5
        logs = logged(sim, lambda: self.run_rank(sim))
        self.assertIn(1000, sim.hits, logs[-6:])
        self.assertEqual([d for d in sim.press_distances if d < 2.0], [], "a spear swing inside its minimum reach")
        self.assertNotIn(500, sim.hits)

    def run_rank(self, sim):
        saved = pilot.time
        pilot.time = types.SimpleNamespace(monotonic=lambda: sim.now, strftime=time.strftime)
        try:
            pilot.rank(pilot.arguments(["runtime"]), sim)
        except Stop:
            pass
        finally:
            pilot.time = saved

    def test_auto_takes_the_melee_weapon_without_piercing(self):
        sim = MarchingRank(0.05)
        sim.hotbar, sim.selected = {0: SPEAR, 1: SWORD}, 0
        logs = logged(sim, lambda: pilot.choose_melee(sim, "auto"))
        self.assertEqual(sim.selected, 1)
        self.assertTrue(any(line.startswith("hotbar 0 ") and "piercing_weapon" in line for line in logs), logs)
        self.assertTrue(any(line.startswith("hotbar 1 ") for line in logs))
        self.assertIn("melee slot 1 selected (melee weapon without piercing_weapon)", logs)

    def test_the_hotbar_is_inventory_slots_0_to_8_not_the_armour_slots(self):
        sim = MarchingRank(0.05)
        # 37 is the cosmetic-leggings carrier and looks like a sword here; hotbar slot 1 is a spear, slot 3 a sword.
        sim.hotbar, sim.other, sim.selected = {0: SPEAR, 1: SPEAR, 3: SWORD}, {36: {"components": {}}, 37: SWORD, 40: SWORD}, 0
        reads = []
        real = sim.tool
        sim.tool = lambda name, arguments=None: (reads.append(arguments["slot"]) if arguments and arguments.get("source") == "player" else None, real(name, arguments))[1]
        logs = logged(sim, lambda: pilot.choose_melee(sim, "auto"))
        self.assertEqual(reads, list(range(9)), "only inventory slots 0-8 are read")
        self.assertEqual(sim.selected, 3, logs)

    def test_auto_keeps_a_selected_non_piercing_weapon(self):
        sim = MarchingRank(0.05)
        sim.hotbar, sim.selected = {0: SWORD, 3: SWORD}, 0
        logs = logged(sim, lambda: pilot.choose_melee(sim, "auto"))
        self.assertEqual(sim.selected, 0)
        self.assertIn("selected item is a melee weapon without piercing_weapon; kept", logs)

    def test_auto_with_only_a_spear_says_so_and_leaves_the_selection(self):
        sim = MarchingRank(0.05)
        sim.hotbar, sim.selected = {0: SPEAR, 2: {"components": {}}}, 2
        logs = logged(sim, lambda: pilot.choose_melee(sim, "auto"))
        self.assertEqual(sim.selected, 2)
        self.assertIn("no non-piercing melee weapon in hotbar", logs)

    def test_a_number_selects_that_slot_and_the_rank_chooses_at_start(self):
        sim = MarchingRank(0.05)
        logged(sim, lambda: pilot.choose_melee(sim, "4"))
        self.assertEqual(sim.selected, 4)
        sim, logs = duel({}, duration=2.0)
        self.assertEqual(sim.selected, 0)
        sim = MarchingRank(0.05, march=0.0, duration=2.0)
        sim.hotbar, sim.selected = {0: SPEAR, 5: SWORD}, 0
        logged(sim, lambda: self.run_rank(sim))
        self.assertEqual(sim.selected, 5, "the rank loop chooses the weapon at join")


if __name__ == "__main__":
    unittest.main()
