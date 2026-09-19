#!/usr/bin/env python3
"""Prepare bounded, GPU-independent inputs for the terrain composite cooker.

Requires NumPy/Pillow and the existing prepared albedo PNGs. Source and derived
pixels stay under assets/local (ignored by Git). No KTX encoder/GPU is needed.
"""
import argparse
import hashlib
import json
import struct
from pathlib import Path

import numpy as np
from PIL import Image
from compile_terrain_textures import SURFACES

SIZE = 128


def linear(x):
    return np.where(x <= .04045, x / 12.92, ((x + .055) / 1.055) ** 2.4)


def srgb(x):
    return np.where(x <= .0031308, x * 12.92, 1.055 * np.maximum(x, 0) ** (1 / 2.4) - .055)


def half(x):
    return (x[::2, ::2] + x[1::2, ::2] + x[::2, 1::2] + x[1::2, 1::2]) * .25


def chain(path, color):
    data = np.asarray(Image.open(path).convert('RGBA'), dtype=np.float32) / 255
    if data.shape[0] != data.shape[1] or data.shape[0] < SIZE or data.shape[0] & (data.shape[0] - 1):
        raise ValueError(f'{path}: expected square power-of-two source >= {SIZE}')
    if color:
        data[..., :3] = linear(data[..., :3])
    # Only AO/roughness (BA) are consumed from the packed normal/material source.
    # Coarse world normals come from final terrain, never averaged octahedral texels.
    while data.shape[0] > SIZE:
        data = half(data)
    levels = []
    while True:
        encoded = data.copy()
        if color:
            encoded[..., :3] = srgb(encoded[..., :3])
        levels.append(np.rint(np.clip(encoded, 0, 1) * 255).astype(np.uint8).tobytes())
        if data.shape[0] == 1:
            return b''.join(levels)
        data = half(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repository', type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    root = args.repository / 'assets/local/terrain/temperate_meadow'
    report = json.loads((root / 'runtime/prepared-build-report.json').read_text())
    period = float(report['period'])
    groups = [(root / 'source' / s / 'base_color.jpg', True) for s in SURFACES]
    groups += [(root / 'source/prepared' / f'{s}.png', True) for s in SURFACES]
    groups += [(root / 'source' / s / 'normal_material.png', False) for s in SURFACES]
    groups += [(root / 'source/macro_variation.png', False)]
    source_hash = hashlib.sha256(b'terrain-bake-input-v1' + struct.pack('<f', period))
    for path, _ in groups:
        source_hash.update(hashlib.sha256(path.read_bytes()).digest())
    payload = bytearray(struct.pack('<8sIIIf', b'YTRBAKE\0', 1, SIZE, len(SURFACES), period))
    payload += source_hash.digest()
    for path, color in groups:
        payload += chain(path, color)
    target = root / 'runtime/material-inputs.terrain-bake'
    pending = target.with_suffix('.pending')
    pending.write_bytes(payload)
    pending.replace(target)
    print(f'{target}: {len(payload)} bytes; source SHA256 {source_hash.hexdigest()}')


if __name__ == '__main__':
    main()
