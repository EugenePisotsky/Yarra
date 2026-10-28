"""Exports a preset as a rigged glTF character for the game.

blender -b --python-exit-code 1 --python export_glb.py -- PRESET.json OUT.glb [CLIPS.glb]

The preset's character on MPFB's game-engine skeleton (53 bones), as one glb with its textures:
body, garments and hair skinned by MPFB's weights, helpers removed, the body under clothes cut
away, sliders baked in, no subdivision, no morph targets yet. With CLIPS (female_main's locomotion
glb), its Idle, Walk and Jog are retargeted onto the skeleton under the same names (retarget.py),
and the joint heights the game's feet need are printed.

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
from retarget import retarget

ExportService = service("exportservice", "ExportService")

GAME = {
    "rig": "game_engine",
    "skin_material_type": "GAMEENGINE",
    "eyes_material_type": "GAMEENGINE",
    "clothes_material_type": "GAMEENGINE",
}

CLIPS = ("Idle", "Walk", "Jog")

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

    mix = obj.shape_key_add(name="mix", from_mix=True)
    positions = np.empty(len(mix.data) * 3, dtype=np.float32)
    mix.data.foreach_get("co", positions)
    obj.shape_key_clear()
    obj.data.vertices.foreach_set("co", positions)
    obj.data.update()


def import_clips(path):
    """The armature of a glTF file with clips, and everything the file brought."""
    before = set(bpy.data.objects)
    bpy.ops.import_scene.gltf(filepath=str(path))
    brought = [obj for obj in bpy.data.objects if obj not in before]
    return next(obj for obj in brought if obj.type == "ARMATURE"), brought


def feet(parts, lowest):
    """What the game's catalog says of a model's feet: the ankle (foot) and toe (ball) joints'
    lowest heights over the clips, and how far the left sole reaches behind the ankle and ahead
    of the ball at rest."""
    sole = [
        obj.matrix_world @ vertex.co
        for obj in parts
        for vertex in obj.data.vertices
        if (obj.matrix_world @ vertex.co).z < 0.12 and (obj.matrix_world @ vertex.co).x > 0.05
    ]
    rig = parts[0].parent
    ankle = rig.matrix_world @ rig.data.bones["foot_l"].head_local
    ball = rig.matrix_world @ rig.data.bones["ball_l"].head_local
    # The model faces -Y.
    heel = max(p.y for p in sole) - ankle.y
    toe = ball.y - min(p.y for p in sole)
    print(
        f"FEET ankle_height_m {lowest['foot_l']:.3f} toe_height_m {lowest['ball_l']:.3f} "
        f"heel_behind_ankle_m {heel:.3f} toe_ahead_m {toe:.3f}"
    )


def main():
    args = [Path(arg) for arg in sys.argv[sys.argv.index("--") + 1 :]]
    preset_path, out, clips = args[0], args[1], args[2] if len(args) > 2 else None
    info = dict(json.loads(preset_path.read_text()), **GAME)
    basemesh, parts, _ = build_human(info, subdivision=0)
    rig = basemesh.parent
    ExportService.bake_modifiers_remove_helpers(basemesh, bake_masks=True, remove_helpers=True)
    bake_shape_keys(basemesh)
    materials = {slot.material for obj in parts for slot in obj.material_slots if slot.material}

    for material in sorted(materials, key=lambda m: m.name):
        print(f"MATERIAL {material.name} {settle_alpha(material)}")

    if clips:
        source, brought = import_clips(clips)
        rate, lowest = retarget(source, rig, CLIPS)
        bpy.data.batch_remove(brought)

        for action in [action for action in bpy.data.actions if action.name not in CLIPS]:
            bpy.data.actions.remove(action)

        bpy.context.scene.render.fps, bpy.context.scene.render.fps_base = rate, 1.0
        feet(parts, lowest)

    bpy.ops.object.select_all(action="DESELECT")
    rig.select_set(True)

    for obj in parts:
        obj.select_set(True)
        print(f"PART {obj.name} {len(obj.data.vertices)} vertices")

    out.parent.mkdir(parents=True, exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=str(out),
        export_format="GLB",
        use_selection=True,
        export_apply=True,
        export_skins=True,
        export_animations=clips is not None,
        export_animation_mode="ACTIONS",
        export_morph=False,
        # MPFB's meshes carry colour layers the game does not read.
        export_vertex_color="NONE",
    )
    print(f"EXPORTED {out} {out.stat().st_size / 1e6:.1f} MB")


main()
