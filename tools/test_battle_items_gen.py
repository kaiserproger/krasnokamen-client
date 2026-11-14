import json
from pathlib import Path
import unittest
from unittest.mock import patch

import battle_items_gen as generator


PACK = Path(__file__).resolve().parents[1] / "assets/battle_video"


class BattleItemBoundsTest(unittest.TestCase):
    def test_resource_metadata_declares_26_1_2_range(self):
        metadata = json.loads((PACK / "pack.mcmeta").read_text())["pack"]
        self.assertEqual(metadata["min_format"], [84, 0])
        self.assertEqual(metadata["max_format"], [84, 0])

    def test_every_pack_element_fits_java_bounds(self):
        for path in PACK.rglob("*.json"):
            model = json.loads(path.read_text())
            for index, element in enumerate(model.get("elements", [])):
                for key in ("from", "to"):
                    for value in element[key]:
                        with self.subTest(model=str(path.relative_to(PACK)), element=index, coordinate=key, value=value):
                            self.assertGreaterEqual(value, -16)
                            self.assertLessEqual(value, 32)

    def test_generated_long_weapons_preserve_displayed_geometry(self):
        def unbounded_model(weapon, display):
            return {"elements": weapon.elements, "display": display}

        for name in ("lance", "halberd"):
            make = getattr(generator, name)
            with patch.object(generator.Weapon, "model", unbounded_model):
                before = make()
            after = make()
            saved = json.loads((PACK / f"assets/solaris_battle/models/item/iron_{name}.json").read_text())
            self.assertEqual(after, saved, name + ": regenerate the checked-in model")
            self.assertEqual(len(before["elements"]), len(after["elements"]))
            for context, transform in before["display"].items():
                actual = after["display"][context]
                self.assertEqual(transform["rotation"], actual["rotation"])
                self.assertEqual(transform["translation"], actual["translation"])
                for old, new in zip(before["elements"], after["elements"]):
                    self.assertEqual(old["faces"], new["faces"])
                    for key in ("from", "to"):
                        for axis in range(3):
                            self.assertAlmostEqual(
                                (old[key][axis] - 8) * transform["scale"][axis],
                                (new[key][axis] - 8) * actual["scale"][axis],
                                places=9, msg=f"{name}/{context}/{key}/{axis}")


if __name__ == "__main__":
    unittest.main()
