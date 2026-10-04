#!/usr/bin/env hython
"""Export a Houdini heightfield for `yarra-world-cook import-heightfield`.

Run with Houdini's Python, which Apprentice and Indie licences both allow:

    hython tools/houdini_export_heightfield.py SCENE.hip OUTPUT_DIR [--node SOP] [--start X Z]
        [--height VOLUME] [--mask NAME[=VOLUME] ...]

It cooks the SOP (by default the display node of the first object whose output has the height
volume), then writes OUTPUT_DIR/height.f32 (little-endian float32, rows along +X, one row per Z)
and OUTPUT_DIR/heightfield.json. Houdini and the engine are both Y-up, so world X and Z carry
over unchanged: in Houdini's top view +X is right and +Z is down.

`--height` names the volume holding heights; a Copernicus network may call its output `mask`.
Each `--mask` exports another volume of the same heightfield as OUTPUT_DIR/NAME.u8, one byte per
sample for 0 to 1. Layers in the project read masks by NAME (lowercase letters, digits and
underscores). Remap masks to 0-1 in Houdini: values outside are clamped, with a warning.
"""
import argparse
import json
import re
import sys
from pathlib import Path

import hou
import numpy as np

MASK_NAME = re.compile(r"[a-z][a-z0-9_]{0,31}")


def volumes(geo):
    if geo is None or geo.findPrimAttrib("name") is None:
        return {}
    return {
        prim.attribValue("name"): prim
        for prim in geo.prims()
        if prim.type() == hou.primType.Volume
    }


def find_node(path, height):
    if path:
        node = hou.node(path)
        if node is None:
            sys.exit(f"no node at {path}")
        return node
    for obj in hou.node("/obj").children():
        sop = getattr(obj, "displayNode", lambda: None)()
        if sop is not None and height in volumes(sop.geometry()):
            return sop
    sys.exit(f"no object displays a heightfield with a {height!r} volume; pass --node or --height")


def mask_argument(text):
    name, _, volume = text.partition("=")
    if not MASK_NAME.fullmatch(name):
        raise argparse.ArgumentTypeError(
            f"{name!r}: mask names are 1-32 lowercase letters, digits or underscores, "
            "starting with a letter")
    return name, volume or name


def grid(prim):
    """The volume as rows along +X, one row per Z, with its first sample and spacing."""
    nx, ny, _ = prim.resolution()
    data = np.frombuffer(prim.allVoxelsAsString(), dtype=np.float32).reshape(ny, nx)
    corner = prim.indexToPos((0, 0, 0))
    along_i = prim.indexToPos((1, 0, 0)) - corner
    along_j = prim.indexToPos((0, 1, 0)) - corner
    # A heightfield's voxel axes lie in XZ; reorder them as rows of X, one row per Z.
    if abs(along_i[0]) > abs(along_i[2]):
        rows, dx, dz = data, along_i[0], along_j[2]
    else:
        rows, dx, dz = data.T, along_j[0], along_i[2]
    if dx < 0:
        rows = rows[:, ::-1]
    if dz < 0:
        rows = rows[::-1, :]
    spacing = abs(dx)
    if abs(abs(dz) - spacing) > 1e-4 * spacing:
        sys.exit("the heightfield's voxels must be square")
    count_z, count_x = rows.shape
    origin = (min(corner[0], corner[0] + dx * (count_x - 1)),
              min(corner[2], corner[2] + dz * (count_z - 1)))
    return rows, origin, spacing


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("scene")
    parser.add_argument("output", type=Path)
    parser.add_argument("--node", help="SOP whose volumes to export")
    parser.add_argument("--height", default="height", help="volume holding heights")
    parser.add_argument("--mask", action="append", default=[], type=mask_argument,
                        metavar="NAME[=VOLUME]", help="also export a 0-1 mask volume")
    parser.add_argument("--sea-level", type=float, default=0.0)
    parser.add_argument("--start", type=float, nargs=2, metavar=("X", "Z"),
                        help="where the player starts, in world metres")
    args = parser.parse_args()
    names = [name for name, _ in args.mask]
    if len(set(names)) != len(names):
        sys.exit("each mask name may be exported once")

    hou.hipFile.load(args.scene, suppress_save_prompt=True, ignore_load_warnings=True)
    node = find_node(args.node, args.height)
    found = volumes(node.geometry())
    missing = [v for v in [args.height, *(v for _, v in args.mask)] if v not in found]
    if missing:
        sys.exit(f"{node.path()} has no volume named {', '.join(missing)}; "
                 f"it has {', '.join(sorted(found)) or 'none'}")
    obj = node.parent()
    while obj is not None and not isinstance(obj, hou.ObjNode):
        obj = obj.parent()
    transform = obj.worldTransform() if obj else hou.hmath.identityTransform()
    if (max(abs(v) for v in transform.extractRotates()) > 1e-6
            or max(abs(v - 1) for v in transform.extractScales()) > 1e-6
            or max(abs(v) for v in transform.extractShears()) > 1e-6):
        sys.exit(f"{obj.path()} must not be rotated, scaled or sheared")
    offset = transform.extractTranslates()

    heights, origin, spacing = grid(found[args.height])
    rows, columns = heights.shape
    origin_x, origin_z = origin[0] + offset[0], origin[1] + offset[2]
    heights = heights + offset[1]

    args.output.mkdir(parents=True, exist_ok=True)
    np.ascontiguousarray(heights, dtype="<f4").tofile(args.output / "height.f32")
    masks = []
    for name, volume in args.mask:
        values, mask_origin, mask_spacing = grid(found[volume])
        if (values.shape != heights.shape or abs(mask_spacing - spacing) > 1e-4 * spacing
                or max(abs(a - b) for a, b in zip(mask_origin, origin)) > 1e-3 * spacing):
            sys.exit(f"mask {volume!r} must share the heightfield's grid")
        outside = np.count_nonzero((values < -1e-3) | (values > 1 + 1e-3)) / values.size
        if outside > 0.001:
            print(f"warning: {outside:.1%} of {volume!r} lies outside 0-1 and was clamped; "
                  "remap it in Houdini")
        np.rint(np.clip(values, 0, 1) * 255).astype(np.uint8).tofile(args.output / f"{name}.u8")
        masks.append({"name": name, "file": f"{name}.u8"})
        print(f"  {name} from {volume}: mean {values.mean():.3f}, "
              f"non-zero {np.count_nonzero(values > 0.5 / 255) / values.size:.1%}")
    manifest = {
        "format": "yarra-heightfield",
        "version": 2,
        "samples": [columns, rows],
        "spacing": spacing,
        "origin": [origin_x, origin_z],
        "heights": "height.f32",
        "sea_level": args.sea_level,
        "masks": masks,
        "source": f"{Path(args.scene).name}:{node.path()}",
    }
    if args.start:
        manifest["start"] = args.start
    (args.output / "heightfield.json").write_text(json.dumps(manifest, indent=2) + "\n")
    if abs(spacing - round(spacing)) > 1e-4 or abs(origin_x - round(origin_x)) > 1e-3 \
            or abs(origin_z - round(origin_z)) > 1e-3:
        print(f"note: samples are {spacing:g} m apart from ({origin_x:g}, {origin_z:g}); whole "
              "metres from a whole-metre origin let the import use samples without resampling")
    print(f"{node.path()}: {columns} x {rows} samples at {spacing:g} m, "
          f"X {origin_x:g}..{origin_x + spacing * (columns - 1):g}, "
          f"Z {origin_z:g}..{origin_z + spacing * (rows - 1):g}, "
          f"heights {heights.min():.1f}..{heights.max():.1f} m, "
          f"{len(masks)} masks -> {args.output}")


main()
