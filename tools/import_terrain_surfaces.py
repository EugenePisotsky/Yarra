#!/usr/bin/env python3
"""Convert a terrain pack's Megascans surfaces into the engine's per-surface source images.

Reads `assets/packs/terrain/<pack>.toml` and writes, for every surface,
`assets/local/terrain/<pack>/source/<key>/`:

- `base_color.png`: sRGB albedo with the normalised height map in alpha (alpha is linear);
- `normal_material.png`: octahedral tangent normal in RG, ambient occlusion in B and
  roughness in A, all linear.

Normals are stored in the engine's frame: +U along world X and +V (down the image) along
world Z. Each scan's green convention is detected from its height map, so packs never need
a per-surface sign. Images are resampled in linear light with wrapped borders, keeping the
scans tileable. Run with `uv run --with numpy --with pillow python tools/import_terrain_surfaces.py`.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import tomllib
from pathlib import Path

import numpy as np
from PIL import Image

Image.MAX_IMAGE_PIXELS = None
PAD = 32


def srgb_to_linear(x):
    # Lanczos rings slightly below zero; those samples take the linear segment.
    x = np.maximum(x, 0.0)
    return np.where(x <= 0.04045, x / 12.92, ((x + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(x):
    x = np.clip(x, 0.0, 1.0)
    return np.where(x <= 0.0031308, x * 12.92, 1.055 * x ** (1 / 2.4) - 0.055)


def find_map(directory: Path, *semantics: str) -> Path | None:
    for semantic in semantics:
        found = [
            p for p in directory.iterdir()
            if p.suffix.lower() in {'.jpg', '.jpeg', '.png'}
            and p.stem.lower().endswith('_' + semantic)
        ]
        if len(found) > 1:
            raise SystemExit(f'{directory}: several {semantic} maps')
        if found:
            return found[0]
    return None


def load(path: Path, size: int, mode: str) -> np.ndarray:
    """A float image at twice `size` (JPEG DCT scaling keeps decoding fast)."""
    image = Image.open(path)
    image.draft(mode, (size * 2, size * 2))
    image = image.convert(mode)
    if image.width != image.height:
        raise SystemExit(f'{path}: expected a square scan')
    data = np.asarray(image, dtype=np.float32) / 255.0
    return data if data.ndim == 3 else data[..., None]


def resample(data: np.ndarray, size: int) -> np.ndarray:
    """Lanczos resample each channel of a tileable image, wrapping its borders."""
    scale = size / data.shape[0]
    pad = PAD
    padded = np.pad(data, ((pad, pad), (pad, pad), (0, 0)), mode='wrap')
    target = round(padded.shape[0] * scale)
    channels = []
    for c in range(data.shape[2]):
        channel = Image.fromarray(padded[..., c], mode='F').resize((target, target), Image.LANCZOS)
        channels.append(np.asarray(channel, dtype=np.float32))
    out = np.stack(channels, axis=-1)
    crop = round(pad * scale)
    return out[crop:crop + size, crop:crop + size]


def octahedral(n: np.ndarray) -> np.ndarray:
    n = n / np.maximum(np.abs(n).sum(axis=-1, keepdims=True), 1e-6)
    x, y, z = n[..., 0], n[..., 1], n[..., 2]
    folded_x = (1 - np.abs(y)) * np.where(x >= 0, 1, -1)
    folded_y = (1 - np.abs(x)) * np.where(y >= 0, 1, -1)
    x, y = np.where(z >= 0, x, folded_x), np.where(z >= 0, y, folded_y)
    return np.stack([x, y], axis=-1) * 0.5 + 0.5


def correlation(a: np.ndarray, b: np.ndarray) -> float:
    a, b = a - a.mean(), b - b.mean()
    return float((a * b).sum() / max(np.sqrt((a * a).sum() * (b * b).sum()), 1e-12))


def mean_color(directory: Path, size: int) -> np.ndarray:
    color = srgb_to_linear(resample(load(find_map(directory, 'basecolor'), size, 'RGB'), size))
    return color.reshape(-1, 3).mean(axis=0)


def import_surface(surface: dict, sources: Path, output: Path, size: int) -> dict:
    directory = sources / surface['source']
    maps = {
        'color': find_map(directory, 'basecolor', 'albedo'),
        'normal': find_map(directory, 'normal'),
        'ao': find_map(directory, 'ao'),
        'roughness': find_map(directory, 'roughness'),
        'height': find_map(directory, 'displacement', 'bump'),
    }
    missing = [name for name, path in maps.items() if path is None]
    if missing:
        raise SystemExit(f'{directory}: missing {", ".join(missing)} maps')

    color = srgb_to_linear(resample(load(maps['color'], size, 'RGB'), size))
    if match := surface.get('color_match'):
        # Albedo scales multiplicatively, which keeps the scan's own relative contrast.
        target = mean_color(sources / match, size)
        color = color * (target / np.maximum(color.reshape(-1, 3).mean(axis=0), 1e-6))
    height = resample(load(maps['height'], size, 'L'), size)[..., 0]
    low, high = np.percentile(height, [0.5, 99.5])
    height_unit = np.clip((height - low) / max(high - low, 1e-6), 0, 1)

    normal = resample(load(maps['normal'], size, 'RGB'), size) * 2 - 1
    # Slopes of the height map in the engine's frame: +U right, +V down the image.
    du = (np.roll(height, -1, axis=1) - np.roll(height, 1, axis=1)) * 0.5
    dv = (np.roll(height, -1, axis=0) - np.roll(height, 1, axis=0)) * 0.5
    agree_x = correlation(normal[..., 0], -du)
    agree_y = correlation(normal[..., 1], -dv)
    if agree_x < 0:
        normal[..., 0] *= -1
    if agree_y < 0:
        normal[..., 1] *= -1
    normal[..., 2] = np.maximum(normal[..., 2], 1e-3)
    normal /= np.linalg.norm(normal, axis=-1, keepdims=True)

    ao = resample(load(maps['ao'], size, 'L'), size)[..., 0]
    roughness = resample(load(maps['roughness'], size, 'L'), size)[..., 0]

    target = output / surface['key']
    target.mkdir(parents=True, exist_ok=True)
    rgba = np.concatenate([linear_to_srgb(color), height_unit[..., None]], axis=-1)
    Image.fromarray(np.rint(rgba * 255).astype(np.uint8), 'RGBA').save(target / 'base_color.png')
    packed = np.concatenate([octahedral(normal), ao[..., None], roughness[..., None]], axis=-1)
    Image.fromarray(np.rint(np.clip(packed, 0, 1) * 255).astype(np.uint8), 'RGBA').save(
        target / 'normal_material.png')
    return {
        'key': surface['key'],
        'source': surface['source'],
        'maps': {k: v.name for k, v in maps.items()},
        'source_sha256': {
            k: hashlib.sha256(v.read_bytes()).hexdigest() for k, v in maps.items()
        },
        # Agreement of the scan's normal X/Y with its height slopes, before any flip.
        'normal_agreement': [round(agree_x, 3), round(agree_y, 3)],
        'flipped': [agree_x < 0, agree_y < 0],
        'mean_srgb': [round(float(v), 4) for v in linear_to_srgb(color.reshape(-1, 3).mean(axis=0))],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pack', default='baltic')
    parser.add_argument('--only', nargs='*', help='surface keys to (re)import')
    parser.add_argument('--repository', type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    manifest = tomllib.loads(
        (args.repository / f'assets/packs/terrain/{args.pack}.toml').read_text())
    sources = Path(manifest['sources']).expanduser()
    size = int(manifest['texture_size'])
    root = args.repository / f'assets/local/terrain/{args.pack}/source'
    root.mkdir(parents=True, exist_ok=True)
    macro = root / manifest['macro_variation']
    if not macro.exists():
        raise SystemExit(f'{macro}: the pack needs its 1024² greyscale macro variation field')
    report_path = root / 'import-report.json'
    report = json.loads(report_path.read_text()) if report_path.exists() else {}
    for surface in manifest['surfaces']:
        if args.only and surface['key'] not in args.only:
            continue
        print(f"Importing {surface['key']} from {surface['source']}", flush=True)
        report[surface['key']] = import_surface(surface, sources, root, size)
        entry = report[surface['key']]
        print(f"  normal agreement {entry['normal_agreement']}, flipped {entry['flipped']}, "
              f"mean sRGB {entry['mean_srgb']}", flush=True)
    keys = {s['key'] for s in manifest['surfaces']}
    report = {k: v for k, v in report.items() if k in keys}
    report_path.write_text(json.dumps(report, indent=2) + '\n')
    print(report_path)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
