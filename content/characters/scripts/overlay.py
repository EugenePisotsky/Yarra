#!/usr/bin/env python3
"""Both bodies of a fit drawn over each other, as fit_body.py matched them.

Usage: uv run --with numpy --with pillow python content/characters/scripts/overlay.py POINTS.npz OUT.png

Front and side views of the model's points (red) and the fitted MPFB body's (cyan), where they
coincide pale, at one scale: what the fit's distances measure.
"""
import argparse
from pathlib import Path

import numpy as np
from PIL import Image

HEIGHT = 1000
MARGIN = 0.05
MODEL = np.array([235, 70, 70])
FITTED = np.array([90, 210, 215])
BOTH = np.array([235, 235, 235])
GROUND = np.array([110, 110, 110])


def splat(points, horizontal, scale, size, origin):
    """Which pixels the points cover, each point a small disc."""
    mask = np.zeros(size[::-1], dtype=bool)
    x = ((points[:, horizontal] - origin[0]) * scale).astype(int)
    y = (size[1] - 1 - (points[:, 2] - origin[1]) * scale).astype(int)

    for dx in range(-2, 3):
        for dy in range(-2, 3):
            if dx * dx + dy * dy <= 5:
                xs, ys = np.clip(x + dx, 0, size[0] - 1), np.clip(y + dy, 0, size[1] - 1)
                mask[ys, xs] = True

    return mask


def view(model, fitted, horizontal):
    both = np.vstack([model, fitted])
    low, high = both.min(axis=0) - MARGIN, both.max(axis=0) + MARGIN
    scale = HEIGHT / (high[2] - low[2])
    size = (int((high[horizontal] - low[horizontal]) * scale) + 1, HEIGHT)
    origin = (low[horizontal], low[2])
    a = splat(model, horizontal, scale, size, origin)
    b = splat(fitted, horizontal, scale, size, origin)
    image = np.tile(GROUND, (HEIGHT, size[0], 1))
    image[a] = MODEL
    image[b] = FITTED
    image[a & b] = BOTH
    return image.astype(np.uint8)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("points", type=Path)
    parser.add_argument("out", type=Path)
    args = parser.parse_args()
    data = np.load(args.points)
    model, fitted = data["model"], data["fitted"]
    front, side = view(model, fitted, 0), view(model, fitted, 1)
    gap = np.full((HEIGHT, 20, 3), GROUND, dtype=np.uint8)
    Image.fromarray(np.hstack([front, gap, side])).save(args.out)
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
