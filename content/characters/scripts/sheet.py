#!/usr/bin/env python3
"""A contact sheet of render_views.py's output: each variant's views side by side, labelled.

Usage: uv run --with pillow python content/characters/scripts/sheet.py DIR [--columns N]
  [--height PX]

Writes DIR/sheet.png: variants in reading order, N to a row, each view scaled to --height pixels
on a mid-grey ground (renders have transparent backgrounds).
"""
import argparse
import json
import textwrap
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

GROUND = (118, 118, 118)
INK = (255, 255, 255)
GAP = 12
LABEL = 54


def font(size):
    for path in ("/System/Library/Fonts/Supplemental/Arial.ttf", "/System/Library/Fonts/Helvetica.ttc"):
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue

    return ImageFont.load_default()


def cell(folder, views, height, label, note):
    tiles = []

    for view in views:
        image = Image.open(folder / f"{view}.png").convert("RGBA")
        width = round(image.width * height / image.height)
        tiles.append(image.resize((width, height), Image.LANCZOS))

    width = sum(tile.width for tile in tiles) + GAP * (len(tiles) - 1)
    out = Image.new("RGB", (width, height + LABEL), GROUND)
    draw = ImageDraw.Draw(out)
    draw.text((4, 2), label, fill=INK, font=font(22))
    line = textwrap.shorten(note, width=max(20, width // 9))
    draw.text((4, 28), line, fill=INK, font=font(15))
    x = 0

    for tile in tiles:
        out.paste(tile, (x, LABEL), tile)
        x += tile.width + GAP

    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("dir", type=Path)
    parser.add_argument("--columns", type=int, default=1)
    parser.add_argument("--height", type=int, default=360)
    args = parser.parse_args()
    manifest = json.loads((args.dir / "manifest.json").read_text())
    cells = [
        cell(args.dir / variant["label"], manifest["views"], args.height, variant["label"], variant["note"])
        for variant in manifest["variants"]
    ]
    rows = [cells[i : i + args.columns] for i in range(0, len(cells), args.columns)]
    width = max(sum(c.width for c in row) + GAP * (len(row) - 1) for row in rows) + 2 * GAP
    height = sum(max(c.height for c in row) for row in rows) + GAP * (len(rows) + 1)
    sheet = Image.new("RGB", (width, height), GROUND)
    y = GAP

    for row in rows:
        x = GAP

        for c in row:
            sheet.paste(c, (x, y))
            x += c.width + GAP

        y += max(c.height for c in row) + GAP

    sheet.save(args.dir / "sheet.png")
    print(f"wrote {args.dir / 'sheet.png'} ({width}x{height})")


if __name__ == "__main__":
    main()
