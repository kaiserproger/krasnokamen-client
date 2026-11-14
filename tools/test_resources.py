"""Checks for the resource pack builder's stale-pack handling."""
import json
import os
import tempfile
import unittest
import zipfile

from tools import resources


def make_jar(path, entries):
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("version.json", json.dumps({"id": "26.3"}))
        for name, body in entries:
            archive.writestr(name, body)


KNOWN_ENTRIES_SHA256 = "ad578fa0a6ded8e432c2ca3bfc35f8fe5b00ccf04aa85a2b13ab175522076939"


class StalePackTests(unittest.TestCase):
    def test_entry_digest_is_the_shared_known_answer(self):
        # The launcher computes the same value for this jar shape
        # (`pack_entry_digest_is_the_shared_known_answer` in bootstrap.rs); both
        # must agree or a pack built here would never look current to the launcher.
        with tempfile.TemporaryDirectory() as tmp:
            jar = os.path.join(tmp, "client.jar")
            make_jar(jar, [("assets/a.json", "{}"), ("assets/b/c.png", "x")])
            _, digest = resources.asset_entries(jar)
            self.assertEqual(digest, KNOWN_ENTRIES_SHA256)

    def test_a_pack_from_another_jar_never_matches(self):
        with tempfile.TemporaryDirectory() as tmp:
            jar = os.path.join(tmp, "client.jar")
            make_jar(jar, [("assets/minecraft/models/block/template_sign_rot_0.json", "{}")])
            pack = os.path.join(tmp, "resources")
            os.makedirs(os.path.join(pack, "assets/minecraft/models/block"))
            # A 26.1.2-era pack: a file the 26.3 jar no longer has, and a marker
            # claiming the current sha1 while the paths digest belongs to the old jar.
            with open(os.path.join(pack, "assets/minecraft/models/block/oak_sign.json"), "w") as f:
                f.write("{}")
            marker = resources.expected_marker(jar, "26.3")
            forced = dict(marker, entries_sha256="0" * 64)
            with open(os.path.join(pack, resources.MARKER), "w") as f:
                json.dump(forced, f)
            self.assertNotEqual(resources.read_marker(pack), marker)
            with self.assertRaises(SystemExit):
                resources.check(jar, pack, "26.3", False)

    def test_rebuild_replaces_the_pack_instead_of_merging(self):
        with tempfile.TemporaryDirectory() as tmp:
            jar = os.path.join(tmp, "client.jar")
            make_jar(
                jar,
                [
                    ("assets/minecraft/models/block/template_sign_rot_0.json", "{}"),
                    ("assets/minecraft/textures/entity/cushion/white_cushion.png", "x"),
                ],
            )
            pack = os.path.join(tmp, "resources")
            os.makedirs(os.path.join(pack, "assets/minecraft/models/block"))
            with open(os.path.join(pack, "assets/minecraft/models/block/oak_sign.json"), "w") as f:
                f.write("{}")
            resources.build(jar, pack, "26.3")
            self.assertTrue(
                os.path.isfile(
                    os.path.join(
                        pack, "assets/minecraft/models/block/template_sign_rot_0.json"
                    )
                )
            )
            self.assertFalse(
                os.path.exists(os.path.join(pack, "assets/minecraft/models/block/oak_sign.json")),
                "an entry the jar dropped must not survive the rebuild",
            )
            self.assertEqual(resources.read_marker(pack), resources.expected_marker(jar, "26.3"))
            self.assertFalse(os.path.exists(pack + ".staging"))

    def test_a_jar_for_another_release_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            jar = os.path.join(tmp, "client.jar")
            with zipfile.ZipFile(jar, "w") as archive:
                archive.writestr("version.json", json.dumps({"id": "26.1.2"}))
                archive.writestr("assets/minecraft/models/x.json", "{}")
            with self.assertRaises(SystemExit):
                resources.check_version(jar, "26.3")


if __name__ == "__main__":
    unittest.main()
