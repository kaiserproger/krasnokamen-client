"""Checks for model-space symmetry around a translated shared root."""
import unittest

from tools.entity_models import symmetry_errors


class ModelSymmetryTests(unittest.TestCase):
    def test_shared_root_offset_preserves_local_symmetry(self):
        model = {"bones": [{"name": "model", "pivot": [0.5, 0, 0], "bones": [
            {"name": "left_leg", "pivot": [2, 0, 0],
             "cubes": [{"origin": [-1, 0, 0], "size": [2, 2, 2]}]},
            {"name": "right_leg", "pivot": [-2, 0, 0],
             "cubes": [{"origin": [-1, 0, 0], "size": [2, 2, 2]}]},
        ]}]}
        self.assertEqual(symmetry_errors(model), [])
        model["bones"][0]["bones"][1]["pivot"][0] = 2
        self.assertTrue(symmetry_errors(model))
