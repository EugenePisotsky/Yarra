#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy>=2", "pillow>=10", "scipy>=1.13"]
# ///
"""Render a hand-coloured engraved world map from a terrain export.

    uv run tools/render_world_map.py MANIFEST MAP_JSON OUTPUT.png [--size 3072] [--seed 11]

MANIFEST is a `yarra-heightfield` version 2 export, the file `yarra-world-cook
import-heightfield` reads. Its optional `wetness` mask finds rivers and `soil` places
woods. MAP_JSON names the title, features and places (see content/maps/). The look follows
18th-century hand-coloured engravings, using Homann symbols by K.M. Alexander (CC0) and IM
Fell English lettering (OFL). Both are restored under assets/local/map/ (assets/README.md).

Writes OUTPUT.png and OUTPUT.json; the JSON records the world rectangle the image covers,
with +X to the right and +Z down. Woods are decorative until forests have source data.
"""
import argparse
import json
import math
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont
from scipy import ndimage as nd

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / 'assets/local/map'

PAPER = np.array([220, 210, 196], np.float32)
# The land wash multiplies the paper; the rim darkens it inside the coast.
WASH = np.array([0.985, 0.90, 0.86], np.float32)
RIM = np.array([0.90, 0.70, 0.63], np.float32)
INK = np.array([40, 31, 28], np.float32)

# Terrain classes, in metres and degrees.
MOUNTAIN_HEIGHT, MOUNTAIN_SLOPE = 110.0, 12.0
HILL_HEIGHT = 45.0
# Rivers: at least this wide, cut this far below their surroundings, this wet, this low, and
# reaching the sea. Banks and old cuts beside a river are only partly wet.
RIVER_WIDTH, RIVER_DEPTH, RIVER_WETNESS, RIVER_MAX_HEIGHT = 24.0, 1.5, 0.8, 40.0


def smoothstep(a, b, x):
    t = np.clip((x - a) / (b - a), 0, 1)
    return t * t * (3 - 2 * t)


class Canvas:
    """The map's pixel grid over a square of the world, and its noise."""

    def __init__(self, size, world_min, extent, seed):
        self.n, self.world_min, self.extent = size, np.asarray(world_min, float), extent
        self.mpp = extent / size
        # Strokes, icons and lettering are sized for a 3072 px map.
        self.k = size / 3072
        self.rng = np.random.default_rng(seed)
        self.seed = seed

    def noise(self, sigma, salt):
        g = np.random.default_rng((self.seed, salt)).standard_normal((self.n, self.n)).astype(np.float32)
        g = nd.gaussian_filter(g, sigma)
        return g / (g.std() + 1e-9)

    def px(self, x, z):
        return (x - self.world_min[0]) / self.mpp, (z - self.world_min[1]) / self.mpp

    def world(self, px, py):
        return self.world_min[0] + px * self.mpp, self.world_min[1] + py * self.mpp


class Terrain:
    """A heightfield export sampled onto a canvas."""

    def __init__(self, manifest_path):
        self.path = Path(manifest_path)
        m = json.loads(self.path.read_text())
        if m.get('format') != 'yarra-heightfield' or m.get('version') != 2:
            raise SystemExit('expected a yarra-heightfield version 2 manifest')
        self.nx, self.nz = m['samples']
        self.spacing, self.origin = float(m['spacing']), np.asarray(m['origin'], float)
        self.sea_level = float(m['sea_level'])
        base = self.path.parent
        self.heights = np.memmap(base / m['heights'], '<f4', 'r', shape=(self.nz, self.nx))
        self.masks = {mask['name']: np.memmap(base / mask['file'], np.uint8, 'r', shape=(self.nz, self.nx))
                      for mask in m.get('masks', [])}

    def land_bounds(self):
        """World XZ rectangle holding all land, from a coarse look."""
        step = max(1, int(20 / self.spacing))
        land = np.asarray(self.heights[::step, ::step]) > self.sea_level
        rows, cols = np.nonzero(land)
        if len(rows) == 0:
            raise SystemExit('the heightfield has no land')
        lo = self.origin + np.array([cols.min(), rows.min()]) * step * self.spacing
        hi = self.origin + np.array([cols.max(), rows.max()]) * step * self.spacing
        return lo, hi

    def sample(self, data, canvas, scale=1.0):
        """Averages 2×2 samples per pixel so fine sources do not alias."""
        out = np.zeros((canvas.n, canvas.n), np.float32)
        centres = np.arange(canvas.n, dtype=np.float32)
        for oy in (0.25, 0.75):
            for ox in (0.25, 0.75):
                x = (canvas.world_min[0] + (centres + ox) * canvas.mpp - self.origin[0]) / self.spacing
                z = (canvas.world_min[1] + (centres + oy) * canvas.mpp - self.origin[1]) / self.spacing
                zz, xx = np.meshgrid(z, x, indexing='ij')
                out += nd.map_coordinates(data, [zz, xx], order=1, mode='nearest', prefilter=False)
        return out * (scale / 4)


class Brushes:
    def __init__(self, root):
        if not root.is_dir():
            raise SystemExit(f'missing Homann brushes in {root}; see assets/README.md')
        load = lambda folder: [self.alpha(p) for p in sorted((root / folder).glob('*.png'))]
        self.mountains = load('Landforms/Mountains')
        self.trees = load('Flora/Trees')
        self.fields = load('Flora/Fields')
        self.houses = load('Settlements/Houses')
        self.anchorage = load('Cartouches/Anchorages')[0]
        self.rose = self.alpha(root / 'Cartouches/Map Elements/Compass Rose.png')
        self.mountain_fills = [silhouette(a) for a in self.mountains]
        self.tree_fills = [silhouette(a) for a in self.trees]
        wide = [i for i, a in enumerate(self.mountains) if a.shape[1] / a.shape[0] > 1.9]
        self.hills = wide or list(range(len(self.mountains)))
        self.peaks = [i for i in range(len(self.mountains)) if i not in wide]

    @staticmethod
    def alpha(path):
        return np.asarray(Image.open(path).convert('RGBA'), np.float32)[..., 3] / 255


def silhouette(a):
    """What a pictorial symbol hides: from its topmost ink in each column down to its base."""
    solid = nd.binary_closing(a > 0.25, iterations=3)
    top = np.where(solid.any(0), solid.argmax(0), a.shape[0])
    fill = (np.arange(a.shape[0])[:, None] >= top[None]) & solid.any(0)[None]
    return nd.gaussian_filter(nd.binary_erosion(fill, iterations=2).astype(np.float32), 1.0)


def resized(a, width, height):
    return np.asarray(Image.fromarray((a * 255).astype(np.uint8)).resize((width, height), Image.LANCZOS),
                      np.float32) / 255


def poisson(rng, candidates, radius, order=None, limit=None):
    """Greedy dart throwing over candidate pixels; `radius` is a scalar or a per-pixel field."""
    pts = np.argwhere(candidates)
    if len(pts) == 0:
        return []
    idx = rng.permutation(len(pts)) if order is None else np.argsort(-order[candidates])
    rad = radius[candidates] if np.ndim(radius) else np.full(len(pts), float(radius))
    cell = max(4.0, float(np.min(rad)))
    grid, chosen = {}, []
    for i in idx:
        y, x, r = pts[i][0], pts[i][1], rad[i]
        gy, gx, reach = int(y // cell), int(x // cell), int(math.ceil(r / cell)) + 1
        if any((py - y) ** 2 + (px - x) ** 2 < max(r, pr) ** 2
               for dy in range(-reach, reach + 1) for dx in range(-reach, reach + 1)
               for py, px, pr in grid.get((gy + dy, gx + dx), ())):
            continue
        grid.setdefault((gy, gx), []).append((y, x, r))
        chosen.append((int(y), int(x)))
        if limit and len(chosen) >= limit:
            break
    return chosen


class Map:
    def __init__(self, canvas, terrain, brushes, fonts):
        self.c, self.b = canvas, brushes
        n = canvas.n
        self.h = terrain.sample(terrain.heights, canvas) - terrain.sea_level
        wet = terrain.masks.get('wetness')
        soil = terrain.masks.get('soil')
        self.wet = terrain.sample(wet, canvas, 1 / 255) if wet is not None else None
        self.soil = terrain.sample(soil, canvas, 1 / 255) if soil is not None else None
        self.river = self.find_river()
        land = (self.h > 0.3) & ~self.river
        land = nd.binary_opening(land, iterations=1)
        lab, count = nd.label(land)
        self.land = np.isin(lab, 1 + np.flatnonzero(nd.sum(land, lab, range(1, count + 1)) > 60 * canvas.k ** 2))
        self.inside = nd.distance_transform_edt(self.land).astype(np.float32)
        self.outside = nd.distance_transform_edt(~self.land).astype(np.float32)
        self.sdf = self.inside - self.outside
        gz, gx = np.gradient(self.h, canvas.mpp)
        self.slope = np.degrees(np.arctan(np.hypot(gx, gz)))
        self.high = (self.h > MOUNTAIN_HEIGHT) & self.land & (self.slope > MOUNTAIN_SLOPE)
        self.ink = np.zeros((n, n), np.float32)
        self.reserved = np.zeros((n, n), bool)
        self.lettering = Image.new('RGBA', (n, n), (0, 0, 0, 0))
        self.places = []
        k = canvas.k
        font = lambda name, size: ImageFont.truetype(str(fonts / name), max(8, round(size * k)))
        self.fonts = {
            'title': font('IMFeENsc28P.ttf', 132), 'range': font('IMFeENrm28P.ttf', 46),
            'river': font('IMFeENit28P.ttf', 42), 'water': font('IMFeENit28P.ttf', 42),
            'place': font('IMFeENit28P.ttf', 42),
        }

    def find_river(self):
        if self.wet is None:
            return np.zeros_like(self.h, bool)
        local = self.h - nd.gaussian_filter(self.h, 120 / self.c.mpp)
        channel = (local < -RIVER_DEPTH) & (self.wet > RIVER_WETNESS) & (self.h < RIVER_MAX_HEIGHT) & (self.h > -5)
        labels, count = nd.label(channel)
        sizes = nd.sum(channel, labels, range(1, count + 1))
        # Closed wet hollows are not rivers: keep channels that reach the sea.
        reaching = np.unique(labels[nd.binary_dilation(self.h < 0, iterations=3) & channel])
        large = 0.035e6 / self.c.mpp ** 2
        channel = np.isin(labels, [i for i in reaching if i and sizes[i - 1] > large])
        channel = nd.binary_closing(channel, iterations=2)
        return nd.binary_opening(channel, iterations=max(1, round(RIVER_WIDTH / 2 / self.c.mpp)))

    # Paper, wash and coast.

    def paint(self):
        c, n, k = self.c, self.c.n, self.c.k
        yy, xx = np.mgrid[0:n, 0:n].astype(np.float32)
        self.tooth = c.noise(0.7, 3)
        grain = 0.012 * c.noise(80 * k, 1) + 0.01 * c.noise(18 * k, 2) + 0.045 * self.tooth
        fibres = c.noise((0.5, 7 * k), 4)
        grain += 0.025 * fibres
        for fold in (n * 0.34, n * 0.67):
            grain -= 0.045 * np.exp(-((xx - fold) ** 2) / (12.5 * k * k)) - 0.02 * np.exp(-((xx - fold - 4 * k) ** 2) / (8 * k * k))
        grain -= 0.04 * np.exp(-((yy - n * 0.5) ** 2) / (12.5 * k * k)) - 0.02 * np.exp(-((yy - n * 0.5 - 4 * k) ** 2) / (8 * k * k))
        grain -= 0.07 * smoothstep(0.45, 0.75, np.hypot(xx / n - 0.5, yy / n - 0.5))
        image = PAPER[None, None] * (1 + grain)[..., None]
        cover = np.clip(self.sdf + 0.5, 0, 1)
        band = 22 * k * (1 + 0.35 * c.noise(45 * k, 5))
        rim = np.exp(-np.clip(self.sdf, 0, None) / np.maximum(band, 5 * k)) * 0.9
        mult = WASH[None, None] * (1 - rim[..., None]) + RIM[None, None] * rim[..., None]
        mult *= (1 - 0.03 * np.abs(c.noise(1.2, 6)) - 0.025 * c.noise(30 * k, 7))[..., None]
        self.image = image * (1 - cover[..., None]) + image * mult * cover[..., None]
        # Water lining: horizontal engraved strokes along every shore, with ragged ends.
        spacing = 6.5 * k
        phase = yy % spacing
        stroke = np.clip(1 - np.abs(phase - spacing / 2) / (0.85 * k), 0, 1)
        ragged = c.noise((0.8 * k, 25 * k), 8)
        reach = np.clip((62 + 22 * ragged) * k, 20 * k, 110 * k)
        self.ink = np.maximum(self.ink, stroke * ~self.land * np.clip((reach - self.outside) / (5 * k), 0, 1) * 0.85)
        # Coast: a pen line about 3.5 px wide, with pressure varying along it.
        edge = self.sdf + 0.8 * k * c.noise(5 * k, 9) + 0.45 * k * c.noise(1.0 * k, 15)
        width = 3.5 * k * (1 + 0.25 * c.noise(25 * k, 10))
        self.ink = np.maximum(self.ink, np.clip((width / 2 + 0.6 - np.abs(edge)) / 1.2, 0, 1))
        self.yy, self.xx = yy, xx

    # Lettering is laid out before symbols, which then leave room for it.

    def text(self, words, kind, angle=0.0):
        font = self.fonts[kind]
        spacing = {'title': 34, 'range': 2, 'river': 3, 'water': 6, 'place': 2}[kind] * self.c.k
        if kind == 'title':
            words = ' '.join(words.upper())
        w = int(sum(font.getlength(ch) for ch in words) + spacing * len(words) + 24)
        layer = Image.new('RGBA', (w, int(font.size * 1.6)), (0, 0, 0, 0))
        draw, x = ImageDraw.Draw(layer), 12.0
        for ch in words:
            draw.text((x, 4), ch, font=font, fill=tuple(int(v) for v in INK) + (240,))
            x += font.getlength(ch) + spacing
        return layer.rotate(angle, expand=True, resample=Image.BICUBIC)

    def place(self, layer, candidates, where=None):
        """Puts lettering at the first clear candidate (x, z); `where` is 'land' or 'water'."""
        n = self.c.n
        alpha = nd.binary_dilation(np.asarray(layer)[..., 3] > 20, iterations=max(1, int(6 * self.c.k)))
        for x, z in candidates:
            px, py = self.c.px(x, z)
            x0, y0 = int(px - layer.width / 2), int(py - layer.height / 2)
            if x0 < 0 or y0 < 0 or x0 + layer.width > n or y0 + layer.height > n:
                continue
            window = (slice(y0, y0 + layer.height), slice(x0, x0 + layer.width))
            if (self.reserved[window] & alpha).any():
                continue
            on_land = self.land[window][alpha].mean()
            if (where == 'land' and on_land < 0.97) or (where == 'water' and on_land > 0.03):
                continue
            self.lettering.alpha_composite(layer, (x0, y0))
            self.reserved[window] |= nd.binary_dilation(alpha, iterations=max(1, int(10 * self.c.k)))
            return True
        return False

    def along(self, mask, near, radius):
        """Centre and reading angle of `mask` within `radius` m of `near`, if any."""
        py, px = np.nonzero(mask)
        wx, wz = self.c.world(px, py)
        sel = (wx - near[0]) ** 2 + (wz - near[1]) ** 2 < radius ** 2
        if sel.sum() < 10:
            return None
        pts = np.stack([wx[sel], wz[sel]], 1)
        centre = pts.mean(0)
        direction = np.linalg.svd(pts - centre, full_matrices=False)[2][0]
        angle = -math.degrees(math.atan2(direction[1], direction[0]))
        angle = (angle + 90) % 180 - 90
        return centre, angle, direction

    def letter(self, spec):
        lo, hi = self.c.world(0, 0), self.c.world(self.c.n, self.c.n)
        cx = (lo[0] + hi[0]) / 2
        if spec.get('title'):
            layer = self.text(spec['title'], 'title')
            band = (hi[1] - lo[1]) * 0.045
            if not self.place(layer, [(cx, lo[1] + band), (cx, hi[1] - band)], 'water'):
                print(f"no room for the title {spec['title']!r}")
        for feature in spec.get('features', []):
            name, kind, at = feature['name'], feature['kind'], feature['at']
            if not getattr(self, 'letter_' + kind)(name, at, feature):
                print(f'no room for {name!r}')

    def letter_range(self, name, at, _):
        """Below, above or beside the high ground nearest `at`."""
        if not hasattr(self, 'ranges'):
            labels, _ = nd.label(nd.binary_closing(self.h > MOUNTAIN_HEIGHT, iterations=8))
            self.ranges = [(self.c.world(s[1].start, s[0].start), self.c.world(s[1].stop, s[0].stop))
                           for s in nd.find_objects(labels)]
        px, pz = at
        gap = lambda r: math.hypot(max(r[0][0] - px, 0, px - r[1][0]), max(r[0][1] - pz, 0, pz - r[1][1]))
        (x0, z0), (x1, z1) = min(self.ranges, key=gap) if self.ranges else ((px, pz), (px, pz))
        layer = self.text(name, 'range')
        half = layer.width * self.c.mpp / 2
        mx, mz = (x0 + x1) / 2, (z0 + z1) / 2
        return self.place(layer, [(mx, z1 + 160), (mx, z0 - 160), (x0 - half - 120, mz), (x1 + half + 120, mz),
                                  (mx, z1 + 320), (mx, z0 - 320)], 'land')

    def letter_river(self, name, at, _):
        """Beside the river near `at`, reading along it."""
        found = self.along(self.river, at, 400)
        if found is None:
            return False
        centre, angle, d = found
        normal = np.array([-d[1], d[0]])
        return self.place(self.text(name, 'river', angle), [tuple(centre + normal * s) for s in (140, -140, 200, -200)], 'land')

    def letter_water(self, name, at, feature):
        """On open water near `at`, along the water body's long direction."""
        found = self.along(~self.land, at, feature.get('radius', 600))
        angle = feature.get('angle', found[1] if found else 0.0)
        return self.place(self.text(name, 'water', angle), [tuple(at)], 'water')

    def letter_place(self, name, at, feature):
        """A place's symbols, with its name beside them."""
        c, k = self.c, self.c.k
        symbols = feature.get('symbols', [])
        self.places.append((at, symbols))
        px, py = c.px(*at)
        # Symbols occupy about 120 × 90 px around the place; the name goes outside that.
        left, right, top, bottom = (40, 110, 50, 60) if symbols else (15, 15, 15, 15)
        y0, y1 = int(max(py - top * k, 0)), int(min(py + bottom * k, c.n))
        x0, x1 = int(max(px - left * k, 0)), int(min(px + right * k, c.n))
        self.reserved[y0:y1, x0:x1] = True
        layer = self.text(name, 'place')
        w, h = layer.width, layer.height
        spots = [(px + right * k + w / 2, py), (px - left * k - w / 2, py),
                 (px + (right - left) * k / 2, py + bottom * k + h / 2), (px + (right - left) * k / 2, py - top * k - h / 2)]
        return self.place(layer, [c.world(x, y) for x, y in spots])

    # Symbols.

    def stamp(self, a, fill, cx, base, width, avoid=True):
        """A symbol with its base centre at (cx, base); `fill` hides what lies behind it."""
        n = self.c.n
        height = max(2, int(a.shape[0] * width / a.shape[1]))
        width = max(2, int(width))
        x0, y0 = int(cx - width / 2), int(base - height)
        sx0, sy0, sx1, sy1 = max(x0, 0), max(y0, 0), min(x0 + width, n), min(y0 + height, n)
        if sx0 >= sx1 or sy0 >= sy1:
            return
        sym = resized(a, width, height)[sy0 - y0:sy1 - y0, sx0 - x0:sx1 - x0]
        window = (slice(sy0, sy1), slice(sx0, sx1))
        if avoid and self.reserved[window][sym > 0.2].any():
            return
        if fill is not None:
            self.ink[window] *= 1 - resized(fill, width, height)[sy0 - y0:sy1 - y0, sx0 - x0:sx1 - x0]
        self.ink[window] = np.maximum(self.ink[window], sym)

    def symbols(self, spec):
        c, b, rng, k = self.c, self.b, self.c.rng, self.c.k
        n, m = c.n, 1 / c.mpp  # pixels per metre
        clear = lambda metres: ~nd.binary_dilation(self.reserved, iterations=max(1, int(metres)))
        # Mountains over the steep high ground and hills on lower rises, drawn back to front.
        relief = nd.maximum_filter(self.h, int(195 * m))
        size = np.clip(310 + 1.07 * relief, 310, 945) * m
        picks = poisson(rng, self.high & clear(70 * k) & (rng.random((n, n)) < 0.02), size * 0.42,
                        order=self.h + 40 * rng.random((n, n)))
        rises = (self.h > HILL_HEIGHT) & (self.h < MOUNTAIN_HEIGHT) & self.land & (self.slope > 7) & (self.sdf > 30 * k)
        hills = poisson(rng, rises & clear(30 * k) & (rng.random((n, n)) < 0.01), 245 * m)
        marks = [(y, x, size[y, x], b.peaks) for y, x in picks]
        marks += [(y, x, rng.uniform(230, 325) * m, b.hills) for y, x in hills]
        for y, x, w, pool in sorted(marks):
            i = pool[rng.integers(len(pool))]
            self.stamp(b.mountains[i], b.mountain_fills[i], x, y + 0.18 * w, w)
        # Woods: groves and single trees on low gentle ground (decorative until forests have data).
        grove = c.noise(30 * k, 12) > 1.05
        fertile = self.soil > 0.35 if self.soil is not None else self.h < 80
        lowland = (self.land & (self.sdf > 24 * k) & (self.slope < 14) & (self.h > 3) & (self.h < 140) & fertile
                   & ~nd.binary_dilation(self.high, iterations=int(25 * k)) & clear(22 * k))
        trees = poisson(rng, lowland & grove & (rng.random((n, n)) < 0.04), 26 * k)
        trees += poisson(rng, lowland & ~grove & (rng.random((n, n)) < 0.002), 120 * k, limit=int(170 * (n * c.mpp / 1e4) ** 2))
        for y, x in sorted(trees):
            i = rng.integers(len(b.trees))
            self.stamp(b.trees[i], b.tree_fills[i], x, y, b.trees[i].shape[1] * rng.uniform(0.75, 0.95) * k)
        # Fields in open flat lowland.
        flat = (self.land & (self.sdf > 70 * k) & (self.slope < 3) & (self.h > 4) & (self.h < 60) & ~grove
                & ~nd.binary_dilation(self.high, iterations=int(40 * k)) & clear(22 * k))
        for y, x in poisson(rng, flat & (rng.random((n, n)) < 0.002), 260 * k, limit=8):
            f = b.fields[rng.integers(len(b.fields))]
            self.stamp(f, None, x, y + 70 * k, f.shape[1] * 0.55 * k)
        # Places.
        for (x, z), kinds in self.places:
            px, py = c.px(x, z)
            if 'hamlet' in kinds:
                for dx, dy, i in ((-26, -4, 3), (6, 10, 7)):
                    hs = b.houses[i % len(b.houses)]
                    self.stamp(hs, silhouette(hs), px + dx * k, py + dy * k, hs.shape[1] * 0.8 * k, avoid=False)
            if 'anchorage' in kinds:
                self.stamp(b.anchorage, None, px + 70 * k, py + 40 * k, b.anchorage.shape[1] * 0.7 * k, avoid=False)
        # Compass rose and its rhumb lines.
        if spec.get('compass'):
            rx, ry = c.px(*spec['compass'])
            rose_w = 420 * k
            free = clear(4 * k)
            for i in range(32):
                a = i * math.pi / 16
                dx, dy = math.cos(a), math.sin(a)
                across = (self.xx - rx) * dy - (self.yy - ry) * dx
                ahead = (self.xx - rx) * dx + (self.yy - ry) * dy
                line = np.clip(1 - np.abs(across) / (0.6 * k), 0, 1) * (ahead > rose_w * 0.45)
                self.ink = np.maximum(self.ink, line * (0.42 if i % 2 == 0 else 0.24) * free)
            self.stamp(b.rose, None, rx, ry + rose_w * b.rose.shape[0] / b.rose.shape[1] / 2, rose_w, avoid=False)

    def finish(self, lettering=True):
        ink = np.clip(nd.gaussian_filter(self.ink, 0.45 * self.c.k) * 1.05, 0, 1)
        # The pen skips where the paper is raised, more often where it ran dry.
        k = self.c.k
        dry = smoothstep(0.8, 1.8, self.c.noise(5 * k, 14))
        relief = 0.45 * self.tooth + self.c.noise(1.4 * k, 16)
        relief /= relief.std()
        skip = smoothstep(1.45 - 0.5 * dry, 2.05 - 0.5 * dry, relief)
        ink *= (1 - 0.9 * skip) * (1 - 0.25 * dry)
        colour = INK[None, None] * (1 + 0.08 * self.c.noise(3 * self.c.k, 13))[..., None]
        image = self.image * (1 - ink[..., None] * 0.92) + colour * ink[..., None] * 0.92
        img = Image.fromarray(np.clip(image, 0, 255).astype(np.uint8)).convert('RGBA')
        if lettering:
            img.alpha_composite(self.lettering)
        return img.convert('RGB')


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('manifest', type=Path, help='yarra-heightfield version 2 manifest')
    parser.add_argument('map', type=Path, help='map description: title, features, places')
    parser.add_argument('output', type=Path, help='PNG to write; a .json beside it records the extent')
    parser.add_argument('--size', type=int, default=3072, help='image width and height in pixels')
    parser.add_argument('--margin', type=float, default=0.1, help='sea around the land, as a share of its size')
    parser.add_argument('--seed', type=int, default=11)
    parser.add_argument('--no-lettering', action='store_true', help='leave names out, for drawing them at runtime')
    args = parser.parse_args()

    spec = json.loads(args.map.read_text())
    terrain = Terrain(args.manifest)
    lo, hi = terrain.land_bounds()
    extent = float(max(hi - lo) * (1 + 2 * args.margin))
    centre = (lo + hi) / 2
    canvas = Canvas(args.size, centre - extent / 2, extent, args.seed)
    world_map = Map(canvas, terrain, Brushes(ASSETS / 'homann'), ASSETS / 'fonts')
    world_map.paint()
    world_map.letter(spec)
    world_map.symbols(spec)
    image = world_map.finish(lettering=not args.no_lettering)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    image.save(args.output)
    args.output.with_suffix('.json').write_text(json.dumps({
        'format': 'yarra-map', 'version': 1, 'image': args.output.name, 'pixels': [args.size, args.size],
        'world_min': [float(v) for v in canvas.world_min], 'world_max': [float(v) for v in canvas.world_min + extent],
        'source': str(args.manifest),
    }, indent=2) + '\n')
    print(f'{args.output}: {args.size} px over {extent:.0f} m ({canvas.mpp:.2f} m/px)')


if __name__ == '__main__':
    main()
