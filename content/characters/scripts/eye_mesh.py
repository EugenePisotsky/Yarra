"""Writes how a bought eye model's sclera texture runs over its eyeball, for the character's eye.

blender -b --python-exit-code 1 --python eye_mesh.py -- EYE.blend OUT.npz

EYE is the Eye3dModels pack's eye.blend: a sphere facing -Y whose sclera map is a disc, the front of
the eye at its centre and the back at its rim. The character's eyeball is a clean surface of
revolution (fit_parts.py), so of the model only this is needed: the map's radius at each angle from
the front. It is opened with its scripts off, and the bought file is read this once.
"""
import sys
from pathlib import Path

import bpy
import numpy as np

# Angles from the front the table holds, degrees.
ANGLES = np.arange(0.0, 181.0, 5.0)


def main():
    source, out = (Path(arg) for arg in sys.argv[sys.argv.index("--") + 1 :])
    bpy.ops.wm.open_mainfile(filepath=str(source), load_ui=False, use_scripts=False)
    obj = next(item for item in bpy.data.objects if item.type == "MESH")
    mesh = obj.data
    sclera = next(index for index, slot in enumerate(obj.material_slots) if slot.material.name == "sclera")
    layer = mesh.uv_layers["UVMap"]
    samples = []

    for polygon in mesh.polygons:
        if polygon.material_index != sclera:
            continue

        for loop in polygon.loop_indices:
            point = obj.matrix_world @ mesh.vertices[mesh.loops[loop].vertex_index].co
            angle = np.degrees(np.arctan2(np.hypot(point.x, point.z), -point.y))
            samples.append((angle, np.hypot(*(np.array(layer.data[loop].uv) - 0.5))))

    samples = np.array(sorted(samples))
    table = np.interp(ANGLES, samples[:, 0], samples[:, 1])
    out.parent.mkdir(parents=True, exist_ok=True)
    np.savez(out, angles=ANGLES, uv_radius=table)
    print(f"EYE_MESH {out}: map radius {table[6]:.3f} at 30 degrees, {table[18]:.3f} at 90")


main()
