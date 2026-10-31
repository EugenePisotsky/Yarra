"""Writes the base mesh's UVs, polygons and vertices for the skin's and hair's bakes.

blender -b --python-exit-code 1 --python head_mesh.py -- PRESET.json OUT.npz

MakeHuman's base mesh has one topology and one atlas, so any preset gives the same file.
"""
import json
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).parent))

from mpfb_services import build_human


def main():
    preset, out = (Path(arg) for arg in sys.argv[sys.argv.index("--") + 1 :])
    basemesh, _, _ = build_human(json.loads(preset.read_text()), subdivision=0)
    mesh = basemesh.data
    uv = np.empty(len(mesh.loops) * 2)
    mesh.uv_layers.active.data.foreach_get("uv", uv)
    starts = np.empty(len(mesh.polygons), dtype=np.int32)
    totals = np.empty(len(mesh.polygons), dtype=np.int32)
    mesh.polygons.foreach_get("loop_start", starts)
    mesh.polygons.foreach_get("loop_total", totals)
    vertices = np.empty(len(mesh.loops), dtype=np.int32)
    mesh.loops.foreach_get("vertex_index", vertices)
    key = basemesh.shape_key_add(name="shaped", from_mix=True)
    positions = np.array([basemesh.matrix_world @ point.co for point in key.data])
    basemesh.shape_key_remove(key)
    out.parent.mkdir(parents=True, exist_ok=True)
    np.savez(out, uv=uv.reshape(-1, 2), starts=starts, totals=totals, vertices=vertices, positions=positions)
    print(f"HEAD_MESH {out}: {len(mesh.polygons)} polygons")


main()
