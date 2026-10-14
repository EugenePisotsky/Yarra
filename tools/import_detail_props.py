"""Converts Megascans 3D prop packs (FBX and 1K textures) into ground-detail glTF binaries.

Run with Blender, once per pack:

    /Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup \
        -P tools/import_detail_props.py -- PACK_DIR OUT.glb [--albedo LUMINANCE]

Each piece of the pack becomes its own mesh, named `piece_NNN`, standing on the origin: its
footprint centred and its lowest point at height zero. All pieces share one material with the
pack's base colour, normal, roughness and ambient occlusion, re-saved as PNG (the game decodes
neither JPEG nor EXR). The ground-detail placer picks pieces at random and sinks them by their
type's `sink`.

`--albedo` scales the base colour so its mean linear luminance is LUMINANCE, keeping its hue:
scanned packs come with their own exposure (the twigs average 0.2, twice the forest floor).
"""
import glob
import os
import sys
import tempfile

import bpy
import numpy as np
from mathutils import Vector


def texture(directory, kind):
    found = glob.glob(os.path.join(directory, f"*_{kind}.jpg"))
    return found[0] if found else None


def main():
    arguments = sys.argv[sys.argv.index("--") + 1:]
    pack, output = arguments[:2]
    albedo = float(arguments[arguments.index("--albedo") + 1]) if "--albedo" in arguments else None
    bpy.ops.wm.read_factory_settings(use_empty=True)
    fbx = glob.glob(os.path.join(pack, "*.fbx"))
    if len(fbx) != 1:
        raise SystemExit(f"{pack}: expected one FBX, found {len(fbx)}")
    bpy.ops.import_scene.fbx(filepath=fbx[0])

    scratch = tempfile.mkdtemp()
    material = bpy.data.materials.new("detail")
    material.use_nodes = True
    nodes = material.node_tree.nodes
    links = material.node_tree.links
    shader = nodes["Principled BSDF"]

    def image(kind, colour, luminance=None):
        path = texture(pack, kind)
        if path is None:
            return None
        # Re-saved as PNG: the exporter keeps each image's source format.
        source = bpy.data.images.load(path)
        if luminance is not None:
            # Blender hands 8-bit images over in their own (sRGB) encoding.
            pixels = np.empty(len(source.pixels), np.float32)
            source.pixels.foreach_get(pixels)
            rgba = pixels.reshape(-1, 4)
            c = rgba[:, :3]
            linear = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
            mean = float((linear @ np.array([0.2126, 0.7152, 0.0722], np.float32)).mean())
            linear = np.clip(linear * (luminance / max(mean, 1e-6)), 0.0, 1.0)
            rgba[:, :3] = np.where(linear <= 0.0031308, linear * 12.92, 1.055 * linear ** (1 / 2.4) - 0.055)
            source.pixels.foreach_set(pixels)
            print(f"{kind}: mean luminance {mean:.4f} -> {luminance:.4f}")
        png = os.path.join(scratch, f"{kind}.png")
        source.file_format = "PNG"
        source.save(filepath=png)
        node = nodes.new("ShaderNodeTexImage")
        node.image = bpy.data.images.load(png)
        if not colour:
            node.image.colorspace_settings.name = "Non-Color"
        return node

    base = image("BaseColor", True, albedo)
    links.new(base.outputs["Color"], shader.inputs["Base Color"])
    rough = image("Roughness", False)
    if rough is not None:
        links.new(rough.outputs["Color"], shader.inputs["Roughness"])
    normal = image("Normal", False)
    if normal is not None:
        map_node = nodes.new("ShaderNodeNormalMap")
        links.new(normal.outputs["Color"], map_node.inputs["Color"])
        links.new(map_node.outputs["Normal"], shader.inputs["Normal"])
    shader.inputs["Metallic"].default_value = 0.0
    occlusion = image("AO", False)
    if occlusion is not None:
        # The glTF exporter writes an occlusion texture from this node group's input.
        group = bpy.data.node_groups.new("glTF Material Output", "ShaderNodeTree")
        group.interface.new_socket("Occlusion", in_out="INPUT", socket_type="NodeSocketFloat")
        output_node = nodes.new("ShaderNodeGroup")
        output_node.node_tree = group
        links.new(occlusion.outputs["Color"], output_node.inputs["Occlusion"])

    pieces = sorted(
        (o for o in bpy.context.scene.objects if o.type == "MESH"), key=lambda o: o.name
    )
    for index, piece in enumerate(pieces):
        piece.data.materials.clear()
        piece.data.materials.append(material)
        bpy.ops.object.select_all(action="DESELECT")
        piece.select_set(True)
        bpy.context.view_layer.objects.active = piece
        bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
        corners = [Vector(v.co) for v in piece.data.vertices]
        low = Vector((min(c.x for c in corners), min(c.y for c in corners), min(c.z for c in corners)))
        high = Vector((max(c.x for c in corners), max(c.y for c in corners), max(c.z for c in corners)))
        # Blender is Z-up: centre the footprint and stand the piece on the origin.
        shift = Vector(((low.x + high.x) / 2, (low.y + high.y) / 2, low.z))
        for vertex in piece.data.vertices:
            vertex.co -= shift
        piece.location = (0.0, 0.0, 0.0)
        piece.name = f"piece_{index:03}"
        piece.data.name = piece.name

    bpy.ops.export_scene.gltf(
        filepath=output,
        export_format="GLB",
        export_yup=True,
        export_apply=True,
        export_image_format="AUTO",
        export_materials="EXPORT",
    )
    print(f"exported {len(pieces)} pieces to {output}")


main()
