#!/usr/bin/env python3
"""Check shipping PE binaries on any host; no Windows runtime is required."""
import argparse
from pathlib import Path
import struct


def check(path: Path) -> None:
    with path.open("rb") as binary:
        dos = binary.read(64)
        if len(dos) != 64 or dos[:2] != b"MZ":
            raise ValueError(f"{path}: missing DOS header")
        binary.seek(struct.unpack_from("<I", dos, 60)[0])
        header = binary.read(24)
        if len(header) != 24 or header[:4] != b"PE\0\0":
            raise ValueError(f"{path}: missing PE header")
        optional = binary.read(struct.unpack_from("<H", header, 20)[0])
        if len(optional) < 70 or struct.unpack_from("<H", optional)[0] not in (0x10B, 0x20B):
            raise ValueError(f"{path}: missing PE32/PE32+ optional header")
        subsystem = struct.unpack_from("<H", optional, 68)[0]
        if subsystem != 2:  # IMAGE_SUBSYSTEM_WINDOWS_GUI
            raise ValueError(f"{path}: subsystem {subsystem}, expected Windows GUI (2)")
    print(f"{path}: Windows GUI (2)")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binaries", type=Path, nargs="+")
    args = parser.parse_args()
    for path in args.binaries:
        check(path)
