"""Planner rules: camera height per column, line of sight over hills and trees, the
standoff / terrain / framing checks every pose of a shot must pass, and the shot order
(no backtrack in battle time unless the cut is a sharp angle change)."""
import contextlib
import io
import json
import math
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest import mock

from tools import battle_shots
from tools.battle_shots import (HIGH, MIN_SHOT, STANDOFF, Shot, Terrain, Timeline, Vet, angle_change, check, crowding,
                                dolly_shots, keyed, off_centre, order_shots, place, pose, terrain_view, view_at)


def terrain(ground=70, hill=None, tree=None):
    """One chunk (0,0) of flat ground; `hill` raises x>=8 to that height, `tree` puts a canopy column."""
    surface, base = [], []
    for index in range(256):
        x, z = index & 15, index >> 4
        g = hill if hill is not None and x >= 8 else ground
        base.append(g)
        surface.append(tree[1] if tree is not None and (x, z) == tree[0] else g)
    return Terrain({(0, 0): (surface, base)})


def field(ground=lambda x, z: 70, surface=None, chunks=range(-3, 3)):
    """Chunks over x, z in [-48, 48): ground(x, z) per column, surface defaults to the ground."""
    out = {}
    for cx in chunks:
        for cz in chunks:
            g = [ground(cx * 16 + (i & 15), cz * 16 + (i >> 4)) for i in range(256)]
            s = g if surface is None else [max(v, surface(cx * 16 + (i & 15), cz * 16 + (i >> 4))) for i, v in enumerate(g)]
            out[(cx, cz)] = (s, g)
    return Terrain(out)


def cluster(cx, cz, half_length=3.0, half_depth=3.0, y=70.0):
    """A contact cluster around (cx, cz), contact line along x."""
    return {"centroid": [cx, y, cz], "min": [cx - half_length, cz - half_depth], "max": [cx + half_length, cz + half_depth],
            "line": [1.0, 0.0], "normal": [0.0, 1.0], "length": 2 * half_length, "depth": 2 * half_depth,
            "count": 100, "swings": 20, "ids": {}}


def sample(t, contacts=(), cavalry=()):
    return {"t": t, "contacts": list(contacts), "cavalry": list(cavalry)}


class TerrainTests(unittest.TestCase):
    def test_camera_height_follows_its_own_column(self):
        t = terrain(hill=80)
        self.assertEqual(place(t, [2.5, 0, 4.5], 2.4), [2.5, 72.4, 4.5])
        self.assertEqual(place(t, [10.5, 0, 4.5], 2.4), [10.5, 82.4, 4.5])
        self.assertIsNone(place(t, [20.5, 0, 4.5], 2.4), "outside the recorded terrain")

    def test_tree_canopy_over_the_lens_is_rejected(self):
        t = terrain(tree=((3, 3), 78))
        self.assertIsNone(place(t, [3.5, 0, 3.5], 2.4))
        self.assertIsNotNone(place(t, [4.5, 0, 3.5], 2.4))

    def test_hill_between_camera_and_fighters_blocks_the_shot(self):
        flat = terrain()
        hill = terrain(hill=80)
        eye, target = [1.5, 72.4, 4.5], [15.5, 71.2, 4.5]
        self.assertTrue(flat.clear(eye, target))
        self.assertFalse(hill.clear(eye, target))
        # Looking down from above the hill top is clear again.
        self.assertTrue(hill.clear([1.5, 95.0, 4.5], [15.5, 81.2, 4.5]))

    def test_grass_tufts_do_not_block_a_low_shot(self):
        t = terrain(tree=((8, 4), 71))  # one block of grass over ground 70
        self.assertTrue(t.clear([1.5, 72.4, 4.5], [15.5, 70.3, 4.5]))

    def test_tree_trunk_in_the_line_blocks_and_unknown_columns_fail(self):
        t = terrain(tree=((8, 4), 78))
        self.assertFalse(t.clear([1.5, 72.4, 4.5], [15.5, 71.2, 4.5]))
        self.assertTrue(t.clear([1.5, 72.4, 6.5], [15.5, 71.2, 6.5]))
        self.assertIsNone(t.clear([1.5, 72.4, 6.5], [30.5, 71.2, 6.5]))


class PoseTests(unittest.TestCase):
    def test_pose_is_the_clients_catmull_rom_with_smoothstep(self):
        keys = keyed([0, 1, 2, 3], [[i, 70.0, 0.0] for i in range(4)], [[i, 70.0, 10.0] for i in range(4)], [60, 70, 80, 90])
        self.assertEqual(pose(keys, 1.0)[0], [1.0, 70.0, 0.0])
        position, look, fov = pose(keys, 1.5)  # camera.rs spline test: linear interior, fov 75
        self.assertEqual((position, look, fov), ([1.5, 70.0, 0.0], [1.5, 70.0, 10.0], 75.0))
        self.assertAlmostEqual(pose(keys, 1.25)[0][0], 1 + 0.25 * 0.25 * (3 - 2 * 0.25))


class StandoffTests(unittest.TestCase):
    def test_lens_beside_the_fighters_is_rejected_and_three_blocks_out_passes(self):
        t, s = field(), sample(0, [cluster(0, 0)])
        look = [0.0, 70.5, 0.0]
        self.assertIn("within", crowding(t, [0.0, 72.8, 5.0], look, s))
        self.assertIsNone(crowding(t, [0.0, 72.8, 7.5], look, s))

    def test_lens_three_blocks_above_the_helmets_passes(self):
        t, s = field(), sample(0, [cluster(0, 0)])
        self.assertIsNone(crowding(t, [0.0, 70 + 2.2 + STANDOFF + 0.1, 0.0], [0.0, 70.5, 8.0], s))
        self.assertIsNotNone(crowding(t, [0.0, 70 + 2.2 + STANDOFF - 0.5, 0.0], [0.0, 70.5, 8.0], s))

    def test_bodies_across_the_near_view_are_rejected(self):
        t, s = field(), sample(0, [cluster(0, -1, half_length=2.0, half_depth=2.0)])
        far = [0.0, 70.5, 20.0]
        self.assertIn("view", crowding(t, [0.0, 71.8, -7.0], far, s))  # low lens: the first rank fills the frame
        self.assertIsNone(crowding(t, [0.0, 76.0, -7.0], far, s))  # the same view over their helmets

    def test_cavalry_box_counts_as_bodies(self):
        t = field()
        group = {"centroid": [0, 70, 0], "min": [-5, 70, -5], "max": [5, 70, 5], "count": 40, "side": "red"}
        self.assertIsNotNone(crowding(t, [0.0, 72.8, 6.5], [0.0, 71.0, 0.0], sample(0, cavalry=[group])))
        self.assertIsNone(crowding(t, [0.0, 72.8, 10.5], [0.0, 71.0, 0.0], sample(0, cavalry=[group])))

    def test_inside_an_army_the_lens_rides_above_every_possible_helmet(self):
        t = field()
        army = {"centroid": [0, 70, 0], "min": [-40, 70, -40], "max": [40, 72, 40]}
        s = {"t": 0, "contacts": [], "cavalry": [], "armies": {"red": army}}
        self.assertIn("within", crowding(t, [0.0, 72.8, 0.0], [0.0, 70.5, 10.0], s), "rear ranks are not in the timeline")
        self.assertIsNone(crowding(t, [0.0, 70 + HIGH, 0.0], [0.0, 70.5, 10.0], s))
        self.assertIsNone(crowding(t, [46.0, 72.8, 0.0], [40.0, 70.5, 0.0], s), "outside the army a low lens is fine")

    def test_dolly_moves_its_camera_out_of_a_deep_contact_line(self):
        t = field()
        deep = cluster(0, 0, half_length=10.0, half_depth=5.0)
        tl = Timeline([sample(i * 0.5, [deep]) for i in range(21)])
        vet = Vet(tl, t)
        shots = dolly_shots(tl, t, vet)
        self.assertTrue(shots)
        self.assertGreater(vet.rejected["fighters within 3 blocks of the lens"], 0, "the 5-7 block placements are inside the line")
        for shot in shots:
            for k in range(int((shot.end - shot.start) / 0.25) + 1):
                eye, look, _ = pose(shot.keys, shot.start + 0.25 * k)
                self.assertIsNone(crowding(t, eye, look, tl.at(shot.start + 0.25 * k)))
                self.assertGreaterEqual(abs(eye[2]), 5.0 + 1.0 + 2.4, shot.why)


class TerrainViewTests(unittest.TestCase):
    def test_hill_crossing_the_view_between_keys_is_caught(self):
        pillar = field(surface=lambda x, z: 80 if (x, z) == (0, -3) else 0)
        keys = keyed([0.0, 4.0], [[-10.5, 72.8, -10.5], [10.5, 72.8, -10.5]], [[-10.5, 70.7, 2.5], [10.5, 70.7, 2.5]], [70.0, 70.0])
        self.assertTrue(all(pillar.clear(p, l) for _, p, l, _ in keys), "both keys see their target")
        tl = Timeline([sample(i * 0.5) for i in range(9)])
        self.assertIsNotNone(Vet(tl, pillar).why_not(keys))
        self.assertIsNone(Vet(tl, field()).why_not(keys))

    def test_bank_beside_the_lens_is_rejected(self):
        t = terrain(hill=74)
        self.assertIn("rises", terrain_view(t, [6.5, 72.8, 4.5], [1.5, 70.5, 4.5], 70.0))
        self.assertIsNone(terrain_view(terrain(), [6.5, 72.8, 4.5], [1.5, 70.5, 4.5], 70.0))

    def test_looking_up_at_fighters_is_rejected_but_volleys_may(self):
        t = terrain()
        eye, high = [2.5, 72.8, 4.5], [12.5, 76.0, 4.5]
        self.assertIn("up", terrain_view(t, eye, high, 70.0))
        self.assertIsNone(terrain_view(t, eye, high, 70.0, looks_up=True))

    def test_low_on_a_slope_the_ground_fills_the_frame(self):
        slope = field(ground=lambda x, z: 70 if x < 4 else math.floor(70 - 0.6 * (x - 4)))
        target = [30.5, 56.5, 8.5]
        self.assertTrue(slope.clear([0.5, 72.8, 8.5], target))
        self.assertIn("fill", terrain_view(slope, [0.5, 72.8, 8.5], target, 70.0))
        self.assertIsNone(terrain_view(field(), [0.5, 72.8, 8.5], [7.5, 70.5, 8.5], 70.0), "a level dolly framing")


class FramingTests(unittest.TestCase):
    def test_subject_must_sit_in_the_central_half(self):
        eye, look = [0.0, 72.0, 0.0], [0.0, 71.0, 10.0]
        self.assertFalse(off_centre(eye, look, 70.0, [1.0, 71.0, 10.0]))
        self.assertTrue(off_centre(eye, look, 70.0, [8.0, 71.0, 10.0]), "at the frame edge")
        self.assertTrue(off_centre(eye, look, 70.0, [0.0, 64.0, 10.0]), "at the frame bottom")


def shot(start, end, eye=(0.0, 80.0, 0.0), look=(100.0, 70.0, 0.0), kind="dolly", attach=None, where=None):
    """A shot holding one fixed pose from `start` to `end`."""
    mid = (start + end) / 2
    keys = keyed([start, mid, end], [list(eye)] * 3, [list(look)] * 3, [70.0] * 3)
    return Shot(kind, start, end, keys, "test", 1.0, attach=attach, where=where)


def violations(shots):
    """Adjacent pairs that start before the previous one ends without a sharp angle change (an independent
    re-derivation of the rule from the views, not from `order_shots`)."""
    bad = []
    for a, b in zip(shots, shots[1:]):
        change = angle_change(view_at(a.keys, a.end, a.attach, a.where), view_at(b.keys, b.start, b.attach, b.where))
        if b.start < a.end - 1e-6 and not change["sharp"]:
            bad.append((a.start, b.start))
    return bad


class ShotOrderTests(unittest.TestCase):
    def test_backtrack_between_similar_shots_is_trimmed(self):
        a, b = shot(10.0, 15.0), shot(12.0, 18.0, eye=(3.0, 80.0, 1.0))
        kept, decisions = order_shots([a, b])
        self.assertEqual([(s.start, s.end) for s in kept], [(10.0, 15.0), (15.0, 18.0)])
        self.assertEqual(violations(kept), [])
        self.assertEqual(len(decisions), 1)
        self.assertTrue(decisions[0].startswith("trim "), decisions[0])
        self.assertIn("12.00 -> 15.00", decisions[0])

    def test_backtrack_leaving_less_than_the_minimum_shot_is_dropped(self):
        a, b = shot(10.0, 15.0), shot(12.0, 15.0 + MIN_SHOT - 0.5)
        kept, decisions = order_shots([a, b])
        self.assertEqual([(s.start, s.end) for s in kept], [(10.0, 15.0)])
        self.assertTrue(decisions[0].startswith("drop "), decisions[0])
        # exactly the minimum survives
        kept, _ = order_shots([shot(10.0, 15.0), shot(12.0, 15.0 + MIN_SHOT)])
        self.assertEqual([(s.start, s.end) for s in kept], [(10.0, 15.0), (15.0, 15.0 + MIN_SHOT)])

    def test_backtrack_across_a_sharp_angle_change_is_allowed(self):
        short = dict(look=(10.0, 70.0, 0.0))
        cases = {  # message word: (shot before, shot after) whose cut is sharp for that reason alone
            "camera": (shot(10.0, 15.0), shot(12.0, 17.0, eye=(0.0, 80.0, 45.0), look=(100.0, 70.0, 20.0))),
            "look target": (shot(10.0, 15.0), shot(12.0, 17.0, look=(100.0, 70.0, 45.0))),
            "heading": (shot(10.0, 15.0, **short), shot(12.0, 17.0, look=(0.0, 70.0, 10.0))),
        }
        for what, (before, after) in cases.items():
            with self.subTest(what):
                change = angle_change(view_at(before.keys, before.end, None, None), view_at(after.keys, after.start, None, None))
                self.assertTrue(change["sharp"])
                self.assertEqual(len(change["why"]), 1, change)
                kept, decisions = order_shots([before, after])
                self.assertEqual([(s.start, s.end) for s in kept], [(10.0, 15.0), (12.0, 17.0)])
                self.assertTrue(decisions[0].startswith("keep "), decisions[0])
                self.assertIn(what, decisions[0])

    def test_just_inside_the_thresholds_is_not_sharp(self):
        a = shot(10.0, 15.0)
        b = shot(12.0, 17.0, eye=(0.0, 80.0, 39.0), look=(100.0, 70.0, 39.0))  # 39 blocks apart, same heading
        self.assertFalse(angle_change(view_at(a.keys, 15.0, None, None), view_at(b.keys, 12.0, None, None))["sharp"])
        short = shot(10.0, 15.0, look=(10.0, 70.0, 0.0))
        for degrees, sharp in ((59.0, False), (61.0, True)):
            turn = math.radians(degrees)
            c = shot(12.0, 17.0, look=(10.0 * math.cos(turn), 70.0, 10.0 * math.sin(turn)))
            change = angle_change(view_at(short.keys, 15.0, None, None), view_at(c.keys, 12.0, None, None))
            self.assertAlmostEqual(change["yaw"], degrees, places=3)
            self.assertEqual(change["sharp"], sharp, change)

    def test_forward_jumps_are_untouched(self):
        shots = [shot(10.0, 15.0), shot(15.0, 19.0, eye=(2.0, 80.0, 0.0)), shot(30.0, 34.0, eye=(1.0, 80.0, 0.0)),
                 shot(200.0, 205.0, eye=(500.0, 90.0, 0.0), look=(400.0, 70.0, 0.0))]
        before = [(s.start, s.end) for s in shots]
        kept, decisions = order_shots(shots)
        self.assertEqual([(s.start, s.end) for s in kept], before)
        self.assertEqual(decisions, [])

    def test_input_in_any_order_comes_out_in_battle_time(self):
        kept, _ = order_shots([shot(30.0, 34.0), shot(10.0, 15.0), shot(20.0, 24.0)])
        self.assertEqual([s.start for s in kept], [10.0, 20.0, 30.0])

    def test_a_trim_moves_the_next_comparison_to_the_trimmed_shot(self):
        kept, decisions = order_shots([shot(10.0, 15.0), shot(12.0, 19.0), shot(16.0, 22.0)])
        self.assertEqual([(s.start, s.end) for s in kept], [(10.0, 15.0), (15.0, 19.0), (19.0, 22.0)])
        self.assertEqual(violations(kept), [])
        self.assertEqual(len(decisions), 2)

    def test_attached_camera_is_placed_at_its_fighter(self):
        far = shot(12.0, 17.0, eye=(1.0, 5.5, -3.0), look=(10.0, 0.3, 0.0), kind="shoulder", attach=7, where=[500.0, 90.0, 0.0])
        near = shot(12.0, 17.0, eye=(1.0, 5.5, -3.0), look=(10.0, 0.3, 0.0), kind="shoulder", attach=7, where=[2.0, 80.0, 0.0])
        free = shot(10.0, 15.0, eye=(0.0, 85.0, -3.0), look=(10.0, 80.0, 0.0))
        self.assertTrue(angle_change(view_at(free.keys, 15.0, None, None), view_at(far.keys, 12.0, 7, far.where))["sharp"])
        self.assertFalse(angle_change(view_at(free.keys, 15.0, None, None), view_at(near.keys, 12.0, 7, near.where))["sharp"])
        unknown = view_at(near.keys, 12.0, 7, None)
        self.assertEqual(unknown[:2], (None, None))
        self.assertFalse(angle_change(view_at(free.keys, 15.0, None, None), unknown)["sharp"], "heading alone decides")

    def test_negative_control_without_the_rule_the_backtrack_stays(self):
        similar = lambda: [shot(10.0, 15.0), shot(12.0, 17.0, eye=(3.0, 80.0, 1.0))]
        self.assertEqual(violations(order_shots(similar())[0]), [])
        # Disabled rule: every cut counts as a sharp angle change, so nothing is trimmed and the check sees the backtrack.
        with mock.patch.object(battle_shots, "angle_change",
                               lambda a, b: {"position": 0.0, "look": 0.0, "yaw": 0.0, "sharp": True, "why": ["disabled"]}):
            kept, _ = order_shots(similar())
        self.assertEqual([(s.start, s.end) for s in kept], [(10.0, 15.0), (12.0, 17.0)])
        self.assertEqual(violations(kept), [(10.0, 12.0)])


class CheckTests(unittest.TestCase):
    def run_check(self, rows, where=None):
        """`check` on a shots.txt of (name, Shot) rows with their cameras written under a temp dir."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "cameras").mkdir()
            lines = ["# NAME CAMERA START END [RADIUS]"]
            for name, s in rows:
                (root / "cameras" / f"{name}.json").write_text(json.dumps(s.camera()))
                lines.append(f"{name} cameras/{name}.json {s.start:.2f} {s.end:.2f} 20")
            (root / "shots.txt").write_text("\n".join(lines) + "\n")
            if where is not None:
                (root / "plan.json").write_text(json.dumps({"shots": [{"name": n, "where": w} for n, w in where.items()]}))
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                status = check(Namespace(shots=str(root / "shots.txt"), camera_dir=str(root)))
            return status, out.getvalue()

    def test_forbidden_backtrack_fails_and_is_listed(self):
        status, out = self.run_check([("a", shot(10.0, 15.0)), ("b", shot(12.0, 17.0, eye=(3.0, 80.0, 1.0)))])
        self.assertEqual(status, 1)
        self.assertIn("a -> b  delta -3.00 s  similar angle", out)
        self.assertIn("FORBIDDEN BACKTRACK", out)
        self.assertIn("1 forbidden backtracks", out)

    def test_sharp_backtrack_and_forward_jumps_pass(self):
        status, out = self.run_check([("a", shot(10.0, 15.0)),
                                      ("b", shot(12.0, 17.0, look=(100.0, 70.0, 45.0))),  # sharp backtrack
                                      ("c", shot(40.0, 44.0, look=(100.0, 70.0, 45.0))),  # forward, similar
                                      ("d", shot(60.0, 64.0, eye=(300.0, 80.0, 0.0), look=(400.0, 70.0, 0.0)))])  # forward, sharp
        self.assertEqual(status, 0, out)
        self.assertIn("a -> b  delta -3.00 s  sharp angle change", out)
        self.assertIn("backtrack allowed", out)
        self.assertIn("b -> c  delta +23.00 s  similar angle", out)
        self.assertNotIn("FORBIDDEN", out)

    def test_a_plan_the_planner_made_passes_its_own_check(self):
        kept, _ = order_shots([shot(10.0, 15.0), shot(12.0, 17.0, eye=(3.0, 80.0, 1.0)), shot(14.0, 18.0)])
        status, out = self.run_check([(f"s{i}", s) for i, s in enumerate(kept)])
        self.assertEqual(status, 0, out)

    def test_attached_shot_world_position_comes_from_plan_json(self):
        a = shot(10.0, 15.0, eye=(0.0, 85.0, -3.0), look=(10.0, 80.0, 0.0))
        b = shot(12.0, 17.0, eye=(1.0, 5.5, -3.0), look=(10.0, 0.3, 0.0), kind="shoulder", attach=7)
        status, out = self.run_check([("a", a), ("b", b)], where={"a": None, "b": [500.0, 90.0, 0.0]})
        self.assertEqual(status, 0, out)
        self.assertIn("sharp angle change (camera 5", out)
        status, out = self.run_check([("a", a), ("b", b)], where={"a": None, "b": [2.0, 80.0, 0.0]})
        self.assertEqual(status, 1, out)


if __name__ == "__main__":
    unittest.main()
