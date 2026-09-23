"""Restore and export the kit's 11 trees and 8 shrubs (summer, mesh LOD0–3).

Run with Blender --background --factory-startup --python this_file --
--source /path/to/Forest_Tree_Starter_Kit. Binaries stay under assets/local;
tracked TOML manifests and a RON import catalog describe the exported assets.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import sys

import bpy

sys.path.insert(0, str(Path(__file__).resolve().parent))
from export_tree_lods import export_lod, runtime_materials, sha256

# Preserve the asset identities established by the legacy kit import.
TREES = [5194, 5639, 5857, 7071, 7339, 7733, 11364, 12762, 14733, 16018, 18195]
SHRUBS = [107, 345, 436, 491, 510, 2558, 2853, 3172]
ROOT = Path(__file__).resolve().parents[1]
PACK = ROOT / "assets/local/forest_tree_starter_kit"
MANIFESTS = ROOT / "assets/packs/forest_tree_starter_kit"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    args = parser.parse_args(sys.argv[sys.argv.index("--") + 1:])
    source = args.source.resolve()
    # Validate the complete source set before replacing any runtime files.
    for triangles in TREES + SHRUBS:
        if not (source / "Model" / f"DA_Forest_Tree_{triangles}_Tris.FBX").is_file():
            raise RuntimeError(f"Missing source model: {triangles}")
    textures = PACK / "source/textures"
    if source / "Textures" != textures:
        shutil.copytree(source / "Textures", textures, dirs_exist_ok=True)
    MANIFESTS.mkdir(parents=True, exist_ok=True)
    catalog = ["(schema_version: 1, assets: ["]
    for kind, triangle_counts in [("tree", TREES), ("shrub", SHRUBS)]:
        for number, triangles in enumerate(triangle_counts, 1):
            slug = f"{kind}_{number:02}"
            key = f"forest_tree_starter_kit/{slug}"
            name = f"Forest {kind.title()} {number:02} (Summer)"
            filename = f"DA_Forest_Tree_{triangles}_Tris.FBX"
            fbx = PACK / "source" / slug / filename
            fbx.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source / "Model" / filename, fbx)
            output = PACK / "runtime" / slug / "summer"
            output.mkdir(parents=True, exist_ok=True)
            bpy.ops.wm.read_factory_settings(use_empty=True)
            bark_variant = "green" if number in (2, 5, 8, 11) else "red"
            bark, foliage = runtime_materials(
                str(textures), 1024, 0.5, bark_variant, "tree" if kind == "tree" else "bush"
            )
            lods = [export_lod(str(fbx), str(output), lod, 0.55, 0.4,
                               bark, foliage, slug, "../../textures") for lod in range(4)]
            # Blender is Z-up; the exported glTF and runtime bounds are Y-up.
            bounds = [max(l.dimensions_m[axis] for l in lods) for axis in (0, 2, 1)]
            uri = str(fbx.relative_to(ROOT / "assets"))
            manifest = [
                "schema_version = 2", f"asset_key = {json.dumps(key)}",
                f"display_name = {json.dumps(name)}", "", "[provenance]",
                'status = "unverified"', "redistributable = false",
                'reason = "The legacy project records neither the original author/source URL nor a redistribution license."',
                "", "[source]", f"local_path = {json.dumps(str(fbx.relative_to(ROOT)))}",
                f"texture_root = {json.dumps(str(textures.relative_to(ROOT)))}",
                'format = "fbx"', f'sha256 = "{sha256(str(fbx))}"',
                'mesh_lods = ["Forest_Tree_Bark_LOD0", "Forest_Tree_Bark_LOD1", "Forest_Tree_Bark_LOD2", "Forest_Tree_Bark_LOD3"]',
                'billboard_status = "unavailable: source texture is missing"',
                "", "[import]", 'tool = "tools/export_forest_pack.py"',
                'runtime_format = "gltf"', "texture_size = 1024",
                'foliage_alpha_mode = "mask"', "foliage_alpha_cutoff = 0.5",
                "foliage_double_sided = true", "source_scale = 0.55",
                "wind_flutter_weight = 0.4", f'bark_variant = "{bark_variant}"',
                'wind_contract = "foliage_uv1_v1"',
                "", "[lod_policy]", 'measurement = "projected logical-pixel height"',
                "hysteresis_fraction = 0.12",
            ]
            catalog += [f"(key: {json.dumps(key)}, display_name: {json.dumps(name)},",
                        f"source_uri: {json.dumps(uri)}, variants: ["]
            used_textures = set()
            for lod, threshold in zip(lods, (320.0, 160.0, 80.0, 0.0), strict=True):
                gltf = output / lod.gltf
                doc = json.loads(gltf.read_text())
                # Also validate every external dependency and the exported cutout contract.
                for dep in doc.get("buffers", []) + doc.get("images", []):
                    if not (output / dep["uri"]).is_file():
                        raise RuntimeError(f"Missing glTF dependency: {dep}")
                leaves = [m for m in doc["materials"] if "leaves" in m["name"]]
                assert len(leaves) == 1 and leaves[0]["alphaMode"] == "MASK"
                assert leaves[0]["doubleSided"]
                assert leaves[0]["extras"]["yarra_wind"] == "foliage_uv1_v1"
                assert all("TEXCOORD_1" in p["attributes"] for m in doc["meshes"] for p in m["primitives"])
                used_textures.update((output / i["uri"]).resolve() for i in doc["images"])
                runtime_uri = str(gltf.relative_to(ROOT / "assets"))
                gpu_bytes = sum(b["byteLength"] for b in doc["buffers"])
                manifest += ["", "[[variants]]", 'appearance = "summer"', f"lod = {lod.lod}",
                             f"runtime_uri = {json.dumps(runtime_uri)}", f'mesh_name = "{lod.mesh_name}"',
                             f"triangles = {lod.triangles}", f"minimum_screen_height = {threshold}",
                             f"runtime_bounds_m = {bounds}", f"gpu_bytes_estimate = {gpu_bytes}",
                             f'gltf_sha256 = "{lod.sha256}"', f'buffer_sha256 = "{lod.buffer_sha256}"']
                catalog += [f"(uri: {json.dumps(runtime_uri)}, bounds: ({', '.join(map(str, bounds))}),",
                            f"gpu_bytes_estimate: {gpu_bytes}, minimum_screen_height: {threshold}),"]
            catalog += ["]),"]
            for texture in sorted(used_textures):
                manifest += ["", "[[textures]]", f'file = "{texture.name}"',
                             f'sha256 = "{sha256(str(texture))}"']
            (MANIFESTS / f"{slug}.toml").write_text("\n".join(manifest) + "\n")
    catalog += ["])"]
    (MANIFESTS / "summer.catalog.ron").write_text("\n".join(catalog) + "\n")
    print("Exported 19 summer assets / 76 mesh LODs. Import summer.catalog.ron with yarra-world-cook import-assets.")


if __name__ == "__main__":
    main()
