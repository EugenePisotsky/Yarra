# Asset layout

The Houdini forest authoring project is tracked separately in
[YarraVegetation](https://github.com/EugenePisotsky/YarraVegetation). Its native
scene, generators, presets, and source-texture inventory are versioned there.
Supplied textures, rendered atlases, and exported meshes remain local in both
projects. Restoring a Git checkout alone does not restore these asset binaries.

`assets/packs/yarra_birches/birches.catalog.ron` records the three imported birch
states and their three mesh LODs. Rebuild them with
`tools/import_vegetation_bundle.py` from the matching authoring bundles (Khronos
KTX 4.4.2, available on PATH or via `--ktx`).
`assets/packs/yarra_oaks/oaks.catalog.ron` records the forest, spreading and sparse
oak forms. Use the same importer with a separate `assets/local/yarra_oaks/` output
pack; the three forms share one texture set and three mesh LODs each.
`assets/packs/yarra_pines/pines.catalog.ron` records the six legacy native-scene
pine variants imported by `tools/houdini_export_tree.py`. Their original scene
and source inputs must also be restored locally. See
[vegetation integration](../docs/WORKFLOWS.md) for the sample placements and cook.
Editor journals and reference captures are local working data as well.

`assets/generated/` contains derived runtime SQLite generations produced by
`yarra-world-cook`. It is ignored because it can be rebuilt from `content/`.

`assets/local/` is intentionally ignored. The legacy Forest Tree Starter Kit
does not record its author, source URL, or redistribution license, so its files
must not enter the public repository until that provenance is recovered.

Tracked pack manifests live in `assets/packs/`. They describe provenance,
expected local files, import decisions, and verified runtime artifacts without
putting licensed source or derived binaries in Git. This gives the editor and
cooker a stable asset identity while storage policy can change independently.

The local pack layout separates source files from engine-ready artifacts:

```text
assets/local/forest_tree_starter_kit/
  source/tree_07/DA_Forest_Tree_11364_Tris.FBX
  source/textures/...
  runtime/tree_07/summer/tree_07_summer_lod{0,1,2,3}.gltf
```

The first terrain pack follows the same local-only rule:

```text
assets/local/terrain/temperate_meadow/
  source/uncut_grass_oilpt20/{base_color.jpg,normal_material.png}
  source/grass_dried_pjwhw0/{base_color.jpg,normal_material.png}
  source/macro_variation.png
  runtime/{universal,astc}/{base_color_array,normal_material_array,macro_variation}.ktx2
```

The first character presentation is also restored from the legacy project into
ignored local storage:

```text
assets/local/characters/female_main/female_main_locomotion.glb
```

Its tracked contract is `assets/packs/characters/female_main.toml`. The GLB is a
validated in-place export containing the body, skeleton, and animation clips;
its expected SHA-256 is recorded in that manifest.

Runtime character composition is separate from pack provenance. The checked-in
`assets/catalogs/character_presentations.catalog.ron` assigns stable IDs to
skeleton contracts, models, animation banks, clips, movement sets, and
presentation profiles. It may reference ignored local assets, but it never
changes their licensing or redistribution policy. See the
[character runtime contract](../docs/ARCHITECTURE.md#characters-and-editor-lifecycle).

After restoring the five source images from the legacy repository, compile the
portable UASTC and native iOS ASTC variants with:

```bash
python3 tools/compile_terrain_textures.py
```

Layer order and source hashes are tracked in
`assets/packs/terrain/temperate_meadow.toml`. The SQLite terrain catalog refers
to these runtime URIs; source images and compiled KTX2 files remain ignored.

The summer catalog now covers all **11 trees and 8 shrubs**, each with authored
LOD0–LOD3 (76 mesh variants). Bark colors follow the source pack's red/green
assignments. Six shared 1024-pixel textures live in `runtime/textures/`.
Seasonal alternatives and billboard baking are deferred.

Foliage materials carry the explicit `foliage_uv1_v1` wind contract in glTF
extras. UV1 retains the source's flutter/branch weights across all four LODs.
The game and editor use those weights with their shared wind field; no FBX or
database conversion is performed at runtime. Re-export restored older local
files to add the metadata required by the tree wind renderer.

Restore and convert the complete pack with Blender, then register it in the
existing project database:

```bash
/Applications/Blender.app/Contents/MacOS/Blender \
  --background --factory-startup --python-exit-code 1 \
  --python tools/export_forest_pack.py -- \
  --source /path/to/content/Forest_Tree_Starter_Kit
cargo run -p yarra-world-cook -- import-assets \
  assets/packs/forest_tree_starter_kit/summer.catalog.ron
```

The exporter copies source files into ignored local storage and regenerates
the tracked per-asset manifests and catalog. Registration is transactional and
repeatable, preserving existing placements, collection references and stable
IDs. An optional final `PROJECT_DB` argument selects a different project.
Reopen the editor after registration to refresh its asset palette. Use manual
placement or an Asset collection to scatter the imported models, then Save &
Publish. Importing does not change the world's existing tree distribution.

To regenerate only Tree 07 (with its own texture directory):

```bash
/Applications/Blender.app/Contents/MacOS/Blender \
  --background --factory-startup \
  --python tools/export_tree_lods.py -- \
  --fbx assets/local/forest_tree_starter_kit/source/tree_07/DA_Forest_Tree_11364_Tris.FBX \
  --textures assets/local/forest_tree_starter_kit/source/textures \
  --output assets/local/forest_tree_starter_kit/runtime/tree_07/summer
```

Tree 07's variants contain 11,364, 4,747, 2,120, and 735 triangles and share the same
four 1024-pixel bark/leaf textures. The source billboard cannot be imported
because its texture is missing; a baked billboard can be added later as LOD4.

See `assets/packs/forest_tree_starter_kit/*.toml` for the local contracts and
hashes. Full-pack export refreshes these hashes; single-tree export produces
its own `.export.json` report. Missing local files are expected in a fresh public clone;
the game can still cook the world database but cannot render this tree until the
pack is restored locally.

## Houdini trees

`assets/local/yarra_trees/` holds forest trees generated in Houdini
(`/obj/FOREST_TREE` in the tree scene). The leaf sprig atlas (Davidia involucrata)
and bark textures stay local. Leaves are 0.75 m branch-cluster cards: the scene's
`BAKE_CLUSTERS` node arranges sprigs on small branches and renders four of them into a
2×2 atlas, so one card carries a whole leafy branch. Export from the repository with
Houdini's Python:

```bash
hython tools/houdini_export_tree.py /path/to/forest_tree.hiplc assets/local/yarra_trees \
  --catalog assets/packs/yarra_trees/trees.catalog.ron --lod-heights 640,320,160,0 \
  --variant forest_a:1:19:5.8:8.5 --variant forest_b:2:17.5:5.2:7.5 \
  --variant forest_c:3:20:6.2:9
cargo run -p yarra-world-cook -- import-assets assets/packs/yarra_trees/trees.catalog.ron
```

Each variant is `NAME:SEED:HEIGHT:WIDTH[:CROWN_BASE]` and gets LOD0–3. LOD1 only drops
cards hidden deep in the crown; LOD2 and LOD3 keep nested subsets of enlarged cards, so
the crown keeps its leaf area. The 640/320/160/0 thresholds put LOD0 within about 35 m
of a 19 m tree, where the Forest Tree Starter Kit's 320/160/80/0 keep it to about 70 m;
switches crossfade (see `ObjectLodPlugin`). `--set PARM=VALUE` overrides a CONTROLS
parameter for an export. The exporter prints each LOD's triangles and leaf-card area;
the area (overlapping alpha-tested layers) drives GPU cost more than triangles do.

The six shared textures are written once to `runtime/textures/` as mipmapped UASTC
KTX2, which needs the Khronos `ktx` 4.4.2 tool used for terrain. Leaf colour mips keep
their alpha coverage, so crowns do not thin out with distance. A copy of the scene goes
to `source/`, because the importer requires a local source file. `--preview DIR` also
writes PNG-textured glTFs for Blender or other viewers.

Leaf cards are lit by the crown, not by their own orientation. `NORMAL` is the
smoothed crown surface normal (blended 85% with the card) and `COLOR_0` is
crown-depth occlusion. The foliage material is tagged `yarra_shading: "crown_v1"`:
both sides of a card keep the crown normal (Bevy's two-sided flip would light every
back face as if it faced into the crown), and the occlusion dims only sky and ambient
light, because shadow maps already darken the sun. Leaves are kept rough and use half
the default reflectance (`KHR_materials_specular`), since glossy sky reflection off
crown normals turns shaded leaves grey. It also carries the usual `yarra_wind`
contract (TEXCOORD_1 = flutter, branch); bark is opaque and rigid, with its
occlusion multiplied into base colour.
