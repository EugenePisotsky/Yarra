"""The character kit: what the game needs to reshape an exported character's face and fit its parts
to it again, written beside the glb export_glb.py exports.

MakeHuman shapes a face by adding targets (offsets of base-mesh vertices) and fits each part (eyes,
brows, lashes, teeth, tongue, hair, garments) to the base mesh by its MHCLO data: every part vertex
is three base-mesh vertices, weighted, plus an offset scaled by the base mesh's size along each
axis, measured between two of its vertices. The kit holds that in glTF's space, metres, where
MakeHuman's axes are glTF's at a tenth of the scale, so a part's offsets are its file's:

- "base": every base-mesh vertex as exported, helpers included (eyes and teeth fit to helpers);
- "sliders": face_sliders.json's sliders, each end's offsets per base-mesh vertex it moves;
- "parts": each part's fitting, by its mesh's name in the glb;
- "skin_layers": the masks its skin paints with, if it has any (skin_export.py).

Each exported vertex names its base-mesh or part vertex in a _SOURCE_VERTEX attribute, since glTF
splits vertices along seams. The game reads the kit in engine/src/character_kit.rs.
"""
import json

import bpy
import numpy as np

from mpfb_services import ROOT, mpfb_attribute, service, sides

VERSION = 4

SLIDERS = ROOT / "content/characters/face_sliders.json"

SOURCE = "_SOURCE_VERTEX"

BODY = "body"

# Offsets smaller than this, metres, leave a vertex out of a slider.
STILL = 1e-7

# A part fitted again from the kit must land this close to where MPFB put it, metres.
REFIT_TOLERANCE = 1e-5

TargetService = service("targetservice", "TargetService")
AssetService = service("assetservice", "AssetService")
ObjectService = service("objectservice", "ObjectService")
GeneralObjectProperties = mpfb_attribute("mpfb.entities.objectproperties", "GeneralObjectProperties")
Mhclo = mpfb_attribute("mpfb.entities.clothes.mhclo", "Mhclo")


def to_gltf(points):
    """Blender's Z-up axes to glTF's Y-up."""
    return points[:, [0, 2, 1]] * np.array([1.0, 1.0, -1.0])


def coordinates(collection):
    out = np.empty(len(collection) * 3)
    collection.foreach_get("co", out)
    return out.reshape(-1, 3)


def in_gltf(obj, local):
    matrix = np.array(obj.matrix_world)
    return to_gltf(local @ matrix[:3, :3].T + matrix[:3, 3])


def base_positions(basemesh):
    """Every base-mesh vertex as its shape keys mix it."""
    key = basemesh.shape_key_add(name="kit-mix", from_mix=True)
    points = in_gltf(basemesh, coordinates(key.data))
    basemesh.shape_key_remove(key)
    return points


def slider_offsets(basemesh, spec):
    """Each slider's offsets at both ends of a unit move: the vertices it moves and by how much."""
    linear = np.array(basemesh.matrix_world)[:3, :3]
    basis = coordinates(basemesh.data.shape_keys.key_blocks[0].data)
    out = []

    for group in spec["groups"]:
        for slider in group["sliders"]:
            entry = {key: value for key, value in slider.items() if key not in ("targets", "view")}
            entry["group"] = group["name"]

            for end, name in zip(("low", "high"), slider["targets"]):
                if name is None:
                    continue

                total = np.zeros_like(basis)

                for target in sides(name):
                    path = TargetService.target_full_path(target)
                    key = TargetService.load_target(basemesh, path, weight=0.0, name=f"kit-{target}")
                    total += coordinates(key.data) - basis
                    basemesh.shape_key_remove(key)

                offsets = to_gltf(total @ linear.T)
                moved = np.flatnonzero(np.abs(offsets).max(axis=1) > STILL)
                entry[end] = {"vertices": moved.tolist(), "offsets": np.round(offsets[moved], 7).tolist()}

            out.append(entry)

    return out


def part_name(part):
    """The part's mesh name in the glb: its kind and asset."""
    kind = str(ObjectService.get_object_type(part)).lower()
    asset = GeneralObjectProperties.get_value("asset_source", entity_reference=part)
    return f"{kind}/{asset.split('/')[0]}"


def fitting(part):
    """The part's MHCLO fitting per vertex, offsets in the file's axes (glTF's)."""
    kind = str(ObjectService.get_object_type(part)).lower()
    source = GeneralObjectProperties.get_value("asset_source", entity_reference=part)
    mhclo = Mhclo()
    mhclo.load(AssetService.find_asset_absolute_path(source, kind))

    if not (mhclo.x_scale and mhclo.y_scale and mhclo.z_scale):
        raise ValueError(f"{part.name}: no x/y/z_scale in its MHCLO; the kit needs them")

    count = len(part.data.vertices)
    verts = [mhclo.verts[i] for i in range(count)]
    # MPFB reads the file's offsets (x, y, z) as Blender's (x, -z, y).
    offsets = [(v["offsets"][0], v["offsets"][2], -v["offsets"][1]) for v in verts]
    return {
        "vertices": [list(v["verts"]) for v in verts],
        "weights": [[round(w, 7) for w in v["weights"]] for v in verts],
        "offsets": np.round(np.array(offsets), 7).tolist(),
        "scales": [list(mhclo.x_scale), list(mhclo.y_scale), list(mhclo.z_scale)],
    }


def fitted(base, fit):
    """The part's vertices fitted to the base mesh: what the game computes."""
    vertices, weights = np.array(fit["vertices"]), np.array(fit["weights"])
    size = np.array([abs(base[a, axis] - base[b, axis]) / length for axis, (a, b, length) in enumerate(fit["scales"])])
    return (weights[:, :, None] * base[vertices]).sum(axis=1) + np.array(fit["offsets"]) * size


def name_meshes(basemesh, parts):
    """Names the meshes as the kit does, just before export: baking the base mesh replaces its
    mesh."""
    for part in parts:
        name_mesh(part, BODY if part == basemesh else part_name(part))


def name_mesh(obj, name):
    """Gives the object's mesh the name, taking it from any other mesh that holds it."""
    holder = bpy.data.meshes.get(name)

    if holder is not None and holder != obj.data:
        holder.name = f"{name} (displaced)"

    obj.data.name = name


def mark_sources(objects):
    for obj in objects:
        attribute = obj.data.attributes.new(SOURCE, "FLOAT", "POINT")
        attribute.data.foreach_set("value", np.arange(len(obj.data.vertices), dtype=np.float32))


def record(basemesh, parts, placed=None):
    """The kit of a built human, taken before the export removes its helpers; marks each vertex's
    source. `placed` holds the fittings of parts the export placed itself (fit_parts.py). Refuses a
    part the kit would fit elsewhere than it stands."""
    base = base_positions(basemesh)
    fits = {}
    placed = placed or {}

    for part in parts:
        if part == basemesh:
            continue

        name = part_name(part)
        fits[name] = placed.get(name) or fitting(part)
        error = np.abs(fitted(base, fits[name]) - in_gltf(part, coordinates(part.data.vertices))).max()
        print(f"KIT {name} fitted again within {error * 1000:.4f} mm")

        if error > REFIT_TOLERANCE:
            raise ValueError(f"{name}: the kit fits it {error * 1000:.3f} mm from where MPFB did")

    mark_sources(parts)
    spec = json.loads(SLIDERS.read_text())
    return {
        "version": VERSION,
        "body": BODY,
        "base": np.round(base, 7).tolist(),
        "parts": fits,
        "sliders": slider_offsets(basemesh, spec),
    }


def write(kit, path):
    path.write_text(json.dumps(kit, separators=(",", ":")))
    print(f"KIT wrote {path} {path.stat().st_size / 1e6:.1f} MB: {len(kit['sliders'])} sliders, {len(kit['parts'])} parts")
