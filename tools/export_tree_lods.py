"""Export one Forest Tree Starter Kit FBX as shared-texture glTF LODs.

Run through Blender, not the system Python:

    /Applications/Blender.app/Contents/MacOS/Blender \
      --background --factory-startup \
      --python tools/export_tree_lods.py -- \
      --fbx assets/local/forest_tree_starter_kit/source/tree_07/DA_Forest_Tree_11364_Tris.FBX \
      --textures assets/local/forest_tree_starter_kit/source/textures \
      --output assets/local/forest_tree_starter_kit/runtime/tree_07/summer

The source FBX contains LOD0 through LOD3. Its LOD4 billboard refers to a
missing texture, so this tool deliberately does not export it as a usable LOD.
"""

from __future__ import annotations

import argparse
import bmesh
import bpy
from dataclasses import asdict, dataclass
import hashlib
import json
from mathutils import Matrix, Vector
import math
import os
import sys


@dataclass(frozen=True)
class LodExport:
    lod: int
    mesh_name: str
    gltf: str
    triangles: int
    dimensions_m: list[float]
    sha256: str
    buffer_sha256: str


def parse_args() -> argparse.Namespace:
    script_args = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    parser = argparse.ArgumentParser()
    parser.add_argument("--fbx", required=True)
    parser.add_argument("--textures", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--scale", type=float, default=0.55)
    parser.add_argument("--texture-size", type=int, default=1024)
    parser.add_argument("--alpha-cutoff", type=float, default=0.5)
    parser.add_argument("--flutter-weight", type=float, default=0.4)
    return parser.parse_args(script_args)


def load_scaled_image(
    path: str,
    name: str,
    maximum_size: int,
    *,
    color_space: str,
) -> bpy.types.Image:
    image = bpy.data.images.load(path, check_existing=False)
    image.name = name
    image.colorspace_settings.name = color_space
    width, height = image.size
    longest_side = max(width, height)
    if longest_side <= 0:
        raise RuntimeError(f"Texture {path!r} has no pixels")
    if longest_side > maximum_size:
        scale = maximum_size / longest_side
        image.scale(max(1, round(width * scale)), max(1, round(height * scale)))
    image.file_format = "PNG"
    image.update()
    image.pack()
    return image


def principled_material(name: str) -> tuple[bpy.types.Material, bpy.types.Node]:
    material = bpy.data.materials.new(name)
    material.use_nodes = True
    material.use_backface_culling = True
    nodes = material.node_tree.nodes
    nodes.clear()
    output = nodes.new("ShaderNodeOutputMaterial")
    shader = nodes.new("ShaderNodeBsdfPrincipled")
    shader.inputs["Metallic"].default_value = 0.0
    shader.inputs["Roughness"].default_value = 0.84
    material.node_tree.links.new(shader.outputs["BSDF"], output.inputs["Surface"])
    return material, shader


def textured_material(
    name: str,
    color: bpy.types.Image,
    normal: bpy.types.Image,
    *,
    alpha_cutoff: float | None = None,
) -> bpy.types.Material:
    material, shader = principled_material(name)
    nodes = material.node_tree.nodes

    color_node = nodes.new("ShaderNodeTexImage")
    color_node.name = f"{name} color"
    color_node.image = color
    material.node_tree.links.new(color_node.outputs["Color"], shader.inputs["Base Color"])

    normal_node = nodes.new("ShaderNodeTexImage")
    normal_node.name = f"{name} normal"
    normal_node.image = normal
    normal_node.image.colorspace_settings.name = "Non-Color"
    normal_map = nodes.new("ShaderNodeNormalMap")
    normal_map.inputs["Strength"].default_value = 0.7
    material.node_tree.links.new(normal_node.outputs["Color"], normal_map.inputs["Color"])
    material.node_tree.links.new(normal_map.outputs["Normal"], shader.inputs["Normal"])

    if alpha_cutoff is not None:
        material.use_backface_culling = False
        material.blend_method = "CLIP"
        material.alpha_threshold = alpha_cutoff
        clip = nodes.new("ShaderNodeMath")
        clip.name = "Runtime alpha clip"
        clip.operation = "GREATER_THAN"
        clip.inputs[1].default_value = alpha_cutoff
        material.node_tree.links.new(color_node.outputs["Alpha"], clip.inputs[0])
        material.node_tree.links.new(clip.outputs[0], shader.inputs["Alpha"])
    return material


def runtime_materials(
    texture_root: str,
    texture_size: int,
    alpha_cutoff: float,
) -> tuple[bpy.types.Material, bpy.types.Material]:
    bark_color = load_scaled_image(
        os.path.join(texture_root, "Bark_RedVariant.tif"),
        "forest_starter_bark_red_runtime",
        texture_size,
        color_space="sRGB",
    )
    bark_normal = load_scaled_image(
        os.path.join(texture_root, "Bark_Normal.tif"),
        "forest_starter_bark_normal_runtime",
        texture_size,
        color_space="Non-Color",
    )
    leaf_root = os.path.join(texture_root, "Leave")
    leaf_color = load_scaled_image(
        os.path.join(leaf_root, "Tree_Leaves_SummerVariant.tif"),
        "forest_starter_tree_summer_leaves_runtime",
        texture_size,
        color_space="sRGB",
    )
    leaf_normal = load_scaled_image(
        os.path.join(leaf_root, "Leave_Normal.tif"),
        "forest_starter_leaf_normal_runtime",
        texture_size,
        color_space="Non-Color",
    )
    return (
        textured_material("forest_starter_bark_red", bark_color, bark_normal),
        textured_material(
            "forest_starter_tree_summer_leaves",
            leaf_color,
            leaf_normal,
            alpha_cutoff=alpha_cutoff,
        ),
    )


def extract_lod_mesh(
    fbx_path: str,
    lod: int,
    scale: float,
    flutter_weight: float,
    bark: bpy.types.Material,
    foliage: bpy.types.Material,
) -> tuple[bpy.types.Mesh, Vector, int]:
    before = set(bpy.data.objects)
    bpy.ops.import_scene.fbx(filepath=fbx_path, use_image_search=False)
    imported = set(bpy.data.objects) - before
    expected_name = f"Forest_Tree_Bark_LOD{lod}"
    candidates = [
        item for item in imported if item.type == "MESH" and item.name == expected_name
    ]
    if len(candidates) != 1:
        names = ", ".join(sorted(item.name for item in imported))
        raise RuntimeError(f"Expected one {expected_name!r}; imported: {names}")
    source = candidates[0]
    if len(source.data.materials) != 2:
        raise RuntimeError(f"Expected two material slots on {expected_name!r}")

    mesh = source.data.copy()
    mesh.transform(source.matrix_world)
    mesh.transform(Matrix.Scale(scale, 4))
    while len(mesh.uv_layers) > 1:
        mesh.uv_layers.remove(mesh.uv_layers[-1])

    edit_mesh = bmesh.new()
    edit_mesh.from_mesh(mesh)
    bmesh.ops.triangulate(edit_mesh, faces=tuple(edit_mesh.faces))
    edit_mesh.to_mesh(mesh)
    edit_mesh.free()

    if len(mesh.color_attributes) != 1:
        raise RuntimeError(f"Expected one wind color attribute on {expected_name!r}")
    wind_attribute = mesh.color_attributes[0]
    if wind_attribute.domain != "CORNER":
        raise RuntimeError(f"Expected per-corner wind colors on {expected_name!r}")
    wind_uv = mesh.uv_layers.new(name="wind_weights")
    for polygon in mesh.polygons:
        for loop_index in polygon.loop_indices:
            alpha = wind_attribute.data[loop_index].color_srgb[3]
            if polygon.material_index == 1:
                wind_uv.data[loop_index].uv = (alpha * flutter_weight, alpha)
            else:
                wind_uv.data[loop_index].uv = (0.0, 0.0)
    while mesh.color_attributes:
        mesh.color_attributes.remove(mesh.color_attributes[-1])

    minimum = Vector(min(vertex.co[axis] for vertex in mesh.vertices) for axis in range(3))
    maximum = Vector(max(vertex.co[axis] for vertex in mesh.vertices) for axis in range(3))
    center = (minimum + maximum) * 0.5
    mesh.transform(Matrix.Translation((-center.x, -center.y, -minimum.z)))

    material_indices = [polygon.material_index for polygon in mesh.polygons]
    mesh.materials.clear()
    mesh.materials.append(bark)
    mesh.materials.append(foliage)
    for polygon, material_index in zip(mesh.polygons, material_indices, strict=True):
        polygon.material_index = material_index
    mesh.update()

    imported_meshes = [item.data for item in imported if item.type == "MESH"]
    for item in imported:
        bpy.data.objects.remove(item, do_unlink=True)
    for imported_mesh in imported_meshes:
        if imported_mesh.users == 0:
            bpy.data.meshes.remove(imported_mesh)

    dimensions = maximum - minimum
    triangles = len(mesh.polygons)
    return mesh, dimensions, triangles


def sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for block in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def export_lod(
    fbx_path: str,
    output_dir: str,
    lod: int,
    scale: float,
    flutter_weight: float,
    bark: bpy.types.Material,
    foliage: bpy.types.Material,
) -> LodExport:
    mesh, dimensions, triangles = extract_lod_mesh(
        fbx_path, lod, scale, flutter_weight, bark, foliage
    )
    mesh_name = f"tree_07_summer_lod{lod}"
    mesh.name = f"{mesh_name}_mesh"
    exported = bpy.data.objects.new(mesh_name, mesh)
    bpy.context.scene.collection.objects.link(exported)
    bpy.ops.object.select_all(action="DESELECT")
    exported.select_set(True)
    bpy.context.view_layer.objects.active = exported

    filename = f"tree_07_summer_lod{lod}.gltf"
    output_path = os.path.join(output_dir, filename)
    bpy.ops.export_scene.gltf(
        filepath=output_path,
        check_existing=False,
        export_format="GLTF_SEPARATE",
        export_texture_dir="textures",
        use_selection=True,
        export_apply=True,
        export_yup=True,
        export_animations=False,
        export_cameras=False,
        export_lights=False,
        export_materials="EXPORT",
        export_image_format="AUTO",
        export_tangents=True,
        export_texcoords=True,
        export_normals=True,
        export_shared_accessors=True,
    )

    buffer_path = os.path.splitext(output_path)[0] + ".bin"
    result = LodExport(
        lod=lod,
        mesh_name=mesh_name,
        gltf=filename,
        triangles=triangles,
        dimensions_m=[round(value, 4) for value in dimensions],
        sha256=sha256(output_path),
        buffer_sha256=sha256(buffer_path),
    )
    bpy.data.objects.remove(exported, do_unlink=True)
    bpy.data.meshes.remove(mesh)
    print(f"Exported {mesh_name}: {triangles} triangles")
    return result


def main() -> None:
    args = parse_args()
    if args.texture_size <= 0:
        raise RuntimeError("--texture-size must be positive")
    if not math.isfinite(args.scale) or args.scale <= 0.0:
        raise RuntimeError("--scale must be finite and greater than zero")
    if not math.isfinite(args.alpha_cutoff) or not 0.0 < args.alpha_cutoff < 1.0:
        raise RuntimeError("--alpha-cutoff must be between zero and one")
    if not math.isfinite(args.flutter_weight) or not 0.0 <= args.flutter_weight <= 1.0:
        raise RuntimeError("--flutter-weight must be between zero and one")

    fbx_path = os.path.abspath(args.fbx)
    texture_root = os.path.abspath(args.textures)
    output_dir = os.path.abspath(args.output)
    os.makedirs(output_dir, exist_ok=True)

    bpy.ops.wm.read_factory_settings(use_empty=True)
    bark, foliage = runtime_materials(texture_root, args.texture_size, args.alpha_cutoff)
    lods = [
        export_lod(
            fbx_path,
            output_dir,
            lod,
            args.scale,
            args.flutter_weight,
            bark,
            foliage,
        )
        for lod in range(4)
    ]
    texture_files = []
    texture_output = os.path.join(output_dir, "textures")
    for filename in sorted(os.listdir(texture_output)):
        path = os.path.join(texture_output, filename)
        if os.path.isfile(path):
            texture_files.append({"file": filename, "sha256": sha256(path)})
    manifest = {
        "schema_version": 1,
        "source_fbx": os.path.relpath(fbx_path),
        "source_sha256": sha256(fbx_path),
        "scale": args.scale,
        "texture_size": args.texture_size,
        "alpha_cutoff": args.alpha_cutoff,
        "flutter_weight": args.flutter_weight,
        "lods": [asdict(item) for item in lods],
        "textures": texture_files,
        "billboard": {"status": "unavailable", "reason": "source texture is missing"},
    }
    manifest_path = os.path.join(output_dir, "tree_07_summer.export.json")
    with open(manifest_path, "w", encoding="utf-8") as file:
        json.dump(manifest, file, indent=2)
        file.write("\n")
    print(f"Wrote {manifest_path}")


if __name__ == "__main__":
    main()
