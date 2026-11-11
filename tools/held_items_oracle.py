#!/usr/bin/env python3
"""Pixel assertions for tools/held_items_scene.json: native client frames against the
vanilla Java oracle frames of the same scene (same server, seed, window, FOV, GUI scale).

Usage: held_items_oracle.py <java-receipt> <native-receipt> [--report out.json] [--panels dir]

Each receipt is a scene-shot validation directory with screenshots/<shot>.png at
1280x720. The scene stands on a flat light-gray concrete floor, so the held items,
the framed map and the potion particles are the only foreground. Item and map shots
compare foreground silhouettes (intersection over union); particle shots compare the
number of potion-coloured pixels and their horizontal spread, because particle motion
is random. Exit status 0 only when every assertion holds.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from PIL import Image

SIZE = (1280, 720)
# GUI scale 3: hotbar (182x22 GUI px) and the held-item name above it, plus chat lines.
HUD = [(360, 590, 920, 720), (0, 380, 640, 640)]
# Foreground: farther than this (sum of |dR|+|dG|+|dB|) from the median floor colour.
FOREGROUND = 60
# Lower-right corner holding the potion bottle in the particle shots.
HELD_BOTTLE = (980, 470)

# shot -> (row, region x0, y0, x1, y1, minimum IoU, allowed area ratio native/java)
SILHOUETTES = {
    "statue-sitting": ("MODEL-ITEM-POSE", (640, 200, 1280, 720), 0.55, (0.6, 1.6)),
    "statue-standing": ("MODEL-ITEM-POSE", (640, 200, 1280, 720), 0.55, (0.6, 1.6)),
    "head": ("MODEL-PLAYER-HEAD-PROFILE", (640, 200, 1280, 720), 0.55, (0.6, 1.6)),
    "held-map": ("NATIVE-MAP-MARKERS", (160, 160, 1120, 720), 0.6, (0.7, 1.4)),
    "held-map-offhand": ("NATIVE-MAP-MARKERS", (0, 160, 1280, 720), 0.55, (0.6, 1.6)),
    "framed-map": ("NATIVE-FRAMED-MAP", (440, 180, 840, 520), 0.6, (0.7, 1.4)),
}
# shot -> (row, colour predicate, region, allowed count ratio, minimum spread ratio)
PARTICLES = {
    "potion-effect": ("NATIVE-POTION-IMPACT-EFFECT", "blue", (0, 0, 1280, 720), (0.35, 3.0), 0.5),
    "potion-instant": ("NATIVE-POTION-IMPACT-EFFECT", "red", (0, 0, 1280, 720), (0.35, 3.0), 0.5),
}


def load(receipt: Path, shot: str) -> Image.Image:
    path = receipt / "screenshots" / f"{shot}.png"
    with Image.open(path) as source:
        image = source.convert("RGB")
    if image.size != SIZE:
        raise ValueError(f"{path}: expected {SIZE}, got {image.size}")
    return image


def in_hud(x: int, y: int) -> bool:
    return any(x0 <= x < x1 and y0 <= y < y1 for x0, y0, x1, y1 in HUD)


def floor_colour(image: Image.Image, region) -> tuple[int, int, int]:
    """Median colour over the region: the floor is the majority of every scene shot."""
    x0, y0, x1, y1 = region
    samples = [image.getpixel((x, y)) for y in range(y0, y1, 4) for x in range(x0, x1, 4)
               if not in_hud(x, y)]
    return tuple(sorted(s[i] for s in samples)[len(samples) // 2] for i in range(3))


def foreground(image: Image.Image, region, floor) -> set[tuple[int, int]]:
    x0, y0, x1, y1 = region
    out = set()
    for y in range(y0, y1, 2):
        for x in range(x0, x1, 2):
            if in_hud(x, y):
                continue
            pixel = image.getpixel((x, y))
            if sum(abs(pixel[i] - floor[i]) for i in range(3)) > FOREGROUND:
                out.add((x, y))
    return out


def potion_pixels(image: Image.Image, region, colour: str) -> list[tuple[int, int]]:
    x0, y0, x1, y1 = region
    out = []
    for y in range(y0, y1, 2):
        for x in range(x0, x1, 2):
            # The next potion of the same colour is already back in the main hand.
            if in_hud(x, y) or (x >= HELD_BOTTLE[0] and y >= HELD_BOTTLE[1]):
                continue
            r, g, b = image.getpixel((x, y))
            if colour == "blue" and b > r + 50 and b > g + 25:
                out.append((x, y))
            elif colour == "red" and r > g + 70 and r > b + 70:
                out.append((x, y))
    return out


def check(java: Path, native: Path) -> dict:
    results = {}
    for shot, (row, region, min_iou, (low, high)) in SILHOUETTES.items():
        frames = [load(receipt, shot) for receipt in (java, native)]
        masks = [foreground(frame, region, floor_colour(frame, region)) for frame in frames]
        union = len(masks[0] | masks[1])
        iou = len(masks[0] & masks[1]) / union if union else 1.0
        ratio = len(masks[1]) / len(masks[0]) if masks[0] else float("inf")
        results[shot] = {
            "row": row, "metric": "silhouette", "iou": round(iou, 3), "area_ratio": round(ratio, 3),
            "java_area": len(masks[0]), "native_area": len(masks[1]),
            "pass": bool(masks[0]) and iou >= min_iou and low <= ratio <= high,
            "limits": {"min_iou": min_iou, "area_ratio": [low, high]},
        }
    for shot, (row, colour, region, (low, high), min_spread) in PARTICLES.items():
        pixels = [potion_pixels(load(receipt, shot), region, colour) for receipt in (java, native)]
        spread = [(max(x for x, _ in p) - min(x for x, _ in p)) if p else 0 for p in pixels]
        ratio = len(pixels[1]) / len(pixels[0]) if pixels[0] else float("inf")
        spread_ratio = spread[1] / spread[0] if spread[0] else 0.0
        results[shot] = {
            "row": row, "metric": f"{colour} particle pixels", "count_ratio": round(ratio, 3),
            "java_count": len(pixels[0]), "native_count": len(pixels[1]),
            "spread_ratio": round(spread_ratio, 3), "java_spread": spread[0], "native_spread": spread[1],
            "pass": bool(pixels[0]) and low <= ratio <= high and spread_ratio >= min_spread,
            "limits": {"count_ratio": [low, high], "min_spread_ratio": min_spread},
        }
    return results


def panels(java: Path, native: Path, out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    for shot in [*SILHOUETTES, *PARTICLES]:
        pair = Image.new("RGB", (SIZE[0] * 2, SIZE[1]))
        pair.paste(load(java, shot), (0, 0))
        pair.paste(load(native, shot), (SIZE[0], 0))
        pair.resize((SIZE[0], SIZE[1] // 2)).save(out / f"{shot}.png")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("java", type=Path)
    parser.add_argument("native", type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--panels", type=Path)
    args = parser.parse_args()
    results = check(args.java, args.native)
    verdicts = {shot: result["pass"] for shot, result in results.items()}
    report = {"java": str(args.java), "native": str(args.native), "verdicts": verdicts, "shots": results}
    text = json.dumps(report, indent=2)
    if args.report:
        args.report.write_text(text + "\n")
    if args.panels:
        panels(args.java, args.native, args.panels)
    print(text)
    return 0 if all(verdicts.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
