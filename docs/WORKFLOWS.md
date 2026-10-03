# Workflows

Commands run from the repository root. [Architecture](ARCHITECTURE.md) describes ownership; [performance](PERFORMANCE.md) describes controlled measurement.

## Prepare, edit and publish

Restore local assets according to [pack contracts](../assets/README.md). Licensed sources and generated databases stay ignored. After restoring terrain inputs:

```sh
python3 tools/compile_terrain_textures.py
python3 tools/prepare_terrain_bake.py
cargo run -p yarra-world-cook -- init
cargo run -p yarra-app-editor
cargo run --release -p yarra-app-game
```

`init` creates source only if absent, then cooks it: the procedural Phase 0 island (8 km of sea and island, a few minutes to cook), with start views in `content/world.project.views/` (`spawn`, `beach`, `summit`, `hills`), e.g. `cargo run --release -p yarra-app-game -- --start-view content/world.project.views/beach.ron`. The local default world is currently imported from Houdini instead (see [Terrain from Houdini](#terrain-from-houdini)). To start over, delete the project and runtime databases and run `init` again. `cook` continues the existing runtime: it recompiles only cells whose sources changed and updates the terrain hierarchy and ground composites above them. Ground-composite cores are kept in `content/world.project.cook-cache.sqlite` for this (about 570 MB for the island). Deleting it is safe; the next cook re-evaluates what it needs. `cook --full` rebuilds the runtime from scratch. `cook` requires existing source and never creates a demo. The editor defaults to `content/world.project.sqlite`; the game reads `assets/generated/world.runtime.sqlite`. A fresh public clone needs the local pack inputs before ordinary material cooking succeeds.

**Save** writes source edits. **Save & Publish** also cooks and atomically replaces the runtime. To publish from a terminal:

```sh
cargo run -p yarra-world-cook -- cook
```

Explicit paths: `init PROJECT_DB RUNTIME_DB`, `cook PROJECT_DB RUNTIME_DB`, editor `--project-db PROJECT_DB --world-db RUNTIME_DB`, game `--world-db RUNTIME_DB`. The editor validates the source/runtime pair before opening its window. Recook outdated runtime data; source incompatibility requires an explicit replacement decision, not an automatic reset.

### Terrain from Houdini

A heightfield from a terrain tool defines the world: its footprint sets the cells, and its heights replace the default world's terrain. Houdini's `hython` (Apprentice or Indie) writes the neutral format, float32 heights plus a JSON manifest:

```sh
/Applications/Houdini/Current/Frameworks/Houdini.framework/Versions/Current/Resources/bin/hython \
  tools/houdini_export_heightfield.py ~/Dev/world_next.hipnc /tmp/island --start 2500 4330
cargo run --release -p yarra-world-cook -- import-heightfield /tmp/island/heightfield.json
cargo run --release -p yarra-app-game
```

The script exports the display node's `height` volume (or `--node SOP`) with Houdini's axes unchanged: in the top view +X is right and +Z is down. `import-heightfield MANIFEST [PROJECT_DB] [RUNTIME_DB]` creates the project if it is missing, then cooks. It samples the heightfield every metre with smooth (Catmull-Rom) interpolation and paints ground from height and slope. The world is widened to whole 1 km blocks of flat sea so the terrain hierarchy closes. The `--start` point (by default the shore nearest the centre) becomes the world's start: the game and editor begin there unless `--start-view` overrides it. `start` and `summit` views are also written beside the project. Moving the start recooks nothing. Re-importing after a change in Houdini rewrites only cells whose heights or paint changed, so the cook that follows is incremental. Objects and roads in the project are kept; a cell the new footprint no longer covers is removed and fails if it still holds them. Sculpt in Houdini, not in the editor: a re-import replaces heights.

To stress streaming on foot, `--render-repro actor-walk --start-view VIEW` walks the player along the view's `route` at 20 m/s. The camera follows, holds its heading for 60 s, then looks back and forth every 20 s. Add `--fps 60` to match a 60 Hz display. `ACTOR_WALK` lines log progress. Whenever an actor waits more than 5 s for ground, the game logs `TERRAIN_STALL` with the loader's state; a loader that stops for good logs `TERRAIN_LOD_FAILED`. To send a log of a normal session: `cargo run --release -p yarra-app-game 2>&1 | tee tmp/walk.log`.

## Game and F1

- Click/tap ground for a destination; WASD/left gamepad stick gives camera-relative movement.
- Right-drag, horizontal trackpad/two-finger gestures or right stick orbit. Wheel/pinch/vertical gestures zoom.
- Tab cycles demo world spaces only with `--debug-world-switch`; ordinary launches have no world-switch shortcut.
- F1 or **Performance** opens the panel; Escape closes it or cancels capture. `--performance-open` starts open. `--diagnostics panel` keeps F1 with frame/app timing only; `--diagnostics off` omits F1 and timing instrumentation. Full diagnostics remain the default.
- The persistent **FPS limit** button cycles Follow display → 30 → 60 → 120. `--fps N` accepts 0 or 15–240; Reset restores the launch cap, including custom values. VSync remains enabled.
- Quality controls resolution (100/75/50/33%), upscaler, MSAA, density and terrain/object detail. Auto/Spatial/Linear are normal choices; Temporal remains an explicit prototype.
- Weather follows a random sequence by default. `--weather clear|scattered|overcast|rain|storm` starts in a held preset; `--weather authored` shows the published profile unchanged. **F1 → Weather** forces presets (60 s, 10 s or instant blends), toggles the automatic sequence, starts the next change and accelerates the weather clock (1×/10×/60×/stopped). Rain and Storm draw rain streaks, splashes and, as wetness builds up (about 1.5 minutes to soak, 4 minutes to dry), wet surfaces and puddles; ground under trees and other objects stays drier. **Rain rendering** hides streaks and splashes for comparisons.

On macOS 14+, capped modes coordinate display callbacks and the Metal minimum presentation interval. Other platforms/older macOS use the timer fallback. This changes app pacing, not the display's system setting. The normal launch follows the display and uses 50% resolution with 4× MSAA.

F1 Advanced owns terrain macro variation, terrain page gizmos and canopy reload. Reset and A/B restore include those settings and the actual canopy values. B/G/H/U/V and the old X/P/O/L/K/I renderer handlers were removed. Normal grass always uses the published catalog; separate legacy overlays and banding controls are gone.

## World authoring

World and Inspector are the default windows. Tools opens optional Assets, Navigator and Diagnostics. Switching workspaces retains source drafts/history; opening a window does not automatically activate its authoring tool.

Navigation: right/middle drag orbits, Shift+right drag pans, wheel/pinch zooms, and right mouse + WASD/QE flies. Navigator bookmarks can be passed to either app as `--start-view FILE`; the file stores a logical view rather than a screenshot.

**Objects:** select visible objects or stable IDs in the bounded asset tree. Shift/Cmd-click builds a multi-selection. 1/2/3 selects move/yaw/uniform-scale gizmos. Delete/Backspace is undoable. Cmd+Z / Cmd+Shift+Z undo/redo; Cmd+S saves. The inspector affects the active object; gizmo operations can apply to selected companions. Source changes are represented by temporary proxies until publication.

**Environment:** choose World → Environment, select a layer, drag to paint, Shift-drag to erase. One stroke is one undo step. Inspector supports Nearby/All, layer creation/order, coverage and per-use settings. **Edit shared preset…** opens Presets with an isolated ground/grass/object preview. **Apply preset changes** and **Apply settings** are distinct undo steps.

**Collections:** Presets → Environment → New → Asset collection. Select registered tree/bush/rock assets, weights, scales, spacing, slope and road clearance. Paint a collection directly or include it in a composition. Layer density/seed control deterministic generated placements. Generated objects are not individually editable; use manual placement when independent identity/editing is needed.

### Creating and revising trees

Tree authoring lives in [YarraVegetation](https://github.com/EugenePisotsky/YarraVegetation),
not in this game's runtime exporter. Its
[authoring guide](https://github.com/EugenePisotsky/YarraVegetation/blob/main/TREE_AUTHORING.md)
records the current recipes, branch/card design lessons, how to add variants or
species, and the LOD/wind contract. Start there for shape changes. Use
`tools/import_vegetation_bundle.py` for the current kit; the older
`houdini_export_tree.py` and Forest Tree Starter Kit paths below reproduce
separate legacy assets.

| Current family | Runtime pack / tracked catalog under `assets/packs/` | Review view |
| --- | --- | --- |
| Birch: leafy, sparse, bare, crown, pendulous, double, triple | `yarra_birches/birches.catalog.ron` | See birch placement workflow below |
| Oak: forest, spreading, sparse | `yarra_oaks/oaks.catalog.ron` | See oak placement workflow below |
| Maple: forest, spreading, sparse | `yarra_maples/maples.catalog.ron` | See maple placement workflow below |
| Tall layered broadleaf | `yarra_tall_forest/tall_forest.catalog.ron` | `tall-forest-stand` |
| Bay shrub | `yarra_bay/bay.catalog.ron` | `bay-stand` |
| Longleaf: healthy, half-bare, nearly-bare, one-sided | `yarra_longleaf/longleaf.catalog.ron` | `longleaf-kit` |
| Norway spruce | `yarra_spruces/spruces.catalog.ron` | `spruce-stand` |
| Dead broadleaf: upright, spreading, split, slender, double, triple | `yarra_dead_trees/dead_trees.catalog.ron` | `dead-trees-stand`, `dead-trees-slender-stand` |

These are 26 approved forms, each with three mesh LODs. Legacy pine packs and branch-study
assets remain archived; do not restore their retired preview placements when
adding new pine variants. The current pine scaffold is `pine_longleaf`.

1. Build and validate the selected family in YarraVegetation. Keep the seed,
   source package and settings in a preset, with a separate output name for an
   experiment. Preserve an approved bundle if a geometry comparison is needed.
2. Import **every variant in the destination pack in one invocation**, repeating
   `--bundle` for each. The importer replaces that pack, rather than appending
   to it. It requires matching shared texture bytes, compresses one texture set
   with Khronos KTX 4.4.2, and preserves coverage mipmaps. Use an explicit
   `--output` and `--catalog`; their defaults target the birch pack. Split
   variants into separate packs if they intentionally use different atlases.
3. Register the regenerated catalog, then place only genuinely new samples and
   cook the world. Re-register even if URIs are unchanged: mesh memory estimates
   can change when vertex attributes change. Registration preserves placements;
   it does not publish by itself. Existing shape updates need no placement rerun.
4. Restart the game for a reliable check of new meshes/materials. Do not restart
   a user's active tuning session without coordinating it: F1 wind settings are
   temporary. Save requested tuning as source defaults separately.

The spruce is a complete single-member example; the family-specific commands
below show multi-bundle packs:

```sh
python3 tools/import_vegetation_bundle.py --ktx /path/to/ktx \
  --bundle /path/to/YarraVegetation/outputs/spruce_forest/current \
  --output assets/local/yarra_spruces \
  --catalog assets/packs/yarra_spruces/spruces.catalog.ron
cargo run --release -p yarra-world-cook -- import-assets assets/packs/yarra_spruces/spruces.catalog.ron
# Only needed to create missing preview instances/views:
python3 tools/place_spruce_preview.py
cargo run --release -p yarra-world-cook -- cook
cargo run --release -p yarra-app-game -- --start-view content/world.project.views/spruce-stand.ron
```

For another family, adapt the existing `place_spruce_preview.py` or
`place_longleaf_preview.py` pattern: unique stable sample IDs, terrain-relative
placement, a source SQLite backup, and preservation of unrelated objects and
existing bookmark edits. Keep new review samples near the birches/longleaf row
around X 2474–2551, Z 4346, with room between crowns. Add whole/close/below/far/
overhead bookmarks. A few rotated/scaled samples help review a shape; they do
not substitute for independently authored variants. Preview scripts describe
local world edits; Git does not carry the resulting SQLite files.

Check identifiable leaves/needles and mesh-to-card joins at character scale,
the whole silhouette from several directions, moving-camera facing behavior,
LOD transitions and reduced internal resolution. Test calm and strong wind,
root contact, branch/card attachment, shadows and Temporal motion. A distant
washed-out crown may involve shadow range or shading as well as asset density;
a missing connection may involve geometry or alpha mip coverage. Diagnose the
layer before enlarging cards or adding more overlap. The browser preview does
not reproduce Yarra's full lighting or connected wind.

Track scripts, presets in the other repo, runtime shaders, pack catalogs and
docs. Keep source/derived textures, local meshes, captures, caches, databases
and their backups ignored. A push to both repos preserves the rebuild recipe;
a fresh checkout still needs licensed sources restored, then build/import/cook.
Run focused checks for the changed contract. GPU checks need native GPU access
and imported local packs; missing prerequisites are not a passing result.
Use [Performance](PERFORMANCE.md) for matched measurements after visual review;
triangle counts and a few trees at a capped frame rate are not forest budgets.

### Existing forest packs and species recipes

**Forest assets:** Follow [asset setup](../assets/README.md) to export the Forest Tree Starter Kit, then run `cargo run -p yarra-world-cook -- import-assets assets/packs/forest_tree_starter_kit/summer.catalog.ron [PROJECT_DB]` (omit the optional project argument for the current world). This registers 11 summer trees and 8 shrubs, each with four authored mesh LODs. Restart the editor to refresh the palette; search “Forest” for manual placement or select the assets in a collection. Registration preserves existing world placements and does not publish automatically. Save & Publish after authoring. The normal renderer retains object pages within 192 m in all directions, subject to residency budgets, and uses their mesh LODs. Off-screen trees remain available to cast shadows after camera turns; Bevy still culls individual draws. Visible pages load before off-screen ones; there are no usable billboards yet. Legacy terrain diagnostics retain their old local object window.

**Birch prototype:** `tools/import_vegetation_bundle.py` imports the vegetation project's built `birch_leafy`, `birch_sparse`, and `birch_bare` bundles. It preserves geometry, UVs, wind weights and the authored rotating-card identities; adapts normals to the current experimental `crown_v1` material; and compresses six shared textures to mipmapped UASTC KTX2. Foliage color uses the baker's per-cell alpha-coverage mip levels. `--canopy-blend` controls the import's normal blend (default 0.85); this is not a final lighting decision. Source and converted binaries stay under ignored `assets/local/yarra_birches/`.

```sh
python3 tools/import_vegetation_bundle.py \
  --bundle /path/to/vegetation/outputs/birch_leafy/current \
  --bundle /path/to/vegetation/outputs/birch_sparse/current \
  --bundle /path/to/vegetation/outputs/birch_bare/current \
  --bundle /path/to/vegetation/outputs/birch_crown/current \
  --bundle /path/to/vegetation/outputs/birch_pendulous/current \
  --bundle /path/to/vegetation/outputs/birch_double/current \
  --bundle /path/to/vegetation/outputs/birch_triple/current
cargo run --release -p yarra-world-cook -- import-assets assets/packs/yarra_birches/birches.catalog.ron
python3 tools/place_birch_preview.py
python3 tools/place_birch_variants_preview.py
cargo run --release -p yarra-world-cook -- cook
cargo run --release -p yarra-app-game -- --start-view content/world.project.views/birch-near.ron
```

The placement helper is specific to the current 32 m island world: it adds nine samples in three groups near the start, saves a source backup and placement IDs under `tmp/birch-placement-*/`, and preserves existing samples on reruns. Six are leafy, two sparse and one bare. Bookmarks `birch-near`, `birch-west`, `birch-east`, and `birch-overhead` cover the groups and an elevated view. The revised catalog has three mesh LODs (3,104–3,212 / 922 / 366–414 triangles, depending on foliage state), with provisional 480/180/0 logical-pixel thresholds. Billboard view selection and the separate translucency map are not integrated; the last mesh LOD remains active at distance. These placements are an appearance check, not a dense-forest or device-performance benchmark.

The refined birch uses smaller leaves, doubled baked twig thickness and 20% thicker mesh branches. The original broad-leaf look remains in authoring presets `generic_deciduous_leafy/sparse/bare`. Birch selects 25% of LOD0 leafy clusters from the inner crown (54 leafy / 11 sparse / 0 bare) and retains them at every mesh LOD. These two-triangle clusters rotate around their centers and use eight dedicated stemless atlas tiles; structural cards stay fixed. The shared foliage atlas is now 2048×3072, with the original tile detail and no additional material. In Houdini, **Moving share of leafy cards** can be set to 0.20–0.30; changing it reuses the atlas. **Stemless moving foliage** retains leaves and fine twigs but removes the main stem and attachment bases. `_CARD_FACING.xy` stores mode (0 legacy axis, 1 camera facing) and elevation follow (0 preserve tilt, 1 full facing). Change **Facing & detail** in Houdini and rebuild, or compare elevation live in the generated browser preview. Older exports without this optional attribute retain legacy behavior. All passes use the main camera; previous-camera poses drive temporal motion, and culling uses each mesh's measured rotation radius. The current lighting is unchanged.

**Additional birch forms:** `birch_crown` exposes curved bare lower twig cards,
with occasional living sprays and a fuller leafy top. `birch_pendulous` has
arched limbs and hanging outer sprays; `birch_double` and `birch_triple` use
separate ground-planted stems of different heights and lean. All four share
the original birch atlas, use connected wind and bake crown occlusion for
`crown_v2`. Their fixed card scales stay constant through LODs. Import all seven
birch bundles together, since each import replaces the complete local pack.
`place_birch_variants_preview.py` adds four samples at X 2490 / 2504 / 2519 /
2535, Z 4356, beside the conifer review row. It preserves existing placements
and bookmark edits; source backups stay under `tmp/birch-variants-placement-*`.
Use `birch-variants-stand`, `crown`, `pendulous`, `double`, `triple`, `close`,
`bare`, `roots`, `overhead` and `far` bookmarks (all share the prefix).
`birch-variants-walk` starts normal play between the hanging birch and double clump.
`render_birch_variants_preview.py` captures native and reduced-resolution views
under ignored `tmp/birch-variants-review/`. These are appearance checks; the
clumps contain multiple stems and are more expensive than one tree. Dense
forest cost remains unmeasured.

**Oak prototypes:** Forest, spreading and sparse oaks use the same importer and existing crown lighting. All three share six compressed textures and the same eight shoot recipes, each with leafy, bare and stemless tiles. Imported triangle counts are 2,616/784/374 (forest), 2,814/858/424 (spreading), and 2,616/736/286 (sparse). The original 25% moving subset is supplemented by 24 forest / 40 spreading interior stemless quads, giving 63 / 79 / 15 facing cards at every mesh LOD. IDs and pivots stay stable across LODs. Added fill reuses the same atlas; transparent overlap still needs dense-forest profiling.

Extra oak foliage is distributed near secondary limbs using local foliage density and spacing, instead of concentrated in a central oval. Raising the authoring count retains existing filler positions. The lower fork stays open; sparse oak has no extra fill.

```sh
python3 tools/import_vegetation_bundle.py \
  --bundle /path/to/vegetation/outputs/oak_forest/current \
  --bundle /path/to/vegetation/outputs/oak_spreading/current \
  --bundle /path/to/vegetation/outputs/oak_sparse/current \
  --output assets/local/yarra_oaks \
  --catalog assets/packs/yarra_oaks/oaks.catalog.ron
cargo run --release -p yarra-world-cook -- import-assets assets/packs/yarra_oaks/oaks.catalog.ron
python3 tools/place_oak_preview.py
cargo run --release -p yarra-world-cook -- cook
cargo run --release -p yarra-app-game -- --start-view content/world.project.views/oak-spreading.ron
```

`place_oak_preview.py` adds three samples east of the birch stand, preserving existing scenery and edits on repeat runs. It backs up the source to `tmp/oak-placement-*/` and creates `oak-forest`, `oak-spreading`, `oak-sparse`, and `oak-overhead` bookmarks. Ground and overhead captures are under local `tmp/oak-playtest/`. These are art/correctness checks; dense-forest performance is not yet measured. Existing LOD stippling remains visible. Billboard selection and the separate translucency texture are still pending runtime work.

**Maple prototypes:** Forest, spreading and sparse forms use paired maple shoots from two supplied leaf sheets, an upright leader and smoother rising forks. All three share six compressed runtime maps. Elm bark is a provisional stand-in. Counts are 2,612/754/362 (forest), 2,968/858/406 (spreading), and 2,630/722/292 (sparse). Their 58/71/17 centered facing cards persist at each mesh LOD, including 16/24/0 density-aware filler cards. The current lighting and renderer are unchanged.

```sh
python3 tools/import_vegetation_bundle.py \
  --bundle /path/to/vegetation/outputs/maple_forest/current \
  --bundle /path/to/vegetation/outputs/maple_spreading/current \
  --bundle /path/to/vegetation/outputs/maple_sparse/current \
  --output assets/local/yarra_maples \
  --catalog assets/packs/yarra_maples/maples.catalog.ron
cargo run --release -p yarra-world-cook -- import-assets assets/packs/yarra_maples/maples.catalog.ron
python3 tools/place_maple_preview.py
cargo run --release -p yarra-world-cook -- cook
cargo run --release -p yarra-app-game -- --start-view content/world.project.views/maple-spreading.ron
```

The maple samples stand east of the oaks. `place_maple_preview.py` preserves existing placements/bookmarks and backs up the source to `tmp/maple-placement-*/`. It creates `maple-forest`, `maple-spreading`, `maple-sparse`, and `maple-overhead` bookmarks. Art review is pending; native captures go under local `tmp/maple-playtest/`. The same billboard and dense-forest profiling limitations as oak apply.

The maple spacing revision reduces each baked spray from 49–65 overlapping leaves to 29–39, with more space between pairs and slightly smaller leaves. The existing sample placements, card counts, mesh LOD budgets and shading remain unchanged. Reimport the three bundles together to refresh their shared texture set and updated mesh bounds.

The user approved the more open maple spacing; retain it as the current maple art checkpoint.

**Tall layered broadleaf:** `tall_broadleaf_forest` is a roughly 24 m foliage-height prototype inspired by local Witcher reference IMG_1369. A low, tapered crown uses staggered limbs, small pointed qgCoa2 leaves and brown elm bark. Its three mesh LODs are 3,466/1,092/592 triangles after the connection trial below. Two compact stemless facing bunches sit within the mid/outer sprays on each leafy side of 28 limbs (112 total), with the same pivots across all mesh LODs. The 232 original sprays stay fixed; narrower, more open bakes and a near-LOD bend reduce long flat panels. The old 40 central fillers and random large rotating sprays are replaced by these branch-facing pairs. Typical leaf length is about 10.8 cm; the atlas remains 2048×3072. See the lighting trial below for its material. This is a larger-tree asset budget; dense-forest/device performance remains unmeasured.

```sh
python3 tools/import_vegetation_bundle.py \
  --bundle /path/to/vegetation/outputs/tall_broadleaf_forest/current \
  --output assets/local/yarra_tall_forest \
  --catalog assets/packs/yarra_tall_forest/tall_forest.catalog.ron
cargo run --release -p yarra-world-cook -- import-assets assets/packs/yarra_tall_forest/tall_forest.catalog.ron
python3 tools/place_tall_forest_preview.py
cargo run --release -p yarra-world-cook -- cook
cargo run --release -p yarra-app-game -- --start-view content/world.project.views/tall-forest-stand.ron
```

Three samples of the same shape at different rotations/scales stand east of the maples, at x=2712/2736/2760, z≈4310. The placement helper backs up the source, uses stable IDs and preserves edits on repeat runs. It creates `tall-forest`, `tall-forest-stand` and `tall-forest-overhead` bookmarks. Native captures go under local `tmp/tall-forest-playtest/`. The authoring billboard is exported but is not imported into the runtime yet.

**Tall-tree lighting trial:** The approved art checkpoint is vegetation `a0818aa` / Yarra `d48e5fd`. The subsequent local trial enables **Crown lighting → Bake crown occlusion**, strength 0.9, for the tall preset. Rebuild and import with the same commands above. The importer selects `crown_v2` only for `crown_sky_v1` source materials; other species retain legacy shading. Broad sky visibility is baked into existing vertex colors from the complete crown before LOD thinning. Beyond dynamic-shadow coverage, the shader uses this data and stable crown normals to preserve sun-responsive contrast. Geometry and original foliage textures are unchanged. Restart the rebuilt game to load the new material pipeline and imported assets. Local native before/after captures and bookmarks are under `tmp/crown-occlusion/`; close, 35/65/100/150 m and overhead views were checked. The distant effect approximates crown self-shadowing; it does not extend cast ground shadows. Full weather/sun and dense-forest device profiling remain pending.

**Tall-tree far connections:** LOD2 now retains two existing inner fixed sprays on each of 28 limbs, adding 112 triangles (480 → 592). LOD1 prioritizes these sprays within its unchanged count. Branch radii increase by 1.3 in LOD1 and 1.6 in LOD2, while the trunk, LOD0 and all 112 facing bunches remain unchanged. Connecting sprays share a 1.12 scale across the two simplified LODs. Authoring controls live under **Distant LOD**; 0 connecting sprays and thickness 1 recover the preceding version. It uses identical textures and the existing `crown_v2` shader. Rebuild/import as above and recook the catalog for its updated mesh memory estimates. Native visual comparisons are under `tmp/far-lod-connectors/`; forest GPU profiling remains separate.

**Houdini trees:** `tools/houdini_export_tree.py` exports the forest trees from the Houdini tree scene as glTF LODs plus `assets/packs/yarra_trees/trees.catalog.ron`; import that catalog the same way. See [Houdini trees](../assets/README.md#houdini-trees) for the export command, the cluster-card bake and the `crown_v1`/`crown_v2` shading contracts.

Legacy imported forest foliage animates in both game and editor using shared wind enable/direction/strength and preview transport, with rigid bark and authored leaf weights. The existing game Wind control affects trees and grass together. `--upscaler metalfx-temporal` uses deformation-aware tree motion vectors automatically. To check the wind implementation, run `cargo test -p yarra-engine tree_wind`; with native GPU access, also run `cargo test -p yarra-engine tree_wind::gpu_tests -- --ignored`. The current local exports already include wind metadata; no world recook is needed for the shader/material update.

**Connected tree wind trial:** Rebuilt vegetation bundles use `hierarchy_v2` on both bark and foliage. `_WIND_PIVOT` stores the primary limb root and tree height; `_WIND_AXIS` stores limb direction and compliance. Descendants share that limb transform. The trunk follows a curved, rooted bend; limbs inherit its pose; cards turn toward the camera and flutter about the transformed attachment. The same functions run for colour, depth, shadows and current/previous motion-vector poses. Retained card bindings are identical across LODs; mesh bounds include the maximum allowed swing. Bark keeps its two-texture transition. Older bundles retain their existing wind path.

Open **F1 → Wind** to drag Strength, Gustiness, Direction, Trunk bend, Branch movement, Flutter and Trunk response period. Calm/Breeze/Gusty/Strong set manual shared wind; Follow weather restores the automatic source; Reset wind tuning restores the default response. Manual strength/direction settle smoothly and affect grass as well. Response gains affect connected trees. Controls are session-only, locked during F1 recordings, and included in the capture context; A/B Restore restores rendering settings, not these scene controls. The Features wind switch still disables all vegetation wind.

This is a shallow GPU hierarchy with four coherent frequency bands filtered by a damped response, not a per-branch physical simulation or a fit to measured turbulence. Species compliance, height-based period and foliage load are starting artistic profiles; strength is dimensionless, not m/s. There is no requested 35° target. Trunk/limb hard limits only bound extreme slider values. Ground stems retain their separate feet; tiny secondary twigs inherit their primary limb rather than flexing independently. Trees currently use their uniform instance scale; collision and weather-shelter footprints remain in the rest pose. Dense-forest/device cost and motion tuning still need acceptance.

Rebuild the current birch/oak/maple/bay/tall-forest/longleaf/spruce bundles, import each complete pack with `tools/import_vegetation_bundle.py`, re-register its catalog and cook to update mesh-memory estimates. The vertex contract adds 32 bytes per exported vertex, no triangles or materials. Validate bindings/rest geometry with vegetation `scripts/test_wind_binding.py` and `scripts/validate_wind.py ... --compare-previous`. Engine checks: `cargo test -p yarra-engine tree_wind`, native `tree_wind::gpu_tests -- --ignored`, and `connected_spruce_lods_compile_colour_depth_and_motion -- --ignored`.

For tree LOD dropout regression, run `cargo test -p yarra-engine imported_tree_lod_materials_remain_drawable -- --ignored --nocapture` with native GPU access and the local forest pack. It switches an imported tree through all four LODs without Temporal AA and checks that bark and foliage both have prepared GPU materials and compiled color/depth/motion pipelines on every transition frame. Material conversion must finish before Bevy's asset events; wind bounds expand separately before culling. `--metalfx-timing-log` also logs every camera/history/target reset as `TEMPORAL_HISTORY_RESET`, so remaining full-view flicker can be distinguished from local LOD draw gaps.

For grass streaming/history regression, run `cargo test -p yarra-vegetation-render --features upscaling/metalfx temporal_offscreen_grass_streaming_preserves_history -- --ignored --nocapture` with native MetalFX access. It repacks offscreen pages while stationary, strafing and animating wind, exercises empty/resident and disabled/enabled transitions, and checks history plus depth-derived motion in prepared/procedural paths. Page residency must not reset whole-view history; catalog edits and camera cuts still do. `--metalfx-timing-log` includes `GRASS_TEMPORAL_HISTORY` events with source/catalog/visibility/discontinuity reasons alongside native reset/timing logs.

**Roads:** activate Roads, choose a style, then New cart road and place two points. Edit control points, tangents and widths; extend/split with the tool controls. Road styles define wheel/center/shoulder wear, retained grass, ground mixtures and rut/relief variation. Explicit junctions connect compatible 2–4-arm endpoints. Save checkpoints roads and painter changes together; publication derives ground, grass and relief from the same source.

**Atmosphere:** open World → Atmosphere for the active space's profile and preview time/weather. **Weather** edits each preset (clouds, visibility, fog and skylight grey, exposure, wind, rain) and the random sequence (change and hold durations, next-state weights); the preview row shows any preset with a chosen wetness, without saving. Apply authored profile edits through normal undo/save/publication. Preview transport and temporary quality controls do not rewrite startup time/weather merely by being adjusted. Clouds Off removes rendering/shadows, not the weather's ambient response.

**Areas:** choose Areas in the World window to paint the named places gameplay reacts to. **Draw new area**, click corners on the terrain, then click the first corner or press Enter to close the shape; Backspace takes the last corner back and Esc cancels. Click an area to select it, drag a corner to move it, click an edge to add a corner there, and press Delete to remove the corner clicked last. The Areas window renames the selected area (lowercase letters, digits and `_ - / .`, the name gameplay content uses, e.g. `guard/gate_post`), optionally limits it to a height range for places under a bridge or on one floor, and deletes it. Areas save with everything else and have ordinary undo; publishing them recompiles no terrain. Outlines are drawn within 800 m of the camera.

Save conflicts indicate newer source revisions: resolve/reload the draft rather than forcing a stale overwrite. Recovery data under `.editor` is separate from saved source. Generated preview failures must remain visible as stale/error state; they are not publication success.

## Vegetation and animation studies

The Vegetation workspace renders isolated specimens/fields with camera, wind/time, catalog, palette, ground, density/LOD and reference-image controls. Shape and Colors edit the draft; **Save study** stores local reproduction settings, while **Save & Publish** applies authored catalog changes to the world. Linked image zoom is not camera movement. Use a full field and multiple views/motion for appearance acceptance, not only an attractive close specimen.

```sh
python3 tools/vegetation_study.py open --load content/vegetation/distance-01.ron
python3 tools/vegetation_study.py capture --camera overhead --time 0
python3 tools/vegetation_study.py capture --camera low --character --time 2.5
```

The helper builds the debug editor unless `--no-build` is supplied. `--output DIR` selects a fresh capture directory. Captures contain `viewport.png`, `editor.png`, `study.ron` and diagnostics. Replay loads an unsaved draft; it does not silently save a catalog. Matching pixels require the same renderer/assets/device. Reference originals live under `.editor/vegetation/references`; capture/study data is local. Study versions 1 and 2 remain readable.

Canopy controls isolate combined/ground/blade treatment. **Save canopy look** writes `content/vegetation/canopy-look.ron`; the game loads it at startup or **F1 → Advanced → Reload canopy look**. This is an artistic approximation, not a shadow solution. Shader-level banding studies remain experimental; normal-game controls were removed. The Animation workspace independently previews catalog models/clips and transport without changing gameplay actor authority.

### Bay shrub prototype

`bay_upright` uses six curved stems spreading directly from the
ground, small branch sprays, and the supplied bay leaf sheet. The three
mesh LODs contain 2,632 / 864 / 426 triangles and preserve all 52 centered facing
cards; 26 are distributed through the interior crown to fill gaps. Crown occlusion is enabled; elm bark is provisional. The material/shader
uses the existing foliage path. No additional shader or runtime feature is needed.

```sh
python3 tools/import_vegetation_bundle.py \
  --bundle /path/to/vegetation/outputs/bay_upright/current \
  --output assets/local/yarra_bay \
  --catalog assets/packs/yarra_bay/bay.catalog.ron
cargo run --release -p yarra-world-cook -- import-assets assets/packs/yarra_bay/bay.catalog.ron
python3 tools/place_bay_preview.py
cargo run --release -p yarra-world-cook -- cook
cargo run --release -p yarra-app-game -- --start-view content/world.project.views/bay-near.ron
```

Three samples stand east of the tall-tree stand at x=2782/2793/2804, z≈4310,
with different rotations/scales. The placement helper backs up the source and
preserves existing placements/bookmarks on repeat runs. Review bookmarks are
`bay-near`, `bay-stand`, and `bay-overhead`; native captures are under
`tmp/bay-playtest/`. As with the other kit assets, the authoring billboard is
exported but not registered in the game pending runtime view selection. This
small stand is an art check, not a dense-forest performance benchmark.

### Current longleaf pine kit

Run `hython scripts/build_longleaf_kit.py` and
`hython scripts/validate_longleaf_kit.py` in the vegetation project, then:

```sh
python3 tools/import_vegetation_bundle.py \
  --bundle /path/to/vegetation/outputs/pine_longleaf/current \
  --bundle /path/to/vegetation/outputs/pine_longleaf_half_bare/current \
  --bundle /path/to/vegetation/outputs/pine_longleaf_nearly_bare/current \
  --bundle /path/to/vegetation/outputs/pine_longleaf_one_sided/current \
  --output assets/local/yarra_longleaf \
  --catalog assets/packs/yarra_longleaf/longleaf.catalog.ron
target/release/yarra-world-cook import-assets assets/packs/yarra_longleaf/longleaf.catalog.ron
python3 tools/place_longleaf_preview.py
target/release/yarra-world-cook cook
python3 tools/render_longleaf_preview.py
```

The healthy tree is beside the birches at X 2510, Z 4346. Half-bare, nearly-bare,
and one-sided forms are at X 2474, 2486, and 2498 on the same row. They retain
the approved shape and needle scale, with 11 / 3 surviving limbs for the first
two forms; the one-sided form retains 15% of its opposite-side groups.

`longleaf-kit` shows the lineup. Individual bookmarks are `longleaf-half`,
`longleaf-nearly`, `longleaf-one-sided`, and `longleaf-bare-close`.
Healthy-tree bookmarks:
`longleaf-whole`, `longleaf-close`, `longleaf-side`, `longleaf-overhead`,
`longleaf-far`. The whole/far views offset their focus along the viewing ray
because bookmarks allow at most a 24 m orbit distance. Placement removes the
six known old-pine and two branch-study preview IDs in the same transaction as
adding the new forms. It backs up SQLite first and verifies unrelated objects
are unchanged. Reruns preserve existing longleaf placements and bookmark edits.

Captures go to `tmp/longleaf-review`; use `--view close --mode half` for a
reduced-resolution check. The helper verifies fresh, non-black output. These
are visual checks; GPU/frame timings during a capture are not a forest benchmark.
The four longleaf forms are the active pine kit. Old source assets remain archived.

### Norway spruce prototype

Build `spruce_forest` in the vegetation project with `scripts/build.py`, then run
`hython scripts/validate_spruce.py`. Import and place the resulting bundle:

```sh
python3 tools/import_vegetation_bundle.py \
  --bundle /path/to/vegetation/outputs/spruce_forest/current \
  --output assets/local/yarra_spruces \
  --catalog assets/packs/yarra_spruces/spruces.catalog.ron
target/release/yarra-world-cook import-assets assets/packs/yarra_spruces/spruces.catalog.ron
python3 tools/place_spruce_preview.py
target/release/yarra-world-cook cook
python3 tools/render_spruce_preview.py
```

Three rotated/scaled instances of the same forest preset sit at X 2525 / 2538 /
2551, Z 4346, continuing the longleaf row near the birches. The helper backs up
the source database and preserves existing placements and bookmark edits.
Bookmarks: `spruce-stand`, `spruce-whole`, `spruce-close`, `spruce-below`,
`spruce-overhead`, and `spruce-far`. Whole/stand/far views offset the focus to
work around the 24 m orbit cap; below uses a low focus at the supported 5° pitch.

The generator uses photographed qgpvu2 spruce shoots, fixed V-shaped branches,
hanging side sprays and a small share of stemless facing fillers. Main sprays
retain their size and fold at every LOD. Pine bark is a provisional stand-in.
It uses existing crown lighting, cutout coverage mips and foliage shaders.
Only three mesh LODs are registered; runtime billboard selection remains pending.
Captures go to `tmp/spruce-review`; `--view close --mode half` tests reduced
resolution. The spruce passed user art review on 2026-10-03; dense-forest
profiling remains pending.

### Dead broadleaf forms

Build the six presets with vegetation `scripts/build_dead_trees.py` (or use
`--slender-only` to rebuild just the three narrow forms), then run
`hython scripts/validate_dead_trees.py`. Import the complete pack together:

```sh
python3 tools/import_vegetation_bundle.py \
  --bundle /path/to/vegetation/outputs/dead_upright/current \
  --bundle /path/to/vegetation/outputs/dead_spreading/current \
  --bundle /path/to/vegetation/outputs/dead_split/current \
  --bundle /path/to/vegetation/outputs/dead_slender/current \
  --bundle /path/to/vegetation/outputs/dead_double/current \
  --bundle /path/to/vegetation/outputs/dead_triple/current \
  --output assets/local/yarra_dead_trees \
  --catalog assets/packs/yarra_dead_trees/dead_trees.catalog.ron
target/release/yarra-world-cook import-assets assets/packs/yarra_dead_trees/dead_trees.catalog.ron
python3 tools/place_dead_trees_preview.py
target/release/yarra-world-cook cook
python3 tools/render_dead_trees_preview.py
```

The original three samples sit at X 2428 / 2443 / 2458, Z 4344, west of the longleaf row.
Slender single/double/triple forms sit in a clear patch farther west at
X 2390 / 2402 / 2414, Z 4338, with thin
trunks, ascending limbs and distinct planted feet. Their shared materials and
wind path need no engine changes. The richer, wider forms were approved on 2026-10-03.
The placement helper backs up the source database and preserves edited objects
and bookmarks. `dead-trees-walk` starts normal play near the spreading tree.
Other bookmarks are `stand`, `upright`, `spreading`, `split`, `close`, `bark`,
`twigs`, `overhead` and `far`, all prefixed `dead-trees-`. Captures go to ignored
`tmp/dead-trees-review/`; the close view also runs at reduced internal resolution.
New bookmarks are `slender-walk`, `slender-stand`, `slender`, `double`, `triple`,
`slender-roots`, `slender-close`, `slender-overhead` and `slender-far`, with the same
`dead-trees-` prefix. The capture helper accepts these names through `--view`;
use `--view slender-close --mode half` to check Temporal reconstruction.

These trees reuse the existing elm bark package with curved bare twig cards.
`vegetation_surface: bare_wood` opts masked materials into existing `plain`
two-sided lighting and skips the importer's canopy normal blend. This retains
rounded bark normal detail without adding a renderer shader. The `hierarchy_v2`
wind bindings remain active; no facing cards or leaf flutter are present. Twig
cards retain their rest positions through all mesh LODs while wood tessellation
reduces. Budgets are 3660/1988/1120, 4094/2214/1246 and 3966/2168/1228 triangles
respectively for the original forms. Slender/double/triple use 2500/1362/750,
3616/1964/1088 and 4614/2496/1380 triangles, with 106/144/171 fixed cards retained at
every LOD. Only the three mesh LODs are registered; runtime billboards remain
pending. Existing living-tree material conversion is unchanged.
The slender forms use smaller staggered twig fans along limbs and upper stems;
their authoring `Twig richness` control changes this fill without rebaking.
Crown-width settings are about 25% higher than in the initial slender study, with slightly
more open double/triple clumps.

Mesh tips now retain the root radius measured by the twig baker, scaled by the
attached card size. Shared tips meet the widest attached stem; the transition
is blended along the supporting mesh and persists through all three LODs.

Resolved wind depth mismatch (2026-10-03): Strong wind exposed dark patches on
both bare and living trees when the depth prepass was enabled. Bypassing MetalFX
reconstruction left the patches intact. The independently compiled depth and
colour vertex variants could evaluate wind arithmetic differently; colour
fragments then failed against their own prepass depth. `tree_wind.wgsl` now owns
the Bevy-compatible vertex output layouts and marks both clip positions
`@invariant`. Keep these locations synchronized with Bevy's forward/prepass IO
when upgrading Bevy. Wind motion, material lighting and pass counts are unchanged.

Run `cargo test -p yarra-engine wind_depth_prepass_preserves_colour_coverage -- --ignored --nocapture`
with native GPU access and local dead-tree/spruce bundles. It compares rendered
colour with/without depth and motion prepasses in Calm and two Strong poses.
Removing `@invariant` reproduced 2176 damaged pixels in the bare-tree Strong
case; the fixed shader produced zero in all six cases. The compute tests under
`tree_wind::gpu_tests` separately cover attached card roots and deformation
history. Native Temporal captures are in `tmp/dead-trees-review/`; this is
correctness coverage, not a dense-forest performance measurement.

### Archived pine kit

Six shared-pipeline pines use `yarra_pines_v2`: mature forest, young, spreading,
half bare, nearly bare and one-sided. They share needle textures and use the
same centered camera-facing cards, coverage mip chain and crown lighting as the
newer vegetation. These are retained as authoring archives; their six preview
trees were replaced by the longleaf kit below. To reimport the archived assets
without placing them, import all six together so textures remain shared:

```sh
python3 tools/import_vegetation_bundle.py \
  --bundle /path/to/vegetation/outputs/pine_forest/current \
  --bundle /path/to/vegetation/outputs/pine_young/current \
  --bundle /path/to/vegetation/outputs/pine_open/current \
  --bundle /path/to/vegetation/outputs/pine_half_bare/current \
  --bundle /path/to/vegetation/outputs/pine_nearly_bare/current \
  --bundle /path/to/vegetation/outputs/pine_one_sided/current \
  --output assets/local/yarra_pines_v2 \
  --catalog assets/packs/yarra_pines_v2/pines.catalog.ron
cargo run --release -p yarra-world-cook -- import-assets assets/packs/yarra_pines_v2/pines.catalog.ron
```

The retired samples were west of the birches (X 2438–2474, Z 4327–4350).
Their old bookmarks no longer represent the current kit.
`tools/place_pine_preview.py` forwards to the current longleaf placement helper.
Exposed crown branches in the archived models use two fixed curved
cards with a short solid attachment, retained across all three LODs.
Mature branches now spread or droop and carry overlapping groups of needles.
About 25% of needle cards face the camera, primarily the two fillers inside
each limb's outer foliage groups. This is an asset update using existing shaders.
The original `yarra_pines` pack remains an archive; use `yarra_longleaf` for the current kit.
Three mesh LODs are imported; billboard support remains separate pending work.

### Archived isolated pine branch study

The grouped-needle pine revision was rejected in art review: shoots and stacked
cards merge into painted-looking clumps. `yarra_pine_branch_study` tested a
simpler branch in isolation. Its two preview fixtures have now been removed;
the commands below reproduce the archived experiment explicitly. Its two
samples use eight separate shoots on one card, then the same card with a smaller
offset companion (8 / 16 triangles). They share textures and identical LODs.
There is no canopy-normal blend, crown occlusion, or camera rotation in this study.

```sh
# First run scripts/build_pine_branch_study.py with hython in vegetation.
python3 tools/import_vegetation_bundle.py \
  --bundle /path/to/vegetation/outputs/pine_branch_single/current \
  --bundle /path/to/vegetation/outputs/pine_branch_depth/current \
  --output assets/local/yarra_pine_branch_study \
  --catalog assets/packs/yarra_pine_branch_study/branches.catalog.ron \
  --canopy-blend 0
target/release/yarra-world-cook import-assets assets/packs/yarra_pine_branch_study/branches.catalog.ron
python3 tools/place_pine_branch_study.py
target/release/yarra-world-cook cook
python3 tools/render_pine_branch_study.py
```

The branches float at chest height beside the birches, at X 2491 / 2497, Z 4338.
Use `pine-branch-compare`, `pine-branch-single`, `pine-branch-depth`,
`pine-branch-below`, and `pine-branch-overhead` bookmarks. Placement preserves
existing scenery and repeat-run edits. Captures go to `tmp/pine-branch-study/`,
with 1920×1080 native and 960×540 internal / 1920×1080 MetalFX Temporal output.
The capture helper requires Pillow, rejects missing/stale/black screenshots,
and accepts `--view overhead --mode half` to repeat an individual check.
The first test is branch readability and card overlap; it does not validate
full-tree silhouette, LOD transitions, or forest performance.

## Explicit fixtures and catalog tools

Use fresh paths for disposable worlds:

```sh
cargo run -p yarra-world-cook -- create-road-demo tmp/roads.project.sqlite
cargo run -p yarra-world-cook -- cook tmp/roads.project.sqlite tmp/roads.runtime.sqlite
cargo run -p yarra-app-editor -- --project-db tmp/roads.project.sqlite --world-db tmp/roads.runtime.sqlite
python3 tools/hill_landscape.py prepare
python3 tools/hill_landscape.py game --view summit
python3 tools/hill_landscape.py editor --view valley
python3 tools/hill_landscape.py descent
```

`create-demo` provides the historical 32 m grass fixture; `create-mountain-fixture` and `create-hill-fixture` provide terrain fixtures. `create-island-fixture` writes the default island (and its views) to another path without cooking. The hill helper uses `tmp/hill-landscape`, preserves existing source, and supports `--recook`. It is separate from normal game content. Pure compiler fixtures remain available as `layered_meadow` and `cart_track` examples in `yarra-environment-compile`.

`export-vegetation PROJECT_DB CATALOG_RON` writes a new catalog file. `import-vegetation CATALOG_RON [PROJECT_DB RUNTIME_DB]` intentionally replaces the catalog and publishes; it is not a read-only preview. The old `demo` and `sync-demo-vegetation` reset commands no longer exist.

## Standalone gameplay and saves

The gameplay content tool runs without game/editor startup, cooked worlds or art assets. Start with the checked-in authored project:

```sh
cargo run --offline -p yarra-game-content -- validate content/gameplay/demo
cargo run --offline -p yarra-game-content -- demo content/gameplay/demo --locale uk
```

The demo loads definitions, actors, starting inventories/wallets, scenario actions and Fluent files from disk. It uses a potion, transfers equipped gear to a companion, stores an item in a chest, trades and completes a dialogue reward. It publishes and retains a content bundle, plays the scenario against that bundle, writes manual/quick/auto saves to a new temporary directory, reloads the manual slot and compares the complete state. It also reports how many dialogue graphs were loaded. Expected results are **75 player health**, **80 party gold**, **10 persuasion XP** and **3 saves**. Use `--locale en` for English, or `--save-dir /path/to/new-directory` to choose a fresh destination. Existing demo save directories are refused.

### Playing the slice in the game

```sh
cargo run --release -p yarra-app-game -- --story content/gameplay/demo
```

A guard and a gate appear a few metres ahead of the start. Walking up to them enters the `guard/approach` area: your companion remarks on the path (an ambient conversation, shown above the panel while you keep moving) and, because you carry the key, the guard walks to the gate, takes the key and opens it. You can also walk to the guard and press **E**; **Space** continues a line, **1–9** pick a reply, **Q** walks away. The world waits during that conversation. **F5** and **F9** quick save and load, including where everyone stands. The corner text shows which named areas you are in.

Returning the key is worth a level, and the lines under the quest show the hero's sheet. With points to spend, **Z**, **X** and **C** raise strength, wisdom and vitality. Near the guard, **T** asks about what he offers once the gate is open: sword lessons for gold and learning points. **H** drinks a potion. Beside the training dummy, **F** strikes it again and again, **G** lines up a power strike (stamina, then a cooldown) and **V** stops; walking away stops too.

The two areas use stand-in shapes beside the guard until the world has areas painted under the names `guard/approach` and `guard/gate_post`; painted ones win. The project is published to a private bundle under the system temp directory (`yarra-story/`), where the saves also live; a save made with different content still loads, and the game says the content has changed.

### Editing and validating gameplay content

Content refers to things by name: `id: "guard/gate"` in the file that defines a quest, `quest: "guard/gate"` wherever it is used. A name is lowercase letters, digits, `_`, `-`, `/` and `.`, and it is the identity: renaming it makes a different thing. By convention a name starts with its package. In Rust, `QuestId::named("guard/gate")` gives the same identity.

Scripts are `.luau` files in a package; the file name is the module name.

```lua
-- packages/guard/scripts/guard.luau
local guard = {}
function guard.take_key(game: Game, scene: Scene)
    if game.get("guard/rewarded") then return end
    game.set("guard/rewarded", true)
    game.consume_item(scene.player, "old_gate_key", 1)
    game.complete_quest("guard/gate")
    game.set_locked("guard/old_gate", false)
end
return guard
```

Use it as `actions: [Script("guard.take_key")]` or `condition: Some(Script("guard.ready"))`. A condition function returns a boolean and can only read. `cargo run --offline -p yarra-game-content -- script-api` prints the full typed API. `validate` compiles every script and rejects references to missing functions. With [Luau](https://github.com/luau-lang/luau/releases) installed (`luau-analyze` on `PATH`, or its path in `YARRA_LUAU_ANALYZE`) it also type-checks them in strict mode against that API, so a misspelled function or a wrong argument type fails validation with the script's own line number.

A package declares the variables it introduces in its `package.ron`: `variables: [(id: "old_gate/rewarded", initial: Bool(false))]`. A variable keeps the type of its initial value (`Bool`, `Int` or `Text`). Test one with `Variable(variable: "...", test: Is(Bool(true)))` (also `AtLeast(n)`, `AtMost(n)`), change it with `Set(variable: "...", value: ...)` and `Add(variable: "...", amount: n)`, or from a script with `game.get`, `game.set` and `game.add`. A scenario can start with `variables: {"name": Int(3)}`.

Add `scope: Actor` to a declaration and every actor has its own value, for things one character remembers: `(id: "guard/insulted", initial: Bool(false), scope: Actor)`. Content then says whose value it means with `of: Some(Speaker)` (or `Player`, or `Actor("name")`), and scripts pass the actor last: `game.set("guard/insulted", true, scene.speaker)`, `game.get("guard/insulted", scene.speaker)`. Leaving the actor out, or naming one for a playthrough variable, is an error.

Copy `content/gameplay/demo` to start a project. In source format **11** a project lists package directories, and each file in a package says what it holds by its name:

```text
project.ron                         # content identity, source locale, packages, scenario
scenario.ron, scenarios/            # starting conditions and steps
packages/guard/                     # a package is named after its directory
  package.ron                       # optional: the areas it names, the variables it declares
  actors.ron, items.ron, loot.ron   # lists: templates, catalog fragments, loot tables
  characters.ron                    # list: the characters content names
  rules.ron                         # the one rules definition of the project
  gate.quest.ron                    # one quest
  guard.profile.ron                 # one interaction profile
  ready.predicate.ron               # one named predicate
  conversations/reward.dialogue.ron # one conversation graph
  world/gate.object.ron             # one world object
  world/escort.trigger.ron          # one trigger
  scripts/guard.luau                # a script module named after its file
  en.ftl, uk.ftl                    # the package's text, one or more files per locale
```

Files are found anywhere under the package directory, in any subdirectories. A `.ron` file whose name says nothing above is an error, so a misnamed file is not silently left out; other files (notes, images) are ignored. `actors.ron`, `characters.ron`, `items.ron` and `loot.ron` may also be called `*.actors.ron` and so on. Catalog fragments share one catalog ID and revision. Every identity is defined once; one defined twice names both files. Paths in `project.ron` are relative to the project root and stay inside it after symlink resolution. Where a file sits and the order of packages change no identity; the order of a node's children does matter.

**Characters.** A template says what a kind of actor is made of; a character is one particular actor, built from a template: `(id: "mira", template: "traveller", name: Some(Message("mira-name")))`. Without a name it goes by its template's. Triggers, conditions, actions and dialogue roles may name only declared characters, so a mistyped name fails the build and says in which record. A scenario's `actors` start declared characters at a position, optionally with a `name` of their own for that playthrough; actors created while playing are not declared.

**Text.** Each package has one text resource, named after the package, made of its `<locale>.ftl` files (several files of one locale are joined, in path order). Content writes `Message("guard-name")` for a message of its own package and `Message("core/item-healing_potion")` for another's; `Literal("Player name")` is shown as written. A message key is unique within its package.

What messages a package has, and what arguments they take, comes from its source-language file (`source_locale` in `project.ron`): an argument used to choose between named variants (`{ $attitude -> [friendly] … *[hostile] … }`) takes one of those names; one used with plural categories or number keys is a number; any other is shown as it is, text or number. A message's arguments include those of the messages it refers to; a term takes only its own. Static labels (item, category, template, character, rule, quest, objective, topic, object and starting names) cannot take arguments. Dialogue nodes pass arguments with `arguments: {"player": ActorName("player")}`; `Localization::format_bound` renders the resulting `BoundText` in a locale.

A translation may leave messages out; they fall back to the source language whole, and `validate` lists them as warnings. It may not define a message the source does not have or use an argument the message does not take. `validate` checks every Fluent branch, reference, attribute, cycle and argument use, including unused terms. Unknown functions and positional term arguments are rejected; literal named term arguments and attributes are supported. A translated message that leans on a term only the source defines falls back whole; a translated line is never mixed with source-language parts. Changing wording changes no mechanical identity and does not touch saves; changing what a message takes does.

Validation also runs the scenario against a disposable session and reports the failed step. It does not prove that every possible story branch succeeds. No validation/build command edits source or existing saves. Each file read is bounded: 16 MiB per RON document, 2 MiB per Fluent source, 1 MiB per script. There is no cap on how many definitions content has. Increment content/catalog revisions for mechanical releases. New-game scenario changes are separate from existing saves' state and RNG.

### Publishing mechanics and language packs

Publish each immutable database at a fresh path:

```sh
mkdir -p tmp/game-content
cargo run --offline -p yarra-game-content -- build content/gameplay/demo tmp/game-content/mechanics-v6.sqlite
cargo run --offline -p yarra-game-content -- build-language content/gameplay/demo en tmp/game-content/en-v1.sqlite
cargo run --offline -p yarra-game-content -- build-language content/gameplay/demo uk tmp/game-content/uk-v1.sqlite
cargo run --offline -p yarra-game-content -- validate tmp/game-content/mechanics-v6.sqlite --language tmp/game-content/en-v1.sqlite --language tmp/game-content/uk-v1.sqlite
cargo run --offline -p yarra-game-content -- demo tmp/game-content/mechanics-v6.sqlite --language tmp/game-content/en-v1.sqlite --language tmp/game-content/uk-v1.sqlite --locale uk
```

Content schema **12** contains one checksummed record per mechanical asset, text contracts and the tool-only scenario. It contains **no FTL**. Language-pack schema **2** stores one locale's resources and the hash of the contract each was checked against. A wording fix uses `build-language` with a fresh pack path and requires no mechanical rebuild and does not affect saves. The caller explicitly selects packs; nothing is discovered implicitly.

`ContentRepository::open(path)` reads the manifest only. `headers(kind)` lists identities and checksums without payloads; `read(&AssetId::Item(id))` and `read_kind(kind)` return verified assets. A session uses it through the `ContentSource` port: `core()` once, then `dialogue(id)` per conversation. Text **contracts** are read explicitly with `AssetId::Text(resource_id)`; commands never read or parse wording.

`LanguageRepository::open` likewise decodes zero resources; `load(resource_id)` is an indexed, checksummed lookup in one locale. Compose `LanguageSource::new(content_repository, language_repositories)` with `Localization::with_source(source_locale, Box::new(source))`. This is the runtime path. A resource is read and parsed the first time a locale needs it and kept: a game has one per package and a few locales. Replace the service to switch to another pack generation. Use these synchronous readers on an I/O worker when integrating the game.

Inspect mechanics without instantiating a session, executing a scenario or reading packs:

```sh
cargo run --offline -p yarra-game-content -- inspect tmp/game-content/mechanics-v6.sqlite --item healing_potion
```

`inspect` accepts repeated `--item` and `--dialogue` flags and prints each requested asset.

`LoadedProject::load_directory` eagerly validates authoring data. `materialize_bundle_for_tools` eagerly validates mechanical assets, fingerprints and scenario without packs. `materialize_with_languages_for_tools(bundle, pack_paths)` additionally validates all supplied wording. These are explicit tool APIs; gameplay reads published bundles through `ContentRepository`. `start` imports the authored seed; `run_scenario` also executes the exercise. Each start creates a fresh playthrough ID; items are numbered by the inventory that makes them, so the same start makes the same items. The demo imports setup/presentation for reporting, then runs commands against the indexed repository and retains content beside its saves.

### Quest-aware NPC interaction

Run the second authored scenario without replacing the inventory/trading regression exercise:

```sh
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard.ron
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard-refusal.ron
```

`LoadedProject::load_directory_with_scenario(root, relative_path)` and the CLI eagerly validate the project and execute the selected exercise in a disposable tool session. Scenario paths use the same project-root bounds as other sources. Scenarios that start the same way share a start: `base: Some("scenarios/guard-start.ron")` takes the whole starting state from that file, which holds a start and no steps, and the scenario itself holds only steps. Runtime coverage in [`game_content/tests/narrative.rs`](../crates/game_content/tests/narrative.rs) runs these same steps against published SQLite content. The CLI does not yet load external exercises against a retained campaign or provide spatial movement.

The guard package owns two quests, a reusable readiness predicate, one interaction profile, a `guard/rewarded` variable and five conversation assets. The scenario starts with the required key already owned, changes attitude, activates both quests, checks opening priority and independent topics, then returns the key for objective completion, quest completion, attitude and XP. The refusal exercise interrupts between speakers, restarts, remembers a refusal, selects its follow-up, then lets the player return the key via an explicit topic. Quest/objective names and topic labels use the package's English/Ukrainian Fluent resource; dialogue lines can share that resource or own a separate one.

An actor template's `interaction: Some(profile_id)` attaches a profile; `None` explicitly declares no selector. A profile has 1–64 rules with a local ID, priority, tie order, optional topic label, condition and 1–16 positively weighted dialogue variants. Higher priority wins, then lower `order`; duplicate `(priority, order)` pairs are rejected. Automatic opening considers only rules without a topic. Every eligible topic is returned independently, keeping concurrent quests discoverable. Weights select within the winning rule. Derived `DialogueContract` records contain role declarations, history scope, repeat policy and line/choice IDs. Profile previews read these contracts and scoped history to filter ineligible variants; `unavailable_variants` explains repeat/cooldown exclusions. Full graphs remain outside the preview dependency closure.

Conditions support `All`, `Any`, `Not`, `Named`, `Present(actor)`, quest status, objective completion, directed attitude, items, variables, `Stat`, `Skill` (rank), `Level`, `Class`, `CanLearn`, `Gold` and scoped `History` counts. History conditions reference a dialogue and an event (`Started`, `Completed`, `Interrupted` or `Node(id)`); the dialogue contract supplies the scope and validates the node ID. `Relationship` conditions and actions name `Player`, `Speaker` or `Actor(id)`. Trees nest only so deep; named predicates are expanded with a shared depth and work budget, so shared references cannot blow up.

Use `preview_interaction(participant, speaker)` for sorted candidates, observed leaf values and eligibility, `opening()` for the chosen rule, and `topics()` for available topics. Preview does not change state, generation or either RNG stream. Submit `Command::Talk { participant, speaker, topic: None, bindings: Default::default() }` to choose an opening or `Some(rule_id)` to request an eligible topic. Only the selected graph loads. The committed outcome includes `InteractionSelected`; its profile/rule/dialogue are persisted separately from graph progress. An active conversation resumes without rerolling or rebinding, including after save/load; a topic request during it is rejected. Greeting variation uses a saved narrative RNG separate from skill rolls.

`Command::Quest` and `Command::AdjustRelationship` are explicit domain commands. Dialogue `Action::Quest` and `Action::Relationship` can combine these changes with item/XP rewards in one transaction. Required objectives gate completion; failed/completed quests are terminal and duplicate transitions fail. There is no implicit quest auto-completion. World distance/access authorization belongs to the future application adapter.

### Conversation runs, history and rewards

A conversation is one `graph.ron`: `roles`, `history_scope`, `repeat`, an optional `mode`, ordered entry nodes in `start`, and a flat list of `nodes`. `mode: Ambient` marks lines spoken while play goes on, such as companion banter started by a trigger: the engine shows each line for a while and acknowledges it itself, and the graph may not contain choices. The default, `Blocking`, has the player's attention. Conditions and actions are written in the node that uses them; a package lists the `variables` it introduces.

```ron
(id: "greeting", kind: Line, speaker: "speaker", text: Message((resource: "...", key: "reward")),
    arguments: {"player": ActorName("player")}, children: ["companion-aside", "answer"]),
(id: "companion-aside", kind: Line, speaker: "companion", text: ..., children: ["guard-reply"]),
(id: "return-key", kind: Choice, speaker: "player", text: ...,
    condition: Some(Named("...")), actions: [If(condition: Variable(variable: "...", test: Is(Bool(false))),
        then: [Set(variable: "...", value: Bool(true)), ...])]),
```

Every node has an `id`, a `kind`, a `speaker` role and `text`; `arguments`, `repeat`, `condition`, `actions` and `children` are optional. After a node (or at the start), its children are tried in order and the first eligible one decides what happens: a `Line` is shown and waits to be acknowledged; a `Choice` means every eligible choice among those children is offered. With no eligible child the conversation is complete. A node is eligible when its speaker is taking part, its `repeat` allows it (`Always`, `OncePerRun`, `OnceEver`) and its condition holds.

Roles say who can speak. `player` and `speaker` are always `Required` and are bound by `Talk`/`StartDialogue`. Further roles are `Required` or `Optional` (supplied in the command's `bindings`) or `Actor("uuid")`, a named character who takes part whenever they are in the party. Nodes of a role nobody fills are skipped, so a companion's reaction is simply an earlier child spoken by that companion, and a line for two companions together adds `condition: Some(Present("other-uuid"))`. `Command::Party { actor, member }` changes who travels with the player; a scenario can start with a `party`.

Use `conversation_view(ConversationKey)` for status, token, the current line and the available choices. A line includes the speaking actor's ID and `BoundText`; gameplay returns references and typed values, never rendered strings. Argument sources are `ActorName(role)`, `Stat { role, stat }` (a stat after equipment and effects, or what is left of a resource; both need a `Required` role), static `Text`, integer `Number` and declared `Select`.

Submit `AdvanceLine { key, expected: view.token }` once a line has been presented: it runs the line's actions and records it. `Choose` likewise takes the token of the view the player saw; a stale token is rejected rather than consuming another step.

Graph repeat policies are `Always`, `OnceCompleted` and `Cooldown { millis }` measured from the last committed start using saved logical time. An interrupted introduction with `OnceCompleted` can restart. History scope is `Playthrough`, `Player`, `Speaker` or `Interaction` (the ordered player/NPC pair) and counts starts, completions, interruptions and each node taken.

A reward given once is a boolean variable and `If { condition, then, otherwise }`: test the variable, set it and give the reward in `then`. Later picks skip the group, including its random rolls, while the choice itself can still repeat. Different graphs and NPCs can test one playthrough variable, or a per-actor one with `of: Some(Player)` to reward each player once. A condition on the choice can hide an offer already given. Actions nest at most eight deep. Everything a command did rolls back if it fails, and previews change nothing.

[`conversation_runs.rs`](../crates/game_content/tests/conversation_runs.rs) exercises hub loops, once-only rewards across graphs/NPCs, three-role localization, mid-line restore, remembered refusals, saved cooldowns, and stale inputs. These are standalone domain contracts. Spatial reach/access, proximity scheduling, recent-variant avoidance and role selection from the live party remain application/world follow-ups.

### Characters, levels and abilities

One package owns `rules`: a single file declaring what characters are made of. [The demo's](../content/gameplay/demo/packages/core/rules.ron) is the reference.

- `stats`: each is `Primary(minimum, maximum)`, `Derived(minimum, maximum)` or `Resource(maximum: "other-stat")`. `life` names the resource whose loss is death.
- `derive` and `check` name two functions in a script module ([`rules.luau`](../content/gameplay/demo/packages/core/scripts/rules.luau)). `derive(c)` returns a table with every derived stat; `c` has `level`, `class`, `stats` (the primaries after equipment and effects) and `skills` (ranks). `check(c, skill, difficulty, roll)` returns whether a check passes; `roll(sides)` is the only source of chance. Keep both free of side effects: `derive` runs whenever a character's build changes.
- `skills`: `ranks` lists the learning points each rank costs.
- `classes`: `starting` primaries, `per_level` grants, `at_level` grants for particular levels (level 1 included), and `skills` with the highest rank a trainer can teach that class. Grants are `attribute_points`, `learning_points`, `bonuses` to primaries and `abilities`.
- `levels`: total experience needed for level 2, 3, …
- `effects`: `modifiers` and an optional `periodic` change to a resource. Modifiers are `(stat, op)` with `Add(n)`, `Multiply(percent)` or `Override(n)`; items use the same in `mechanics.modifiers`.
- `abilities`: `duration_ms`, optional `cooldown_ms` and `costs`, `targeted`, and `resolve`, a script function that runs when the ability takes effect. In it `scene.player` is the user and `scene.speaker` the target.

An actor template names its `class` and optionally a `level`, `base` values that differ from the class, starting `skills`, the `equipment` it wears and a `loot` table. Loot tables live in files a package lists under `loot`; an entry is `(item, quantity: (least, most), chance)`. A container object can name one too. Inventories made from them appear when a command first needs them; send `OpenInventory { actor }` before showing a trade or loot screen.

Actions for rewards and trainers: `AwardExperience(amount)` (to the party), `Teach(skill)`, `Pay(amount)`, `ChangeResource(of, resource, amount)`, `ApplyEffect(of, effect, duration_ms)`, `RemoveEffect(of, effect)` and `Check(skill, difficulty, success, failure)`. A trainer is a dialogue choice with `condition: Some(All([CanLearn(skill: "…"), Gold(minimum: 20)]))` and `actions: [Pay(amount: 20), Teach(skill: "…")]`. Scripts have the same through `game.stat`, `game.skill`, `game.level`, `game.class`, `game.gold`, `game.has_effect`, `game.award_experience`, `game.teach`, `game.pay`, `game.change_resource`, `game.apply_effect`, `game.remove_effect`, `game.check` and `game.random`.

Commands a game sends: `Party { actor, member }`, `Control { actor }`, `SpendAttributePoint { actor, stat }`, `Intend { actor, intent, clear }`, `Interrupt { actor }` and `AdvanceTime { millis }`. Time is what makes effects tick and actions resolve; advance it in small steps (the slice uses 100 ms) and not at all while a blocking conversation has the player's attention. `state().actor(id)` has `acting`, `intents` and `cooldowns` for a UI.

A scenario lists `party`, who is `controlled` and the `purse` wallet; an actor may start with `resources: {"health": 50}`. Steps include `SpendAttributePoint`, `Intend`, `StopActing`, `ExpectStat`, `ExpectLevel` and `ExpectSkill`; see [`guard-training.ron`](../content/gameplay/demo/scenarios/guard-training.ron).

### Session and save APIs

`GameSession::new(content_source, state)` reads the always-loaded definitions, brings the state in line with them and checks it in full, reading the graphs of conversations in progress; other graphs are read when first needed. Production uses `ContentRepository`; tools and tests use `ToolContent` over a loaded project. `apply(Command)` returns `CommandOutcome { header, events }`, or an error with state, clock and random streams unchanged. An accepted command also carries out the work it gave rise to (triggers, the conversations they start, walks whose time ran out) before it returns, so the outcome is settled. When a rule refused the command, `error.rejection()` gives a `Rejection` to match on (`Locked`, `NotEnoughGold`, `PartyFull`, `ConversationMoved`…); anything else is a mistake in content or code. `state()` is the whole playthrough; `stat`, `conversation_view`, `preview_interaction`, `quote_trade` and `container_contents` are read models. Absent records read as defaults through the state accessors (`quest`, `relationship`, `history`, `object`, `areas`, `trigger`).

`LoadedProject::start` builds the authored starting state in a tool session and `run_scenario` also plays the scenario steps. To play against published content, take `project.start()?.into_state()` and open a session over a `ContentRepository`.

Before gameplay uses a published bundle, create `ContentLibrary::new(retention_directory)`, call `retain(bundle_path)` and open its returned identity through `library.open(&identity)`. Retained bundles are independent of source files and are not removed automatically. Saves reference mechanics; language packs are selected independently and are not part of save identity.

`SaveDirectory` supports manual slots, quicksave and autosave retention (1–32). A save is one `.save` file: a header line and the state as JSON, written beside the slot and renamed into place. Save format **16** rejects other formats; regenerate fixtures rather than migrate them. `saves.load(slot, content_source)` restores a slot against the content at hand, whichever content it was saved with: stats are worked out again, resources kept within their caps and party levels caught up, then the whole state is checked, and a save that still does not fit is refused with the reason. Compare `saves.content_identity(slot)` with the content's identity to tell the player the content has changed; `library.load(&saves, slot)` restores against the exact content the slot was saved with.

`Driver::new(session, step_ms, trace_capacity)` moves a session through time for the game and for headless runs alike. `elapse(real_time)` turns real time into game time in whole steps (at most a second at once) and keeps the remainder; `submit`, `step` and `advance_until(max_steps, predicate)` use explicit logical time, so no real-time sleeps are required. A trace capacity above zero keeps the events of the last commands; failed commands add nothing. An unmet predicate returns an error after its step budget; steps already accepted stay accepted.

Run the standalone checks:

```sh
cargo test --offline -p yarra-game-content -p yarra-game-types -p yarra-gameplay -p yarra-localization -p yarra-save
cargo clippy --offline -p yarra-game-content -p yarra-game-types -p yarra-gameplay -p yarra-localization -p yarra-save --all-targets -- -D warnings
```

See [gameplay architecture](ARCHITECTURE.md#standalone-gameplay-foundations) for ownership and deferred features. The authoring tool and demo remain separate from game entities and UI.

## Validation and platforms

```sh
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets
cargo test --offline --workspace
python3 -m unittest discover -s tools -p test_grass_profile.py
```

`--streaming-smoke` explicitly installs `StreamingSmokePlugin`; ordinary game/editor composition contains no smoke state or exit system. It runs the demo-world traversal, cooling/ownership and second-world gameplay checks, then exits. Use the cooked overworld/interior fixture (zero initial gameplay objects, one interior object), not arbitrary authored worlds. It accepts both hierarchy and `--terrain-legacy`; traversal checks canonical destination cells and current source demand after rebasing. The existing 3/7.5/11-second checkpoints are readiness assertions, not performance measurements. It cannot share control/exit ownership with a profile, repro or capture. The default island is not a smoke fixture; run against a separately cooked historical demo. With unused database paths:

```sh
cargo run -p yarra-world-cook -- create-demo tmp/smoke.project.sqlite
cargo run -p yarra-world-cook -- cook tmp/smoke.project.sqlite tmp/smoke.runtime.sqlite
cargo run -p yarra-app-game -- --world-db tmp/smoke.runtime.sqlite --streaming-smoke --diagnostics off
```

Add `--terrain-legacy` to exercise the older renderer with the same fixture.

Native GPU/large-fixture tests are ignored by default and run explicitly for relevant changes. For example:

```sh
cargo test --offline -p yarra-engine mountain_cover_uploads_draws_moves_and_rebases -- --ignored --nocapture
cargo test --offline -p yarra-engine msaa_store -- --include-ignored --nocapture
```

Use [iOS setup](../ios/README.md) for device build/packaging and [performance](PERFORMANCE.md) for capture conditions. Passing CPU tests does not establish visual parity, smooth pacing or sustained power/thermal acceptance.

### Headless world-action exercise

Run the guard/gate sequence without the engine:

```sh
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard-gate.ron
cargo test --offline -p yarra-game-content --test world_actions
```

A package lists the `areas` it refers to by name and its `objects` and `triggers` as files under `world/`. A trigger names who it is about, what it listens for, an optional condition and its actions:

```ron
(
    id: "guard/escort",
    player: "hero",
    speaker: Some("guard"),
    on: [Entered(actor: "hero", area: "guard/approach"), QuestStarted("guard/gate")],
    condition: Some(All([QuestStatus(quest: "guard/gate", status: Active), InsideArea(area: "guard/approach")])),
    actions: [Move(actor: Speaker, to: "guard/gate_post", timeout_ms: Some(10000))],
    repeat: Always,
)
```

`on` takes `Entered`, `Exited`, `Arrived`, `MoveFailed`, `ItemAcquired`, `QuestStarted`, `QuestChanged`, `VariableChanged` and `DialogueCompleted`. Conditions are checked when the signal is processed, so when several things must all be true, listen for each of them and test them all. `repeat` is `Once` (the default), `Always` or `Cooldown(millis: …)`. The actions take effect together or not at all; a failure is reported as `TriggerFailed` and the trigger can fire another time. Two trigger-only actions: `Move(actor, to, timeout_ms)` asks the engine to walk an actor into an area, and `StartDialogue(dialogue, speaker)` queues a conversation with the trigger's player. What should happen on arrival is a second trigger listening for `Arrived`.

The engine's side is three commands under `Command::World`:

- `Observe { actor, position, areas }` reports where an actor is and which named areas contain it. Send it when the set of areas changes, for every actor `observed(actor)` names (party members, anyone asked to walk, anyone a trigger's `Entered`/`Exited` names) and for as long as the engine is still moving one. It produces `Entered`/`Exited`, and `Arrived` when the actor was walking to one of them.
- `MoveFailed { actor, request }` gives up the walk in `state().world.movements` with that request number. The engine must answer every walk it is asked for, including one for an actor it cannot move.
- `Record { positions }` stores where actors stand, e.g. before a save. It leaves occupancy alone, so it never sets off a trigger.

Signals, triggers and queued conversations are carried out one after another inside the command that raised them, each in its own step. Content whose triggers keep setting each other off is cut short after 256 steps; the rest waits for the next command (`world_work_pending()`), and a scenario step that leaves work behind fails. Occupancy and walks are saved; after a load the engine continues from `state().world.movements` instead of expecting new requests. `AdvanceTime` moves the clock that walk timeouts and cooldowns use.

Conversations under way wait their turn in `state().floor`: one queue for blocking conversations and one for ambient ones, in the order they started, saved with the rest. The first blocking one is on screen; while there is one, `AdvanceTime` changes nothing, so the world holds still.

For normal object access, `Open` checks lock/destruction state, then `container_contents` exposes the bound inventory. Use `state().object(content, id)`, `.areas(actor)` and `.trigger(id)` for read models and diagnostics. Engine adapters must authorize access and control. The `--story` slice in the game is one such adapter ([`story/scene.rs`](../crates/app_game/src/story/scene.rs)).
