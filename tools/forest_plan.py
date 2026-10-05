#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy>=2", "pillow>=10", "scipy>=1.13"]
# ///
"""Plan a forest on one landmass from a terrain export, preview it and plant it.

    uv run tools/forest_plan.py MANIFEST OUT_DIR [--at X Z] [--density 200] [--seed 7] [--apply]

A prototype of the forest rules before they move into the cooker. Terrain fields (height,
slope, sun, hollows, coast distance and the soil, wetness, sand, rock and scree masks) score
three forest types. Open meadows are kept, with a clearing at the start. The forest is cut into
stands of about 0.3-3 ha, each with one type and an age. Trees are placed largest first: each
species keeps its own spacing, some clump, old stands hold dead trees, and a fringe of shrubs
and young trees lines every edge.

The landmass is the one holding --at (by default the manifest's start). Writes
OUT_DIR/preview.png and OUT_DIR/forest.json. --apply replaces the kit's trees and shrubs on that
landmass in --project with these placements and adds forest-* views beside it. Cook afterwards.
"""
import argparse
import json
import math
import sqlite3
import uuid
from collections import Counter, defaultdict
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw
from scipy import ndimage as nd
from scipy.spatial import cKDTree

ROOT = Path(__file__).resolve().parents[1]
GRID = 2.0  # metres per field sample
CELL = 32.0

# Crown width (m) of each kit form at scale 1, from the pack catalogs.
CROWN = {
    'yarra_longleaf/pine_longleaf': 8.6, 'yarra_longleaf/pine_longleaf_half_bare': 8.6,
    'yarra_longleaf/pine_longleaf_nearly_bare': 8.6, 'yarra_longleaf/pine_longleaf_one_sided': 8.6,
    'yarra_longleaf/pine_longleaf_tall_bole': 8.5, 'yarra_longleaf/pine_longleaf_broad': 9.7,
    'yarra_longleaf/pine_longleaf_leaning': 8.3, 'yarra_longleaf/pine_longleaf_flat_top': 8.4,
    'yarra_spruces/spruce_forest': 7.8,
    'yarra_oaks/oak_forest': 16.8, 'yarra_oaks/oak_spreading': 20.6, 'yarra_oaks/oak_sparse': 16.2,
    'yarra_maples/maple_forest': 13.8, 'yarra_maples/maple_spreading': 17.3, 'yarra_maples/maple_sparse': 13.2,
    'yarra_tall_forest/tall_broadleaf_forest': 13.2,
    'yarra_birches/birch_leafy': 8.5, 'yarra_birches/birch_sparse': 7.9, 'yarra_birches/birch_bare': 7.7,
    'yarra_birches/birch_crown': 6.2, 'yarra_birches/birch_pendulous': 9.0, 'yarra_birches/birch_double': 7.9,
    'yarra_birches/birch_triple': 8.3,
    'yarra_dead_trees/dead_upright': 11.0, 'yarra_dead_trees/dead_spreading': 15.6, 'yarra_dead_trees/dead_split': 12.7,
    'yarra_dead_trees/dead_slender': 3.9, 'yarra_dead_trees/dead_double': 4.0, 'yarra_dead_trees/dead_triple': 4.2,
    'yarra_shrubs/shrub_rounded': 2.3, 'yarra_shrubs/shrub_spreading': 2.8, 'yarra_shrubs/shrub_sparse': 2.4,
    'yarra_shrubs/shrub_medium_rounded': 3.3, 'yarra_shrubs/shrub_medium_spreading': 4.2,
    'yarra_shrubs/shrub_medium_upright': 3.3, 'yarra_bay/bay_upright': 8.8,
}
PINE, SPRUCE, BROADLEAF, BIRCH = 'pine', 'spruce', 'broadleaf', 'birch'
TYPES = (PINE, SPRUCE, BROADLEAF, BIRCH)
YOUNG, MATURE, OLD = 'young', 'mature', 'old'
# Per age: share of stands, relative density, and the uniform scale range of its trees.
AGES = {YOUNG: (0.25, 1.3, (0.55, 0.8)), MATURE: (0.5, 1.0, (0.85, 1.15)), OLD: (0.25, 0.7, (0.95, 1.25))}
# Canopy composition per forest type and age: (form, weight). Dead forms appear with age.
P, S, B = 'yarra_longleaf/', 'yarra_spruces/', 'yarra_birches/'
O, M, T, D = 'yarra_oaks/', 'yarra_maples/', 'yarra_tall_forest/', 'yarra_dead_trees/'
COMPOSITION = {
    (PINE, YOUNG): [(P + 'pine_longleaf', 25), (P + 'pine_longleaf_tall_bole', 10), (P + 'pine_longleaf_broad', 10),
                    (P + 'pine_longleaf_leaning', 10), (P + 'pine_longleaf_flat_top', 10),
                    (B + 'birch_leafy', 20), (B + 'birch_crown', 15)],
    (PINE, MATURE): [(P + 'pine_longleaf', 24), (P + 'pine_longleaf_tall_bole', 14), (P + 'pine_longleaf_broad', 12),
                     (P + 'pine_longleaf_leaning', 11), (P + 'pine_longleaf_flat_top', 11),
                     (P + 'pine_longleaf_half_bare', 10), (P + 'pine_longleaf_one_sided', 6),
                     (B + 'birch_leafy', 8), (D + 'dead_slender', 4)],
    (PINE, OLD): [(P + 'pine_longleaf', 14), (P + 'pine_longleaf_tall_bole', 9), (P + 'pine_longleaf_broad', 8),
                  (P + 'pine_longleaf_leaning', 7), (P + 'pine_longleaf_flat_top', 7),
                  (P + 'pine_longleaf_half_bare', 22), (P + 'pine_longleaf_nearly_bare', 12),
                  (P + 'pine_longleaf_one_sided', 8), (D + 'dead_upright', 5), (D + 'dead_slender', 4), (D + 'dead_double', 4)],
    (SPRUCE, YOUNG): [(S + 'spruce_forest', 75), (B + 'birch_crown', 15), (B + 'birch_pendulous', 10)],
    (SPRUCE, MATURE): [(S + 'spruce_forest', 85), (B + 'birch_pendulous', 7), (D + 'dead_slender', 5), (D + 'dead_double', 3)],
    (SPRUCE, OLD): [(S + 'spruce_forest', 80), (D + 'dead_slender', 8), (D + 'dead_triple', 5), (B + 'birch_pendulous', 7)],
    (BROADLEAF, YOUNG): [(M + 'maple_forest', 30), (O + 'oak_forest', 20), (B + 'birch_leafy', 30), (B + 'birch_double', 20)],
    (BROADLEAF, MATURE): [(O + 'oak_forest', 30), (M + 'maple_forest', 25), (T + 'tall_broadleaf_forest', 22),
                          (B + 'birch_leafy', 10), (O + 'oak_sparse', 5), (M + 'maple_sparse', 5), (D + 'dead_split', 3)],
    (BROADLEAF, OLD): [(O + 'oak_spreading', 28), (O + 'oak_forest', 20), (M + 'maple_spreading', 18),
                       (T + 'tall_broadleaf_forest', 16), (O + 'oak_sparse', 6), (D + 'dead_spreading', 6), (D + 'dead_split', 6)],
    (BIRCH, YOUNG): [(B + 'birch_leafy', 40), (B + 'birch_crown', 20), (B + 'birch_double', 20), (B + 'birch_triple', 20)],
    (BIRCH, MATURE): [(B + 'birch_leafy', 35), (B + 'birch_pendulous', 15), (B + 'birch_double', 15), (B + 'birch_triple', 12),
                      (B + 'birch_crown', 13), (B + 'birch_sparse', 6), (P + 'pine_longleaf', 2), (P + 'pine_longleaf_broad', 2)],
    (BIRCH, OLD): [(B + 'birch_leafy', 30), (B + 'birch_pendulous', 20), (B + 'birch_sparse', 15), (B + 'birch_bare', 8),
                   (B + 'birch_triple', 12), (D + 'dead_slender', 8), (P + 'pine_longleaf', 4), (P + 'pine_longleaf_flat_top', 3)],
}
# Clustering: the share of candidates drawn around parent trees instead of evenly.
CLUMPING = {PINE: 0.2, SPRUCE: 0.45, BROADLEAF: 0.3, BIRCH: 0.6}
SHRUBS = [('yarra_shrubs/shrub_medium_rounded', 22), ('yarra_shrubs/shrub_medium_spreading', 18),
          ('yarra_shrubs/shrub_medium_upright', 15), ('yarra_shrubs/shrub_rounded', 15),
          ('yarra_shrubs/shrub_spreading', 12), ('yarra_shrubs/shrub_sparse', 10), ('yarra_bay/bay_upright', 8)]
UNDERSTORY = {PINE: 15, SPRUCE: 5, BROADLEAF: 30, BIRCH: 20}  # shrubs per hectare inside stands
SPACING = 0.42  # neighbouring trunks keep this share of their mean crown width apart
# Forest types share the stands (by area) roughly like this; birch takes young and some other stands.
TYPE_SHARE = {PINE: 0.38, BROADLEAF: 0.40, SPRUCE: 0.22}
FRINGE = 12.0  # metres of shrubs and young trees along every forest edge
COLOURS = {PINE: (196, 160, 92), SPRUCE: (52, 96, 88), BROADLEAF: (104, 146, 70), BIRCH: (190, 200, 120)}


def smoothstep(a, b, x):
    t = np.clip((x - a) / (b - a), 0, 1)
    return t * t * (3 - 2 * t)


class Field:
    """Terrain samples every GRID metres over one landmass and its surroundings."""

    def __init__(self, manifest_path, at, seed):
        m = json.loads(Path(manifest_path).read_text())
        if m.get('format') != 'yarra-heightfield' or m.get('version') != 2:
            raise SystemExit('expected a yarra-heightfield version 2 manifest')
        base = Path(manifest_path).parent
        nx, nz = m['samples']
        self.spacing, self.origin = float(m['spacing']), np.asarray(m['origin'], float)
        self.sea = float(m['sea_level'])
        heights = np.memmap(base / m['heights'], '<f4', 'r', shape=(nz, nx))
        masks = {k['name']: np.memmap(base / k['file'], np.uint8, 'r', shape=(nz, nx)) for k in m.get('masks', [])}
        self.at = np.asarray(at if at is not None else m.get('start'), float)
        self.rng = np.random.default_rng(seed)
        self.seed = seed
        # Find the landmass on a coarse grid, then sample its box finely.
        step = max(1, int(16 / self.spacing))
        coarse = np.asarray(heights[::step, ::step]) > self.sea
        labels, _ = nd.label(coarse)
        cx, cz = ((self.at - self.origin) / (self.spacing * step)).round().astype(int)
        label = labels[cz, cx]
        if label == 0:
            raise SystemExit(f'{tuple(self.at)} is not on land')
        rows, cols = np.nonzero(labels == label)
        lo = self.origin + np.array([cols.min(), rows.min()]) * step * self.spacing - 150
        hi = self.origin + np.array([cols.max(), rows.max()]) * step * self.spacing + 150
        self.lo = np.floor(lo / GRID) * GRID
        n = np.ceil((hi - self.lo) / GRID).astype(int)
        self.shape = (n[1], n[0])
        xs = self.lo[0] + (np.arange(n[0]) + 0.5) * GRID
        zs = self.lo[1] + (np.arange(n[1]) + 0.5) * GRID
        zz, xx = np.meshgrid((zs - self.origin[1]) / self.spacing, (xs - self.origin[0]) / self.spacing, indexing='ij')
        sample = lambda a, scale=1.0: nd.map_coordinates(a, [zz, xx], order=1, mode='nearest', prefilter=False).astype(np.float32) * scale
        self.h = sample(heights) - self.sea
        self.mask = {k: sample(v, 1 / 255) for k, v in masks.items()}
        land = self.h > 0
        lab, _ = nd.label(land)
        iz, ix = self.index(*self.at)
        self.land = lab == lab[iz, ix]
        self.heights, self.manifest = heights, m

    def index(self, x, z):
        return int((z - self.lo[1]) / GRID), int((x - self.lo[0]) / GRID)

    def world(self, iz, ix):
        return self.lo[0] + (ix + 0.5) * GRID, self.lo[1] + (iz + 0.5) * GRID

    def noise(self, metres, salt):
        g = np.random.default_rng((self.seed, salt)).standard_normal(self.shape).astype(np.float32)
        g = nd.gaussian_filter(g, metres / GRID)
        return g / (g.std() + 1e-9)

    def height_at(self, x, z):
        """Bilinear height from the export at world (x, z), as the import samples it."""
        u = (np.atleast_1d(x) - self.origin[0]) / self.spacing
        v = (np.atleast_1d(z) - self.origin[1]) / self.spacing
        h = nd.map_coordinates(self.heights, [v, u], order=1, mode='nearest') - self.sea
        return h if np.ndim(x) else float(h[0])


class Plan:
    def __init__(self, f, density):
        self.f, self.density = f, density
        rng = f.rng
        h, mask = f.h, f.mask
        zero = np.zeros_like(h)
        wet, soil = mask.get('wetness', zero), mask.get('soil', zero)
        sand, rock, scree = mask.get('sand', zero), mask.get('rock', zero), mask.get('scree', zero)
        gz, gx = np.gradient(h, GRID)
        self.slope = np.degrees(np.arctan(np.hypot(gx, gz)))
        south = -gz / np.sqrt(1 + gx * gx + gz * gz) * 4  # +Z is south; about ±1 on a 14° slope
        hollow = h - nd.gaussian_filter(h, 40 / GRID)
        coast = nd.distance_transform_edt(f.land) * GRID
        # Where trees can grow, and the meadows kept open.
        grows = f.land & (h > 1.5) & (sand < 0.35) & (rock < 0.35) & (scree < 0.5) & (self.slope < 38) & (coast > 20)
        iz, ix = np.mgrid[0:f.shape[0], 0:f.shape[1]]
        wx, wz = f.world(iz, ix)
        start = np.hypot(wx - f.at[0], wz - f.at[1])
        openness = f.noise(260, 1) + 0.6 * f.noise(90, 2)
        open_land = (openness > np.quantile(openness[grows], 0.62)) | (start < 160)
        forest = grows & ~open_land
        forest = self.tidy(forest, 0.2e4, 0.1e4)
        self.forest = forest
        self.edge = nd.distance_transform_edt(forest) * GRID
        self.outside = nd.distance_transform_edt(~forest) * GRID
        # Forest types scored by their ground.
        near_coast = 1 - smoothstep(80, 600, coast)
        convex, concave = smoothstep(-0.5, 3, hollow), smoothstep(0.5, -3, hollow)
        self.score = {
            PINE: 0.45 * (1 - wet) * (1 - 0.5 * soil) + 0.35 * near_coast + 0.25 * convex + 0.1 * np.clip(south, -1, 1),
            SPRUCE: 0.6 * wet + 0.4 * concave + 0.15 * np.clip(-south, -1, 1) + 0.05,
            BROADLEAF: 0.55 * soil + 0.2 * (1 - near_coast) + 0.15 * (1 - wet) + 0.05,
        }
        self.stands(rng)

    @staticmethod
    def tidy(mask, min_area, min_hole):
        lab, n = nd.label(mask)
        big = nd.sum(mask, lab, range(1, n + 1)) * GRID * GRID >= min_area
        mask = np.isin(lab, 1 + np.flatnonzero(big))
        lab, n = nd.label(~mask)
        small = nd.sum(~mask, lab, range(1, n + 1)) * GRID * GRID < min_hole
        return mask | np.isin(lab, 1 + np.flatnonzero(small))

    def stands(self, rng):
        """Cuts the forest into stands with warped Voronoi cells of 0.3-3 ha."""
        f = self.f
        pts = np.argwhere(self.forest)
        size = 55 + 35 * (f.noise(400, 3) + 1.5).clip(0, 3)  # metres between stand centres
        order = rng.permutation(len(pts))
        seeds, tree = [], None
        grid = {}
        for i in order[:: max(1, len(order) // 60000)]:
            z, x = pts[i]
            r = size[z, x] / GRID
            key = (int(z // 40), int(x // 40))
            if any((sz - z) ** 2 + (sx - x) ** 2 < r * r for dz in range(-2, 3) for dx in range(-2, 3)
                   for sz, sx in grid.get((key[0] + dz, key[1] + dx), ())):
                continue
            grid.setdefault(key, []).append((z, x))
            seeds.append((z, x))
        seeds = np.array(seeds, float)
        warp = 30 / GRID
        wz = pts[:, 0] + warp * f.noise(120, 4)[self.forest]
        wx = pts[:, 1] + warp * f.noise(120, 5)[self.forest]
        tree = cKDTree(seeds)
        _, nearest = tree.query(np.stack([wz, wx], 1))
        self.stand = np.full(f.shape, -1, np.int32)
        self.stand[self.forest] = nearest
        n = len(seeds)
        area = np.bincount(nearest, minlength=n) * GRID * GRID / 1e4
        scores = {t: np.bincount(nearest, weights=s[self.forest], minlength=n) / np.maximum(area * 1e4 / GRID / GRID, 1)
                  for t, s in self.score.items()}
        # Each type competes on how unusual its ground is here, plus regional noise so that
        # neighbouring stands tend to share a type. Biases settle each type near its share.
        types = list(TYPE_SHARE)
        counts = np.maximum(area * 1e4 / GRID / GRID, 1)
        z = []
        for k, t in enumerate(types):
            s_ = scores[t]
            region = np.bincount(nearest, weights=f.noise(500, 10 + k)[self.forest], minlength=n) / counts
            z.append((s_ - np.average(s_, weights=area)) / (np.sqrt(np.cov(s_, aweights=area)) + 1e-9)
                     + 0.6 * region + rng.normal(0, 0.25, n))
        z, bias = np.array(z), np.zeros(len(types))
        target = np.array([TYPE_SHARE[t] for t in types])
        for _ in range(300):
            pick = np.argmax(z + bias[:, None], 0)
            share = np.array([area[pick == k].sum() for k in range(len(types))]) / area.sum()
            bias += 0.3 * (target - share)
        self.kind, self.age, self.area = [], [], area
        ages = list(AGES)
        weights = np.array([AGES[a][0] for a in ages])
        for i in range(n):
            age = ages[rng.choice(len(ages), p=weights / weights.sum())]
            kind = types[pick[i]]
            if rng.random() < (0.3 if age == YOUNG else 0.08):
                kind = BIRCH
            self.kind.append(kind)
            self.age.append(age)

    def trees(self):
        """Places canopy trees largest first, then the edge fringe and understory shrubs."""
        f, rng = self.f, self.f.rng
        candidates = []
        for i in range(len(self.kind)):
            if self.area[i] <= 0:
                continue
            kind, age = self.kind[i], self.age[i]
            target = int(round(self.area[i] * self.density * AGES[age][1]))
            if target == 0:
                continue
            cells = np.argwhere(self.stand == i)
            n = target * 3
            pick = cells[rng.integers(len(cells), size=n)]
            pos = (pick + rng.random((n, 2))) * GRID
            clumped = rng.random(n) < CLUMPING[kind]
            if clumped.any():
                parents = pos[rng.integers(n, size=max(1, n // 10))]
                pos[clumped] = parents[rng.integers(len(parents), size=clumped.sum())] + rng.normal(0, 6, (clumped.sum(), 2))
            forms, weights = zip(*COMPOSITION[(kind, age)])
            weights = np.array(weights, float) / sum(weights)
            for (pz, px), form in zip(pos, rng.choice(len(forms), size=n, p=weights)):
                iz, ix = int(pz / GRID), int(px / GRID)
                if not (0 <= iz < f.shape[0] and 0 <= ix < f.shape[1]) or self.stand[iz, ix] != i:
                    continue
                lo, hi = AGES[age][2]
                scale = rng.uniform(lo, hi) * (0.72 + 0.28 * smoothstep(0, 30, self.edge[iz, ix]))
                candidates.append((CROWN[forms[form]] * scale, px, pz, forms[form], scale, i))
        candidates.sort(key=lambda c: -c[0])
        placed, hash_, per_stand = [], defaultdict(list), Counter()
        targets = {i: int(round(self.area[i] * self.density * AGES[self.age[i]][1])) for i in range(len(self.kind))}
        def clear(px, pz, r, cell=12.0):
            k = (int(px // cell), int(pz // cell))
            reach = int(math.ceil((r + 12) / cell))
            return all((qx - px) ** 2 + (qz - pz) ** 2 >= (SPACING * (r + qr)) ** 2
                       for dx in range(-reach, reach + 1) for dz in range(-reach, reach + 1)
                       for qx, qz, qr in hash_.get((k[0] + dx, k[1] + dz), ()))
        for width, px, pz, form, scale, i in candidates:
            if per_stand[i] >= targets[i] or not clear(px, pz, width / 2):
                continue
            hash_[(int(px // 12), int(pz // 12))].append((px, pz, width / 2))
            per_stand[i] += 1
            placed.append((form, px, pz, scale, i))
        # Shrubs: a patchy fringe along every edge, and a light understory inside.
        patch = f.noise(25, 6) > -0.2
        fringe = (((self.edge > 0) & (self.edge < FRINGE)) | ((self.outside > 0) & (self.outside < 5))) & patch & f.land
        shrub_names, shrub_w = zip(*SHRUBS)
        shrub_w = np.array(shrub_w, float) / sum(shrub_w)
        cells = np.argwhere(fringe)
        wanted = int(len(cells) * GRID * GRID / 45)
        inner = [(i, UNDERSTORY[self.kind[i]] * self.area[i]) for i in range(len(self.kind))]
        for pz, px in list((cells[rng.integers(len(cells), size=wanted * 2)] + rng.random((wanted * 2, 2))) * GRID):
            form = shrub_names[rng.choice(len(shrub_names), p=shrub_w)]
            scale = rng.uniform(0.8, 1.15)
            if clear(px, pz, CROWN[form] * scale / 2 * 0.7):
                hash_[(int(px // 12), int(pz // 12))].append((px, pz, CROWN[form] * scale / 2 * 0.7))
                placed.append((form, px, pz, scale, -1))
                wanted -= 1
                if wanted <= 0:
                    break
        for i, count in inner:
            cells = np.argwhere(self.stand == i)
            for pz, px in list((cells[rng.integers(len(cells), size=int(count) * 3 + 1)] + rng.random((int(count) * 3 + 1, 2))) * GRID)[:int(count) * 3]:
                form = shrub_names[rng.choice(len(shrub_names) - 1)]  # no bay under the canopy
                scale = rng.uniform(0.75, 1.05)
                if clear(px, pz, CROWN[form] * scale / 2 * 0.7):
                    hash_[(int(px // 12), int(pz // 12))].append((px, pz, CROWN[form] * scale / 2 * 0.7))
                    placed.append((form, px, pz, scale, i))
                    count -= 1
                    if count <= 0:
                        break
        self.placed = [(form, f.lo[0] + px, f.lo[1] + pz, scale, i) for form, px, pz, scale, i in placed]
        return self.placed

    def stats(self):
        forest_ha = self.forest.sum() * GRID * GRID / 1e4
        land_ha = self.f.land.sum() * GRID * GRID / 1e4
        trees = [p for p in self.placed if 'shrub' not in p[0] and 'bay' not in p[0]]
        kinds = Counter()
        for i, a in enumerate(self.area):
            kinds[self.kind[i]] += a
        return {
            'land_ha': round(land_ha, 1), 'forest_ha': round(forest_ha, 1),
            'stands': len(self.kind), 'mean_stand_ha': round(float(np.mean(self.area)), 2),
            'forest_share_by_type': {k: round(v / forest_ha, 3) for k, v in kinds.items()},
            'trees': len(trees), 'shrubs': len(self.placed) - len(trees),
            'trees_per_forest_ha': round(len(trees) / forest_ha, 1),
            'forms': dict(Counter(p[0].split('/')[1] for p in self.placed).most_common()),
        }

    def preview(self, path, width=1600):
        f = self.f
        gz, gx = np.gradient(f.h, GRID)
        n = np.dstack([-gx, np.ones_like(f.h), -gz])
        n /= np.linalg.norm(n, axis=2, keepdims=True)
        light = np.clip(n @ np.array([-0.5, 0.75, -0.45]) / np.linalg.norm([-0.5, 0.75, -0.45]), 0, 1)
        img = np.where(f.land[..., None], np.array([214, 206, 170.]) * (0.55 + 0.5 * light[..., None]), np.array([150, 180, 196.]))
        for i, kind in enumerate(self.kind):
            sel = self.stand == i
            shade = {YOUNG: 1.18, MATURE: 1.0, OLD: 0.8}[self.age[i]]
            img[sel] = np.clip(np.array(COLOURS[kind]) * shade, 0, 255) * (0.65 + 0.45 * light[sel, None])
        edges = (nd.grey_dilation(self.stand, size=3) != nd.grey_erosion(self.stand, size=3)) & self.forest
        img[edges] *= 0.7
        scale = width / f.shape[1]
        im = Image.fromarray(img.clip(0, 255).astype(np.uint8)).resize((width, int(f.shape[0] * scale)), Image.LANCZOS)
        draw = ImageDraw.Draw(im)
        for form, x, z, s, _ in self.placed:
            px, pz = (x - f.lo[0]) / GRID * scale, (z - f.lo[1]) / GRID * scale
            r = max(0.6, CROWN[form] * s / 2 / GRID * scale * 0.5)
            colour = (60, 40, 30) if 'dead' in form else (40, 70, 40) if ('shrub' in form or 'bay' in form) else (25, 40, 30)
            draw.ellipse((px - r, pz - r, px + r, pz + r), fill=colour)
        sx, sz = (f.at[0] - f.lo[0]) / GRID * scale, (f.at[1] - f.lo[1]) / GRID * scale
        draw.ellipse((sx - 7, sz - 7, sx + 7, sz + 7), outline=(200, 30, 30), width=3)
        im.save(path)


def views(plan):
    """Bookmarks: inside the largest older stand of each type, an edge seen from a meadow, the
    view from the landmass's summit across the forest, and a steep look down into a stand."""
    f, out = plan.f, {}
    def bookmark(x, z, look, pitch, distance):
        y = float(f.height_at(x, z))
        yaw = math.degrees(math.atan2(-look[0], -look[1]))
        return f'(position: ({x:.1f}, {y:.3f}, {z:.1f}), yaw_degrees: {yaw:.1f}, pitch_degrees: {pitch}, distance: {distance}, fog_visibility: 20000.0, route: [])\n'
    trunks = cKDTree(np.array([(x, z) for _, x, z, _, _ in plan.placed]))
    for kind in TYPES:
        best = [i for i in range(len(plan.kind)) if plan.kind[i] == kind and plan.age[i] != YOUNG]
        if not best:
            continue
        i = max(best, key=lambda j: plan.area[j])
        # The most open spot well inside the stand, so the camera stands between trees.
        cells = np.argwhere((plan.stand == i) & (plan.edge > 25))
        if len(cells) == 0:
            cells = np.argwhere(plan.stand == i)
        x, z = f.world(cells[:, 0], cells[:, 1])
        gap, _ = trunks.query(np.stack([x, z], 1))
        k = int(np.argmax(gap))
        out[f'forest-{kind}'] = bookmark(float(x[k]), float(z[k]), (1, -1), 6.0, 9.7)
    # A meadow point 50-80 m from a long edge, looking at the forest.
    meadow = f.land & ~plan.forest & (plan.outside > 50) & (plan.outside < 80) & (plan.slope < 6)
    pts = np.argwhere(meadow)
    if len(pts):
        iz, ix = pts[len(pts) // 2]
        gz, gx = np.gradient(-plan.outside)
        x, z = f.world(iz, ix)
        out['forest-edge'] = bookmark(x, z, (gx[iz, ix], gz[iz, ix]), 5.0, 9.7)
    summit = np.unravel_index(np.argmax(np.where(f.land, f.h, -1e9)), f.shape)
    middle = np.argwhere(plan.forest).mean(0)
    x, z = f.world(*summit)
    mx, mz = f.world(*middle)
    out['forest-summit'] = bookmark(x, z, (mx - x, mz - z), 10.0, 12.0)
    if 'forest-broadleaf' in out:
        out['forest-top'] = out['forest-broadleaf'].replace('pitch_degrees: 6.0, distance: 9.7', 'pitch_degrees: 75.0, distance: 24.0')
    return out


def apply(plan, project, view_text, clear_only=False):
    """Replaces the kit's trees and shrubs on this landmass with the plan, or only removes them."""
    f = plan.f
    db = sqlite3.connect(f'file:{project.resolve()}?mode=rw', uri=True)
    db.execute('PRAGMA foreign_keys=ON')
    size = db.execute('SELECT cell_size FROM world_spaces WHERE id=1').fetchone()[0]
    if size != CELL:
        raise SystemExit(f'expected {CELL} m cells')
    keys = {k: db.execute('SELECT definition_id FROM object_definitions WHERE definition_key=?', ('asset/' + k,)).fetchone()
            for k in CROWN}
    missing = [k for k, v in keys.items() if v is None]
    if missing:
        raise SystemExit('register these packs first (yarra-world-cook import-assets): ' + ', '.join(sorted({m.split('/')[0] for m in missing})))
    kit = [v[0] for v in keys.values()]
    # The landmass's cells: any 32 m cell with land of this landmass.
    iz, ix = np.nonzero(f.land)
    wx, wz = f.world(iz, ix)
    cells = set(zip((wx // CELL).astype(int).tolist(), (wz // CELL).astype(int).tolist()))
    xs = np.array([p[1] for p in plan.placed])
    zs = np.array([p[2] for p in plan.placed])
    ys = f.height_at(xs, zs) - 0.025
    rng = np.random.default_rng((f.seed, 99))
    yaws = rng.uniform(0, 2 * math.pi, len(xs))
    with db:
        db.execute('BEGIN IMMEDIATE')
        db.execute('CREATE TEMP TABLE landmass(cell_x INTEGER, cell_z INTEGER, PRIMARY KEY(cell_x, cell_z)) WITHOUT ROWID')
        db.executemany('INSERT INTO landmass VALUES (?, ?)', cells)
        removed = db.execute(
            f'DELETE FROM object_placements WHERE world_space_id=1 AND definition_id IN ({",".join("?" * len(kit))}) '
            'AND (owner_cell_x, owner_cell_z) IN (SELECT cell_x, cell_z FROM landmass)', kit).rowcount
        rows = []
        for n, ((form, x, z, scale, _), y, yaw) in enumerate(zip([] if clear_only else plan.placed, ys, yaws)):
            cx, cz = math.floor(x / CELL), math.floor(z / CELL)
            object_id = uuid.uuid5(uuid.NAMESPACE_URL, f'yarra:forest-plan/v1/{f.seed}/{n}').bytes
            rows.append((object_id, 1, cx, cz, keys[form][0], x - cx * CELL, float(y), z - cz * CELL, float(yaw), float(scale), 1))
        db.executemany('INSERT INTO object_placements VALUES (?,?,?,?,?,?,?,?,?,?,?)', rows)
        db.execute('DROP TABLE landmass')
        assert not db.execute('PRAGMA foreign_key_check').fetchall()
    db.close()
    if clear_only:
        return removed, 0
    views_dir = project.with_suffix('.views')
    views_dir.mkdir(exist_ok=True)
    for name, text in view_text.items():
        (views_dir / f'{name}.ron').write_text(text)
    return removed, len(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('manifest', type=Path)
    parser.add_argument('output', type=Path, help='folder for preview.png and forest.json')
    parser.add_argument('--at', type=float, nargs=2, metavar=('X', 'Z'), help='a point on the landmass; default: the start')
    parser.add_argument('--density', type=float, default=200, help='canopy trees per hectare in mature stands')
    parser.add_argument('--seed', type=int, default=7)
    parser.add_argument('--apply', action='store_true', help='plant the plan in --project')
    parser.add_argument('--clear', action='store_true', help='only remove the kit from this landmass in --project, for comparisons')
    parser.add_argument('--project', type=Path, default=ROOT / 'content/world.project.sqlite')
    args = parser.parse_args()
    field = Field(args.manifest, args.at, args.seed)
    plan = Plan(field, args.density)
    plan.trees()
    stats = plan.stats()
    args.output.mkdir(parents=True, exist_ok=True)
    plan.preview(args.output / 'preview.png')
    marks = views(plan)
    (args.output / 'forest.json').write_text(json.dumps({
        'stats': stats, 'seed': args.seed, 'density': args.density,
        'placements': [{'form': form, 'x': round(x, 2), 'z': round(z, 2), 'scale': round(s, 3)} for form, x, z, s, _ in plan.placed],
    }, indent=1) + '\n')
    print(json.dumps(stats, indent=2))
    if args.clear:
        removed, _ = apply(plan, args.project, {}, clear_only=True)
        print(f'{args.project}: removed {removed} placements. Cook next.')
    elif args.apply:
        removed, added = apply(plan, args.project, marks)
        print(f'{args.project}: removed {removed} and planted {added} placements; views: {", ".join(sorted(marks))}. Cook next.')


if __name__ == '__main__':
    main()
