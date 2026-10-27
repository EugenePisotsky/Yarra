#!/usr/bin/env python3
"""Writes a review spec of faces drawn from MPFB's face sliders, for render_views.py.

Usage: python3 content/characters/scripts/face_variants.py [--count N] [--seed S] [--spread X]
  [--out content/characters/reviews/faces.json]

Each face draws every slider below from a bell around zero (standard deviation --spread, clipped
at twice that) and keeps the preset's body. A slider is a pair of MPFB targets, the first taken
for negative values; "l-"/"r-" pairs get one value, so faces stay symmetric. A face picked from the
sheet is kept by copying its "targets" into a preset.
"""
import argparse
import json
import random
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]

SLIDERS = [
    ("head-scale-horiz-decr", "head-scale-horiz-incr"),
    ("head-scale-vert-decr", "head-scale-vert-incr"),
    ("head-fat-decr", "head-fat-incr"),
    ("forehead-scale-vert-decr", "forehead-scale-vert-incr"),
    ("forehead-temple-decr", "forehead-temple-incr"),
    ("eyebrows-angle-down", "eyebrows-angle-up"),
    ("eyebrows-trans-down", "eyebrows-trans-up"),
    ("eye-scale-decr", "eye-scale-incr"),
    ("eye-trans-in", "eye-trans-out"),
    ("eye-trans-down", "eye-trans-up"),
    ("eye-corner1-down", "eye-corner1-up"),
    ("eye-corner2-down", "eye-corner2-up"),
    ("eye-height2-decr", "eye-height2-incr"),
    ("eye-eyefold-angle-down", "eye-eyefold-angle-up"),
    ("eye-bag-decr", "eye-bag-incr"),
    ("nose-scale-horiz-decr", "nose-scale-horiz-incr"),
    ("nose-scale-vert-decr", "nose-scale-vert-incr"),
    ("nose-scale-depth-decr", "nose-scale-depth-incr"),
    ("nose-hump-decr", "nose-hump-incr"),
    ("nose-curve-concave", "nose-curve-convex"),
    ("nose-point-width-decr", "nose-point-width-incr"),
    ("nose-nostrils-width-decr", "nose-nostrils-width-incr"),
    ("nose-point-down", "nose-point-up"),
    ("mouth-scale-horiz-decr", "mouth-scale-horiz-incr"),
    ("mouth-lowerlip-volume-decr", "mouth-lowerlip-volume-incr"),
    ("mouth-upperlip-volume-decr", "mouth-upperlip-volume-incr"),
    ("mouth-cupidsbow-decr", "mouth-cupidsbow-incr"),
    ("mouth-angles-down", "mouth-angles-up"),
    ("mouth-trans-backward", "mouth-trans-forward"),
    ("chin-prominent-decr", "chin-prominent-incr"),
    ("chin-width-decr", "chin-width-incr"),
    ("chin-height-decr", "chin-height-incr"),
    ("chin-bones-decr", "chin-bones-incr"),
    ("chin-prognathism-decr", "chin-prognathism-incr"),
    ("cheek-bones-decr", "cheek-bones-incr"),
    ("cheek-volume-decr", "cheek-volume-incr"),
    ("cheek-inner-decr", "cheek-inner-incr"),
]

# Sliders written without a side apply to both: the eyes' and cheeks' targets come in pairs.
SIDED = ("eye-", "cheek-")


def targets_for(slider, value):
    negative, positive = slider
    name = negative if value < 0 else positive
    names = [f"l-{name}", f"r-{name}"] if name.startswith(SIDED) else [name]
    return [{"target": target, "value": round(abs(value), 3)} for target in names]


def face(rng, spread):
    targets = []

    for slider in SLIDERS:
        value = max(-2 * spread, min(2 * spread, rng.gauss(0.0, spread)))

        if abs(value) >= 0.02:
            targets.extend(targets_for(slider, value))

    return targets


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--count", type=int, default=15)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--spread", type=float, default=0.45)
    parser.add_argument("--out", type=Path, default=ROOT / "content/characters/reviews/faces.json")
    args = parser.parse_args()
    rng = random.Random(args.seed)
    variants = [{"label": "preset face"}]

    for number in range(1, args.count + 1):
        variants.append({"label": f"face {number:02}", "human": {"targets": face(rng, args.spread)}})

    spec = {
        "preset": "content/characters/presets/probe_female.json",
        "views": ["face_front", "face_three_quarter", "face_profile"],
        "variants": variants,
    }
    args.out.write_text(json.dumps(spec, indent=1) + "\n")
    print(f"wrote {len(variants)} faces to {args.out}")


if __name__ == "__main__":
    main()
