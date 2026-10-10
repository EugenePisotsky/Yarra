# Asset layout

The Houdini forest authoring project is tracked separately in
[YarraVegetation](https://github.com/EugenePisotsky/YarraVegetation). Its native
scene, generators, presets, and source-texture inventory are versioned there.
Supplied textures, rendered atlases, and exported meshes remain local in both
projects. Restoring a Git checkout alone does not restore these asset binaries.

For new trees or variants, use the
[authoring guide](https://github.com/EugenePisotsky/YarraVegetation/blob/main/TREE_AUTHORING.md)
and [game integration workflow](../docs/WORKFLOWS.md#creating-and-revising-trees).
Import complete texture-sharing families together. The current 16 forms use
`hierarchy_v2` bindings on bark and cards; rebuilt bundles require the matching
importer and engine. Legacy exports retain their earlier wind path.

`assets/packs/yarra_birches/birches.catalog.ron` records the three imported birch
states and their three mesh LODs. Rebuild them with
`tools/import_vegetation_bundle.py` from the matching authoring bundles (Khronos
KTX 4.4.2, available on PATH or via `--ktx`).
`assets/packs/yarra_oaks/oaks.catalog.ron` records the forest, spreading and sparse
oak forms. Use the same importer with a separate `assets/local/yarra_oaks/` output
pack; the three forms share one texture set and three mesh LODs each.
`assets/packs/yarra_maples/maples.catalog.ron` records forest, spreading and sparse
maples, rebuilt into `assets/local/yarra_maples/` with the same importer. Both
source leaf packages are combined into one shared texture set; the current bark
uses the supplied elm texture as a provisional stand-in.
`assets/packs/yarra_tall_forest/tall_forest.catalog.ron` records the taller layered
broadleaf prototype inspired by Witcher reference IMG_1369. It uses
small pointed qgCoa2 leaves on slim fixed sprays and compact facing bunches,
with brown elm bark, and imports into `assets/local/yarra_tall_forest/`.
The editable generator and preset live in YarraVegetation.
`assets/packs/yarra_spruces/spruces.catalog.ron` records the Norway spruce
prototype in `assets/local/yarra_spruces/`. It uses the supplied qgpvu2 shoots on
fixed V-shaped sprays and provisional pine bark. Main folds remain through all
three mesh LODs. The placement helper adds three review instances beside the
longleafs and birches; use `spruce-stand`. See the workflow for rebuilding.
Editor journals and reference captures are local working data as well.

`assets/generated/` contains derived runtime SQLite generations produced by
`yarra-world-cook`. It is ignored because it can be rebuilt from `content/`.

`assets/local/` is intentionally ignored: supplied sources and derived binaries
stay out of the public repository.

Tracked pack manifests live in `assets/packs/`. They describe provenance,
expected local files, import decisions, and verified runtime artifacts without
putting licensed source or derived binaries in Git. This gives the editor and
cooker a stable asset identity while storage policy can change independently.

The local pack layout separates source files from engine-ready artifacts:

```text
assets/local/yarra_trees/
  source/forest_tree.hiplc
  runtime/forest_b/forest_b_lod{0,1,2,3}.{gltf,bin}
```

The terrain pack follows the same local-only rule:

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
of a 19 m tree; switches crossfade (see `ObjectLodPlugin`). `--set PARM=VALUE` overrides a CONTROLS
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
back face as if it faced into the crown). Occlusion dims sky and ambient light;
past the shadow range it also supplies a coarse self-shadow fallback. Leaves are kept rough and use half
the default reflectance (`KHR_materials_specular`), since glossy sky reflection off
crown normals turns shaded leaves grey. It also carries the usual `yarra_wind`
contract (TEXCOORD_1 = flutter, branch); bark is opaque and rigid, with its
occlusion multiplied into base colour.

The tall-tree lighting trial exports `vegetation_occlusion: "crown_sky_v1"`.
`import_vegetation_bundle.py` preserves this marker and selects
`yarra_shading: "crown_v2"`. Its vertex colors contain spatial sky visibility
from the complete crown before LOD thinning. Facing clumps retain constant AO
at their pivot; the approved geometry and foliage textures are unchanged.
Near direct sunlight still uses dynamic shadows. Over the outer 30% of the
shadow range, `crown_v2` blends to coarse self-shadowing from AO and the stable
crown normal relative to the light. Sun-facing areas stay brighter than sheltered
areas after shadow maps run out. It adds no texture samples or geometry.
Unmarked bundles keep `crown_v1`. This is an art trial, not final foliage shading;
it does not extend ground-shadow range or implement runtime billboards.

The bay shrub trial is rebuilt from vegetation's `bay_upright` bundle using
`tools/import_vegetation_bundle.py`, with tracked catalog
`packs/yarra_bay/bay.catalog.ron` and local textures/geometry in `local/yarra_bay`.

`packs/yarra_longleaf/longleaf.catalog.ron` registers the current four-form pine
kit: healthy, half bare, nearly bare with top needles, and mostly one-sided.
Local source/runtime geometry is under `local/yarra_longleaf`.
It uses rfefw2 needle scans on upward forked sprays, with 2,662 / 1,346 / 676
triangles and 23 centered facing cards. Every branch group retains a fixed spray
at far LOD. The existing renderer and other tree assets are unchanged by this
kit. All four forms share their textures and woody scaffold; bare fork cards
remain fixed. Review the forms in the LOD lab or in a forest planted by
`tools/forest_plan.py` ([workflows](../docs/WORKFLOWS.md#creating-and-revising-trees)).

The earlier native-scene pine pack, shared-pipeline pine experiments and isolated
branch studies have been removed from game assets. Authoring experiments remain
in YarraVegetation for historical reference; use the longleaf kit for game content.

## Map symbols and lettering

`tools/render_world_map.py` reads two free sets from `assets/local/map/` (ignored by Git):

```text
assets/local/map/homann/   K.M. Alexander's Homann Cartography Brushes 2.0 PNG Pack (CC0),
                           unpacked as distributed: Landforms/, Flora/, Settlements/, Cartouches/
assets/local/map/fonts/    IMFeENrm28P.ttf, IMFeENit28P.ttf, IMFeENsc28P.ttf: IM Fell English
                           roman, italic and small caps (SIL Open Font License)
```

Download the brushes from <https://kmalexander.com/free-stuff/fantasy-map-brushes/> and the
fonts from Google Fonts (`ofl/imfellenglish`, `ofl/imfellenglishsc`). CC0 needs no
attribution. A shipped game that includes the fonts must include the OFL text.

