# Asset layout

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

Regenerate the four local variants with Blender:

```bash
/Applications/Blender.app/Contents/MacOS/Blender \
  --background --factory-startup \
  --python tools/export_tree_lods.py -- \
  --fbx assets/local/forest_tree_starter_kit/source/tree_07/DA_Forest_Tree_11364_Tris.FBX \
  --textures assets/local/forest_tree_starter_kit/source/textures \
  --output assets/local/forest_tree_starter_kit/runtime/tree_07/summer
```

The variants contain 11,364, 4,747, 2,120, and 735 triangles and share the same
four 1024-pixel bark/leaf textures. The source billboard cannot be imported
because its texture is missing; a baked billboard can be added later as LOD4.

See `assets/packs/forest_tree_starter_kit/tree_07.toml` for the exact local
contract and hashes. Missing local files are expected in a fresh public clone;
the game can still cook the world database but cannot render this tree until the
pack is restored locally.
