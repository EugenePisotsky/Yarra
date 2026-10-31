"""Writes a bought hair pack's meshes for hair_fit.py: every hair and the pack's own head.

blender -b --python-exit-code 1 --python hair_pack.py -- PACK.blend OUT.npz

PACK lays each hair out on its own copy of a head, a row of them along X (Female Hairs' Blender file:
hair1 to hair12 on Shared_basemesh2 to 13). Each hair is written in its head's frame. It is opened with
its scripts off, and the bought file is read this once.
"""
import sys
from pathlib import Path

import bpy
import numpy as np

BASE, HAIR, HEAD_MATERIAL = "Shared_basemesh", "hair", "woman:head_mtl"


def arrays(obj):
    mesh = obj.data
    points = np.array([obj.matrix_world @ vertex.co for vertex in mesh.vertices])
    starts = np.empty(len(mesh.polygons), dtype=np.int32)
    totals = np.empty(len(mesh.polygons), dtype=np.int32)
    indices = np.empty(len(mesh.polygons), dtype=np.int32)
    mesh.polygons.foreach_get("loop_start", starts)
    mesh.polygons.foreach_get("loop_total", totals)
    mesh.polygons.foreach_get("material_index", indices)
    loops = np.empty(len(mesh.loops), dtype=np.int32)
    mesh.loops.foreach_get("vertex_index", loops)
    uv = np.empty(len(mesh.loops) * 2)
    mesh.uv_layers.active.data.foreach_get("uv", uv)
    return points, starts, totals, indices, loops, uv.reshape(-1, 2)


def main():
    source, out = (Path(arg) for arg in sys.argv[sys.argv.index("--") + 1 :])
    bpy.ops.wm.open_mainfile(filepath=str(source), load_ui=False, use_scripts=False)
    bases = [obj for obj in bpy.data.objects if obj.name.startswith(BASE)]
    data = {}
    base = min(bases, key=lambda obj: abs(obj.matrix_world.translation.x))
    points, starts, totals, indices, loops, _ = arrays(base)
    head = [slot.material.name for slot in base.material_slots].index(HEAD_MATERIAL)
    used = [loops[s : s + t] for s, t, i in zip(starts, totals, indices) if i == head]
    data["head"] = points[np.unique(np.concatenate(used))] - np.array(base.matrix_world.translation)

    for obj in bpy.data.objects:
        if not obj.name.removeprefix(HAIR).isdigit() or obj.type != "MESH":
            continue

        points, starts, totals, _, loops, uv = arrays(obj)
        centre = points.mean(0)
        origin = min(bases, key=lambda b: abs(b.matrix_world.translation.x - centre[0])).matrix_world.translation
        name = obj.name
        data |= {
            f"{name}_points": points - np.array(origin),
            f"{name}_starts": starts,
            f"{name}_totals": totals,
            f"{name}_loops": loops,
            f"{name}_uv": uv,
        }
        print(f"HAIR {name}: {int((totals - 2).sum())} triangles")

    out.parent.mkdir(parents=True, exist_ok=True)
    np.savez(out, **data)


main()
