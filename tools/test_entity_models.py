"""Checks for model-space symmetry around a translated shared root."""
import unittest

from tools.entity_models import selection_report, symmetry_errors


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


class SelectionReportTests(unittest.TestCase):
    def report(self, classification="simplified", passed=True):
        return {"schema": 1, "game_version": "26.3", "rows": [{
            "id": "selection/minecraft:cow/adult/received/minecraft:cold",
            "class": "selection", "classification": classification,
            "required_rig": "minecraft:cold_cow#main",
            "coverage_pass": passed, "accepted": False,
        }]}

    def test_catalog_presence_is_not_visual_acceptance(self):
        summary = selection_report(self.report())["summary"]
        self.assertEqual(summary["classifications"], {"simplified": 1})
        self.assertEqual(summary["unaccepted"], 1)
        self.assertEqual(summary["catalog_failures"], [])

    def test_missing_and_rejected_selection_rows_stay_failures(self):
        for classification in ("missing", "rejected"):
            report = self.report(classification, False)
            summary = selection_report(report)["summary"]
            self.assertEqual(summary["catalog_failures"], [report["rows"][0]["id"]])
            self.assertEqual(summary["selection_failures"], summary["catalog_failures"])

    def test_duplicate_rows_cannot_inflate_coverage(self):
        report = self.report()
        report["rows"] *= 2
        with self.assertRaisesRegex(ValueError, "uniquely identified"):
            selection_report(report)
