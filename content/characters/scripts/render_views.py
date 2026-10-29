"""Renders review views of characters under one fixed light rig, for contact sheets.

blender -b --python-exit-code 1 --python render_views.py -- SPEC.json OUT_DIR

SPEC names a preset (MPFB's saved-human JSON), the views, the variants and optionally the engine
("CYCLES", the default, or "EEVEE"), "clay": true (one grey material on everything, so only
shapes compare) and "body_frame_m" (every body view frames this height from the feet, so heights
compare):
  {"preset": "content/characters/presets/probe_female.json",
   "views": ["face_front", "face_three_quarter", "body_front"],
   "variants": [{"label": "default"},
                {"label": "freckles", "human": {"skin_mhmat": "toigo.../....mhmat"}}]}
A variant's "human" is merged over the preset: objects key by key, anything else replaced, so
"targets" (sliders and ARKit face units alike, {"target": name, "value": w}) replace the preset's.
A variant's own "preset" replaces the spec's. A variant with "import": PATH (a glTF file, from the
repository root) shows that model in its rest pose instead, facing glTF's front. Writes
OUT_DIR/<label>/<view>.png and OUT_DIR/manifest.json; sheet.py lays them out.
"""
import json
import math
import sys
from pathlib import Path

import bpy
from mathutils import Vector

sys.path.insert(0, str(Path(__file__).parent))

from mpfb_services import ROOT, build_human, merged

STUDIO = Path(bpy.utils.system_resource("DATAFILES", path="studiolights/world"))

# Each view: what it frames, the camera's angle round the character from the front, degrees
# (positive towards the character's left), and the frame's pixels.
VIEWS = {
    "face_front": ("face", 0, (800, 800)),
    "face_three_quarter": ("face", 35, (800, 800)),
    "face_profile": ("face", 90, (800, 800)),
    "eyes_front": ("eyes", 0, (800, 400)),
    "eyes_three_quarter": ("eyes", 35, (800, 400)),
    "nose_front": ("nose", 0, (800, 500)),
    "nose_three_quarter": ("nose", 35, (800, 500)),
    "mouth_front": ("mouth", 0, (800, 500)),
    "mouth_three_quarter": ("mouth", 35, (800, 500)),
    "body_front": ("body", 0, (560, 1000)),
    "body_three_quarter": ("body", 35, (560, 1000)),
    "body_profile": ("body", 90, (560, 1000)),
    "body_back": ("body", 180, (560, 1000)),
}

FRONT = Vector((0.0, -1.0, 0.0))

# Where the eyes lie below the top of an imported model's head, metres.
EYES_BELOW_TOP = 0.115

# Lens (mm) and the height framed, metres (face) or as a share of the character's height (body).
FACE_LENS, FACE_HEIGHT = 85.0, 0.30

# Close views of one feature: the height framed and its centre's rise above the face's, metres.
FEATURES = {"eyes": (0.08, 0.035), "nose": (0.10, 0.0), "mouth": (0.10, -0.045)}
BODY_LENS, BODY_MARGIN = 50.0, 1.08
SENSOR_HEIGHT = 24.0


def clear_scene():
    bpy.data.batch_remove(list(bpy.data.objects))
    bpy.data.orphans_purge(do_local_ids=True, do_linked_ids=True, do_recursive=True)


def import_model(path):
    """A glTF model's meshes in their rest pose. In a skinned model only the skinned meshes show;
    other meshes in the file (helpers such as female_main's stray icosphere) are hidden."""
    bpy.ops.import_scene.gltf(filepath=str(ROOT / path))

    for armature in bpy.data.armatures:
        armature.pose_position = "REST"

    meshes = [obj for obj in bpy.data.objects if obj.type == "MESH"]
    skinned = [obj for obj in meshes if any(m.type == "ARMATURE" for m in obj.modifiers)]
    shown = skinned or meshes

    for obj in meshes:
        obj.hide_render = obj not in shown

    return shown, None


def clay(parts):
    material = bpy.data.materials.new("clay")
    material.diffuse_color = (0.5, 0.5, 0.5, 1.0)
    shader = material.node_tree.nodes["Principled BSDF"]
    shader.inputs["Base Color"].default_value = (0.5, 0.5, 0.5, 1.0)
    shader.inputs["Roughness"].default_value = 0.6

    for obj in parts:
        obj.data.materials.clear()
        obj.data.materials.append(material)


def world_bounds(objects):
    depsgraph = bpy.context.evaluated_depsgraph_get()
    points = []

    for obj in objects:
        evaluated = obj.evaluated_get(depsgraph)
        points.extend(obj.matrix_world @ Vector(corner) for corner in evaluated.bound_box)

    low = Vector(tuple(min(p[i] for p in points) for i in range(3)))
    high = Vector(tuple(max(p[i] for p in points) for i in range(3)))
    return low, high


def framing(parts, eyes):
    """Where the face and the body are. MPFB's characters and imported glTF models face -Y. The
    body takes its garments: MPFB hides the skin under them. Without eyes, a model's eyes are taken
    to lie EYES_BELOW_TOP under its top."""
    body_low, body_high = world_bounds(parts)
    body_centre = (body_low + body_high) / 2

    if eyes is None:
        eye_centre = Vector((body_centre.x, body_centre.y, body_high.z - EYES_BELOW_TOP)) + FRONT * 0.05
    else:
        eye_low, eye_high = world_bounds([eyes])
        eye_centre = (eye_low + eye_high) / 2

    face = eye_centre + Vector((0.0, 0.0, -0.035)) - FRONT * 0.05
    return face, body_centre, body_low.z, body_high.z - body_low.z


def place_camera(camera, target, front, angle, distance):
    turn = math.radians(angle)
    side = Vector((-front.y, front.x, 0.0))
    direction = front * math.cos(turn) + side * math.sin(turn)
    camera.location = target + direction * distance
    camera.rotation_euler = (-direction).to_track_quat("-Z", "Y").to_euler()


def light_rig(front):
    """A sun key from the front left and above, a soft fill, a rim from behind, under an
    outdoor sky at low strength for the eyes' and skin's reflections."""
    world = bpy.data.worlds.new("review")
    world.use_nodes = True
    nodes = world.node_tree.nodes
    sky = nodes.new("ShaderNodeTexEnvironment")
    sky.image = bpy.data.images.load(str(STUDIO / "forest.exr"))
    background = nodes["Background"]
    background.inputs["Strength"].default_value = 0.35
    world.node_tree.links.new(sky.outputs["Color"], background.inputs["Color"])
    bpy.context.scene.world = world

    side = Vector((-front.y, front.x, 0.0))
    lights = [
        ("key", "SUN", 3.2, front * 0.75 - side * 0.65 + Vector((0, 0, 0.8))),
        ("fill", "AREA", 60.0, front * 0.8 + side * 0.9 + Vector((0, 0, 0.2))),
        ("rim", "AREA", 90.0, -front * 1.0 + side * 0.4 + Vector((0, 0, 0.6))),
    ]

    for name, kind, energy, toward in lights:
        data = bpy.data.lights.new(name, kind)
        data.energy = energy

        if kind == "SUN":
            data.angle = math.radians(4.0)
        else:
            data.size = 2.0

        light = bpy.data.objects.new(name, data)
        light.location = toward.normalized() * 3.0 + Vector((0, 0, 1.0))
        light.rotation_euler = (-toward).to_track_quat("-Z", "Y").to_euler()
        bpy.context.scene.collection.objects.link(light)


def render_settings(engine):
    """Cycles by default: EEVEE shows MPFB's procedural eyes as mirrors (its corneas reflect
    the sky over the iris)."""
    scene = bpy.context.scene

    if engine == "EEVEE":
        scene.render.engine = "BLENDER_EEVEE"
        scene.eevee.taa_render_samples = 64
        scene.eevee.use_raytracing = True
    else:
        cycles = bpy.context.preferences.addons["cycles"].preferences
        cycles.compute_device_type = "METAL"
        cycles.get_devices()

        for device in cycles.devices:
            device.use = device.type == "METAL"

        scene.render.engine = "CYCLES"
        scene.cycles.device = "GPU"
        scene.cycles.samples = 128
        scene.cycles.use_denoising = True
        # Hair and eyelash cards stack many transparent layers.
        scene.cycles.transparent_max_bounces = 48

    scene.render.film_transparent = True
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGBA"
    scene.view_settings.view_transform = "AgX"


def mesh_counts(parts):
    """Vertices of each drawn part as rendered (helpers masked, subdivision applied)."""
    depsgraph = bpy.context.evaluated_depsgraph_get()
    return {obj.name: len(obj.evaluated_get(depsgraph).data.vertices) for obj in parts}


def render_variant(preset, variant, spec, out):
    clear_scene()

    if "import" in variant:
        parts, eyes = import_model(variant["import"])
    else:
        if "preset" in variant:
            preset = json.loads((ROOT / variant["preset"]).read_text())

        _, parts, eyes = build_human(merged(preset, variant.get("human", {})))

    if spec.get("clay"):
        clay(parts)

    placement = framing(parts, eyes)
    light_rig(FRONT)
    camera = add_camera()
    folder = out / variant["label"]
    folder.mkdir(parents=True, exist_ok=True)

    for view in spec["views"]:
        render_view(camera, view, placement, folder / f"{view}.png", spec.get("body_frame_m"))

    return mesh_counts(parts)


def add_camera():
    camera = bpy.data.objects.new("camera", bpy.data.cameras.new("camera"))
    camera.data.sensor_fit = "VERTICAL"
    camera.data.sensor_height = SENSOR_HEIGHT
    bpy.context.scene.collection.objects.link(camera)
    bpy.context.scene.camera = camera
    return camera


def render_view(camera, view, placement, path, body_frame=None):
    """Renders a view of a character placed as framing() found it. A body_frame (metres) frames
    that height from the feet instead of the character's own."""
    face, body, feet, height = placement
    subject, angle, (width, pixels_high) = VIEWS[view]

    if subject == "face":
        lens, target, framed = FACE_LENS, face, FACE_HEIGHT
    elif subject in FEATURES:
        framed, rise = FEATURES[subject]
        lens, target = FACE_LENS, face + Vector((0.0, 0.0, rise))
    else:
        lens, target, framed = BODY_LENS, body, height * BODY_MARGIN

        if body_frame:
            framed = body_frame
            target = Vector((body.x, body.y, feet + framed / 2))

    camera.data.lens = lens
    distance = framed / 2 * lens / (SENSOR_HEIGHT / 2)
    place_camera(camera, target, FRONT, angle, distance)
    scene = bpy.context.scene
    scene.render.resolution_x, scene.render.resolution_y = width, pixels_high
    scene.render.filepath = str(path)
    bpy.ops.render.render(write_still=True)


def main():
    spec_path, out = (Path(arg) for arg in sys.argv[sys.argv.index("--") + 1 :])
    spec = json.loads(spec_path.read_text())
    preset = json.loads((ROOT / spec["preset"]).read_text())
    views = spec["views"]
    render_settings(spec.get("engine", "CYCLES"))
    out.mkdir(parents=True, exist_ok=True)
    rendered = []

    for variant in spec["variants"]:
        counts = render_variant(preset, variant, spec, out)
        rendered.append({"label": variant["label"], "note": variant.get("note", ""), "vertices": counts})
        print(f"RENDERED {variant['label']} {counts}")

    manifest = {"views": views, "variants": rendered}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2))
    print(f"RENDER_DONE {len(rendered)} variants")


if __name__ == "__main__":
    main()
