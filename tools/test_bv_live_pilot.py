"""PovRed's live pilot (`tools/bv_live_pilot.py`) against a simulated client: a red block
marching +Z, PovRed placed in its front rank, every MCP call costing client time. He must keep
his place: never walk far past it (held keys on a slow step) and never fall far behind it (a
place measured late, or eased per step instead of per second)."""
import math
import time
import types
import unittest

from tools import bv_live_pilot as pilot

TICK = 1 / 20


class Stop(Exception):
    """End of the simulated run (not a RuntimeError, so the pilot does not swallow it)."""


class MarchingRank:
    """The client side of the pilot's MCP calls. Soldiers: 3 ranks x 13 files, 1 block apart in a
    rank, 1.5 between ranks; PovRed's slot is rank 0, file 0 (left empty). `call` seconds of
    client time pass on every MCP call, as with a slow frame rate."""

    def __init__(self, call: float, march: float = 1.2, duration: float = 60.0):
        self.now, self.call, self.march, self.duration = 0.0, call, march, duration
        self.me, self.yaw = [0.0, 0.0], 0.0
        self.held: set[str] = set()
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
            for position in self.soldiers.values():
                position[1] += self.march * TICK
            self.now += TICK
            slot = [0.0, self.march * self.now]
            self.samples.append((self.now, self.me[1] - slot[1], math.dist(self.me, slot)))
        if self.now >= self.duration:
            raise Stop

    def entity(self, ident, position):
        dx, dz = position[0] - self.me[0], position[1] - self.me[1]
        return {"entity_id": ident, "entity_uuid": f"uuid-{ident}", "entity_type": "minecraft:husk",
                "x": position[0], "y": 64.0, "z": position[1], "distance": math.hypot(dx, dz),
                "velocity": {"x": 0.0, "y": 0.0, "z": self.march * TICK}, "health": 20.0}

    def tool(self, name, arguments=None):
        arguments = arguments or {}
        self.advance(self.call)
        if name == "minecraft_wait_for_play":
            return {}
        if name == "minecraft_observe":
            return {"state_version": 1, "player": {"x": self.me[0], "y": 64.0, "z": self.me[1], "yaw": self.yaw,
                                                  "pitch": 0.0, "health": 20.0, "horizontal_collision": False}}
        if name == "minecraft_list_entities":
            listed = sorted((self.entity(i, p) for i, p in self.soldiers.items()), key=lambda e: e["distance"])
            return {"entities": [e for e in listed if e["distance"] <= arguments["radius"]][: arguments["limit"]]}
        if name == "minecraft_read_item_components":
            if arguments["source"] == "selected":
                raise RuntimeError("no held item data")
            return {"components": {"minecraft:item_model": "krasnokamen_battle:red_shield"}}
        if name == "minecraft_look":
            self.yaw = float(arguments["yaw_deg"])
            return {}
        if name == "minecraft_press_inputs":
            keys = set(arguments["keys"])
            if "hold" in arguments:
                self.held = self.held | keys if arguments["hold"] else self.held - keys
            else:
                self.advance(arguments["ticks"] * TICK, self.held | keys)
            return {}
        if name == "minecraft_wait_ticks":
            self.advance(arguments["ticks"] * TICK)
            return {}
        raise AssertionError(f"unexpected tool {name}")


def march(call: float) -> MarchingRank:
    sim = MarchingRank(call)
    saved = pilot.time, pilot.log
    pilot.time = types.SimpleNamespace(monotonic=lambda: sim.now, strftime=time.strftime)
    pilot.log = lambda *parts: None
    try:
        pilot.rank(pilot.arguments(["runtime"]), sim)
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
        self.assertLess(max(s[2] for s in self.settled(sim)), 1.5)


if __name__ == "__main__":
    unittest.main()
