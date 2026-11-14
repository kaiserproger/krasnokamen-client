"""Planner rules: camera height per column, line of sight over hills and trees, the
standoff / terrain / framing checks every pose of a shot must pass, and the shot order
(no backtrack in battle time unless the cut is a sharp angle change)."""
import contextlib
import copy
import io
import json
import math
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest import mock

from tools import battle_shots
from tools.battle_shots import (HIGH, HIGH_PITCH, MAX_RUN, MIN_SHOT, MIN_VARIED, STANDOFF, Shot, Terrain, Timeline, Vet,
                                angle_change, check, crowding, diversify, dolly_shots, enfilade_shots, flank_shots,
                                ground_relief, shoulder_shots,
                                frame_share, keyed, median_pitch, off_centre, order_shots, place, pose, shot_class,
                                terrain_view, variety_problems, view_at)


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
        status, out = self.run_check([("a", shot(10.0, 15.0, look=(100.0, 20.0, 0.0))),  # a steep view: a high shot
                                      ("b", shot(12.0, 17.0, look=(100.0, 70.0, 45.0))),  # sharp backtrack
                                      ("c", shot(40.0, 44.0, look=(100.0, 70.0, 45.0))),  # forward, similar
                                      ("d", shot(60.0, 64.0, eye=(300.0, 80.0, 0.0), look=(400.0, 10.0, 0.0)))])  # forward, sharp, high
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
        b = shot(12.0, 17.0, eye=(1.0, 5.5, -3.0), look=(30.0, 1.5, 0.0), kind="shoulder", attach=7)
        status, out = self.run_check([("a", a), ("b", b)], where={"a": None, "b": [500.0, 90.0, 0.0]})
        self.assertEqual(status, 0, out)
        self.assertIn("sharp angle change (camera 5", out)
        status, out = self.run_check([("a", a), ("b", b)], where={"a": None, "b": [2.0, 80.0, 0.0]})
        self.assertEqual(status, 1, out)

    def test_three_high_shots_in_a_row_fail_the_check_and_alternating_ones_pass(self):
        high = lambda start, x: shot(start, start + 5.0, eye=(x, 100.0, 0.0), look=(x + 30.0, 70.0, 0.0))
        flat = lambda start, x: shot(start, start + 5.0, eye=(x, 80.0, 0.0), look=(x + 100.0, 70.0, 0.0))
        status, out = self.run_check([("a", high(10.0, 0.0)), ("b", high(20.0, 1.0)), ("c", high(30.0, 2.0)), ("d", flat(40.0, 3.0))])
        self.assertEqual(status, 1, out)
        self.assertIn("VARIETY VIOLATION: 3 high shots in a row (shots 1-3), at most 2", out)
        self.assertIn("classes: a high (-45 deg), b high (-45 deg), c high (-45 deg), d low-side (-6 deg)", out)
        status, out = self.run_check([("a", high(10.0, 0.0)), ("b", high(20.0, 1.0)), ("c", flat(30.0, 2.0)), ("d", flat(40.0, 3.0))])
        self.assertEqual(status, 0, out)
        self.assertIn("0 variety violations", out)

    def test_too_few_low_or_side_shots_fail_the_check(self):
        high = lambda start, x: shot(start, start + 5.0, eye=(x, 100.0, 0.0), look=(x + 30.0, 70.0, 0.0))
        flat = lambda start, x: shot(start, start + 5.0, eye=(x, 80.0, 0.0), look=(x + 100.0, 70.0, 0.0))
        rows = [("a", high(10.0, 0.0)), ("b", high(20.0, 1.0)), ("c", flat(30.0, 2.0)), ("d", high(40.0, 3.0)), ("e", high(50.0, 4.0)),
                ("f", flat(60.0, 5.0)), ("g", high(70.0, 6.0)), ("h", high(80.0, 7.0)), ("i", flat(90.0, 8.0)), ("j", high(100.0, 9.0))]
        status, out = self.run_check(rows)
        self.assertEqual(status, 1, out)
        self.assertIn("VARIETY VIOLATION: 3 of 10 shots are low-side or shoulder, under 40%", out)
        self.assertNotIn("in a row", out)


def high_shot(start, end, kind="dolly", x=0.0, score=1.0):
    """A steep (-45 deg) view; `x` keeps two such shots from being similar cameras."""
    return with_score(shot(start, end, eye=(x, 100.0, 0.0), look=(x + 30.0, 70.0, 0.0), kind=kind, where=[x, 70.0, 0.0]), score)


def flat_shot(start, end, kind="flank", x=0.0, score=1.0):
    """A flat (-6 deg) view."""
    return with_score(shot(start, end, eye=(x, 80.0, 0.0), look=(x + 100.0, 70.0, 0.0), kind=kind, where=[x, 70.0, 0.0]), score)


def with_score(s, score):
    s.score = score
    return s


class ShotClassTests(unittest.TestCase):
    def keys(self, eye, look):
        return keyed([0.0, 1.0, 2.0], [list(eye)] * 3, [list(look)] * 3, [70.0] * 3)

    def test_median_pitch_below_the_limit_is_high(self):
        steep = self.keys((0.0, 100.0, 0.0), (30.0, 70.0, 0.0))  # 45 deg down
        self.assertAlmostEqual(median_pitch(steep), -45.0, places=3)
        self.assertEqual(shot_class(steep), "high")
        edge = self.keys((0.0, 100.0, 0.0), (100.0, 100.0 - 100.0 * math.tan(math.radians(-HIGH_PITCH)) - 1.0, 0.0))
        self.assertEqual(shot_class(edge), "high")

    def test_flat_views_are_low_side_and_attached_flat_views_are_shoulder(self):
        flat = self.keys((0.0, 76.0, 0.0), (100.0, 71.0, 0.0))
        self.assertEqual(shot_class(flat), "low-side")
        self.assertEqual(shot_class(flat, attach=5), "shoulder")
        mild = self.keys((0.0, 100.0, 0.0), (100.0, 100.0 - 100.0 * math.tan(math.radians(-HIGH_PITCH)) + 1.0, 0.0))
        self.assertEqual(shot_class(mild), "low-side", "just above the limit")

    def test_an_attached_steep_view_is_still_high(self):
        self.assertEqual(shot_class(self.keys((0.0, 5.5, 0.0), (6.0, 0.3, 0.0)), attach=5), "high")

    def test_a_volley_that_looks_up_is_low_side(self):
        self.assertEqual(shot_class(self.keys((0.0, 74.0, 0.0), (50.0, 80.0, 0.0))), "low-side")


class VarietyRuleTests(unittest.TestCase):
    def test_more_than_two_of_a_class_in_a_row_is_a_run(self):
        self.assertEqual(MAX_RUN, 2)
        problems = variety_problems(["high", "low-side", "low-side", "high", "shoulder", "high"])
        self.assertEqual(problems, [])
        problems = variety_problems(["high", "low-side", "low-side", "low-side", "high", "shoulder"])
        self.assertEqual([(k, a, b) for k, a, b, _ in problems], [("run", 1, 3)])
        self.assertIn("3 low-side shots in a row (shots 2-4)", problems[0][3])

    def test_shoulder_and_low_side_are_different_classes_for_runs(self):
        self.assertEqual(variety_problems(["low-side", "low-side", "shoulder", "shoulder", "high"]), [])

    def test_low_side_and_shoulder_share_is_at_least_the_minimum(self):
        self.assertEqual(MIN_VARIED, 0.4)
        self.assertEqual(variety_problems(["high", "low-side", "high", "shoulder", "high"]), [], "2 of 5 is exactly 40 %")
        problems = variety_problems(["high", "low-side", "high", "high", "shoulder", "high", "high"])
        self.assertEqual([k for k, *_ in problems], ["share"])
        self.assertIn("2 of 7", problems[0][3])

    def test_empty_plan_has_no_problems(self):
        self.assertEqual(variety_problems([]), [])


class DiversifyTests(unittest.TestCase):
    def classes(self, shots):
        return [s.cls for s in order_shots([copy.copy(s) for s in shots])[0]]

    def test_a_run_of_high_shots_is_broken_with_unused_low_side_candidates(self):
        picked = [high_shot(10.0 + 12.0 * i, 15.0 + 12.0 * i, x=10.0 * i, score=1.0 + i) for i in range(5)]
        spare = [flat_shot(16.0 + 12.0 * i, 21.0 + 12.0 * i, x=50.0 + 10.0 * i) for i in range(4)]
        self.assertTrue(variety_problems(self.classes(picked)), "the raw pick is five high shots in a row")
        chosen, decisions = diversify(picked, picked + spare)
        self.assertEqual(variety_problems(self.classes(chosen)), [])
        self.assertTrue(decisions and all(d.startswith("swap high") for d in decisions), decisions)
        self.assertIn("for low-side flank", decisions[0])
        self.assertEqual(len(chosen), len(picked), "swaps keep the cut as long")

    def test_the_weakest_shot_of_the_run_is_the_one_swapped_out(self):
        picked = [high_shot(10.0, 15.0, x=0.0, score=9.0), high_shot(20.0, 25.0, x=10.0, score=1.0),
                  high_shot(30.0, 35.0, x=20.0, score=8.0), flat_shot(40.0, 45.0, x=30.0), flat_shot(50.0, 55.0, x=40.0)]
        spare = [flat_shot(21.0, 26.0, x=60.0)]
        chosen, decisions = diversify(picked, picked + spare)
        self.assertEqual(len(decisions), 1, decisions)
        self.assertIn("swap high dolly 20.00-25.00", decisions[0])
        self.assertNotIn(picked[1], chosen)
        self.assertIn(spare[0], chosen)

    def test_without_an_unused_candidate_the_offender_is_dropped(self):
        picked = [high_shot(10.0, 15.0, x=0.0, score=3.0), high_shot(20.0, 25.0, x=10.0, score=1.0),
                  high_shot(30.0, 35.0, x=20.0, score=2.0), flat_shot(40.0, 45.0, x=30.0)]
        chosen, decisions = diversify(picked, picked)
        self.assertEqual([d.split(":")[0] for d in decisions], ["drop high dolly 20.00-25.00", "drop high dolly 30.00-35.00"])
        self.assertEqual(variety_problems(self.classes(chosen)), [])
        self.assertEqual([s.cls for s in chosen], ["high", "low-side"])

    def test_a_plan_that_already_varies_is_untouched_and_a_short_cut_is_filled_with_whole_candidates(self):
        picked = [flat_shot(10.0, 15.0, x=0.0), high_shot(20.0, 25.0, x=10.0), flat_shot(30.0, 35.0, x=20.0)]
        chosen, decisions = diversify(picked, picked)
        self.assertEqual((chosen, decisions), (picked, []))
        spare = [high_shot(40.0, 45.0, x=40.0, kind="push-in"), flat_shot(32.0, 37.0, x=90.0, kind="enfilade")]
        chosen, decisions = diversify(picked, picked + spare, seconds=20.0)
        self.assertEqual(len(chosen), 4, decisions)
        self.assertIn(spare[0], chosen, "a whole later candidate fills the cut")
        self.assertNotIn(spare[1], chosen, "an overlapping candidate would be trimmed")
        self.assertEqual(len(decisions), 1)
        self.assertTrue(decisions[0].startswith("add high push-in 40.00-45.00"), decisions)


def sided_cluster(cx, cz, red, blue, half_length=3.0, half_depth=3.0):
    """A contact cluster with red on its -normal (z-) side and blue on +normal (z+)."""
    c = cluster(cx, cz, half_length, half_depth)
    c.update(red=red, blue=blue, count=red + blue)
    return c


class FrameShareTests(unittest.TestCase):
    def test_down_the_line_both_colours_are_in_frame_and_across_it_from_behind_only_the_near_side_counts(self):
        s = sample(0, [sided_cluster(0, 0, 50, 50, half_length=10.0, half_depth=10.0)])
        look = [0.0, 71.2, 0.0]
        red, blue = frame_share(s, [-30.0, 75.0, 0.0], look, 70.0)
        self.assertAlmostEqual(red, 50.0, delta=1.0)
        self.assertAlmostEqual(blue, 50.0, delta=1.0)
        red, blue = frame_share(s, [0.0, 75.0, 40.0], [0.0, 75.0, 60.0], 70.0)
        self.assertEqual((red, blue), (0.0, 0.0), "looking away from the cluster")
        red, blue = frame_share(s, [0.0, 75.0, -9.0], [0.0, 74.0, 60.0], 40.0)
        self.assertGreater(blue, 0.0)
        self.assertLess(red + blue, 100.0)

    def test_colour_split_follows_the_counts(self):
        s = sample(0, [sided_cluster(0, 0, 90, 10, half_length=10.0, half_depth=10.0)])
        red, blue = frame_share(s, [-30.0, 75.0, 0.0], [0.0, 71.2, 0.0], 70.0)
        self.assertGreater(red, 7 * blue)

    def test_fighters_beyond_the_reach_do_not_count(self):
        s = sample(0, [sided_cluster(0, 0, 50, 50)])
        self.assertEqual(frame_share(s, [-200.0, 75.0, 0.0], [0.0, 71.2, 0.0], 70.0), (0.0, 0.0))


class ContactFramingTests(unittest.TestCase):
    def line_vet(self, red=50, blue=50):
        tl = Timeline([sample(i * 0.5, [sided_cluster(0, 0, red, blue, half_length=10.0, half_depth=10.0)]) for i in range(21)])
        return tl, Vet(tl, field())

    def keys(self, eye, look):
        return keyed([5.0, 7.0, 9.0], [list(eye)] * 3, [list(look)] * 3, [70.0] * 3)

    def test_a_shot_with_no_contact_in_frame_is_rejected_when_sides_are_asked_for(self):
        _, vet = self.line_vet()
        away = self.keys([0.0, 78.0, 30.0], [0.0, 76.0, 45.0])
        self.assertTrue(vet(away), "the other checks pass")
        self.assertFalse(vet(away, sides=True))
        self.assertEqual(vet.rejected["no contact in frame"], 1)

    def test_a_frame_filled_by_one_colour_is_rejected_and_a_balanced_one_passes(self):
        eye, look = [-30.0, 75.0, 0.0], [0.0, 71.2, 0.0]
        _, vet = self.line_vet(red=95, blue=5)
        self.assertTrue(vet(self.keys(eye, look)))
        self.assertFalse(vet(self.keys(eye, look), sides=True))
        self.assertEqual(vet.rejected["one colour fills the frame"], 1)
        _, vet = self.line_vet()
        self.assertTrue(vet(self.keys(eye, look), sides=True))

    def test_enfilade_looks_down_the_line_flat_and_frames_both_colours(self):
        tl, vet = self.line_vet()
        shots = enfilade_shots(tl, field(), vet)
        self.assertTrue(shots)
        for s in shots:
            self.assertEqual(s.kind, "enfilade")
            self.assertEqual(s.cls, "low-side", s.why)
            self.assertGreater(median_pitch(s.keys), HIGH_PITCH)
            eye, look, fov = pose(s.keys, (s.start + s.end) / 2)
            self.assertGreaterEqual(abs(eye[0]), 10.0 + 10.0, "beyond the line's end (half length 10 + 10 back)")
            self.assertLessEqual(abs(eye[2]), 2.5, "on the line's axis")
            red, blue = frame_share(tl.at((s.start + s.end) / 2), eye, look, fov)
            self.assertGreater(min(red, blue), 0.15 * (red + blue))

    def test_enfilade_of_a_one_sided_line_is_rejected(self):
        tl, vet = self.line_vet(red=100, blue=0)
        self.assertEqual(enfilade_shots(tl, field(), vet), [])
        self.assertGreater(vet.rejected["one colour fills the frame"], 0)

    def shoulder_world(self, red, blue, half_length=10.0, half_depth=2.0):
        c = sided_cluster(0, 0, red, blue, half_length=half_length, half_depth=half_depth)
        c["ids"] = {"red": [1], "blue": [2]}
        tl = Timeline([sample(i * 0.5, [c]) for i in range(21)])
        entities = {i: {"role": "fighter", "vehicle": None, "first_contact_t": 6.0, "spawn_t": 0.0, "death_t": None, "removed_t": None}
                    for i in (1, 2)}
        return tl, entities, Vet(tl, field())

    def test_shoulder_camera_rides_a_fighter_flat_and_frames_both_colours(self):
        tl, entities, vet = self.shoulder_world(50, 50)
        shots = shoulder_shots(tl, field(), entities, vet)
        self.assertTrue(shots)
        for s in shots:
            self.assertEqual((s.kind, s.cls), ("shoulder", "shoulder"), s.why)
            self.assertIn(s.attach, (1, 2))
            self.assertGreater(median_pitch(s.keys), HIGH_PITCH)
            _, red, blue = battle_shots.mid_frame(tl, field(), s)
            self.assertGreater(min(red, blue), 0.15 * (red + blue), s.why)
        self.assertTrue(any("looking along the line" in s.why for s in shots), "a long thin line is framed along its length")

    def test_shoulder_lens_rises_with_the_ground_steps_around_the_cluster(self):
        flat_tl, entities, flat_vet = self.shoulder_world(50, 50)
        flat = shoulder_shots(flat_tl, field(), entities, flat_vet)
        self.assertTrue(flat)
        for s in flat:
            self.assertAlmostEqual(s.keys[1][1][1], HIGH, places=6)
        stepped = field(ground=lambda x, z: 70 if x < 2 else 73)  # a 3-block step beside the cluster's centre
        tl, entities, vet = self.shoulder_world(50, 50)
        lifted = shoulder_shots(tl, stepped, entities, vet)
        self.assertTrue(lifted)
        for s in lifted:
            self.assertAlmostEqual(s.keys[1][1][1], HIGH + 3.0, places=6, msg=s.why)
            self.assertEqual(s.cls, "shoulder", "the lift keeps the view flat")
        self.assertEqual(ground_relief(stepped, [0.0, 70.0, 0.0], 5.0), 3)
        self.assertEqual(ground_relief(field(), [0.0, 70.0, 0.0], 5.0), 0)

    def test_shoulder_over_a_one_sided_cluster_is_rejected(self):
        tl, entities, vet = self.shoulder_world(100, 0)
        self.assertEqual(shoulder_shots(tl, field(), entities, vet), [])
        self.assertGreater(vet.rejected["one colour fills the frame"] + vet.rejected["no contact in frame"], 0)

    def test_flank_dolly_is_a_flat_side_view_with_both_colours(self):
        tl, vet = self.line_vet()
        shots = flank_shots(tl, field(), vet)
        self.assertTrue(shots)
        for s in shots:
            self.assertEqual(s.cls, "low-side", s.why)
            eye, look, fov = pose(s.keys, (s.start + s.end) / 2)
            self.assertGreaterEqual(abs(eye[2]), 10.0 + 1.0 + STANDOFF - 1e-6, "outside the line's ground by the standoff")
            red, blue = frame_share(tl.at((s.start + s.end) / 2), eye, look, fov)
            self.assertGreater(min(red, blue), 0.15 * (red + blue))


class PlanEndToEndTests(unittest.TestCase):
    """`plan` on a small synthetic timeline file, then `check` on the shots.txt it wrote."""

    def write_timeline(self, path):
        arrows = {"launched": 0, "launch": None}
        rows = [{"kind": "header"}]
        for i in range(70):
            t = i * 0.5
            contacts = [sided_cluster(0, 0, 60, 60, half_length=10.0, half_depth=4.0),
                        sided_cluster(-24, 30, 60, 40, half_length=8.0, half_depth=3.0)] if t >= 4.0 else []
            army = {"centroid": [0, 70, 0], "min": [-45, 70, -45], "max": [45, 72, 45]}  # a lens inside rides HIGH
            rows.append({"kind": "sample", "t": t, "contacts": contacts, "cavalry": [], "arrows": {"red": arrows, "blue": arrows},
                         "armies": {"red": army, "blue": army},
                         "contact_fighters": sum(c["count"] for c in contacts)})
        for cx in range(-3, 3):
            for cz in range(-3, 3):
                rows.append({"kind": "chunk", "x": cx, "z": cz, "surface": [70] * 256, "ground": [70] * 256})
        rows.append({"kind": "summary", "first_tick": 0})
        Path(path).write_text("\n".join(json.dumps(r) for r in rows) + "\n")

    def test_the_plan_has_both_low_side_and_high_shots_and_passes_its_own_check(self):
        with tempfile.TemporaryDirectory() as tmp:
            timeline = Path(tmp) / "timeline.jsonl"
            self.write_timeline(timeline)
            out = Path(tmp) / "plan"
            with contextlib.redirect_stdout(io.StringIO()):
                battle_shots.plan(Namespace(timeline=str(timeline), out=str(out), name="t", seconds=30.0, radius=20))
            report = json.loads((out / "plan.json").read_text())
            classes = [s["class"] for s in report["shots"]]
            self.assertGreaterEqual(len(classes), 4, classes)
            self.assertIn("low-side", classes)
            self.assertIn("high", classes)
            self.assertEqual(variety_problems(classes), [], classes)
            for s in report["shots"]:
                self.assertIn(s["class"], ("high", "low-side", "shoulder"))
                self.assertIsNotNone(s["height"])
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                status = check(Namespace(shots=str(out / "shots.txt"), camera_dir=str(out)))
            self.assertEqual(status, 0, buf.getvalue())
            self.assertIn("0 variety violations", buf.getvalue())


if __name__ == "__main__":
    unittest.main()
