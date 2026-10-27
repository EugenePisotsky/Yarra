"""Renders review views of MPFB characters under one fixed light rig, for contact sheets.

blender -b --python-exit-code 1 --python render_views.py -- SPEC.json OUT_DIR

SPEC names a preset (MPFB's saved-human JSON), the views, the variants and optionally the engine
("CYCLES", the default, or "EEVEE"):
  {"preset": "content/characters/presets/probe_female.json",
   "views": ["face_front", "face_three_quarter", "body_front"],
   "variants": [{"label": "default"},
                {"label": "freckles", "human": {"skin_mhmat": "toigo.../....mhmat"}}]}
A variant's "human" is merged over the preset: objects key by key, anything else replaced, so
"targets" (sliders and ARKit face units alike, {"target": name, "value": w}) replace the preset's.
Writes OUT_DIR/<label>/<view>.png and OUT_DIR/manifest.json; sheet.py lays them out.
"""
import json
import math
import sys
from pathlib import Path

import bpy
from mathutils import Vector

sys.path.insert(0, str(Path(__file__).parent))

from mpfb_services import service

HumanService = service("humanservice", "HumanService")
ObjectService = service("objectservice", "ObjectService")

ROOT = Path(__file__).resolve().parents[3]
STUDIO = Path(bpy.utils.system_resource("DATAFILES", path="studiolights/world"))

# Each view: what it frames, the camera's angle round the character from the front, degrees
# (positive towards the character's left), and the frame's pixels.
VIEWS = {
    "face_front": ("face", 0, (800, 800)),
    "face_three_quarter": ("face", 35, (800, 800)),
    "face_profile": ("face", 90, (800, 800)),
    "body_front": ("body", 0, (560, 1000)),
    "body_three_quarter": ("body", 35, (560, 1000)),
    "body_back": ("body", 180, (560, 1000)),
}

# Lens (mm) and the height framed, metres (face) or as a share of the character's height (body).
FACE_LENS, FACE_HEIGHT = 85.0, 0.30
BODY_LENS, BODY_MARGIN = 50.0, 1.08
SENSOR_HEIGHT = 24.0


def merged(base, override):
    out = dict(base)

    for key, value in override.items():
        if isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = merged(out[key], value)
        else:
            out[key] = value

    return out


def clear_scene():
    bpy.data.batch_remove(list(bpy.data.objects))
    bpy.data.orphans_purge(do_local_ids=True, do_linked_ids=True, do_recursive=True)


def build_human(info):
    settings = HumanService.get_default_deserialization_settings()
    settings["material_instances"] = "NEVER"
    return HumanService.deserialize_from_dict(info, settings)


def world_bounds(objects):
    depsgraph = bpy.context.evaluated_depsgraph_get()
    points = []

    for obj in objects:
        evaluated = obj.evaluated_get(depsgraph)
        points.extend(obj.matrix_world @ Vector(corner) for corner in evaluated.bound_box)

    low = Vector(tuple(min(p[i] for p in points) for i in range(3)))
    high = Vector(tuple(max(p[i] for p in points) for i in range(3)))
    return low, high


def drawn_parts(basemesh):
    return [obj for obj in (basemesh, *ObjectService.get_list_of_children(basemesh)) if obj.type == "MESH"]


def framing(basemesh):
    """Where the face and the body are, and the character's front as a unit vector in XY. The
    body takes its garments: MPFB hides the skin under them."""
    eyes = ObjectService.find_object_of_type_amongst_nearest_relatives(basemesh, "Eyes")
    eye_low, eye_high = world_bounds([eyes])
    body_low, body_high = world_bounds(drawn_parts(basemesh))
    eye_centre = (eye_low + eye_high) / 2
    body_centre = (body_low + body_high) / 2
    front = Vector((0.0, eye_centre.y - body_centre.y, 0.0)).normalized()
    face = eye_centre + Vector((0.0, 0.0, -0.035)) - front * 0.05
    return face, body_centre, body_high.z - body_low.z, front


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


def mesh_counts(basemesh):
    """Vertices of each drawn part as rendered (helpers masked, subdivision applied)."""
    depsgraph = bpy.context.evaluated_depsgraph_get()
    return {obj.name: len(obj.evaluated_get(depsgraph).data.vertices) for obj in drawn_parts(basemesh)}


def render_variant(preset, variant, views, out):
    clear_scene()
    basemesh = build_human(merged(preset, variant.get("human", {})))
    face, body, height, front = framing(basemesh)
    light_rig(front)

    camera = bpy.data.objects.new("camera", bpy.data.cameras.new("camera"))
    camera.data.sensor_fit = "VERTICAL"
    camera.data.sensor_height = SENSOR_HEIGHT
    bpy.context.scene.collection.objects.link(camera)
    bpy.context.scene.camera = camera
    folder = out / variant["label"]
    folder.mkdir(parents=True, exist_ok=True)

    for view in views:
        subject, angle, (width, pixels_high) = VIEWS[view]

        if subject == "face":
            lens, target, framed = FACE_LENS, face, FACE_HEIGHT
        else:
            lens, target, framed = BODY_LENS, body, height * BODY_MARGIN

        camera.data.lens = lens
        distance = framed / 2 * lens / (SENSOR_HEIGHT / 2)
        place_camera(camera, target, front, angle, distance)
        scene = bpy.context.scene
        scene.render.resolution_x, scene.render.resolution_y = width, pixels_high
        scene.render.filepath = str(folder / f"{view}.png")
        bpy.ops.render.render(write_still=True)

    return mesh_counts(basemesh)


def main():
    spec_path, out = (Path(arg) for arg in sys.argv[sys.argv.index("--") + 1 :])
    spec = json.loads(spec_path.read_text())
    preset = json.loads((ROOT / spec["preset"]).read_text())
    views = spec["views"]
    render_settings(spec.get("engine", "CYCLES"))
    out.mkdir(parents=True, exist_ok=True)
    rendered = []

    for variant in spec["variants"]:
        counts = render_variant(preset, variant, views, out)
        rendered.append({"label": variant["label"], "note": variant.get("note", ""), "vertices": counts})
        print(f"RENDERED {variant['label']} {counts}")

    manifest = {"views": views, "variants": rendered}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2))
    print(f"RENDER_DONE {len(rendered)} variants")


main()
