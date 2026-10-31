"""A skin on an exported character: the head on its own material and layout, the textures beside
the glb (skin_bake.py).

Before the export the head's polygons take a material of their own and the head's layout
(head_layout.py), and head and body show a small stand-in image, so the glb holds no large
textures. After it, the glb's materials are rewritten to name the KTX2 files: Blender's exporter
embeds images and has no KTX2 or mipmaps, and a face without mipmaps shimmers.
"""
import json
import struct

import bpy
import numpy as np

from fit_parts import IRIS, LASH, SCLERA, WET
from head_layout import Layout, island_at, uv_islands
from mpfb_services import ROOT

HEAD, BODY, CAVITY = "skin_head", "skin_body", "skin_cavity"

# Where the textures lie, from the glb.
FOLDER = "skin"

# The hair styles a character offers, and where they are built.
HAIR_STYLES = ROOT / "content/characters/hairs/female_hair_12.json"
HAIR_FOLDER = ROOT / "assets/local/characters/hairs"

STAND_IN = 4

JSON_CHUNK, BIN_CHUNK = 0x4E4F534A, 0x004E4942

LINEAR, LINEAR_MIPMAP_LINEAR, CLAMP = 9729, 9987, 33071

# The mouth's parts sit in shade the lighting does not know of (no occlusion inside a closed mouth),
# so their colour carries it, or teeth shine through the lips' gap.
MOUTH_PARTS, MOUTH_SHADE = ("teeth", "tongue"), 0.25

# The insides of the mouth and the eye sockets, in the same shade: MakeHuman's atlas paints them
# as bright as skin.
CAVITY_COLOR = [0.025, 0.008, 0.008, 1.0]

# The eye's surfaces: roughness, and for the wet layer how little of its diffuse colour there is.
SCLERA_ROUGHNESS, IRIS_ROUGHNESS, WET_ROUGHNESS, LASH_ROUGHNESS = 0.45, 0.8, 0.07, 0.6


def stand_in_material(name):
    material = bpy.data.materials.new(name)
    material.use_nodes = True
    nodes = material.node_tree.nodes
    image = bpy.data.images.new(f"{name}_stand_in", STAND_IN, STAND_IN)
    texture = nodes.new("ShaderNodeTexImage")
    texture.image = image
    material.node_tree.links.new(texture.outputs["Color"], nodes["Principled BSDF"].inputs["Base Color"])
    return material


def apply(basemesh, skin_path):
    """Gives a baked base mesh (no helpers) its head material and layout."""
    skin = json.loads((ROOT / skin_path).read_text())
    mesh = basemesh.data
    uv = np.empty(len(mesh.loops) * 2)
    layer = mesh.uv_layers.active
    layer.data.foreach_get("uv", uv)
    uv = uv.reshape(-1, 2)
    starts = np.empty(len(mesh.polygons), dtype=np.int32)
    totals = np.empty(len(mesh.polygons), dtype=np.int32)
    mesh.polygons.foreach_get("loop_start", starts)
    mesh.polygons.foreach_get("loop_total", totals)
    islands = uv_islands(uv, starts, totals)
    head = island_at(uv, islands, skin["head_uv_point"])
    cavity = np.zeros_like(head)

    for point in skin["cavity_uv_points"]:
        cavity |= island_at(uv, islands, point)

    uv[head] = Layout(skin).texture_uv(uv[head])
    layer.data.foreach_set("uv", uv.ravel())
    mesh.materials.clear()

    for name in (BODY, HEAD, CAVITY):
        mesh.materials.append(stand_in_material(name))

    indices = head[starts].astype(np.int32) + 2 * cavity[starts].astype(np.int32)
    mesh.polygons.foreach_set("material_index", indices)
    mesh.update()
    print(f"SKIN {skin_path}: {int(head[starts].sum())} of {len(starts)} polygons on the head")


def layers(glb):
    """What the kit says of the skin's masks: the rectangle of the head's texture the brows' cover,
    and each mask by its path from the asset root (skin_layers_bake.py)."""
    folder = glb.resolve().parent / FOLDER
    manifest = json.loads((folder / "skin_layers.json").read_text())
    root = folder.relative_to(ROOT / "assets").as_posix()
    manifest["makeup"] = f"{root}/{manifest['makeup']}"
    manifest["hairs"] = hair_styles()

    for brow in manifest["brows"]:
        brow["mask"] = f"{root}/{brow['mask']}"

    return manifest


def hair_styles():
    """The styles built (hair_fit.py), each by its label and folder from the asset root."""
    styles = json.loads(HAIR_STYLES.read_text())["styles"]
    built = [style for style in styles if (HAIR_FOLDER / style["id"] / "style.hair").is_file()]
    print(f"SKIN hair styles: {len(built)} of {len(styles)} built")
    folder = HAIR_FOLDER.relative_to(ROOT / "assets").as_posix()
    return [{"label": style["label"], "folder": f"{folder}/{style['id']}"} for style in built]


def read_glb(path):
    data = path.read_bytes()
    length = struct.unpack_from("<I", data, 12)[0]
    document = json.loads(data[20 : 20 + length])
    return document, data[20 + length :]


def write_glb(path, document, rest):
    text = json.dumps(document, separators=(",", ":")).encode()
    text += b" " * (-len(text) % 4)
    total = 12 + 8 + len(text) + len(rest)
    path.write_bytes(
        struct.pack("<III", 0x46546C67, 2, total) + struct.pack("<II", len(text), JSON_CHUNK) + text + rest
    )


def rewrite(path):
    """Points the glb's skin materials at the textures beside it."""
    document, rest = read_glb(path)
    images, textures = document.setdefault("images", []), document.setdefault("textures", [])
    samplers = document.setdefault("samplers", [])
    samplers.append({"magFilter": LINEAR, "minFilter": LINEAR_MIPMAP_LINEAR, "wrapS": CLAMP, "wrapT": CLAMP})
    sampler = len(samplers) - 1

    def texture(name):
        images.append({"uri": f"{FOLDER}/{name}.ktx2"})
        textures.append({"sampler": sampler, "source": len(images) - 1})
        return {"index": len(textures) - 1}

    def surface(color, roughness):
        return {
            "baseColorTexture": texture(color),
            "metallicFactor": 0.0,
            "roughnessFactor": roughness,
        }

    done = set()

    for material in document["materials"]:
        name = material["name"]

        if any(part in name for part in MOUTH_PARTS):
            shade = [MOUTH_SHADE, MOUTH_SHADE, MOUTH_SHADE, 1.0]
            material["pbrMetallicRoughness"]["baseColorFactor"] = shade
            continue

        if name == BODY:
            material["pbrMetallicRoughness"] = {
                "baseColorTexture": texture("body_color"),
                "metallicFactor": 0.0,
                "roughnessFactor": 0.5,
            }
        elif name == HEAD:
            material["pbrMetallicRoughness"] = {
                "baseColorTexture": texture("head_color"),
                "metallicRoughnessTexture": texture("head_orm"),
                "metallicFactor": 1.0,
                "roughnessFactor": 1.0,
            }
            material["normalTexture"] = texture("head_normal")
            material["occlusionTexture"] = texture("head_orm")
        elif name == CAVITY:
            material["pbrMetallicRoughness"] = {
                "baseColorFactor": CAVITY_COLOR,
                "metallicFactor": 0.0,
                "roughnessFactor": 0.9,
            }
        elif name == SCLERA:
            material["pbrMetallicRoughness"] = surface("eye_sclera_color", SCLERA_ROUGHNESS)
            material["normalTexture"] = texture("eye_sclera_normal")
        elif name == IRIS:
            # No normal map: a bought iris's deep relief turns each fibre bright or dark by the
            # light's direction, and the slightest head movement sweeps rays around the eye. A real
            # iris lies under the cornea and shows none of that.
            material["pbrMetallicRoughness"] = surface("eye_iris_color", IRIS_ROUGHNESS)
        elif name == LASH:
            material["pbrMetallicRoughness"] = surface("eye_lash", LASH_ROUGHNESS)
            material["alphaMode"] = "BLEND"
            material["doubleSided"] = True
            material.pop("alphaCutoff", None)
        elif name == WET:
            material["pbrMetallicRoughness"] = {
                "baseColorTexture": texture("eye_wet"),
                "metallicFactor": 0.0,
                "roughnessFactor": WET_ROUGHNESS,
            }
            material["alphaMode"] = "BLEND"
            material.pop("alphaCutoff", None)
        else:
            continue

        done.add(name)

    if not {HEAD, BODY, CAVITY} <= done:
        raise ValueError(f"the glb has only the skin materials {sorted(done)}")

    write_glb(path, document, rest)
