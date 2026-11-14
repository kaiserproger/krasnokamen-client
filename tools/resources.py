#!/usr/bin/env python3
"""Rebuild the client resource pack (`dist/resources`) from a local vanilla client jar.

The client's model tests and `tools/entity_models.py` read `dist/resources`, and the
launcher's stage 5 (`launcher/src/bootstrap.rs::extract_resources`) writes the same
pack into the user's data root. Both produce `assets/**` plus a `.resources.json`
marker, and a pack is trusted only when that marker names the jar that would build
it (sha1, entry count, digest of the sorted paths): the installed manifest's
`resources_for` string is only a claim, and a pre-populated or half-written tree
carries a stale one, which left 26.3-only assets (`models/block/template_sign_*`,
`textures/entity/cushion/*`, `textures/entity/sulfur_cube/*`) missing after an
upgrade from an older client jar.

  resources.py build [--jar PATH] [--pack DIR]
      Extract `assets/**` into a fresh staging pack, write the marker, then swap.
      An entry the jar dropped never survives the rebuild.

  resources.py check [--jar PATH] [--pack DIR] [--models]
      Fail unless the pack's marker matches what the jar produces; with `--models`
      also fail unless every embedded entity model's declared vanilla texture
      exists with its declared size.

Default jar: $KRASNOKAMEN_CLIENT_JAR, else
$KRASNOKAMEN_CLIENT_DATA_DIR/mojang/versions/<ver>/client.jar, else
<repo>/dist/mojang/versions/<ver>/client.jar, else
$HOME/.local/share/krasnokamen-client/mojang/versions/<ver>/client.jar, where
<ver> is the launcher's GAME_VERSION. Default pack: <repo>/dist/resources.

Mojang bytes never enter Git: the pack defaults under the ignored `dist/`, and the
tool refuses a pack directory that Git would track.
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MODELS = os.path.join(ROOT, "assets/krasnokamen/models/entity")
MARKER = ".resources.json"


def game_version():
    text = open(os.path.join(ROOT, "launcher/src/paths.rs"), encoding="utf-8").read()
    match = re.search(r'pub const GAME_VERSION: &str = "([0-9.]+)";', text)
    if not match:
        raise SystemExit("resources.py: launcher GAME_VERSION not found")
    return match.group(1)


def default_pack():
    return os.path.join(ROOT, "dist/resources")


def default_jar(version):
    candidates = [os.environ.get("KRASNOKAMEN_CLIENT_JAR")]
    data_dir = os.environ.get("KRASNOKAMEN_CLIENT_DATA_DIR")
    if data_dir:
        candidates.append(os.path.join(data_dir))
    candidates = [c for c in candidates if c]
    for root in candidates + [
        os.path.join(ROOT, "dist"),
        os.path.join(os.path.expanduser("~"), ".local/share/krasnokamen-client"),
    ]:
        path = os.path.join(root, "mojang/versions", version, "client.jar")
        if os.path.isfile(path):
            return path
    return None


def require(jar):
    if not jar:
        raise SystemExit(
            "resources.py: no client jar found; pass --jar or set KRASNOKAMEN_CLIENT_JAR"
        )
    if not os.path.isfile(jar):
        raise SystemExit(f"resources.py: client jar {jar} does not exist")
    return os.path.abspath(jar)


def tracked(path):
    """True when the repository's Git would track this path (Mojang bytes stay out)."""
    relative = os.path.relpath(path, ROOT)
    if relative.startswith(os.pardir) or os.path.isabs(relative):
        return False
    try:
        # 0 = ignored, 1 = not ignored, other = no answer (treat as outside Git).
        result = subprocess.run(
            ["git", "-C", ROOT, "check-ignore", "-q", "--", relative],
            capture_output=True,
        )
    except OSError:
        return False
    return result.returncode == 1


def asset_entries(jar):
    with zipfile.ZipFile(jar) as archive:
        names = sorted(
            info.filename
            for info in archive.infolist()
            if not info.is_dir() and info.filename.startswith("assets/")
        )
    digest = hashlib.sha256()
    for name in names:
        digest.update(name.encode() + b"\n")
    return names, digest.hexdigest()


def jar_sha1(jar):
    digest = hashlib.sha1()
    with open(jar, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def expected_marker(jar, version):
    names, paths_sha256 = asset_entries(jar)
    return {
        "game_version": version,
        "client_sha1": jar_sha1(jar),
        "entries": len(names),
        "paths_sha256": paths_sha256,
    }


def read_marker(pack):
    try:
        with open(os.path.join(pack, MARKER), encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return None


def check_version(jar, version):
    """The jar must be the release this client targets (jar entry or sibling json)."""
    release = None
    with zipfile.ZipFile(jar) as archive:
        if "version.json" in archive.namelist():
            release = json.loads(archive.read("version.json")).get("id")
    if release is None:
        sibling = os.path.join(os.path.dirname(jar), f"{version}.json")
        if os.path.isfile(sibling):
            release = json.load(open(sibling, encoding="utf-8")).get("id")
    if release is not None and release != version:
        raise SystemExit(
            f"resources.py: {jar} is Minecraft {release}, this client targets {version}"
        )


def build(jar, pack, version):
    check_version(jar, version)
    if tracked(pack):
        raise SystemExit(
            f"resources.py: {pack} is tracked by Git; Mojang bytes must stay out of the repository"
        )
    marker = expected_marker(jar, version)
    staging = pack + ".staging"
    if os.path.isdir(staging):
        shutil.rmtree(staging)
    os.makedirs(staging)
    with zipfile.ZipFile(jar) as archive:
        for info in archive.infolist():
            if info.is_dir() or not info.filename.startswith("assets/"):
                continue
            target = os.path.normpath(os.path.join(staging, info.filename))
            if not target.startswith(os.path.join(staging, "assets") + os.sep):
                raise SystemExit(f"resources.py: jar entry {info.filename} escapes the pack")
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with archive.open(info) as source, open(target, "wb") as sink:
                shutil.copyfileobj(source, sink, 1 << 20)
    with open(os.path.join(staging, MARKER), "w", encoding="utf-8") as handle:
        json.dump(marker, handle, indent=2)
        handle.write("\n")
    if os.path.isdir(pack):
        shutil.rmtree(pack)
    os.rename(staging, pack)
    print(
        f"resources.py: {pack} <- {os.path.basename(jar)} "
        f"({marker['entries']} asset files, sha1 {marker['client_sha1'][:12]})"
    )


def png_size(path):
    with open(path, "rb") as handle:
        header = handle.read(24)
    if header[:8] != b"\x89PNG\r\n\x1a\n" or header[12:16] != b"IHDR":
        raise SystemExit(f"resources.py: {path} is not a PNG")
    return struct.unpack(">II", header[16:24])


def check_models(pack, version):
    textures = os.path.join(pack, "assets/minecraft/textures")
    failures = []
    for name in sorted(os.listdir(MODELS)):
        if not name.endswith(".json") or name.startswith("battle_"):
            continue
        with open(os.path.join(MODELS, name), encoding="utf-8") as handle:
            model = json.load(handle)
        texture = model.get("texture", {})
        path, size = texture.get("path"), texture.get("size")
        if not path or not size:
            continue
        png = os.path.join(textures, path)
        if not os.path.isfile(png):
            failures.append(f"{name}: {path} is absent from the pack")
        elif list(png_size(png)) != list(size):
            failures.append(
                f"{name}: {path} is {list(png_size(png))}, model declares {size}"
            )
    if failures:
        print(f"resources.py: {len(failures)} model texture problem(s):")
        for line in failures[:20]:
            print(f"  {line}")
        raise SystemExit(1)
    print(f"resources.py: {pack} serves every embedded model texture ({version})")


def check(jar, pack, version, models):
    check_version(jar, version)
    expected = expected_marker(jar, version)
    found = read_marker(pack)
    if found != expected:
        reason = "no marker" if found is None else f"marker {found}"
        raise SystemExit(
            f"resources.py: {pack} does not match {os.path.basename(jar)}: {reason}"
        )
    print(
        f"resources.py: {pack} matches {os.path.basename(jar)} "
        f"({expected['entries']} asset files, sha1 {expected['client_sha1'][:12]})"
    )
    if models:
        check_models(pack, version)


def main(argv):
    version = game_version()
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("action", choices=["build", "check"])
    parser.add_argument("--jar")
    parser.add_argument("--pack", default=default_pack())
    parser.add_argument("--models", action="store_true", help="also check embedded model textures")
    args = parser.parse_args(argv)
    jar = require(args.jar or default_jar(version))
    pack = os.path.abspath(args.pack)
    if args.action == "build":
        build(jar, pack, version)
        check(jar, pack, version, args.models)
    else:
        check(jar, pack, version, args.models)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
