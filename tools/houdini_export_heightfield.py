#!/usr/bin/env hython
"""Export a Houdini heightfield for `yarra-world-cook import-heightfield`.

Run with Houdini's Python, which Apprentice and Indie licences both allow:

    hython tools/houdini_export_heightfield.py SCENE.hip OUTPUT_DIR [--node SOP] [--start X Z]

It cooks the SOP (by default the display node of the first object whose output has a
`height` volume), then writes OUTPUT_DIR/height.f32 (little-endian float32, rows along +X,
one row per Z) and OUTPUT_DIR/heightfield.json. Houdini and the engine are both Y-up, so
world X and Z carry over unchanged: in Houdini's top view +X is right and +Z is down.
"""
import argparse
import json
import sys
from pathlib import Path

import hou
import numpy as np


def find_node(path):
    if path:
        node = hou.node(path)
        if node is None:
            sys.exit(f"no node at {path}")
        return node
    for obj in hou.node("/obj").children():
        sop = getattr(obj, "displayNode", lambda: None)()
        if sop is not None and height_volume(sop.geometry()) is not None:
            return sop
    sys.exit("no object displays a heightfield; pass --node")


def height_volume(geo):
    if geo is None or geo.findPrimAttrib("name") is None:
        return None
    for prim in geo.prims():
        if prim.type() == hou.primType.Volume and prim.attribValue("name") == "height":
            return prim
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("scene")
    parser.add_argument("output", type=Path)
    parser.add_argument("--node", help="SOP whose height volume to export")
    parser.add_argument("--sea-level", type=float, default=0.0)
    parser.add_argument("--start", type=float, nargs=2, metavar=("X", "Z"),
                        help="where the player starts, in world metres")
    args = parser.parse_args()

    hou.hipFile.load(args.scene, suppress_save_prompt=True, ignore_load_warnings=True)
    node = find_node(args.node)
    prim = height_volume(node.geometry())
    if prim is None:
        sys.exit(f"{node.path()} has no height volume")
    obj = node.parent()
    while obj is not None and not isinstance(obj, hou.ObjNode):
        obj = obj.parent()
    transform = obj.worldTransform() if obj else hou.hmath.identityTransform()
    if (max(abs(v) for v in transform.extractRotates()) > 1e-6
            or max(abs(v - 1) for v in transform.extractScales()) > 1e-6
            or max(abs(v) for v in transform.extractShears()) > 1e-6):
        sys.exit(f"{obj.path()} must not be rotated, scaled or sheared")
    offset = transform.extractTranslates()

    nx, ny, _ = prim.resolution()
    data = np.frombuffer(prim.allVoxelsAsString(), dtype=np.float32).reshape(ny, nx)
    corner = prim.indexToPos((0, 0, 0))
    along_i = prim.indexToPos((1, 0, 0)) - corner
    along_j = prim.indexToPos((0, 1, 0)) - corner
    # A heightfield's voxel axes lie in XZ; reorder them as rows of X, one row per Z.
    if abs(along_i[0]) > abs(along_i[2]):
        grid, dx, dz = data, along_i[0], along_j[2]
    else:
        grid, dx, dz = data.T, along_j[0], along_i[2]
    if dx < 0:
        grid = grid[:, ::-1]
    if dz < 0:
        grid = grid[::-1, :]
    spacing = abs(dx)
    if abs(abs(dz) - spacing) > 1e-4 * spacing:
        sys.exit("the heightfield's voxels must be square")
    rows, columns = grid.shape
    origin_x = min(corner[0], corner[0] + dx * (columns - 1)) + offset[0]
    origin_z = min(corner[2], corner[2] + dz * (rows - 1)) + offset[2]
    heights = grid + offset[1]

    args.output.mkdir(parents=True, exist_ok=True)
    np.ascontiguousarray(heights, dtype="<f4").tofile(args.output / "height.f32")
    manifest = {
        "format": "yarra-heightfield",
        "version": 1,
        "samples": [columns, rows],
        "spacing": spacing,
        "origin": [origin_x, origin_z],
        "heights": "height.f32",
        "sea_level": args.sea_level,
        "source": f"{Path(args.scene).name}:{node.path()}",
    }
    if args.start:
        manifest["start"] = args.start
    (args.output / "heightfield.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"{node.path()}: {columns} x {rows} samples at {spacing:g} m, "
          f"X {origin_x:g}..{origin_x + spacing * (columns - 1):g}, "
          f"Z {origin_z:g}..{origin_z + spacing * (rows - 1):g}, "
          f"heights {heights.min():.1f}..{heights.max():.1f} m -> {args.output}")


main()
