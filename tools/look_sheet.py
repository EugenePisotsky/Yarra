#!/usr/bin/env python3
"""A contact sheet of a look capture (yarra-app-game --look-capture DIR).

Usage: uv run --with numpy --with pillow python tools/look_sheet.py DIR [--width PX] [--crop X0,Y0,X1,Y1]

Writes DIR/sheet.png (or sheet-crop.png with --crop, shares 0-1 of the frame): one tile per
variant, in a grid when names read `row|column`, each labelled with its settings and the
frame's brightness: mean lightness (L*, 0-100) of the land below the horizon band, and the
shares of nearly white and nearly black pixels. Prints the same numbers.
"""
import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

FONTS = ['/System/Library/Fonts/Supplemental/Arial Unicode.ttf', '/System/Library/Fonts/Supplemental/Arial.ttf',
         '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf']


def font(size):
    for path in FONTS:
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    return ImageFont.load_default()


def lightness(rgb):
    """CIE L* of 8-bit sRGB."""
    c = rgb.astype(np.float32) / 255
    linear = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    y = linear @ np.array([0.2126, 0.7152, 0.0722], np.float32)
    return np.where(y > 0.008856, 116 * np.cbrt(y) - 16, 903.3 * y)


def stats(img):
    """Brightness of the frame's lower 55%, which holds the land in every capture view."""
    lower = np.asarray(img)[int(img.height * 0.45):]
    l = lightness(lower)
    return {'land_lightness': float(l.mean()), 'land_p90': float(np.percentile(l, 90)),
            'white': float((lower.min(-1) >= 245).mean()), 'black': float((lower.max(-1) <= 12).mean())}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('dir', type=Path)
    parser.add_argument('--width', type=int, default=1100, help='tile width in pixels')
    parser.add_argument('--crop', help='X0,Y0,X1,Y1 as shares of the frame')
    args = parser.parse_args()
    manifest = json.loads((args.dir / 'manifest.json').read_text())
    frames = manifest['frames']
    rows, columns = [], []
    for f in frames:
        row, _, column = f['name'].partition('|')
        f['row'], f['column'] = (row, column) if column else ('', f['name'])
        if f['row'] not in rows:
            rows.append(f['row'])
        if f['column'] not in columns:
            columns.append(f['column'])
    if rows == ['']:
        per_row = min(len(columns), 3)
        for i, f in enumerate(frames):
            f['row'], f['column'] = i // per_row, i % per_row
        rows, columns = sorted({f['row'] for f in frames}), list(range(per_row))

    tiles = {}
    for f in frames:
        img = Image.open(args.dir / f['file']).convert('RGB')
        f['stats'] = stats(img)
        if args.crop:
            x0, y0, x1, y1 = (float(v) for v in args.crop.split(','))
            img = img.crop((int(x0 * img.width), int(y0 * img.height), int(x1 * img.width), int(y1 * img.height)))
        h = round(img.height * args.width / img.width)
        tiles[(f['row'], f['column'])] = (img.resize((args.width, h), Image.LANCZOS), f)
    tile_h = max(t.height for t, _ in tiles.values())
    label_h, pad = 64, 8
    sheet = Image.new('RGB', (len(columns) * (args.width + pad) + pad, len(rows) * (tile_h + label_h + pad) + pad),
                      (24, 24, 24))
    draw = ImageDraw.Draw(sheet)
    title, text = font(24), font(18)
    for (row, column), (tile, f) in tiles.items():
        x = pad + columns.index(column) * (args.width + pad)
        y = pad + rows.index(row) * (tile_h + label_h + pad)
        sheet.paste(tile, (x, y))
        s = f['stats']
        draw.text((x + 4, y + tile.height + 4), f['name'], fill=(240, 240, 240), font=title)
        draw.text((x + 4, y + tile.height + 34),
                  f"EV {f['ev100']:.1f}  {f['tonemapping']}  sky ×{f['ambient']:g}  sun ×{f['sun']:g}  "
                  f"canopy {f.get('canopy', 0):g}  auto {'on' if f.get('auto_exposure') else 'off'}  "
                  f"fog {'on' if f.get('fog', True) else 'off'}  phase {f.get('phase', 0):.2f}   "
                  f"land L* {s['land_lightness']:.0f} (p90 {s['land_p90']:.0f})  white {s['white']:.1%}",
                  fill=(170, 170, 170), font=text)
    out = args.dir / ('sheet-crop.png' if args.crop else 'sheet.png')
    sheet.save(out)
    for f in frames:
        s = f['stats']
        print(f"{f['name']:24s} EV {f['ev100']:4.1f} {f['tonemapping']:16s} sky x{f['ambient']:<4g} sun x{f['sun']:<4g} "
              f"land L* {s['land_lightness']:5.1f} p90 {s['land_p90']:5.1f} white {s['white']:6.2%} black {s['black']:6.2%}")
    print(f"→ {out}")


if __name__ == '__main__':
    main()
