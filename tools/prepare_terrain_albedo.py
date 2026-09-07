#!/usr/bin/env python3
"""Bake periodic stochastic albedo in linear light, then encode mipmapped KTX2.

Requires numpy and Pillow. This changes the spatial pattern, not the source texel
density: the default 4096 texture covers four 1024-texel lattice cells per axis.
The triangular lattice repeats on a torus; shared vertices make wrap edges seamless.
Run separately from performance measurements: texture compression is CPU intensive.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from PIL import Image

from compile_terrain_textures import SURFACES, compile_image, find_ktx


def random01(x, y, seed):
    value = np.asarray(x, dtype=np.uint32) * np.uint32(0x9E3779B9)
    value ^= np.asarray(y, dtype=np.uint32) * np.uint32(0x85EBCA6B)
    value ^= np.uint32(seed)
    value ^= value >> np.uint32(16)
    value *= np.uint32(0x7FEB352D)
    value ^= value >> np.uint32(15)
    value *= np.uint32(0x846CA68B)
    value ^= value >> np.uint32(16)
    return (value >> np.uint32(8)).astype(np.float32) / np.float32(1 << 24)


def sample_repeat(source, u, v):
    height, width = source.shape[:2]
    x, y = u * width - 0.5, v * height - 0.5
    ix, iy = np.floor(x).astype(np.int32), np.floor(y).astype(np.int32)
    fx, fy = (x - ix)[..., None], (y - iy)[..., None]
    lower = source[iy % height, ix % width] * (1 - fx)
    lower += source[iy % height, (ix + 1) % width] * fx
    upper = source[(iy + 1) % height, ix % width] * (1 - fx)
    upper += source[(iy + 1) % height, (ix + 1) % width] * fx
    return lower * (1 - fy) + upper * fy


def evaluate(source, x, y, period, layer):
    """x/y are triangular-lattice coordinates, not world-aligned texture UVs."""
    cx, cy = np.floor(x).astype(np.int32), np.floor(y).astype(np.int32)
    fx, fy = x - cx, y - cy
    lower = fx + fy <= 1
    vertices = [(cx + np.where(lower, 0, 1), cy + np.where(lower, 0, 1)),
                (cx + np.where(lower, 1, 0), cy + np.where(lower, 0, 1)),
                (cx + np.where(lower, 0, 1), cy + np.where(lower, 1, 0))]
    weights = [np.where(lower, 1 - fx - fy, fx + fy - 1),
               np.where(lower, fx, 1 - fx), np.where(lower, fy, 1 - fy)]
    weights = [np.maximum(w, 0) ** 4 for w in weights]
    denominator = np.maximum(sum(weights), 1e-12)
    result = np.zeros((*np.broadcast_shapes(x.shape, y.shape), 3), dtype=np.float32)
    for (vx, vy), weight in zip(vertices, weights):
        # Subtract the unwrapped vertex before applying its periodic transform.
        # Translating by a complete period therefore preserves every source lookup.
        dx, dy = x - vx, y - vy
        u, v = dx - dy * 0.5, dy * np.float32(0.8660254038)
        hx, hy = vx % period, vy % period
        turn = (random01(hx, hy, 0x51A37 + layer * 101) * 4).astype(np.int32)
        ru = np.select([turn == 0, turn == 1, turn == 2], [u, -v, -u], default=v)
        rv = np.select([turn == 0, turn == 1, turn == 2], [v, u, -v], default=-u)
        ru += random01(hx, hy, 0xA731F + layer * 101)
        rv += random01(hx, hy, 0x9C119 + layer * 101)
        result += sample_repeat(source, ru, rv) * (weight / denominator)[..., None]
    return result


def bake(source_path, output, size, period, layer):
    srgb = np.asarray(Image.open(source_path).convert('RGB'), dtype=np.float32) / 255
    source = np.where(srgb <= 0.04045, srgb / 12.92, ((srgb + 0.055) / 1.055) ** 2.4)
    # Test torus continuity at arbitrary coordinates, including either side of zero.
    x = np.array([[-0.01, 0, 0.37, 1.12, period - 0.01]], dtype=np.float32)
    y = np.array([[0.13], [1.23], [period - 0.01]], dtype=np.float32)
    reference = evaluate(source, x, y, period, layer)
    for dx, dy in [(period, 0), (0, period), (-period, period)]:
        error = np.abs(reference - evaluate(source, x + dx, y + dy, period, layer)).max()
        if error > 0.0005:
            raise ValueError(f'periodic surface discontinuity: {error}')
    result = np.empty((size, size, 3), dtype=np.uint8)
    x = (np.arange(size, dtype=np.float32)[None, :] + 0.5) * period / size
    for start in range(0, size, 64):
        y = (np.arange(start, min(start + 64, size), dtype=np.float32)[:, None] + 0.5) * period / size
        linear = np.clip(evaluate(source, x, y, period, layer), 0, 1)
        encoded = np.where(linear <= 0.0031308, linear * 12.92, 1.055 * linear ** (1 / 2.4) - 0.055)
        result[start:start + len(y)] = np.rint(encoded * 255).astype(np.uint8)
    output.parent.mkdir(parents=True, exist_ok=True)
    Image.fromarray(result).save(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--size', type=int, default=4096)
    parser.add_argument('--period', type=int, default=4)
    parser.add_argument('--ktx')
    args = parser.parse_args()
    if args.size < 512 or args.size > 8192 or args.size & (args.size - 1) or not 2 <= args.period <= 16:
        parser.error('size must be a power of two in 512..8192 and period in 2..16')
    repository = Path(__file__).resolve().parents[1]
    root = repository / 'assets/local/terrain/temperate_meadow'
    ktx = find_ktx(repository, args.ktx)
    sources = [root / 'source' / surface / 'base_color.jpg' for surface in SURFACES]
    outputs = [root / 'source/prepared' / f'{surface}.png' for surface in SURFACES]
    records = []
    for layer, (source, output) in enumerate(zip(sources, outputs)):
        print(f'Baking {source.name}: layer {layer}, {args.size}px, period {args.period}', flush=True)
        bake(source, output, args.size, args.period, layer)
    entries = []
    for family in ['universal', 'astc']:
        target = root / 'runtime' / family / 'prepared_albedo_array.ktx2'
        records.append(compile_image(ktx, outputs, target, family, 'color', args.size))
        prefix = f'local/terrain/temperate_meadow/runtime/{family}'
        entries.append(f'        (source: "{prefix}/base_color_array.ktx2", image: "{prefix}/prepared_albedo_array.ktx2", period: {float(args.period)}),')
    manifest = repository / 'assets/packs/terrain/prepared.terrain-prepared'
    manifest.write_text('(version: 1, entries: [\n' + '\n'.join(entries) + '\n])\n')
    report = dict(version=1, size=args.size, period=args.period,
                  sources=[dict(path=str(p), sha256=hashlib.sha256(p.read_bytes()).hexdigest()) for p in sources],
                  outputs=records)
    (root / 'runtime/prepared-build-report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(manifest, flush=True)


if __name__ == '__main__':
    main()
