#!/usr/bin/env python3
"""Prepare bounded, GPU-independent inputs for the terrain composite cooker.

Writes `assets/local/terrain/<pack>/runtime/material-inputs.terrain-bake`: 128-pixel mip
chains of every surface's base colour (sRGB albedo, height in alpha) and normal/material
image, in the pack's array order, then the macro variation. Requires NumPy/Pillow; no KTX
encoder or GPU is needed. List the pack's runtime arrays in `assets/packs/terrain/bake.ron`.
"""
import argparse
import hashlib
import struct
from pathlib import Path

import numpy as np
from PIL import Image
from compile_terrain_textures import pack_sources

SIZE = 128
VERSION = 2


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
    # Colour is filtered in linear light; alpha (height) and all material channels are data.
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
    parser.add_argument('--pack', default='baltic')
    parser.add_argument('--repository', type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    root, colors, normals, macro = pack_sources(args.repository, args.pack)
    groups = [(p, True) for p in colors] + [(p, False) for p in normals] + [(macro, False)]
    source_hash = hashlib.sha256(b'terrain-bake-input-v2')
    for path, _ in groups:
        source_hash.update(hashlib.sha256(path.read_bytes()).digest())
    payload = bytearray(struct.pack('<8sIIIf', b'YTRBAKE\0', VERSION, SIZE, len(colors), 0.0))
    payload += source_hash.digest()
    for path, color in groups:
        payload += chain(path, color)
    target = root / 'runtime/material-inputs.terrain-bake'
    target.parent.mkdir(parents=True, exist_ok=True)
    pending = target.with_suffix('.pending')
    pending.write_bytes(payload)
    pending.replace(target)
    print(f'{target}: {len(payload)} bytes, {len(colors)} layers; source SHA256 {source_hash.hexdigest()}')


if __name__ == '__main__':
    main()
