#!/usr/bin/env python3
"""Contact sheets and metrics for a LOD lab capture (yarra-app-game --lod-lab-capture DIR).

Usage: uv run --with numpy --with pillow python tools/lod_lab_sheet.py DIR

Writes into DIR:
  sheet-bXXX.png  per bearing: every switch at 0.8-1.2x its distance, the representation on
                  each side forced, the game's own choice, and |before - after| x4, with
                  metrics; then the game's choice at each fixed distance
  strip.png       per switch, the game's choice in 1.25% steps through it, with the mean
                  brightness of the tree's box (with LODs faded over time, a step there is
                  expected: the dissolve happens in time, not across distance)
  fade.png        per switch, frames over time as the camera crosses it and comes back, with
                  the tree's coverage and its shadow as shares of the steady frame before it
                  (* marks frames taken while fading)
  sway.png        just past the impostor switch and at 1.75x it, the last mesh LOD and the
                  impostor sampled in turn for a few seconds: how far the top of each sways
  metrics.json    the numbers behind the sheets

Each frame is compared with the frame of the same pose with the tree hidden: the pixels that
differ are the tree (inside its projected box) and its shadow (outside it). Crops wider than
a sheet column are reduced; smaller ones are enlarged with nearest-neighbour sampling, so
pixels stay visible.
"""
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROW = 300          # sheet row height
STRIP = 220        # strip frame height
DIFF = 8           # 8-bit channel difference that counts as the tree or its shadow
FLAGS = {'coverage': 0.10, 'surface': 0.08, 'colour': 6.0, 'shadow': 0.25}


FONTS = ['/System/Library/Fonts/Supplemental/Arial Unicode.ttf', '/System/Library/Fonts/Supplemental/Arial.ttf',
         '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf']


def font(size):
    for path in FONTS:
        if Path(path).exists():
            return ImageFont.truetype(path, size)
    try:
        return ImageFont.load_default(size=size)
    except TypeError:
        return ImageFont.load_default()


def load(folder, frame):
    return np.asarray(Image.open(folder / frame['file']).convert('RGB'), np.float32)


def linear(rgb):
    c = rgb / 255
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def lab(rgb):
    """Mean sRGB colour (0-255) to CIELAB, D65."""
    l = linear(np.asarray(rgb, np.float32))
    xyz = l @ np.array([[0.4124, 0.2126, 0.0193], [0.3576, 0.7152, 0.1192], [0.1805, 0.0722, 0.9505]])
    xyz = xyz / np.array([0.9505, 1.0, 1.089])
    f = np.where(xyz > 0.008856, np.cbrt(xyz), 7.787 * xyz + 16 / 116)
    return np.array([116 * f[1] - 16, 500 * (f[0] - f[1]), 200 * (f[1] - f[2])])


def luminance(img):
    return linear(img) @ np.array([0.2126, 0.7152, 0.0722])


def measure(img, hidden, box):
    """The tree's coverage of its box, its mean colour and brightness, and its shadow."""
    h, w = img.shape[:2]
    differs = np.abs(img - hidden).max(-1) > DIFF
    x, y, bw, bh = (int(round(v)) for v in box)
    inside = np.zeros((h, w), bool)
    inside[max(y, 0):min(y + bh, h), max(x, 0):min(x + bw, w)] = True
    tree = differs & inside
    area = max(inside.sum(), 1)
    darkening = np.clip(luminance(hidden) - luminance(img), 0, None)
    out = {
        'coverage': float(tree.sum() / area),
        'shadow': float(darkening[differs & ~inside].sum() / area),
    }
    if tree.sum() > 0:
        colour = img[tree].mean(0)
        out['colour'] = colour.tolist()
        out['brightness'] = float(luminance(img)[tree].mean())
    return out


def top_x(img, hidden, box):
    """The horizontal centre of the tree's top fifth, in pixels across its box."""
    x, y, bw, bh = (int(round(v)) for v in box)
    x0, y0 = max(x, 0), max(y, 0)
    tree = (np.abs(img - hidden).max(-1) > DIFF)[y0:y + bh, x0:x + bw]
    rows = np.nonzero(tree.any(1))[0]
    if len(rows) == 0:
        return None
    band = tree[rows[0]:rows[0] + max(1, (rows[-1] - rows[0]) // 5)]
    return float(np.nonzero(band)[1].mean()) + x0 - x


def sway_series(frames, hidden, get):
    """The top's sway of the mesh LOD and the impostor sampled in turn at one distance."""
    series = {}
    for f in sorted(frames, key=lambda f: f['clock']):
        x = top_x(get(f), get(hidden), f['tree_box'])
        if x is not None:
            series.setdefault(f['role'], []).append((f['clock'], x, f['representation']))
    if set(series) != {'mesh', 'impostor'}:
        return None
    start = min(v[0][0] for v in series.values())
    base = np.mean([x for _, x, _ in series['mesh']])
    out = {role: {'representation': v[0][2], 't': [t - start for t, _, _ in v], 'top_x': [x - base for _, x, _ in v]}
           for role, v in series.items()}
    for v in out.values():
        v['mean'] = float(np.mean(v['top_x']))
        v['sway'] = float(np.std(v['top_x']))
    n = min(len(out['mesh']['top_x']), len(out['impostor']['top_x']))
    a, b = np.array(out['mesh']['top_x'][:n]), np.array(out['impostor']['top_x'][:n])
    out['correlation'] = float(np.corrcoef(a, b)[0, 1]) if n > 2 and a.std() > 0 and b.std() > 0 else None
    out['distance'] = hidden['distance']
    out['factor'] = hidden['factor']
    return out


def sway_sheet(folder, header, frames, get):
    """Plots the top's sway of the mesh LOD and the impostor at each sway distance."""
    panels = []
    for hidden in (f for f in frames if f['kind'] == 'sway' and f['role'] == 'hidden'):
        poses = [f for f in frames if f['kind'] == 'sway' and f['role'] != 'hidden'
                 and abs(f['distance'] - hidden['distance']) < 1e-3]
        out = sway_series(poses, hidden, get)
        if out:
            panels.append(out)
    if not panels:
        return None
    width, height, pad = 1400, 520, 60
    sheet = Image.new('RGB', (width, height * len(panels)), (24, 24, 24))
    draw = ImageDraw.Draw(sheet)
    title, text = font(22), font(16)
    colours = {'mesh': (235, 235, 235), 'impostor': (255, 160, 40)}
    span = max(1.0, max(abs(v) for out in panels for r in colours for v in out[r]['top_x'])) * 1.15
    for i, out in enumerate(panels):
        top = i * height
        end = max(out['mesh']['t'][-1], out['impostor']['t'][-1]) or 1.0
        px = lambda t: pad + (width - 2 * pad) * t / end
        py = lambda v: top + height / 2 - (height / 2 - pad) * v / span
        draw.line([(pad, py(0)), (width - pad, py(0))], fill=(70, 70, 70))
        for role, colour in colours.items():
            draw.line([(px(t), py(v)) for t, v in zip(out[role]['t'], out[role]['top_x'])], fill=colour, width=3)
        corr = out['correlation']
        draw.text((pad, top + 14), f"{header['asset']}  treetop at {out['distance']:.1f} m ({out['factor']:.2f}× "
                  f"the impostor switch): px from the mesh's mean, {end:.1f} s", fill=(235, 235, 235), font=title)
        for k, (role, colour) in enumerate(colours.items()):
            v = out[role]
            draw.text((pad + 380 * k, top + height - 40),
                      f"{v['representation']}: mean {v['mean']:+.1f} px, sway ±{v['sway']:.1f} px", fill=colour, font=text)
        draw.text((pad + 760, top + height - 40), f"correlation {corr:.2f}" if corr is not None else 'correlation -',
                  fill=(200, 200, 200), font=text)
        draw.text((8, py(span / 1.15) - 8), f"{span / 1.15:+.0f}", fill=(150, 150, 150), font=text)
        draw.text((8, py(-span / 1.15) - 8), f"{-span / 1.15:+.0f}", fill=(150, 150, 150), font=text)
    sheet.save(folder / 'sway.png')
    return panels


def erode(mask, steps=2):
    for _ in range(steps):
        p = np.pad(mask, 1)
        mask = mask & p[:-2, 1:-1] & p[2:, 1:-1] & p[1:-1, :-2] & p[1:-1, 2:]
    return mask


def surface(before, after, hidden):
    """Mean brightness of each image where both show the tree and neither is near an edge:
    the surfaces themselves, without the background showing through thin foliage."""
    core = erode((np.abs(before - hidden).max(-1) > DIFF) & (np.abs(after - hidden).max(-1) > DIFF))
    if core.sum() < 16:
        return None
    return float(luminance(before)[core].mean()), float(luminance(after)[core].mean())


def compare(before, after, core=None):
    out = {}
    if core:
        out['surface'] = core[1] / core[0] - 1
    if before['coverage'] > 0:
        out['coverage'] = after['coverage'] / before['coverage'] - 1
    if 'brightness' in before and 'brightness' in after and before['brightness'] > 0:
        out['brightness'] = after['brightness'] / before['brightness'] - 1
        out['colour'] = float(np.linalg.norm(lab(after['colour']) - lab(before['colour'])))
    if before['shadow'] > 1e-4:
        out['shadow'] = after['shadow'] / before['shadow'] - 1
    out['flags'] = [k for k, limit in FLAGS.items() if abs(out.get(k, 0)) > limit]
    return out


def fit(img, height, width=None):
    """Scales a crop to `height` (and at most `width`): nearest when enlarging."""
    im = Image.fromarray(np.clip(img, 0, 255).astype(np.uint8))
    scale = height / im.height
    if width:
        scale = min(scale, width / im.width)
    size = (max(1, round(im.width * scale)), max(1, round(im.height * scale)))
    return im.resize(size, Image.Resampling.NEAREST if scale > 1 else Image.Resampling.LANCZOS)


def label(rep):
    return rep if rep != 'auto' else 'game'


def describe(change):
    parts = []
    for key, unit in (('coverage', '%'), ('brightness', '%'), ('surface', '%'), ('colour', ''), ('shadow', '%')):
        if key in change:
            v = change[key]
            parts.append(f"{key} {v * 100:+.0f}%" if unit else f"ΔE {v:.1f}")
    return '  '.join(parts)


def main():
    folder = Path(sys.argv[1])
    manifest = json.loads((folder / 'manifest.json').read_text())
    header, frames = manifest['header'], manifest['frames']
    key = lambda f: (round(f['bearing'], 2), round(f['distance'], 3))
    hidden = {key(f): f for f in frames if f['role'] == 'hidden'}
    images = {}

    def get(frame):
        if frame['file'] not in images:
            images[frame['file']] = load(folder, frame)
        return images[frame['file']]

    metrics = {'header': header, 'switches': [], 'fixed': []}
    bands = header['bands']
    title_font, text_font = font(22), font(16)
    for bearing in sorted({round(f['bearing'], 2) for f in frames if f['kind'] != 'strip'}):
        rows = {}
        for f in frames:
            if f['kind'] == 'switch' and round(f['bearing'], 2) == bearing:
                rows.setdefault((f['switch'], f['factor']), {})[f['role']] = f
        fixed = [f for f in frames if f['kind'] == 'fixed' and f['role'] == 'auto' and round(f['bearing'], 2) == bearing]
        lines = []
        for (switch, factor), roles in sorted(rows.items()):
            if not all(r in roles for r in ('hidden', 'before', 'after', 'auto')):
                continue
            back = get(roles['hidden'])
            m = {r: measure(get(roles[r]), back, roles[r]['tree_box']) for r in ('before', 'after', 'auto')}
            change = compare(m['before'], m['after'], surface(get(roles['before']), get(roles['after']), back))
            d = roles['auto']
            entry = {'bearing': bearing, 'switch': switch, 'factor': factor, 'distance': d['distance'],
                     'from': roles['before']['representation'], 'to': roles['after']['representation'],
                     'height_px_physical': d['height_px_physical'],
                     'impostor_texels_per_pixel': d['impostor_texels_per_pixel'],
                     'measures': m, 'change': change}
            metrics['switches'].append(entry)
            diff = np.abs(get(roles['before']) - get(roles['after'])) * 4
            tiles = [fit(get(roles[r]), ROW, 420) for r in ('before', 'after', 'auto')] + [fit(diff, ROW, 420)]
            text = [f"{entry['from']} → {entry['to']}  at {factor:.1f}× = {d['distance']:.1f} m",
                    f"{d['height_px']:.0f} px tall ({d['height_px_physical']:.0f} physical)",
                    f"game draws: {' + '.join(d['drawn'])}"]
            if d['impostor_texels_per_pixel'] is not None and 'impostor' in (entry['from'], entry['to']):
                ratio = d['impostor_texels_per_pixel']
                text.append(f"impostor {ratio:.2f} texels/px{' (magnified)' if ratio < 1 else ''}")
            text.append(describe(change))
            if change['flags']:
                text.append('CHECK: ' + ', '.join(change['flags']))
            lines.append((tiles, text, bool(change['flags'])))
        for f in fixed:
            if key(f) in hidden:
                metrics['fixed'].append({'bearing': bearing, 'distance': f['distance'], 'drawn': f['drawn'],
                                         'measure': measure(get(f), get(hidden[key(f)]), f['tree_box'])})
        text_width = 470
        width = max([text_width + sum(t.width + 8 for t in tiles) for tiles, _, _ in lines] + [1200])
        fixed_tiles = [(fit(get(f), ROW, 360), f) for f in fixed]
        fixed_width = sum(t.width + 8 for t, _ in fixed_tiles) + 16
        width = max(width, fixed_width)
        height = 60 + len(lines) * (ROW + 16) + (ROW + 90 if fixed_tiles else 0)
        sheet = Image.new('RGB', (width, height), (24, 24, 24))
        draw = ImageDraw.Draw(sheet)
        draw.text((16, 14), f"{header['asset']}  bearing {bearing:.0f}° from the sun  ·  "
                  f"{header['render']['upscaler']} {header['render']['msaa']} scale {header['render']['resolution_scale']}  ·  "
                  f"columns: before, after, game, |before − after| ×4", fill=(235, 235, 235), font=title_font)
        y = 60
        for tiles, text, flagged in lines:
            draw.multiline_text((16, y + 8), '\n'.join(text), fill=(255, 170, 120) if flagged else (220, 220, 220),
                                font=text_font, spacing=6)
            x = text_width
            for tile in tiles:
                sheet.paste(tile, (x, y))
                x += tile.width + 8
            y += ROW + 16
        if fixed_tiles:
            draw.text((16, y + 8), "game's choice at fixed distances", fill=(235, 235, 235), font=title_font)
            x, y = 16, y + 44
            for tile, f in fixed_tiles:
                sheet.paste(tile, (x, y))
                draw.text((x, y + ROW + 4), f"{f['distance']:.0f} m · {' + '.join(f['drawn'])}",
                          fill=(220, 220, 220), font=text_font)
                x += tile.width + 8
        sheet.save(folder / f"sheet-b{bearing % 360:03.0f}.png")

    strips = {}
    for f in frames:
        if f['kind'] == 'strip':
            strips.setdefault(f['switch'], []).append(f)
    if strips:
        blocks = []
        for switch, items in sorted(strips.items()):
            items.sort(key=lambda f: f['distance'])
            tiles = [fit(get(f), STRIP, 260) for f in items]
            curve = []
            for f in items:
                x, y, w, h = (int(round(v)) for v in f['tree_box'])
                box = get(f)[max(y, 0):y + h, max(x, 0):x + w]
                curve.append(float(luminance(box).mean()) if box.size else 0.0)
            blocks.append((switch, items, tiles, curve))
            metrics.setdefault('strips', []).append({'switch': switch, 'distances': [f['distance'] for f in items],
                                                     'box_brightness': curve})
        width = max(sum(t.width + 4 for t in tiles) for _, _, tiles, _ in blocks) + 32
        block_h = 40 + STRIP + 30 + 130 + 20
        strip = Image.new('RGB', (width, block_h * len(blocks)), (24, 24, 24))
        draw = ImageDraw.Draw(strip)
        for i, (switch, items, tiles, curve) in enumerate(blocks):
            top = i * block_h
            a, b = bands[switch]['representation'], bands[switch + 1]['representation']
            draw.text((16, top + 10), f"{header['asset']}  {a} → {b} at {header['switches'][switch]:.1f} m: "
                      f"{items[0]['distance']:.1f}–{items[-1]['distance']:.1f} m, the game's choice", fill=(235, 235, 235),
                      font=title_font)
            x = 16
            xs = []
            for tile in tiles:
                strip.paste(tile, (x, top + 40))
                xs.append(x + tile.width / 2)
                x += tile.width + 4
            plot_top = top + 40 + STRIP + 30
            # Change from the first frame on a fixed ±40% axis, so strips compare.
            base = max(curve[0], 1e-6)
            mid = plot_top + 60
            for level in (-0.4, -0.2, 0.0, 0.2, 0.4):
                yy = mid - level * 125
                draw.line([(16, yy), (x, yy)], fill=(60, 60, 60) if level else (110, 110, 110), width=1)
                draw.text((x + 6, yy - 8), f"{level * 100:+.0f}%", fill=(140, 140, 140), font=text_font)
            points = [(px, mid - max(-0.48, min(0.48, v / base - 1)) * 125) for px, v in zip(xs, curve)]
            draw.line(points, fill=(120, 200, 255), width=3)
            for p in points:
                draw.ellipse((p[0] - 3, p[1] - 3, p[0] + 3, p[1] + 3), fill=(120, 200, 255))
            steps = [abs(b / a - 1) for a, b in zip(curve, curve[1:]) if a > 0]
            draw.text((16, plot_top - 24), f"tree box brightness against the first frame: overall "
                      f"{curve[-1] / base - 1:+.0%}, largest step {max(steps, default=0):.1%}",
                      fill=(180, 180, 180), font=text_font)
        strip.save(folder / 'strip.png')

    # The dissolve over time: per switch, the steady side, then the fade out and back.
    fades = {}
    for f in frames:
        if f['kind'] == 'fade':
            fades.setdefault(f['switch'], []).append(f)
    if fades:
        rows = []
        for switch, items in sorted(fades.items()):
            for role in ('out', 'back'):
                seq = sorted((f for f in items if f['role'] == role), key=lambda f: f['t'] or 0)
                steady = [f for f in items if f['role'] == 'before']
                if seq:
                    rows.append((switch, role, steady[:1] + seq if role == 'out' else seq))
        tiles_per_row = [[fit(get(f), STRIP, 260) for f in seq] for _, _, seq in rows]
        # The tree's coverage and its shadow in every frame, against the ground without it.
        measures = [[measure(get(f), get(hidden[key(f)]), f['tree_box']) if key(f) in hidden else None
                     for f in seq] for _, _, seq in rows]
        width = max(sum(t.width + 4 for t in tiles) for tiles in tiles_per_row) + 32
        block_h = 40 + STRIP + 46
        sheet = Image.new('RGB', (width, block_h * len(rows)), (24, 24, 24))
        draw = ImageDraw.Draw(sheet)
        for i, ((switch, role, seq), tiles, ms) in enumerate(zip(rows, tiles_per_row, measures)):
            top = i * block_h
            # Shares of the steady frame before the switch.
            steady = next((m for (sw, _, sq), mm in zip(rows, measures) if sw == switch
                           for f, m in zip(sq, mm) if f['role'] == 'before' and m), None)
            a, b = bands[switch]['representation'], bands[switch + 1]['representation']
            direction = f"{a} → {b}" if role == 'out' else f"{b} → {a}"
            draw.text((16, top + 10), f"{header['asset']}  {direction}: frames as the camera crosses "
                      f"{header['switches'][switch]:.1f} m, time since it crossed", fill=(235, 235, 235), font=title_font)
            x = 16
            for tile, f, m in zip(tiles, seq, ms):
                sheet.paste(tile, (x, top + 40))
                label_text = 'steady' if f['t'] is None else f"{f['t'] * 1000:.0f} ms" + (' *' if f['fading'] else '')
                draw.text((x + 4, top + 40 + STRIP + 4), label_text, fill=(200, 200, 200), font=text_font)
                if m and steady and steady['shadow'] > 1e-4 and steady['coverage'] > 0:
                    draw.text((x + 4, top + 40 + STRIP + 24),
                              f"tree {m['coverage'] / steady['coverage']:.0%} shadow {m['shadow'] / steady['shadow']:.0%}",
                              fill=(160, 160, 160), font=text_font)
                x += tile.width + 4
        sheet.save(folder / 'fade.png')
        metrics['fades'] = [{'switch': sw, 'role': role, 't': [f['t'] for f in seq], 'fading': [f['fading'] for f in seq],
                             'coverage': [m and m['coverage'] for m in ms], 'shadow': [m and m['shadow'] for m in ms]}
                            for (sw, role, seq), ms in zip(rows, measures)]

    sway = sway_sheet(folder, header, frames, get)
    if sway:
        metrics['sway'] = sway

    (folder / 'metrics.json').write_text(json.dumps(metrics, indent=1))
    print(f"{header['asset']}: bands " + ', '.join(
        f"{b['representation']}" + (f" to {sum(b['fade_out']) / 2:.0f} m" if b['fade_out'] else '') for b in bands))
    for e in metrics['switches']:
        if abs(e['factor'] - 1.0) < 1e-3:
            ratio = e['impostor_texels_per_pixel']
            print(f"  bearing {e['bearing']:4.0f}  {e['from']:>8} → {e['to']:<8} {e['distance']:6.1f} m  "
                  f"{e['height_px_physical']:5.0f} px  {describe(e['change'])}"
                  + (f"  impostor {ratio:.2f} tex/px" if ratio is not None and e['to'] == 'impostor' else '')
                  + (f"  CHECK {','.join(e['change']['flags'])}" if e['change']['flags'] else ''))
    for out in sway or []:
        print(f"  sway at {out['distance']:.1f} m ({out['factor']:.2f}×): " + ', '.join(
            f"{out[r]['representation']} mean {out[r]['mean']:+.1f} px ±{out[r]['sway']:.1f}" for r in ('mesh', 'impostor'))
            + (f", correlation {out['correlation']:.2f}" if out['correlation'] is not None else ''))


if __name__ == '__main__':
    main()
