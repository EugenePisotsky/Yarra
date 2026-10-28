"""Exports a preset as a static glTF model for the game's character probe.

blender -b --python-exit-code 1 --python export_glb.py -- PRESET.json OUT.glb

The preset's character in its rest pose, as one glb with its textures: helpers removed, the
body under clothes cut away, sliders baked in, no subdivision, no rig or morph targets yet.
Materials are MPFB's game-engine kind (one Principled BSDF each); a texture with real
transparency (hair, eyebrows, eyelashes) is cut out at half alpha, which glTF exports as MASK, and
any other texture's alpha is dropped, so skin and eyes stay opaque.
"""
import json
import sys
from pathlib import Path

import bpy
import numpy as np

sys.path.insert(0, str(Path(__file__).parent))

from mpfb_services import build_human, service

ExportService = service("exportservice", "ExportService")

GAME_MATERIALS = {
    "skin_material_type": "GAMEENGINE",
    "eyes_material_type": "GAMEENGINE",
    "clothes_material_type": "GAMEENGINE",
}

CUTOFF = 0.5


def transparent(image):
    if image.channels != 4:
        return False

    pixels = np.empty(len(image.pixels), dtype=np.float32)
    image.pixels.foreach_get(pixels)
    return float(pixels[3::4].min()) < CUTOFF


def settle_alpha(material):
    """MASK where the texture has holes, OPAQUE otherwise."""
    nodes = material.node_tree.nodes
    links = material.node_tree.links
    alpha = nodes["Principled BSDF"].inputs["Alpha"]

    if not alpha.is_linked:
        return "OPAQUE"

    link = alpha.links[0]
    image = getattr(link.from_node, "image", None)

    if image is None or not transparent(image):
        links.remove(link)
        return "OPAQUE"

    cut = nodes.new("ShaderNodeMath")
    cut.operation = "GREATER_THAN"
    cut.inputs[1].default_value = CUTOFF
    links.new(link.from_socket, cut.inputs[0])
    links.new(cut.outputs[0], alpha)
    return "MASK"


def bake_shape_keys(obj):
    """Writes the shape keys' mix into the mesh. MPFB's bake leaves the body's slider keys in
    place, and glTF without morph targets exports the basis: MakeHuman's neutral base, under
    eyes, teeth, hair and clothes fitted to the shaped body."""
    if obj.data.shape_keys is None:
        return

    shaped = obj.evaluated_get(bpy.context.evaluated_depsgraph_get()).data
    positions = np.empty(len(shaped.vertices) * 3, dtype=np.float32)
    shaped.vertices.foreach_get("co", positions)
    obj.shape_key_clear()
    obj.data.vertices.foreach_set("co", positions)
    obj.data.update()


def main():
    preset_path, out = (Path(arg) for arg in sys.argv[sys.argv.index("--") + 1 :])
    info = dict(json.loads(preset_path.read_text()), **GAME_MATERIALS)
    basemesh, parts, _ = build_human(info, subdivision=0)
    ExportService.bake_modifiers_remove_helpers(basemesh, bake_masks=True, remove_helpers=True)
    bake_shape_keys(basemesh)
    materials = {slot.material for obj in parts for slot in obj.material_slots if slot.material}

    for material in sorted(materials, key=lambda m: m.name):
        print(f"MATERIAL {material.name} {settle_alpha(material)}")

    bpy.ops.object.select_all(action="DESELECT")

    for obj in parts:
        obj.select_set(True)
        print(f"PART {obj.name} {len(obj.data.vertices)} vertices")

    out.parent.mkdir(parents=True, exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=str(out),
        export_format="GLB",
        use_selection=True,
        export_apply=True,
        export_animations=False,
        export_skins=False,
        export_morph=False,
        # MPFB's meshes carry colour layers the game does not read.
        export_vertex_color="NONE",
    )
    print(f"EXPORTED {out} {out.stat().st_size / 1e6:.1f} MB")


main()
