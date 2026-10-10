#!/usr/bin/env python3
"""Derive the island's ground masks and write a heightfield manifest that includes them.

Reads a Houdini `yarra-heightfield` v2 manifest (heights plus sand, rock, scree, ... masks) and
optionally `tools/forest_plan.py`'s `forest.json`, and writes into OUTPUT:

- `beach`: sand near the sea, and the whole seabed;
- `dune`: sand above the beach;
- `coast`: a band of coastal grass inland of the sand;
- `pine_forest`, `spruce_forest`, `broadleaf_forest`: forest floor under the planned trees
  of each kind, so small groves and single trees get their litter too;
- `heightfield.json`: the input manifest with these masks added. Its other files are
  referenced by absolute path, so nothing large is copied.

Import the result with `yarra-world-cook import-heightfield OUTPUT/heightfield.json`.
Run with `uv run --with numpy --with scipy python tools/island_masks.py MANIFEST OUTPUT
[--forest FOREST_JSON]`.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
from scipy import ndimage as nd

# Forest kinds by form-name prefix; shrubs and the bay leave the floor as it is.
FOREST_KINDS = {
    'pine_forest': ('pine_',),
    'spruce_forest': ('spruce_',),
    'broadleaf_forest': ('birch_', 'oak_', 'maple_', 'tall_broadleaf', 'dead_'),
}
FOREST_GRID = 2.0  # metres per forest density sample


def smoothstep(a, b, x):
    t = np.clip((x - a) / (b - a), 0, 1)
    return t * t * (3 - 2 * t)


def write(path: Path, values: np.ndarray):
    np.rint(np.clip(values, 0, 1) * 255).astype(np.uint8).tofile(path)


def forest_masks(placements, shape, origin, spacing):
    """Canopy density per forest kind on a coarse grid, blurred to a soft floor, at full size."""
    nz, nx = shape
    coarse = (int(np.ceil(nz * spacing / FOREST_GRID)), int(np.ceil(nx * spacing / FOREST_GRID)))
    result = {}
    for name, prefixes in FOREST_KINDS.items():
        # Forms are `pack/form`.
        trees = [p for p in placements if p['form'].rsplit('/', 1)[-1].startswith(prefixes)]
        density = np.zeros(coarse, np.float32)
        if trees:
            x = np.array([t['x'] for t in trees])
            z = np.array([t['z'] for t in trees])
            # Larger trees shade and litter a wider floor.
            weight = np.array([t['scale'] for t in trees], np.float32) ** 2
            ix = ((x - origin[0]) / FOREST_GRID).astype(int)
            iz = ((z - origin[1]) / FOREST_GRID).astype(int)
            inside = (ix >= 0) & (ix < coarse[1]) & (iz >= 0) & (iz < coarse[0])
            np.add.at(density, (iz[inside], ix[inside]), weight[inside])
            # Trees per 50 m², where one reads as closed forest floor.
            density = nd.gaussian_filter(density, 4.0 / FOREST_GRID) * 50.0 / FOREST_GRID ** 2
        floor = smoothstep(0.12, 0.6, density)
        # Back to the heightfield grid.
        result[name] = nd.zoom(floor, (nz / coarse[0], nx / coarse[1]), order=1,
                               grid_mode=True, mode='nearest')[:nz, :nx]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('manifest', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--forest', type=Path, help="forest_plan.py's forest.json")
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    if manifest.get('format') != 'yarra-heightfield' or manifest.get('version') != 2:
        raise SystemExit('expected a yarra-heightfield version 2 manifest')
    source = args.manifest.parent.resolve()
    nx, nz = manifest['samples']
    spacing = float(manifest['spacing'])
    origin = np.asarray(manifest['origin'], float)
    sea = float(manifest['sea_level'])
    heights = np.fromfile(source / manifest['heights'], '<f4').reshape(nz, nx) - sea
    masks = {m['name']: source / m['file'] for m in manifest.get('masks', [])}
    sand = np.fromfile(masks['sand'], np.uint8).reshape(nz, nx).astype(np.float32) / 255

    args.output.mkdir(parents=True, exist_ok=True)
    derived = {}
    # Sand splits by height into the beach the sea reaches and dunes above it; the seabed is
    # sand everywhere (rock layers draw over it where Houdini marks rock).
    upper = smoothstep(2.0, 3.5, heights)
    derived['beach'] = np.maximum(sand * (1 - upper), 1 - smoothstep(-0.5, 1.0, heights))
    derived['dune'] = sand * upper
    # Coastal grass fades inland over about 40 m from the sand, never on it.
    reach = nd.gaussian_filter(sand, 15.0 / spacing)
    derived['coast'] = smoothstep(0.03, 0.35, reach) * (1 - smoothstep(0.2, 0.6, sand)) \
        * (1 - smoothstep(25.0, 45.0, heights))
    del reach, upper, sand
    if args.forest:
        placements = json.loads(args.forest.read_text())['placements']
        derived.update(forest_masks(placements, (nz, nx), origin, spacing))

    entries = [m for m in manifest.get('masks', []) if m['name'] not in derived]
    for name, values in derived.items():
        write(args.output / f'{name}.u8', values)
        print(f'{name}: {float(values.mean()) * 100:.2f}% of the domain', flush=True)
    out = dict(manifest)
    out['heights'] = str(source / manifest['heights'])
    out['masks'] = [{'name': m['name'], 'file': str(source / m['file'])} for m in entries] + [
        {'name': name, 'file': f'{name}.u8'} for name in derived]
    out['source'] = f"{manifest.get('source', '')} | island_masks.py: {', '.join(derived)}"
    (args.output / 'heightfield.json').write_text(json.dumps(out, indent=2) + '\n')
    print(args.output / 'heightfield.json')


if __name__ == '__main__':
    main()
