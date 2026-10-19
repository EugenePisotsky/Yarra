"""Start views facing each mountain from open low ground about 1 km away: line of sight to the
upper face, no forest around the player or in the first stretch toward the mountain.
Usage: mountain_views.py HEIGHT_MANIFEST FOREST_MANIFEST VIEW_DIR"""
import json
import math
import sys
from pathlib import Path

import numpy as np

hm, fm, views = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
H = json.loads(hm.read_text()); F = json.loads(fm.read_text())
nx, nz = H['samples']; ox, oz = H['origin']
h = np.memmap(Path(H['heights']) if Path(H['heights']).is_absolute() else hm.parent / H['heights'], dtype='<f4', mode='r', shape=(nz, nx))
resolve = lambda f: Path(f) if Path(f).is_absolute() else fm.parent / f
forest = sum(np.memmap(resolve(next(e['file'] for e in F['masks'] if e['name'] == k)), dtype='u1', mode='r', shape=(nz, nx))[::4, ::4].astype('f4')
             for k in ('pine_forest', 'spruce_forest', 'broadleaf_forest')) / 255
PEAKS = {  # name: (x, z) of the summit, found from the island heights
    'starter': (2478, 2112), 'central': (120, -247), 'north': (-1409, -1975), 'west': (-2468, -251), 'southwest': (-2737, 1482)}

def height(x, z):
    j = int(np.clip(round(x - ox), 0, nx - 1)); i = int(np.clip(round(z - oz), 0, nz - 1)); return float(h[i, j])

def wooded(x, z, r):
    j = int((x - ox) / 4); i = int((z - oz) / 4); k = max(1, int(r / 4))
    return float(forest[max(0, i - k):i + k + 1, max(0, j - k):j + k + 1].mean())

def clear(x0, z0, y0, x1, z1, y1):
    n = int(math.hypot(x1 - x0, z1 - z0) / 2)
    for t in np.linspace(.01, .97, n):
        x, z, y = x0 + (x1 - x0) * t, z0 + (z1 - z0) * t, y0 + (y1 - y0) * t
        if height(x, z) > y - 1.5:
            return False
        if t * math.hypot(x1 - x0, z1 - z0) < 160 and wooded(x, z, 8) > .15:
            return False
    return True

for name, (px, pz) in PEAKS.items():
    top = height(px, pz); best = None
    for dist in range(800, 1700, 100):
        for a in range(0, 360, 4):
            x = px + dist * math.sin(math.radians(a)); z = pz + dist * math.cos(math.radians(a))
            g = height(x, z)
            if not (2 < g < 90) or wooded(x, z, 30) > .05:
                continue
            eye = g + 4
            # The upper face: the summit and a point a third of the way down toward the viewer.
            mx, mz = px + (x - px) * .25, pz + (z - pz) * .25
            if not (clear(x, z, eye, px, pz, top + 2) and clear(x, z, eye, mx, mz, height(mx, mz) + 2)):
                continue
            score = -abs(dist - 1100) - 40 * wooded(x, z, 60)
            if best is None or score > best[0]:
                best = (score, x, z, g, dist)
    if best is None:
        print(f'{name}: no clear vantage'); continue
    _, x, z, g, dist = best
    yaw = math.degrees(math.atan2(-(px - x), -(pz - z)))
    tilt = math.degrees(math.atan2(top * .7 - g, dist))
    (views / f'{name}-far.ron').write_text(
        f'(position: ({x:.1f}, {g:.3f}, {z:.1f}), yaw_degrees: {yaw:.1f}, pitch_degrees: 6.0, distance: 9.7, fog_visibility: 20000.0, route: [])\n')
    print(f'{name}-far: ({x:.0f}, {z:.0f}) ground {g:.0f} m, {dist} m from the {top:.0f} m summit, yaw {yaw:.1f}, capture tilt ~{tilt:.0f}')
