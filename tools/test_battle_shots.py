"""Planner rules: camera height per column, line of sight over hills and trees, and the
standoff / terrain / framing checks every pose of a shot must pass."""
import math
import unittest

from tools.battle_shots import (STANDOFF, Terrain, Timeline, Vet, crowding, dolly_shots, keyed, off_centre, place,
                                pose, terrain_view)


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


if __name__ == "__main__":
    unittest.main()
