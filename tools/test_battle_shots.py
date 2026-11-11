"""Planner terrain rules: camera height per column, line of sight over hills and trees."""
import unittest

from tools.battle_shots import Terrain, place


def terrain(ground=70, hill=None, tree=None):
    """One chunk (0,0) of flat ground; `hill` raises x>=8 to that height, `tree` puts a canopy column."""
    surface, base = [], []
    for index in range(256):
        x, z = index & 15, index >> 4
        g = hill if hill is not None and x >= 8 else ground
        base.append(g)
        surface.append(tree[1] if tree is not None and (x, z) == tree[0] else g)
    return Terrain({(0, 0): (surface, base)})


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


if __name__ == "__main__":
    unittest.main()
