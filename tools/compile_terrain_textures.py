#!/usr/bin/env python3
"""Compile a terrain pack's surfaces into mipmapped KTX2 texture arrays.

`assets/packs/terrain/<pack>.toml` lists the surfaces in array order; their sources are the
`base_color.png` (sRGB albedo, height in alpha) and `normal_material.png` images written by
`tools/import_terrain_surfaces.py`. Outputs go to `assets/local/terrain/<pack>/runtime`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import struct
import subprocess
import tomllib
from pathlib import Path


KTX_VERSION = "4.4.2"
ASTC_BLOCK_EXTENTS = {
    vk_format: extent
    for index, extent in enumerate(
        ((4, 4), (5, 4), (5, 5), (6, 5), (6, 6), (8, 5), (8, 6), (8, 8))
    )
    for vk_format in (157 + index * 2, 158 + index * 2)
}


def find_ktx(repository: Path, explicit: str | None) -> Path:
    candidates = (
        explicit,
        os.environ.get("KTX_BIN"),
        repository.parent / "yarra" / ".tools" / "ktx" / KTX_VERSION / "bin" / "ktx",
        shutil.which("ktx"),
    )
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return Path(candidate).resolve()
    raise RuntimeError("Khronos ktx 4.4.2 was not found; pass --ktx or set KTX_BIN")


def run(command: list[str]) -> None:
    subprocess.run(command, check=True)


def validate_astc(path: Path) -> None:
    data = path.read_bytes()
    if len(data) < 48 or data[:12] != b"\xabKTX 20\xbb\r\n\x1a\n":
        raise RuntimeError(f"{path} is not KTX2")
    vk_format, _, width, height = struct.unpack_from("<4I", data, 12)
    block = ASTC_BLOCK_EXTENTS.get(vk_format)
    if block is None or width % block[0] or height % block[1]:
        raise RuntimeError(f"{path} is not block-aligned native ASTC")


def compile_image(
    ktx: Path,
    sources: list[Path],
    destination: Path,
    family: str,
    semantic: str,
    size: int,
) -> dict[str, object]:
    missing = [source for source in sources if not source.is_file()]
    if missing:
        raise RuntimeError(f"missing terrain source {missing[0]}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    pending = destination.with_suffix(".ktx2.pending")
    astc_format = {
        "color": "ASTC_8x8_SRGB_BLOCK",
        "normal-material": "ASTC_4x4_UNORM_BLOCK",
        "data": "ASTC_8x8_UNORM_BLOCK",
    }[semantic]
    universal_format = "R8G8B8A8_SRGB" if semantic == "color" else "R8G8B8A8_UNORM"
    command = [
        str(ktx),
        "create",
        "--format",
        astc_format if family == "astc" else universal_format,
    ]
    if len(sources) > 1:
        command.extend(["--layers", str(len(sources)), "--width", str(size), "--height", str(size)])
    command.extend(
        [
            "--generate-mipmap",
            "--mipmap-wrap",
            "wrap",
            "--assign-tf",
            "srgb" if semantic == "color" else "linear",
        ]
    )
    if family == "astc":
        command.extend(["--astc-quality", "fast"])
        if semantic == "color":
            command.append("--astc-perceptual")
    else:
        command.extend(["--encode", "uastc", "--uastc-quality", "2", "--zstd", "8"])
    command.extend(["--testrun", *(str(source) for source in sources), str(pending)])
    run(command)
    run([str(ktx), "validate", str(pending)])
    if family == "astc":
        validate_astc(pending)
    pending.replace(destination)
    data = destination.read_bytes()
    return {
        "path": destination.as_posix(),
        "bytes": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
    }


def load_pack(repository: Path, pack: str) -> dict:
    return tomllib.loads((repository / f"assets/packs/terrain/{pack}.toml").read_text())


def pack_sources(repository: Path, pack: str) -> tuple[Path, list[Path], list[Path], Path]:
    """The pack's local root, then its base colour and normal/material sources in array
    order, then its macro variation."""
    manifest = load_pack(repository, pack)
    root = repository / "assets/local/terrain" / pack
    keys = [surface["key"] for surface in manifest["surfaces"]]
    return (
        root,
        [root / "source" / key / "base_color.png" for key in keys],
        [root / "source" / key / "normal_material.png" for key in keys],
        root / "source" / manifest["macro_variation"],
    )


def gpu_bytes(size: int, layers: int, bytes_per_texel: float) -> int:
    texels = 0
    while size >= 1:
        texels += size * size
        size //= 2
    return int(texels * layers * bytes_per_texel)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pack", default="baltic")
    parser.add_argument("--ktx")
    parser.add_argument("--repository", type=Path)
    args = parser.parse_args()
    repository = (args.repository or Path(__file__).resolve().parents[1]).resolve()
    ktx = find_ktx(repository, args.ktx)
    root, colors, normals, macro = pack_sources(repository, args.pack)
    size = int(load_pack(repository, args.pack)["texture_size"])
    records: list[dict[str, object]] = []
    for family in ("universal", "astc"):
        output = root / "runtime" / family
        records.append(compile_image(ktx, colors, output / "base_color_array.ktx2", family, "color", size))
        records.append(compile_image(ktx, normals, output / "normal_material_array.ktx2", family, "normal-material", size))
        records.append(compile_image(ktx, [macro], output / "macro_variation.ktx2", family, "data", 1024))
    layers = len(colors)
    report = {
        "pack": args.pack,
        "layers": layers,
        "texture_size": size,
        # Resident bytes after transcoding: UASTC becomes BC7 on desktop (1 byte per texel);
        # iOS keeps ASTC 8x8 colour/data (0.125) and 4x4 normal/material (1).
        "gpu_bytes": {
            "universal": gpu_bytes(size, layers * 2, 1.0) + gpu_bytes(1024, 1, 1.0),
            "astc": gpu_bytes(size, layers, 0.125) + gpu_bytes(size, layers, 1.0)
            + gpu_bytes(1024, 1, 0.125),
        },
        "outputs": records,
    }
    path = root / "runtime/build-report.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
