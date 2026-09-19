# Distant world and terrain rendering

Status: default game/editor integration implemented, 2026-09-19.
The default terrain renderer now selects and draws the hierarchy in both
applications, including synchronized geometry/normal morphing and protected
actor/grass contact, distance-based source streaming, staged world-space entry,
publication-generation handoff, and origin rebasing in the game. Large-world
authoring and sustained performance acceptance remain open. CPU material baking/storage, bounded coarse and fine material
residency, and blended composite rendering are implemented. A bounded close-range
surface cache now adds tiled/prepared albedo, micro normals and canopy treatment to
that path (2026-09-19). Art-pack transition review, material cost measurements and
far-material minification remain open. Scenery proxies remain planned. This is not
a performance claim.

## Implementation checkpoint

An editable [hill and valley test landscape](HILL_LANDSCAPE_TEST.md) now provides
the elevated-view acceptance scene: a 1.5 km meadow landscape, a summit spawn,
curved road and saved summit/slope/valley viewpoints. It uses the existing
production cook and renderer; performance acceptance remains open.

### Default renderer checkpoint (2026-09-19)

Normal game/editor launches now enable the terrain hierarchy, material streaming,
contact protection and regional live authoring. No `--terrain-lod` argument is needed.
`--terrain-legacy` explicitly selects the previous local renderer for performance
comparisons. It is a diagnostic option, not a quality preset or automatic fallback.
The change preserves the hierarchy's existing budgets, density and render settings;
it is not an additional optimization or a sustained-performance claim.

CLI `init`, `cook` and `import-vegetation` now include material baking by default,
using the repository's assets; `--terrain-materials ASSET_ROOT` overrides that root.
`--geometry-only` is an explicit cooker option for geometry diagnostics/fixtures.
Editor Save & Publish includes materials even when viewing the legacy renderer.
Missing CPU bake inputs fail publication without replacing the previous runtime.
Prepare them with `python3 tools/prepare_terrain_bake.py` after the terrain texture
pack. Previously unbaked runtimes require one normal recook for final ground shading.

The hill launcher no longer passes an enable flag. The profiler defaults to
`terrain_lod: true`; `false` maps to `--terrain-legacy` and remains checked against
runtime audit output. Old saved reports retain their measured settings. Historical
checkpoints and archived commands below describe the defaults at their recording
time; their opt-in wording is superseded by this checkpoint.

Validation: 218 Rust CPU tests (including explicit renderer/cooker selection and
publication failure in both editor modes), 29 profiler/report tests, and the native
Metal terrain/movement/live-edit/publication integration check passed. Release game,
editor and cooker builds passed; Clippy completed with existing warnings. Short game
launches reported `terrain_lod=true` by default and `false` with `--terrain-legacy`;
the editor loaded baked ground without an enable flag. A disposable CLI world verified
material publication through init, recook and vegetation import, explicit geometry-only
cooking, and preservation of the prior runtime on missing-input failure. These launch
checks do not measure performance; no new sustained run was required.

### Remaining priorities after the hill test (2026-09-19)

The chronological checkpoints below include historical next steps that later
checkpoints have implemented. Geometry, material cooking/streaming, close shading,
rebasing and publication handoff are functional; the remaining foundation work is:

1. **Contact readiness under budget pressure — corrected.** Actor, vegetation,
   camera-contact and visual priorities are now distinct. Stitched-edge requirements
   compete in the same queue as patch bodies, and budget-limited swaps may retire
   grass safely to admit actor ground. See the allocation checkpoint below. Hard
   limits still apply; this does not guarantee every requested grass page can fit.
2. **Regional live authoring on the hierarchy — integrated with limits.** The
   editor's default terrain path now stages applied source edits into geometry and
   material overrides, with cancelled-job rejection and a GPU-ready handoff shared
   with the nearby preview. Paint retains unchanged geometry. The current regional
   admission is 256 source/affected cells; broader shared edits require publication.
   See the live-authoring checkpoint for limits and acceptance coverage.
3. **Acceptance on the landscape.** Use short summit rotation, descent and budget
   stress checks for transitions, grass contact and bounded residency. Coarse-root
   material borders, extreme minification and steep-slope projection remain review
   items. Sustained GPU headroom is still unresolved; no new obvious GPU speedup is
   established by the hill appearance experiments.

Keep the current hill relief/shading for now. Distant forest/rock proxies, meadow
representations beyond blade range and a representative cliff asset remain planned
content/scenery work. They are separate from these foundation corrections. General
terrain sculpting, slope/elevation authoring rules and dynamic relief smoothing are
not required for the next checkpoint.

Implemented:

- Canonical f32 cooked heights and matching triangle interpolation in terrain CPU
  queries, vegetation CPU queries and the grass compute shader. Runtime schema is
  **18**, page payload is **8**, terrain-node payload is **1**, terrain-composite payload
  is **1**; project schema **22**
  and journal **11** are unchanged. Recook existing projects; no source migration.
- Typed hierarchy keys, nested height grids, exact retained samples/normals,
  conservative propagated error, descendant height bounds and sparse coverage.
  The initial parents retain canonical fine-surface normals; filtered/coarse-surface
  normals and their transition belong to the renderer/material work below.
- SQLite node descriptors/payloads and a bounded drawable coarse cover. Reads allow
  128 metadata keys, payloads cap at 1 MiB, and roots cap at 256 nodes / 32 MiB decoded
  / 128 MiB estimated geometry per world. Missing cells are never filled.
- A staged hierarchy pass over finalized terrain pages, with one leaf page or four
  child nodes decoded at a time and an 8 MiB SQLite page-cache target. Hierarchy
  checksums participate in generation identity; roots validate before publication.
- Tests for negative coordinates, overflow, skipped peaks, sparse cells, corrupted
  payloads, query/cover budgets, deterministic generation and failed publication.
  A native Metal test checks the production grass shader against CPU sampling on
  both sides of a nonplanar quad, at its diagonal, boundaries and clamped positions.

The existing 512 m fixture with mountain relief has 256 detailed cells and four
33×33 coarse roots: **8,192 triangles / 307,392 estimated geometry bytes** in that
cover. A separate road fixture verifies 1 cm road depth plus 5 mm track depth at
1,500 m elevation against the final cooked leaves. These are correctness/topology
results from the data checkpoint, not render timings. The geometry preview below
now draws these products.

Slice 1's data contracts and bounded production cook are now implemented. Both CLI
cooking and editor publication use `ProjectCookSnapshot` and `RuntimeCookWriter`:

- Keep one read transaction across all source reads. Concurrent saved edits belong
  to the next cook rather than being mixed into the current generation.
- Read the ordered union of terrain and painted cells in batches of 128 keys. Both
  branches use primary-key range seeks. Validate painted cells outside terrain too.
- Compile one output cell from at most nine source heightfields and a 4 MiB coverage
  halo. Road snapshots retain their existing record/query limits; manual placements
  cap at 8,192 per cell. Truncation is an error, never an empty/partial world.
- Bound global catalog variable data to 32 MiB and total catalog rows to 32,768 before
  decoding it. Catalogs and compile plans remain resident; they do not grow with
  terrain-cell count. Each SQLite connection targets an 8 MiB page cache.
- Write each cell immediately to a private staging database; encoded and decoded
  cell outputs each cap at 32 MiB. Build hierarchy products after the source pass.
  Validate source references and road indexes, then validate roots before atomic
  publication. Failed cooks remove their own staging files and retain the live DB.
- Reuse the existing compiler and page encoder. `ProjectDocument`/`RuntimeBuild`
  contain only global metadata plus one cell on this path; the whole-document
  `build_runtime` remains a small-fixture/reference API. Global catalog validation
  is currently repeated by the common packer per cell; caching those validated
  lookup tables is a possible cook-time improvement, not a spatial-memory dependency.

A 2,048 × 2,048 m synthetic mountain fixture cooked **4,096 cells in 10.92 s** on
this development machine (optimized debug test build, September 18). The largest
input batch was **9,801 height samples** (39,204 bytes of heights), the same as the
256 m and 512 m fixtures. Maximum encoded/decoded cell outputs were 8,150 / 8,768
bytes. The result has four level-5 roots. This fixture has no painted grass or
objects; road relief and collections are tested separately against the reference
cooker. It measures cook correctness and bounded arrays, not game frame time.
These counters exclude catalog copies, compiler scratch, allocator overhead and
SQLite caches; they are not a total-RSS measurement.

Reproduce the larger acceptance case with:

```sh
cargo test --offline -p yarra-world-cook two_kilometre_mountain_cooks_with_bounded_source_samples --lib -- --ignored --nocapture
```

CLI `cook` now prints cell counts and source/output high-water counters. Tests cover
reference-page equivalence with roads and generated collections, deterministic
publication, a saved edit during an open snapshot, missing road-index membership,
invalid out-of-terrain paint, oversized catalogs/manual-object cells, and preservation
of the previous runtime after failure. A project-import ordering issue discovered by
the collection fixture was also fixed: asset metadata is stored before validating
presets that reference it.

Slice 2 has started with the opt-in geometry preview described below. Normal
authoring still uses the detailed nearby renderer; its view distance is unchanged.
The saved authoring world is preserved; synthetic fixtures use separate databases.

Validation: world/database/cooker/environment/vegetation tests and all 123 editor
tests passed, along with the native Metal interpolation regression, workspace
compilation and focused Clippy with warnings denied. SQLite query plans confirm
primary-key range seeks for both hierarchy metadata and staged leaf iteration.
The bounded-cooker checkpoint recooked the runtime as generation `5db3ff451dffc61a`: overworld
256 leaves / 340 nodes / 4 roots; interior 81 leaves / 119 nodes / 21 roots.
SQLite integrity passed, and the source project checksum stayed unchanged. Every
runtime product table matches the previous cooker (only generation metadata changed).
The current scene's maximum input batch was 9,801 height samples / 126,750 mask bytes /
1 manual object / 1 road span; peak encoded/decoded cell output was 26,389 / 46,510
bytes. The cooker/database suite passed 53 tests plus the separate 2 km acceptance;
all 123 editor tests passed. Core Clippy with warnings denied and workspace checks passed.

### Geometry preview checkpoint

`--terrain-lod` enables a shared game/editor geometry preview of the **published**
hierarchy. Normal launches retain the existing authoring renderer. This is a
functional validation mode with one plain lit material, not a final visual result.

Implemented in this checkpoint:

- Full authored coverage with projected-error refinement, 2/1 pixel hysteresis,
  perspective/orthographic projection and three-dimensional contact distance.
  Camera matrices and node positions use canonical f64 coordinates for selection.
- Balanced 2:1 neighbours and all 16 edge-stitch index patterns. Selection includes
  the parent-surface error introduced by stitching and refines the coarse neighbour
  where a seam would exceed the pixel/contact tolerance. Balancing dependencies
  are admitted as one group; budget pressure cannot oscillate fine/coarse ownership.
  Sparse root forests first acquire their minimum balanced cover.
- One owner per terrain region. The previous complete cover remains while an entire
  replacement is built. GPU acknowledgement checks both prepared meshes and allocated
  vertex/index buffers before swapping the group at one command boundary.
- Root/metadata/payload requests share the existing SQLite worker. Payload decoding
  and mesh construction run in tasks; at most four terrain requests/decodes and two
  mesh jobs run concurrently. Responses carry monotonically increasing request IDs
  and generation identity, and stale work is discarded on world/generation changes.
- Separate prototype limits: 512 patches, 1,048,576 pre-stitch triangles, 4,096 cached
  descriptors, 32 MiB decoded node bytes and 128 MiB estimated geometry (including
  old/new overlap and mesh jobs). Selection has a one-million-work-unit cap and at
  most 128 metadata requests per plan. Root coverage must fit the profile. These
  limits are additional to the existing nearby-page budgets, not total process RSS.
- Eviction retains active/staged geometry and required hierarchy metadata. A
  render-origin change repositions the existing patches without reloading them.
  Stationary cameras reuse their plan and entity transforms until view, quality
  settings, hierarchy metadata or active-cover membership changes.
  `TerrainLodStats` reports levels, triangles, error, contact/budget shortfalls,
  pending/staged work and residency estimates; `TERRAIN_LOD` logs repeat every 2 s.

Flat worlds now cook at least **3×3** hierarchy grids: a coarse edge needs a real
midpoint shared by its two fine neighbours. Authoritative source/leaf pages are
unchanged. Previously cooked 2×2 hierarchies require a recook before this preview;
the reader reports this explicitly. The hierarchy encoding/schema is unchanged.
The default runtime was recooked as `ae988818fdab712f`, with 33×33 overworld grids
and 3×3 interior grids. SQLite integrity passed; the authoring source checksum is unchanged.

Run the preview on the current published world:

```sh
cargo run -p yarra-app-editor
```

For the explicit 2 km mountain fixture (never substituted for the normal world):

```sh
mkdir -p tmp/terrain-lod
cargo run -p yarra-world-cook -- create-mountain-fixture tmp/terrain-lod/project.sqlite
cargo run -p yarra-world-cook -- cook tmp/terrain-lod/project.sqlite tmp/terrain-lod/runtime.sqlite
cargo run -p yarra-app-editor -- --project-db tmp/terrain-lod/project.sqlite --world-db tmp/terrain-lod/runtime.sqlite
```

The create command requires a new output path. The game also accepts `--world-db`. Use the editor for elevated cameras in this unpopulated fixture.
The existing 2 km acceptance fixture is now reused by the CLI and renderer tests.

**Remaining before normal use:** detailed-to-distant material integration and
broader production integration. World-space switches and publication reloads now
retain the old cover until replacement terrain is ready (checkpoints below).
Applied unsaved authoring edits are not reflected in this published-only preview.
Nearby source pages still load for grass/gameplay. The source-streaming checkpoint
below removes their duplicate ground meshes in this preview. No terrain or full-scene frame-time
improvement is claimed from this preview.

Validation: 123 editor tests passed; focused engine/terrain/cooker tests and the
workspace check passed. Clippy passed with warnings denied except the existing
engine lint categories `too_many_arguments`, `type_complexity` and
`large_enum_variant`. The native Metal test cooks the separate 2 km fixture,
renders to a **512×512** target, verifies pixels and complete cover ownership, then
checks delayed uploads, a summit/downward view, a valley teleport, origin rebasing,
and release on a world switch. The final run completed in about 15 seconds including
its cook; that is a test duration, not a renderer benchmark. Example settled states:

| Camera | Patches | Triangles before stitching | Estimated mesh bytes | Decoded node bytes | Maximum projected error |
| --- | ---: | ---: | ---: | ---: | ---: |
| Overview at (1800,1500,2200) m | 16 | 32,768 | 1,229,568 | 175,260 | 1.455 px |
| Valley at (-350,160,-450) m, after rebase | 133 | 272,384 | 10,220,784 | 1,200,531 | 1.863 px |

Neither settled state reported a budget/contact shortfall. These are geometry-only
snapshots with plain lighting and shadows disabled, not 1440p scene-cost evidence.
The pure tests separately cover all stitch patterns, sparse/unready roots, negative
coordinates, hysteresis, orthographic projection and insufficient patch budgets.

```sh
cargo test --offline -p yarra-engine mountain_cover_uploads_draws_moves_and_rebases --lib -- --ignored --nocapture
```

### Smooth transition checkpoint

The published geometry preview now blends geometry **and vertex normals** over
0.25 s, using one shared Bevy morph weight for a complete replacement group. It
supports simultaneous refinement and collapse, skipped hierarchy levels, negative
coordinates and changes in edge stitching. A stalled frame advances at most 1/30 s
of morph time; weight 1 is extracted before the final static meshes take ownership.
Unchanged patches keep their existing meshes. Normal launches still use the existing
authoring renderer; no source/runtime schema or saved-world changes are needed.

The transient partition is the common finer cover of the old and new partitions.
Each endpoint collapses its grid vertices onto the corresponding retained vertices
of that endpoint's actual stitched mesh. This moves XZ as well as height; it is a
triangulation-preserving collapse, not a height-only interpolation through a coarse
surface. Both endpoints reproduce the original triangles. At shared edges/corners,
the coarsest closed-boundary owner supplies the canonical position and normal, and
all changing patches use the same eased blend factor. Conservative mesh bounds
include both poses. This avoids introducing a second overlapping ground surface.

The original cover remains drawn while the final meshes and transient meshes are
prepared. Readiness includes vertex/index/morph GPU buffers. A zero-area probe
prepares the standard material's morph pipelines without painting pixels; the
renderer waits for compiled main/prepass/shadow variants observed for that probe.
The production Bevy vertex paths also handle morph normals and motion vectors.
A render-origin change repositions static and transient patches without rebuilding;
a world/generation change or disabling the preview releases the in-progress group.

Limits remain **128 MiB** estimated geometry and **32 MiB** decoded samples.
Geometry admission reserves the complete transient meshes, including Bevy's padded
morph attributes, in addition to both endpoint covers; cloned source arrays in the
two asynchronous morph jobs count toward the sample cap. A common cover is limited
to twice the configured steady patch/triangle limits (at defaults, 1,024 patches /
2,097,152 pre-stitch triangles). Exceeding a limit retains the old cover and reports
an error. These are admission estimates, excluding allocator/pipeline overhead and
CPU/GPU asset duplication, not total process memory measurements. Stationary
settled scenes release the transient meshes and retain their previous static cost.

`TerrainLodStats` adds morph weight, transient cover counts and `quality_pending`.
The original error/contact metrics describe the **target cover**. The contact
checkpoint below adds separate certificates and diagnostics for the drawn cover.

Validation for this checkpoint:

- Pure geometry tests cover all 256 old/new stitch-mask combinations, multilevel
  changes, irregular balanced partitions and negative coordinates. They check exact
  endpoint triangle sets, non-inverted intermediate triangles, total coverage,
  shared-edge/corner normals, internal cracks and conservative pose bounds.
- A budget rejection leaves the original cover intact without allocating entities
  or mesh assets.
- The native Metal test uses the 2 km fixture at 512×512, now with asynchronous
  pipeline compilation, directional shadows and depth/normal/motion prepasses.
  Readback checks the start pose against the original image, verifies an intermediate
  pose changes pixels, and checks rebasing halfway through a held blend. It also
  exercises delayed acknowledgements, summit/valley views and cancellation during
  a world switch. The final native run passed in 23.59 s including fixture cooking.
  These are functional checks, not a 1440p frame-time benchmark.
- All 37 focused engine/terrain unit tests passed. Workspace compilation and focused
  Clippy passed, with the existing engine lint exceptions listed above.

### Protected contact checkpoint

The opt-in hierarchy preview now collects contact demand separately from camera
detail. Grounded actors request exact leaf terrain around their canonical position,
even offscreen or before their ground height is loaded. Prospective motor steps
wait for certified ground without clearing movement intent; the grounding resolver
samples the published leaf payload directly. A teleported actor remains hidden
until the drawn surface is exact and its root height has been updated. Normal
launches retain their existing grounding path.

Grass pages within the blade range request exact terrain too, with a 16 m planning
guard beyond the consumer footprint. Root range culling now uses 3D distance in
both GPU scheduling and individual candidate rejection. Thus a camera high above
a valley no longer requests grass just because it shares the valley's XZ position.
The 96 m range includes the existing blade/wind reach bound; this range correction
also applies to normal launches. The next checkpoint extends source loading for
the hierarchy preview. A distant meadow representation remains separate work.

Readiness certifies the **drawn** cover, with complete XZ coverage checked separately
from accuracy so missing sparse-world ground cannot pass. Static certificates
include stitch error (conservatively twice the parent error). Morph certificates
bound the entire swept mesh against the authoritative descendant height extrema;
they cover horizontal collapse as well as vertical movement. This bound is
deliberately conservative and can overestimate error on steep terrain. If it cannot
certify a required contact region, the fully uploaded target replaces the old cover
atomically instead of animating through that region. If the target would remove
contact already certified by the old cover, the old cover is restored/retained and
selection replans with current demand. This may produce a local geometry pop under
abrupt demand changes; it avoids displaying unsupported grass roots or feet.

Grass rendering filters whole source pages until their drawn terrain is certified.
It also compares the grass height field with the published leaf over their nested
grids and requires agreement within the configured contact tolerance (default 1 cm).
A readiness change invalidates generation without editing authored coverage; even
the profiling `DrawFrozen` mode clears stale draw arguments. Applied unsaved terrain
edits can therefore hide mismatched grass in this **published-only diagnostic**
view. Regional authoring overrides remain future work. Independent preset preview
cameras are not gated by the world view.

Contact requests cap at 256 regions and the source-page cache at 4,096 pages
(expanded from 1,024 for small-cell source streaming below).
Exceeding either cap reports an error and blocks consumers rather than certifying
unknown ground. Terrain/mesh residency limits are unchanged. Contact data follows
generation, world and origin identity; actor sampling rejects an old-world
certificate. Missing data and insufficient budgets are visible readiness failures,
not a promise that every requested contact can be satisfied within current limits.

Additional `TerrainLodStats` fields report drawn contact shortfalls, conservative
drawn pixel error, blocked actors, blocked/mismatched grass pages and atomic contact
handoffs. The target metrics remain separate. Source mismatches have their own
counter even when the terrain geometry itself is exact.

Validation adds sparse coverage and offscreen-demand budget cases, source-height
agreement in both grid-resolution directions, rebased/world-specific readiness,
and preservation of movement intent while ground is unavailable. Native checks
cover an actor teleported into a held morph, exact offscreen grounding, rejection of
an obsolete coarse target, frozen grass invalidation/resumption and elevated-view
grass culling. The engine integration test also offsets a grass source over real
cooked mountain relief, checks that it is blocked, then repairs it and checks that
the gate reopens. These are functional tests; they do not measure scene performance.

Recorded validation (2026-09-18): all 76 focused CPU tests passed; the native Metal
mountain/contact test passed in 24.39 s including cooking, and the grass gate/range
test passed in 3.09 s. Workspace compilation passed. Focused Clippy passed with the
existing engine exceptions plus existing vegetation lint categories allowed
(`precedence`, `nonminimal_bool`, `useless_conversion`, `collapsible_if`,
`needless_update`, `derivable_impls`, `assertions_on_constants`,
`format_in_format_args`, `field_reassign_with_default`). Strict Clippy still reports
those pre-existing vegetation warnings; unrelated cleanup is outside this change.

### Distance-based source streaming checkpoint

`--terrain-lod` now requests terrain/vegetation sources by distance in metres,
independently of the hierarchy cover and source cell size. The camera requests the
96 m blade radius plus the shared blade/wind reach bound and a 16 m preload guard.
Each compiled cell's full height bounds participate in 3D distance rejection, so a
valley far below a summit does not load grass merely because its XZ position is near.
Descriptor bounds also include objects; they are conservative and may over-request
some source data. This does not increase the blade renderer's draw distance.

The gameplay/editing focus keeps its own local window. The game keeps height data
in its one-cell actor preload ring; the editor retains its three-cell tool window.
Individual visible objects remain restricted to the local three-cell window, and
gameplay pages remain in the one-cell ring. A detached camera can therefore request
nearby terrain/grass without loading all objects between itself and the player.
These are bounded local policies, not distant scenery proxies or general NPC demand.

Index reads use at most two windows (camera and focus), cap their combined requested
area at 4,096 cells, and deduplicate overlaps. Oversized requests report a source
demand error and retain current residency; they are never silently truncated.
SQLite performs a primary-key seek for each X row with a bounded Z interval rather
than scanning every distant Z entry in an X strip. Camera rotation reuses the index;
descriptor queries change only when the covered cell windows change. The source
view is captured after transform propagation and consumed on the following update;
the preload guard provides headroom but is not a load-latency guarantee.

In this preview, terrain source attachments contain the canonical height/normal
field and page identity only. They fetch no terrain material resources/dependencies
and allocate no meshes, materials or images. They retain the `StreamedTerrainSurface`
interface used by grass and terrain sampling. The existing page payload is still
decoded, including its ground weights, which are discarded on attachment; a compact
source-only payload is not yet introduced. Published hierarchy meshes remain the
sole ground renderer. Normal launches keep their detailed terrain renderer and
existing source footprint; toggling the diagnostic mode rebuilds nearby attachments.
There is no schema change, recook, source edit or saved-world replacement.

Requests prioritize local consumers, then nearby camera sources, then static objects.
Loading, decoding and prepared pages share a cap of 16 outstanding page slots; the
worker's queue alone is no longer treated as the admission limit. Attachment remains
limited to two pages per frame. The existing 64 MiB resident decoded-data and
256 MiB estimated resident GPU-data caps are unchanged. Prepared data is reported
separately; the reader's 64 MiB per-page format limit gives 16 slots a conservative
1 GiB decoded-payload ceiling (the production cooker limits its output pages to
32 MiB). This is a count-based staging bound, not a claim
that total process memory fits the resident cap: decoder scratch, encoded replies,
allocators and the hierarchy's separate residency still cost memory.
Byte-based admission before decompression remains necessary before production use
with large page payloads; the current source fixture pages are much smaller.

Grass contact planning coalesces page footprints into 32 m bins while preserving
their full protected extents. This lets hundreds of small source cells share the
256-region planning limit. Readiness still checks actual source pages individually;
the metadata-only contact cache caps at 4,096 pages. This does not relax exact-ground
requirements or the terrain patch/triangle budgets. Very small cells or extreme
density/range profiles can still report a budget shortfall.

`StreamingStats` now includes indexed cells, height-only page counts, prepared
decoded bytes, pages waiting on the residency budget, and source-demand errors.
Residency/query failures appear in the existing status text.

Validation covers 8 m cells beyond the old three-cell radius, elevated views,
detached camera/focus windows, negative coordinates, query overflow, preserved relief
without GPU assets, and a drained worker queue that cannot bypass pending-page limits.
SQLite query-plan checks confirm seeks on both spatial axes. The native Metal 2 km
fixture passed in 24.61 s including cooking: its valley state indexed 113 cells and
held 99 height-only pages (868,032 declared decoded bytes) with zero source-page GPU
bytes, meshes or terrain materials. Movement, delayed uploads, morphs, contact gates,
rebasing and world switches still passed. These are functional/representation checks,
not frame-time measurements; the fixture has no dense grass or object population.
All 191 engine/database/editor CPU tests passed, along with workspace compilation
and focused Clippy using the previously documented engine/vegetation exceptions.

### Staged world-space entry and game rebasing (2026-09-18)

The opt-in hierarchy path stages a **balanced coarse cover** for a requested world
without changing `ActiveWorldSpace`, the camera, origin, or source-page residency.
It waits for actual mesh allocations and compiled static main/prepass/shadow
pipelines. Once ready, it installs that complete cover and commits the world change
at the same deferred-command boundary. Refinement and exact actor/grass contact
loading then proceed normally; coarse coverage alone does not release grounded
actors. Same-world teleports reuse the existing cover and its contact gate.

The current cover's LOD work pauses during staging. Already-admitted mesh/morph
preparation drains first; a running morph can retain its current pose. Current and
staged declared node/mesh allocations share the **32 MiB / 128 MiB** limits, including
the entry pipeline probe. The four active IO/decode slots and two mesh-job slots are
shared by draining old work before new work is admitted. Each metadata cache remains
capped at 4,096 records; entry can temporarily retain two metadata caches. A budget
rejection, failed destination decode, or replaced request releases only the staged
resources. Monotonic request IDs prevent cancelled replies from entering a new stage.
The existing HUD status reports pending/rejected entry, and `TerrainLodStats`
records entry status and staged declared bytes separately. Empty worlds require no
terrain upload. This is an atomic world switch, not a crossfade.

With `--terrain-lod`, the game now rebases beyond eight cells (256 m for 32 m cells).
`WorldRenderRoot` marks game-owned root transforms: the player, camera and movement
indicator use it; future positional lights/effects should also use it. Children are
left in local coordinates. Actor destinations translate without changing gait,
motor state, or destination revision. Root global transforms update before pointer
queries. Streamed object roots shift once; canonical CPU height/grass source pages,
pending requests and their entities survive a same-world rebase. The hierarchy
continues to position its static and morph meshes from canonical keys. Editor roots
already reconstructed from canonical records retain their own coordinate contract.
The camera-relative sun updates before transform propagation.

Rebasing also revealed that grass lattice seeds and wind had been using render
coordinates. Placement/grouping now use the canonical lattice, then subtract the
render offset before terrain/coverage/culling queries. Wind and blade shading bands
use the canonical position too. Both applications synchronize the grass coordinate
frame after source streaming; isolated preset previews keep origin zero. This adds
16 bytes to the camera uniform, with no new per-blade storage. Positions/phases on
the GPU remain f32: the tests cover kilometre-scale terrain and a 320 m offset in
both axes, not arbitrary planetary coordinates. The normal game launch still uses
its previous non-rebasing renderer until near/far material integration is ready.

Validation: the CPU rebase test checks world position, destination, camera global
transform, child transforms, unchanged page entities, pending requests and a second
frame without drift. The extended native Metal terrain test uses a second populated
world, pauses upload acknowledgements, injects a destination failure, cancels/retries
requests, and verifies the old live morph survives until a complete replacement.
It passed in 24.75 s including cooking. The grass GPU readback test retained identical
instance counts in all four topology bins after rebasing, for prepared and fallback
rendering. Mean byte error was 0.023689 / 0.023849 on a 256×256 image (small f32
subpixel differences), and the test completed in 3.10 s. These are functional checks,
not frame-time or sustained thermal measurements.

All 217 engine, vegetation, vegetation-renderer, editor and game CPU tests passed.
The workspace compiles, and focused Clippy passes with the existing engine/vegetation
lint exceptions. Normal editor/game defaults and the checked-in authoring databases
were not changed.

Publication swaps use a new immutable SQLite reader; the checkpoint below adds
that reader's readiness/rollback contract. Startup currently uses the existing
coarse-cover load and actor contact gate, with no loading-screen or scenery-readiness
contract. Applied unsaved authoring overrides remain slice 4. The preview is not
enabled for normal launches yet.

### Staged publication-generation handoff (2026-09-18)

Publishing still atomically replaces the runtime database on disk. Adopting that
publication now has two phases: the worker opens and validates a **candidate**
immutable reader while retaining the current reader, then commits it only when the
main world requests the matching operation ID and generation. A completed open is
not an adoption acknowledgement. Source index/page requests carry their generation;
queued requests cannot accidentally read a replacement database. Terrain requests
can address either snapshot during preparation. At most two readers are retained.

With `--terrain-lod`, the candidate stages a balanced coarse cover through the same
bounded entry path as world-space changes. Current source pages and terrain stay
live during opening, decoding, mesh construction and GPU/pipeline preparation.
After readiness, the main world requests commit, waits for the worker's matching
acknowledgement, then adopts the catalog, replaces the complete terrain cover and
invalidates old source pages together. Nearby sources repopulate from the new
snapshot; existing contact gates protect actors and grass until matching detailed
ground is ready. This is an atomic terrain replacement, not a relief crossfade or
a guarantee of uninterrupted nearby scenery while source pages reload.

Opening, validation, stage/decode and budget failures discard only the candidate
and report an adoption error. The old reader, catalog and resident terrain/pages
remain usable, and the same publication can be retried. This does **not** undo the
already published file on disk. Bounded request queues retry preparation, commit
and discard without reporting early completion. Stale operation acknowledgements
are ignored. World-space requests wait during adoption and remain queued afterwards.
Changing/removing the active world's coordinate grid is explicitly rejected: a
cell-size change needs reopening the world, and removing the active world needs
entry into another retained world first. Live grid migration is not implemented.

The normal renderer also uses the reader prepare/commit acknowledgement but retains
its existing nearby-page reload behavior; only the hierarchy preview stages a full
GPU-ready terrain cover. Default launches and checked-in authoring databases are
unchanged. No database format change was needed.

Validation: five new CPU tests cover matching/stale acknowledgements, preparation
and commit failures, bounded queue retries, incompatible active worlds, and actual
atomic database replacement. The worker test proves the old source pages remain
readable while candidate terrain comes from the new snapshot, and rejects old
source work after commit. All 35 engine CPU tests pass. The extended native Metal
2 km test paused upload acknowledgements, preserved old terrain and source entities,
injected a publication decode failure, retried, then verified the replacement's
45 m relief in both drawn node data and newly streamed CPU height sources. It passed
in **24.49 s**, including the existing movement/morph/rebase/world-entry checks and
temporary fixture cooking. All 123 editor and 16 game tests also pass, and the
workspace compiles. This is functional validation, not a GPU cost measurement.

The material-baking checkpoint below begins slice 3. Keep the hierarchy opt-in
until runtime material residency, shading and their transitions are validated.

### CPU ground composites and bounded material cooking (2026-09-18)

The optional material pass now consumes finalized cooked ground weights and
preprocessed CPU texture inputs. It runs inside the existing private publication
staging file after geometry cooking. It requires no graphics device. Normal cooking
and editor Publish still omit this experimental pass; the CLI/API opt in explicitly.
The renderer is unchanged in this checkpoint: these products are **not yet drawn**.

`tools/prepare_terrain_bake.py` prepares 128×128 mip chains from the plain/prepared
albedo, packed AO/roughness and macro sources. Albedo reduction happens in linear
light. `assets/packs/terrain/bake.ron` maps texture-set URIs to these inputs. The
current two-surface pack is **611,716 bytes**. Source/derived pixels remain under
ignored `assets/local`; the manifest and tool are tracked. Regenerate inputs when
source textures or prepared-albedo settings change. The loader validates version,
length, layer count, period and texture-set identity; it limits individual files to
4 MiB, the library to 16 MiB and the manifest to eight entries. Tests use synthetic
assets and do not require this local texture pack.

Each `TerrainMaterialKey` uses the terrain hierarchy's canonical spatial grid but
is an independent material product. A tile has **64×64 interior texels**, a four-texel
neighbor gutter and **three** GPU-compatible mip levels: 72×72, 36×36, 18×18.
At each level the interior and gutters halve together, retaining globally aligned
filter footprints. The future material selector must choose a parent beyond this
mip range rather than generate a clamped full mip chain which would lose valid
neighbor gutters. Material detail selection must remain independent of mesh detail.
The future shader maps tile-local normalized coordinates with `(uv * 64 + 4) / 72`.

Products contain two RGBA8 maps: sRGB base color/opaque alpha, and linear-data world
normal (octahedral X/Z), perceptual roughness and material AO. Their declared GPU
allocation is **54,432 bytes per tile** (about 53.2 KiB, excluding allocation overhead).
Decoded/encoded records cap at 64 KiB and use versioned checksummed Zstd payloads.
The DB exposes descriptor-only reads (up to 128 keys) for admission before fetching
and decoding; material bytes are not bundled into geometry node reads.

Baking follows the detailed material's surface-slot order, normalized ground weights,
prepared albedo coordinates (or plain/reference stochastic sampling), roughness
ranges and macro variation. Coordinates remain canonical f64 until texture wrapping;
there is no render-origin input. Macro modulation is applied once in linear color.
No sun, exposure, dynamic shadow, fine tangent-space normal detail, grass canopy or
forest color is baked. Coarse world normals come from final terrain, are filtered as
vectors and renormalized. Fine roads are area-filtered using stratified ground
coverage samples and prefiltered source texture mips. CPU-versus-GPU material image
parity and the visible near/far blend remain acceptance work for renderer integration.

The material pass uses bounded metadata pages, one leaf at a time, four children
for parent reduction, and up to nine same-level cores for neighbor gutters. Cores
are compressed in a disk-backed SQLite temporary table, with an 8 MiB cache target.
Partial nodes participate in filtering but never become drawable tiles. Missing
exterior samples clamp to the current tile's edge; no terrain holes are filled.
A completed bake verifies material coverage against drawable terrain coverage,
limits each world's coarse material cover to 32 MiB, and validates root payloads
before atomic publication. Failed baking preserves the previous runtime database.

Independent material fingerprints include the baker contract, packed texture bytes,
profile/surface settings, weights, final terrain normals and neighboring/child
fingerprints. Geometry payload checksums are unchanged by material-only edits.
There is no incremental cook cache yet: dependency separation is ready for later
reuse, while this pass rebuilds its outputs. A constant elevation offset leaves
these ground appearance products unchanged when normals and paint are unchanged.

Runtime schema is now **18**; the authoring schema and source are unchanged. The
current world was recooked as `7ed6140eddb5aa94` with **441 drawable material tiles**
(340 overworld, 101 interior). Compressed payloads total **12,266,979 bytes**; all
levels together declare 24,004,512 GPU bytes, which is storage inventory, **not** a
runtime residency target. The peak was nine filtering cores / 36,864 core pixels,
also observed on the smaller fixture; output arrays, codecs, catalogs and SQLite
scratch are additional. SQLite integrity passes and the source SHA256 is unchanged.

Rebuild the local input pack once, then opt into baking:

```sh
python3 tools/prepare_terrain_bake.py  # Python with NumPy and Pillow
cargo run -p yarra-world-cook -- cook content/world.project.sqlite assets/generated/world.runtime.sqlite --terrain-materials assets
```

A normal editor publication currently produces a valid generation without these
optional products. The material renderer will need to request this pass before it
can rely on them. Normal game/editor launches and `--terrain-lod` retain their current
appearance until that integration lands. Other old runtime databases require a
recook for schema 18; no source migration is needed.

Validation covers linear-light filtering, vector-normal filtering, exact same-level
gutter agreement at all three mips including negative coordinates, sparse coverage,
codec/version/truncation rejection, actual road weights, deterministic production
cooking, material-only fingerprint changes with unchanged geometry, bounded
metadata requests, corruption detection, and failed-bake preservation of the old
publication. The world/database/cooker suite passes 77 CPU tests; all 174 engine,
editor and game CPU tests also pass (251 total). The workspace compiles and focused
Clippy passes with the existing lint exceptions. The separately invoked 2 km and
native GPU acceptance checks are not part of that count and were not rerun for this
CPU-only material stage.

Next: load/admit these tiles independently from geometry, add the cheap lit far
shader, and blend it with detailed near shading on the same mesh. Verify material
parity, rebase continuity, changing light and road/biome transitions before making
this the normal renderer or claiming a GPU performance improvement.

### Resident coarse materials and lit rendering (2026-09-18)

The shared `--terrain-lod` game/editor path now reads and draws baked ground. This
checkpoint implements the **coarse fallback cover**, not the complete material LOD
selector or the near/far blend. Normal launches retain the detailed authoring path.
There is no source or runtime format change and the existing material publication
`7ed6140eddb5aa94` can be used directly.

Material presence, descriptors and payloads use generation-tagged worker requests,
independent of geometry payloads. The complete coarse material cover is reserved
before payload IO; metadata reads contain at most 128 keys. Geometry and material
IO/decode share the existing four-job limit. Material admission allows up to 512
tiles per cover and **32 MiB of declared texture data across active and staged
covers together**. Exceeding this bound rejects the replacement and retains the
current cover. This counts all three mips of both maps, including tiles in flight;
texture/binding allocation overhead, encoded buffers and transient CPU/GPU copies
are additional. Main-world image bytes are released after render extraction.

Coarse material roots stay resident while geometry refines, stitches and morphs.
Each static or transient mesh binds the enclosing material root; world-projected
sampling does not stretch the texture over each new patch. This gives a stable
fallback without tying its residency to the current mesh partition. Finer material
selection must still be independent of geometry error: simply choosing a texture
at each mesh's level would under-detail large flat patches. A spatial lookup for
finer resident tiles and gradual material transitions remains the next step.

The fragment shader takes two filtered samples: sRGB base color and linear packed
world normal/roughness/AO. It uses the baked gutters and exactly three supplied mip
levels, decodes the normal, and feeds Bevy's dynamic PBR lighting, shadows and fog.
Macro color is already baked and is not applied again. It requires no mesh UVs or
tangents. CPU projection subtracts the cell origin in integer/f64 coordinates
before converting to render-space f32; rebasing updates small material uniforms
alongside mesh transforms, without rebuilding/reloading any tile.

World entry/publication preparation waits for **every material's prepared GPU bind
group**, as well as the existing mesh and pipeline readiness checks. Until then
the old cover remains intact. Decode/identity/budget failures discard only the
candidate. A publication that explicitly has no composites retains the plain
geometry diagnostic and reports that state; a missing tile in a declared composite
cover is an error. `TerrainLodStats` reports material status, resident tile count,
reserved bytes, and staged reserved bytes.

Editor **Publish when launched with `--terrain-lod` now includes the material bake**,
using the resolved asset root. Missing CPU input packs fail publication before
replacement; there is no silent plain-material fallback for a requested bake.
Normal editor publication and CLI cooking without `--terrain-materials` still omit
these experimental products. The input preparation command above remains necessary
after changing source texture/prepared-albedo settings.

The current overworld uses four coarse tiles: **217,728 bytes (about 213 KiB)** of
declared texture data. The interior uses 21 tiles / 1,143,072 bytes. These are coarse
fallback allocations, not the final near-material cost or GPU frame-time results.
Close-up ground is deliberately low resolution on this experimental path. Different
root resolutions may show appearance seams; footprint-driven tile refinement,
parent/child blending, extreme-minification handling, detailed normal/canopy shading
and CPU/GPU appearance parity remain acceptance work before making it the default.

Validation adds admission-before-payload, absent-versus-missing products, stale
replies, complete binding readiness, shared material ownership across mesh levels,
correct mip formats/packing, large negative coordinate precision, and failed editor
bake preservation tests. A native Metal readback test draws textured ground,
compares images before/after rebasing, and verifies that changing light intensity
changes the result. The 2 km streaming test now uses synthetic composite records
with full production coverage, including static/morph/prepass/shadow paths and
world/publication handoffs. It separately withholds material acknowledgements after
geometry/pipelines are ready to verify that entry still waits. These tests use
synthetic inputs and do not establish visual parity with the local art pack or
measure 1440p performance.

Results: 201 focused CPU tests passed (38 engine, 23 terrain renderer, 124 editor,
16 game). The native shader test passed in 2.36 s; the final textured 2 km streaming
test passed in 31.96 s including fixture cooking. Workspace compilation and
engine/terrain Clippy pass with the existing lint exceptions. The broader editor
`-D warnings` check still reports existing vegetation-editor lints
(`needless_range_loop`, `wrong_self_convention`, `collapsible_match`, `clone_on_copy`,
`items_after_test_module`); these unrelated files were not changed for this step.

### Independent fine-material streaming and transitions (2026-09-18)

The `--terrain-lod` preview now streams finer baked ground tiles independently of
mesh tessellation. A flat region can retain a large mesh and still show several
material resolutions within it. The selector uses projected texel size, conservative
3D height bounds, frustum visibility and the actual viewport. It refines above two
pixels per texel and retains selected branches down to 1.25 pixels; these are initial
quality thresholds, not measured final settings. Perspective and orthographic views
use the same canonical f64 projection. Looking down from altitude reduces demand
rather than loading every leaf under the camera's XZ position.

Coarse roots remain pinned. Fine tiles occupy a fixed **128-slot** cache shared by
all terrain meshes in the active world: two 72×72 texture arrays, each with the
three supplied mips, plus a 512-entry spatial lookup buffer. Allocation is
**6,983,680 bytes**, including the lookup buffer, regardless of occupied slot count.
The four-root overworld therefore reserves **7,201,408 bytes (about 6.87 MiB)** for
ground composites. The existing **32 MiB** material admission limit includes this
cache, coarse roots, and a staged destination's roots. This remains logical payload
accounting; driver overhead and upload/retirement copies are additional.

Selection retains ancestry and caps material descriptors at **2,048**, with at most
128 missing descriptors requested per batch. Descriptor reads now include final
height bounds from the existing terrain-node rows; no DB format or pixel recook is
needed. Required geometry gets request priority over optional material refinement.
Material payloads continue to share the four IO/decode slots. At most four completed
tiles enter a render upload transaction. Pending CPU pixels, lookup tables and
residency are bounded, and old branches are evicted as the camera moves.

Uploads write only the new physical layers. A texture write and the lookup table
that removes a slot's previous occupant are submitted together before terrain draws.
The main thread waits for the render transaction to be consumed before advancing
that cache again; it does not rely on a fixed frame delay. A new tile initially has
zero availability and fades in over 0.3 seconds after upload. Children wait for a
fully available parent; eviction fades descendants out before releasing ancestors.
Stationary views do not keep uploading texture pixels, and settled lookup tables
are not rewritten. World-entry preparation keeps the existing fine cache frozen
while staging the new coarse cover.

The fragment shader combines integer canonical cell addressing with local fractional
coordinates. Lookup survives negative cells, hash collisions and origin rebasing;
no absolute large world position is converted to f32 for fine addressing. Pixel
footprint chooses the material level and its valid mip range. Missing data resolves
to resident ancestors and ultimately the pinned root. Temporal availability,
continuous level blending and a two-texel edge transition handle partially loaded
neighbours. Ancestor edge transitions participate too, including when adjacent
regions differ by several material levels. Color blends in linear light and world
normals are renormalized. Fully available interiors normally sample one fine tile;
transition pixels can sample more ancestors, in addition to the current root sample.

This completes blending **between baked material resolutions**. It does not yet
restore the original detailed tiled surface shader, tangent-space micro normals or
grass canopy shading on the hierarchy path. Finest baked texel size is cell size / 64
(12.5 cm on the current 8 m authoring grid). Fine roads can now use those leaf products,
but close-up texture parity remains a separate acceptance step. Mixed-resolution
coarse-root seams and extreme minification beyond the root's valid mip range also
remain open; do not describe this preview as the finished terrain renderer.

Validation covers altitude/flat-geometry selection, request/capacity limits, complete
ancestry and parent/child fade ordering. The Metal readback test verifies finer
texture blending on one unchanged mesh, zero-availability fallback, negative-key
hash collisions, rebasing, slot reuse, dynamic light, and no repeated pixel uploads
while fading or stationary. The 2 km runtime test exercises the real worker/cache
through camera movement, geometry morphing, world entry and publication rollback.
`TerrainLodStats` adds fine tile count, material metadata count, upload count and a
material-detail budget indicator. These are functional tests and residency counters,
not performance measurements or a visual parity claim for the local art pack.

Verification for this checkpoint: **125 CPU tests passed** across the engine,
terrain renderer, world database and cooker. The native Metal material readback
test passed in **2.46 s**, and the 2 km runtime/entry/publication test passed in
**26.23 s**. Workspace compilation and focused Clippy checks passed. The runtime
test reached 125 resident fine tiles while keeping the four-root material allocation
at 7,201,408 bytes; a stationary interval produced no additional tile uploads.

### Close-range surface integration (2026-09-19)

The published `--terrain-lod` path now blends original detailed surface shading into
its baked ground. This uses the existing final surface weights (including roads),
shared texture set, normal strength/sign, AO, roughness, macro settings and optional
prepared albedo. It does not create a second terrain mesh or force geometry to leaf
resolution. Normal authoring remains on its existing editable renderer.

Nearby terrain source pages now fetch surface metadata with the already decoded
height/weight payload. A separate selector admits at most **64 cells**, using
canonical camera-to-3D-bounds distance, stable ordering and a residency preference.
It considers cells within **48 m**; the shader fades full close-range surface shading
between **24 and 40 m**, also reducing it for footprints between 6 and 18 cm per pixel.
These are provisional quality limits. A high view does not admit the valley merely
because it shares the camera's XZ position. Capacity pressure keeps baked fallback;
it is visible in `NearStats` and the `TerrainLodStats.near_material_*` counters.

The GPU cache has 64 layers of **257×257 RG8** source weights, **130×130 RG8** filtered
canopy coverage and a 256-entry spatial table. Its fixed payload is **10,695,296 bytes
(about 10.20 MiB)**, checked against a 12 MiB control-cache ceiling. Original weight
resolution is preserved up to 257 endpoint samples; smaller pages occupy a smaller
rectangle in the layer. Canopy masks are resampled from the existing filtered baker
at 128 interior samples per cell plus a one-texel halo. Two new carriers and at most
two tile uploads are admitted per frame. Texture writes and their addressing/settings
table publish in one render transaction, after actual GPU image/buffer readiness.
Availability fades in/out over 0.3 seconds. Missing neighbours receive a one-metre
transition to baked shading, so cache edges do not expose unrelated slots.

This control cache is **additional to** the 32 MiB baked-composite budget. With the
current four-root world and 128-slot fine composite cache, their combined logical
payload is **17,896,704 bytes (about 17.07 MiB)**. Shared original/prepared surface
textures, existing canopy source masks, CPU source data, driver overhead and temporary
retirement/upload allocations are additional. The world texture-set declaration is
still checked against the existing 256 MiB development profile; these figures are
not a complete GPU residency measurement. Material-only carriers reuse the prepared
albedo/canopy integrations, share a one-texel dummy weight image, and skip the old
per-page prepared-control and stochastic caches. Empty near demand unbinds/releases
its atlas and shared texture references. No surface weights are uploaded twice.

Canonical cell addressing is integer-based. Repeating UV, prepared-lattice and macro
phases are computed in f64 on the CPU and combined with cell-local shader offsets;
rebasing cannot move the texture pattern. Non-prepared anti-tiling retains the original
hash/rotation rule, while prepared albedo uses its declared periodic lattice. Macro
modulation is evaluated once in each representation, then complete linear colors,
AO, roughness and normalized world normals blend. Heightfield tangent orientation
matches the original +X/+Z surface mapping without adding tangents to hierarchy meshes.

The editor and game canopy integrations also cover these material-only source entities.
The game now discovers newly admitted terrain even when grass source revision stays
unchanged, and invalidates its canopy frame on a rebase. Canopy darkness transitions
with the close-range surface; extending it across the full 96 m grass domain is still
a separate coverage/range decision, not silently assumed by this cache.

Validation includes canonical phases across large/negative cell boundaries and
altitude rejection. Native Metal readback exercises two painted surfaces on one
unchanged mesh, patterned albedo, normal-strength changes, canopy coverage, negative
origin rebasing, source eviction, no stationary reuploads, and the optional prepared
albedo path. The detailed interior is compared with the original terrain shader.
The 2 km runtime test continues to cover source streaming, mesh morphs, world entry
and publication rollback/retry. These establish functionality, not a 1440p timing
budget or a finished art-pack transition. Coarse-root seams, extreme minification,
cliff projection and distant scenery remain open.

Verification for this checkpoint: **204 CPU tests passed** (124 editor, 16 game,
40 engine and 24 terrain renderer). The final near-material Metal readback test
passed in **3.59 s**, the coarse/fine fallback test in **3.09 s**, and the 2 km
runtime/entry/publication test in **26.71 s**. Workspace compilation and focused
Clippy checks passed with the repository's existing lint exceptions. The elevated
runtime views retained zero near-material pages/bytes; the dedicated close-up test
exercised populated near shading and release. Existing baked publications need no
schema change or recook for this integration.

The desktop target is 60 fps at 1440p on mainstream gaming GPUs. Record the actual
internal render resolution as well as display resolution: current game defaults
render the world at 75% scale with 4× MSAA. A MacBook Pro M2 remains a development
and comparison device, not proof of performance on that target class.

### Streaming responsiveness correction (2026-09-19)

The close-range integration's functional tests did not establish interactive
performance. Repeated navigation in the editor exposed long loading periods and
unresponsive UI; the game also regressed in a **release** build. A native CPU
sample taken while returning to the authoring field identified grass/terrain
contact certification as a dominant editor streaming task.

The correction preserves the rendered density and contact requirements:

- Height sampling no longer validates every height in a field for each individual
  sample. Full validation stays at construction/decoding/scene admission; sampling
  uses constant-time shape assertions. The former debug path made a dense field's
  repeated samples quadratic in its sample count.
- Contact checks compare only heights on the shared nested triangle grids.
  Equal-resolution fields use a direct height comparison. Previously each sample
  also interpolated/decoded normals that the certificate did not use.
- A streamed page or coverage edit preserves certificates for unchanged heights
  at the same canonical page key. A rebase preserves them too; changed heights,
  resolution, page size, world space or publication generation invalidate them.
  Fresh certification is capped at 66,049 grid vertices per frame. Unchecked pages
  remain hidden until certified; the budget never permits unsafe grass.
- Immutable vegetation snapshots share their storage with render extraction and
  the contact cache. They no longer deep-copy all resident pages every frame.
- Terrain readiness updates only compact grass work-item flags. They preserve
  surface/coverage/species buffers, peer indices and candidate acceptance caches.
  Changing authored source data still repacks and invalidates those caches.
- An uploaded contact refinement that needs an atomic handoff is recognized
  before preparing transient morph meshes. A target that would remove already
  safe contact ground is rejected. Distant changes still use normal morphing.

`TerrainLodStats.contact_source_checks` and `contact_source_samples` count actual
fresh certification work. On a stable loaded scene both counters should stop;
page arrivals should not recertify every resident page. Readiness changes alone
must not increment grass `source_repacks` or restart candidate-cache builds.

The isolated development-build certification benchmark, on this M2 Max, measured:

| Vegetation grid / terrain grid | Before | After |
| --- | ---: | ---: |
| 33×33 / 33×33 | 1.407 ms | 0.028 ms |
| 65×65 / 33×33 | 13.045 ms | 0.068 ms |
| 257×257 / 33×33 | 1913.325 ms | 1.421 ms |

These are single-run flat-grid CPU measurements, not whole-frame timings or GPU
savings. Reproduce with `cargo test --offline -p yarra-engine
profile_surface_certification -- --ignored --nocapture`. Release builds did not
have the repeated debug validation, but benefit from retained certificates,
shared snapshots, height-only evaluation and the readiness/cache changes.

Verification: **107 CPU tests passed**, plus the native grass readiness/cache test
and the 2 km terrain movement/rebase/publication test. The grass test compares the
exact restored instance multiset, checks the rendered image, and verifies that
readiness alone neither repacks source buffers nor rebuilds cached acceptance.
Workspace compilation and focused Clippy passed with existing lint exceptions.

A native editor replay at 2880×1800 repeatedly left and re-entered the field.
Menus remained usable during reloading; contact checks stopped at 253, 506, 759
and 1012 after successive complete loads (253 populated pages per load). The
follow-up CPU sample no longer showed contact validation dominating streaming.
The replay used panning, not the user's exact trackpad zoom gesture, so that
gesture still needs a user check. Grass retains incremental loading, and initial
loading/shader preparation is not instantaneous.

A separate short release-game comparison used `--terrain-lod --render-repro
grass-stream --profile-warmup 2 --profile-seconds 12 --profile-fps 60
--profile-size 2560x1440`, with the Metal HUD enabled. Before/after update averages
were 59.77/59.91 fps; worst measured updates were 47.97/33.32 ms; late updates were
8/7. Both runs stayed focused. This capped, single-pair smoke check does not prove
a general frame-time improvement, lower GPU power, or sustained 120 fps. It uses
the repro route's render settings, not all normal-game defaults. One intermediate
after run overlapped compilation and is excluded from the comparison.

Commands, logs, CPU samples and compact results are retained in
[the streaming evidence record](performance/20260919-terrain-streaming/README.md).
No publication/schema change or recook is required for this correction.

### Close-ground GPU regression correction (2026-09-19)

The streaming correction above did not resolve the release game's high-camera
cost. The user's Metal HUD showed fragment work dominating the frame. A subsequent
120 fps comparison isolated the independently resident **close-ground shader**:
at 2592×1456 internal pixels the initial LOD run averaged 8.35 ms GPU, versus
3.77 ms when only close-ground shading was bypassed. That bypass retained the
same grass, geometry and source residency; it is diagnostic, not a shipping
quality setting. The old terrain renderer averaged 4.12 ms in a separate smoke
check, but its 49 source pages versus the LOD path's 256 make it a less controlled
comparison.

The retained fix keeps all surface samples, material blends, canopy evaluation,
lighting, distances and detail. The shader hash search now returns a table index
and probes only keys, rather than returning a complete 304-byte `NearEntry` from
its dynamic loop. Neighbour availability reads only the fade value. Surface and
macro helpers also take an index and load the fields they use; they no longer
take that entire material structure as a value parameter. Native image tests
still compare the detailed interior with the original terrain shader and cover
mixed surfaces, normals, canopy, negative-coordinate rebasing and source eviction.

An experimental zero-weight layer shortcut was removed: its measured result was
not better. Skipping the distant material evaluation underneath fully detailed
ground remains a possible separate optimization; this correction does not change
the normal basis or near/far blending to accomplish that.

The stronger comparison uses the actual gameplay camera and controls, fullscreen,
the normal depth prepass and normal event-loop/VSync behavior. At 2592×1456
internal pixels, 4× MSAA, with 12 s warmup and 20 s measurement on the M2 Max:

| Metric | Original close-ground shader | Optimized close-ground shader |
| --- | ---: | ---: |
| Application updates/s | 117.35 | 120.00 |
| Updates exceeding 12.5 ms | 53 | 0 |
| Maximum update interval | 17.59 ms | 8.84 ms |
| Metal HUD mean GPU duration | 8.51 ms | 5.92 ms |
| Metal HUD p95 GPU duration | 8.74 ms | 6.04 ms |

Both measurements stayed focused with nominal reported thermal pressure. GPU HUD
samples overlap/correlate; the paired short runs establish a regression fix, not
a sustained thermal/power claim or a mainstream-PC performance guarantee.
Follow-up zoom cycles held 120 updates/s with no updates over 12.5 ms. The close
third-person run averaged 119.90 updates/s with two updates over that threshold
in 20 seconds, so isolated hitches still need observation.

Profiling now supports `--profile-native-pacing`: `--profile-fps` remains the
reference for late-update thresholds, but the normal event loop and presentation
mode are preserved. `--render-audit` can record a timed run without requiring a
scripted camera. Repro routes accept `--render-prepass` and `--render-ui-off`, and
log the actual choices. The ground-only/shading diagnostic now recognizes the
hierarchy's composite materials, including newly streamed meshes. The opt-in
`--terrain-near-off` shader bypass is logged as `terrain_near=off`; it retains the
near cache so attribution does not silently change residency.

See the [GPU regression evidence](performance/20260919-terrain-gpu/README.md) for
commands, controls, intermediate experiments and follow-up camera checks. No
source/publication data change or recook is required.

### Sustained GPU headroom follow-up (2026-09-19)

The preceding short-run fix did **not** establish sustained 120 fps. A repeating
30-second walking route now exercises the same meadow for arbitrary durations,
with a fixed oblique camera and moving streaming/detail focus. The controlled
legacy → ten-minute LOD → legacy suite reproduced the user's report: the LOD
path began near 120 updates/s and 5.57 ms GPU, reached about 111 updates/s and
9.13 ms in minute six, and the following legacy run immediately held about 120
updates/s and 4.58 ms. LOD page/mesh counts stayed fixed and process memory did
not grow. Sustained performance acceptance remains open.

This follow-up skips baked root/hierarchy material evaluation under fully
available close ground, reads near fields directly, and handles the first hash
probe before the collision loop. The close material now uses the actual mesh
normal, matching the original detailed renderer; native sloped-image and fallback
tests cover this basis change. Short stationary tests improved from 5.94 to
5.31 ms GPU (about 11%), with no resolution, density or geometry reduction.
The sustained results above already include these changes.

Late rootless GPU telemetry showed changing frequency/activity between paths;
GPU milliseconds alone are not a stable workload or energy measure. The profiler
now optionally accepts `--power macmon --macmon PATH` and records the executable
version/hash, system-wide power, active frequency/residency, temperatures and fan
speed. It does not download or install a collector, or change system power settings.
The existing `powermetrics` mode remains available. See the
[sustained investigation](performance/20260919-terrain-headroom/README.md) for
the measured limits, partial telemetry coverage and reproduction commands.

### Terrain allocation under budget pressure (2026-09-19)

Three focused cases reproduced lost contact despite enough capacity for its surface:
a visible patch crossing the camera plane tied contact's infinite score, vegetation
could consume the actor's capacity, and visual refinement ran before required edge
refinement. All three failed before the allocation change and pass afterward.

The planner now orders actor contact, vegetation contact, camera contact, then visual
error as separate priority classes. Projected error can still be infinite within the
visual class. Coarse neighbours inherit the priority of fine patches whose stitched
edges require them. Body and edge requests therefore share the same queue. After a
split, only children and adjacent patches have their priorities refreshed; versioned
entries reject obsolete queue priorities. Balanced split groups remain atomic. Work
is reserved for the final seam pass, so exhausted selection work can return a valid
coarser cover. Triangle, patch, metadata-request and work limits are unchanged.

At runtime a target still cannot discard certified actor ground. A budget-limited
target may retire previously certified vegetation ground through an atomic handoff;
the existing contact gate hides the affected grass before extraction. Otherwise an
old grass page could veto every plan that reallocates its capacity to the character.
Normal non-budget handoffs retain their previous protection. Missing or impossible
contact is still reported explicitly; the change never permits floating grass or
grounding against an uncertified surface.

The CPU-only probe of the current hill publication uses all nearby leaf cells as
meadow demand, including cells where actual painted coverage may be empty. At a
2560 × 1440 planning viewport it reported:

| Pitch | Triangle limit | Planned triangles | Actor ready | Satisfied grass guard regions |
| --- | ---: | ---: | --- | ---: |
| 35° | 1,048,576 | 755,712 | Yes | 44/44 |
| 35° | 262,144 | 258,048 | Yes | 2/44 |
| 70° | 1,048,576 | 485,376 | Yes | 41/41 |
| 70° | 262,144 | 258,048 | Yes | 2/41 |

Grass guard regions include prefetch/edge padding; these are not visible grass-page
counts. The smaller limit deliberately demonstrates graceful loss of lower-priority
detail. It is not a proposed production setting, a GPU timing measurement, or a
rerun of the original 65 × 65 terrain generation.

The reusable probe reads an existing publication and bookmark without changing them:

```sh
YARRA_TEST_WORLD_DB="$PWD/tmp/hill-landscape/runtime.sqlite" \
YARRA_TEST_START_VIEW="$PWD/tmp/hill-landscape/project.views/summit.ron" \
cargo test -p yarra-engine --lib published_landscape_contact_budget_probe -- --ignored --nocapture
```

Regression coverage also includes bounded metadata requests, insufficient capacity,
contact-order determinism, selection-work exhaustion, and priority-aware handoffs.
The publication test harness now initializes the launch-view resource added with
the hill fixture. CPU validation passes 45 engine and 29 terrain-renderer tests.
The native Metal regression also passes movement, morphs, rebasing, contact gating
and publication rollback/retry. Its new stress phase increases visual refinement
for the small test viewport, then lowers the triangle ceiling: the active cover
changes from 450,560 to 260,096 triangles with zero blocked actors. Those quality
settings exist only in the test. Logs and reproduction commands are preserved in
[the allocation evidence](performance/20260919-terrain-budget/README.md).
Regional live authoring is implemented in the following checkpoint.

### Live editor hierarchy checkpoint (2026-09-19)

The editor's opt-in distant path accepts applied environment paint and road edits.
The normal launch path is unchanged. The new path uses one background compiler,
debounced requests and cancellation between cells/filter steps. It keeps the last
accepted ground, grass and objects while replacement work runs. The source and
published databases are read-only during preview; Save and Save & Publish keep
their existing meanings.

Finalized edited leaf heights feed the same `TerrainNode` builder as publication.
Only changed geometry paths are rebuilt, using published siblings. Shared heights
and encoded normals are checked across leaf borders, including separate coarse
roots. Ground-only edits produce no geometry replacements. Changed material
interiors propagate to their ancestors and same-level filter gutters. Published
composite interiors are decoded in linear light for unchanged filter inputs;
preview filtering does not repeatedly filter an earlier preview. This can differ
slightly from a full cook's unquantized intermediates, so publication is the final
appearance reference. It does not add a runtime terrain filtering shader pass.

The renderer stages affected active meshes and coarse materials, waits for actual
GPU upload acknowledgements, then exposes a ready revision to the editor. The
matching nearby source/grass catalog and terrain region commit in one frame.
Stale database/decode replies cannot overwrite that revision. Changed fine/near
material caches retire at the handoff and refill against the new revision, using
its baked coarse materials in the meantime. A brief loss of close texture detail
is possible during refill. Unchanged camera-only source refreshes reuse terrain
products and do not reset these caches.

Canonical CPU heightfields now support paint and road ray picking directly. The
LOD path does not allocate duplicate detailed meshes just to make tools work.
Picking visits the grid squares crossed by the ray and intersects their actual
triangles, including after origin rebasing. Painting still requires resident local
source; this does not add arbitrary distant editing or navigation picking.

Dirty terrain products remain available after their source cells leave the nearby
working set. Undo carries explicit baseline replacements for previously changed
nodes/materials. Save updates source without changing the publication; an unsaved
undo after Save still overrides both. A new published generation discards the old
preview overlay and recompiles local preview from that generation.

Current limits and remaining integration work:

- One region admits at most 256 existing source/affected cells, 16 MiB compiled
  source products and 4,096 generated objects. Old and new source snapshots can
  coexist. The existing nearby source stream and geometry budgets remain bounded.
- Terrain override products and filter cores have a 96 MiB logical allocation
  limit per candidate. Old accepted products may coexist with the candidate.
  Filter reads are bounded separately. Staged meshes/materials must fit the
  renderer's existing replacement budgets; these counters are not total RSS.
- Shared preset/style edits conservatively visit the whole current world. If
  that exceeds admission, the editor retains the previous view and requests
  Save & Publish. A reverse-dependency index and incremental large-world shared
  edits remain follow-up work; there is no silent partial shared-preset update.
- This is an overlay on the published domain: adding/removing terrain cells still
  requires publication. Distant source edits saved in an earlier editor session
  should be published before opening the landscape; startup does not scan the
  entire source database for unpublished spatial differences.
- The overlay is session-local. Save & Publish is the way to make the complete
  distant result available to the game and future sessions.

Validation includes CPU paint/ancestor/gutter tests, deterministic repeated baking,
cross-root seam rejection and direct picking against canonical triangles. The
native Metal integration test also delays upload acknowledgements, supersedes an
uncommitted revision, applies paint without changing mesh handles, applies relief,
and undoes both before continuing its movement/rebase/publication checks. These
are correctness checks, not a GPU speedup or sustained-performance claim.

The editor worker's disposable-road-world test covers leaving the edited area,
Save, unsaved undo after Save, and cancelled requests. The CPU suites pass 124
editor, 46 engine, 29 terrain-renderer and 26 cooker tests. The two explicit
integration checks are:

```sh
cargo test --offline -p yarra-app-editor live_road_edit_move_save_undo_and_cancel -- --ignored --nocapture
cargo test --offline -p yarra-engine --lib mountain_cover_uploads_draws_moves_and_rebases -- --ignored --nocapture
```

The first requires prepared terrain bake assets and uses no GPU. The second
requires native Metal/GPU access. Both use disposable source/runtime databases.
The release editor builds successfully and remains running in an 18-second native
startup check against a separately cooked road world. Its startup log contains only
the existing Metal/egui bindless-texture warning. This smoke check does not replace
interactive brush/road acceptance on the user's landscape.

## Decisions and scope

1. Use a hierarchy of conventional indexed heightfield meshes for terrain.
2. Stream distant visual products independently of nearby source data, objects,
   collision and gameplay. Seeing a mountain must not load its detailed forest.
3. Derive every terrain resolution from the same final authored surface, including
   road and junction relief. There is no separately authored distant copy to maintain.
4. Use cheaper composite materials at distance. Texture mipmaps alone are insufficient.
5. Combine heightfield landforms with separate rock/cliff meshes. Procedural tools
   may generate or place those meshes during authoring/cooking.
6. Add distant object/forest representations after the terrain foundation works.
7. Share the hierarchy and rendering path between game and editor, with different
   demand policies and budgets where necessary.

Meshlets and mesh shaders are out of scope. This implementation does not require
virtualized geometry, a voxel world, runtime erosion, virtual texturing, or a new
grass renderer. General terrain sculpting and automatic slope/elevation material
rules are separate authoring work. They will feed the same terrain products.

No backwards compatibility or migration layer is required. When implementation
changes formats, bump and validate them explicitly and recook the testing world.
The checkpoint above records the format versions currently implemented.

## Current implementation and gaps

Checked against the working tree on 2026-09-18:

| Existing component | Change required |
| --- | --- |
| Local object/gameplay streaming uses a 7×7 window; the default terrain hierarchy has independent distance-based source queries | Keep these local consumers separate from the spatial hierarchy; the hierarchy is already independent of the local index. |
| `terrain_render::build_heightfield_mesh` creates one mesh per chunk, one vertex per height sample and two triangles per grid quad | Select coarser geometry for larger areas; avoid retaining one draw per tiny source cell throughout the visible world. |
| Terrain textures have offline mip chains; the renderer includes prepared-ground and reference paths | Add regional composites and a distinct cheap distant material path. |
| Source heightfields and the road compiler produce a shared final terrain surface | Preserve that authority and build the hierarchy after relief evaluation. |
| Generated and manual objects use ordinary static meshes with object LODs | Add aggregate distant scenery products without loading every placement. |
| Database requests, decoding, attachment and residency already have limits | Extend accounting and scheduling to hierarchy metadata, fallback nodes and staged replacements. |
| The editor and opt-in game hierarchy path support rebasing; normal game launches retain the previous renderer | Validate near/far material continuity before enabling this path by default. |
| The production cooker uses one consistent snapshot and writes each compiled cell to staging; the whole-document path remains a fixture/reference API | Preserve these bounds when adding composite materials and scenery products. |

The existing contracts remain relevant: [terrain surfaces](TERRAIN.md),
[environment authoring](ENVIRONMENT_AUTHORING_ARCHITECTURE.md),
[vegetation](GROUND_COVER_ARCHITECTURE.md), and [editor ownership](EDITOR.md).

## Separate demand and ownership

| Product | Demand | Source of truth |
| --- | --- | --- |
| Terrain visual hierarchy | Camera frustum, projected error and visual budgets | Final compiled terrain |
| Detailed height/vegetation fields | Explicit vegetation range and local consumers | Compiled leaf data |
| Individual static objects | Object visibility and transition into scenery proxies | Manual placements and deterministic generated placements |
| Distant scenery proxies | Camera visibility and projected size | Cooked grouping of those same placements |
| Collision, navigation and gameplay | Player/actor demand and gameplay readiness | Dedicated authoritative products, never a visual LOD |
| Editor editable source | Active tool, editing focus and dirty/history ownership | Project database plus applied unsaved changes |

A terrain node being visible does not imply that its source masks, individual
objects or gameplay pages are resident. Conversely, a vegetation consumer may need
detailed height samples even when the ground renderer can use a coarser mesh.
Unloaded data is never interpreted as empty terrain or empty vegetation.

## Terrain hierarchy and runtime data

### Addressing and coverage

Introduce a renderer-neutral `TerrainNodeKey { space, level, x, z }`.
Level zero covers one source cell. Level `L` covers `2^L` source cells along each
axis. Its lower cell coordinate is `(x * 2^L, z * 2^L)`; its parent uses Euclidean
division of `x` and `z` by two. Checked arithmetic and tests must cover negative
coordinates, level limits and overflow.

Do not overload the existing `PageKey.cell` with an ambiguous mixture of source-cell
and parent-node coordinates. Add a typed terrain-node address to the storage and
streaming APIs, leaving local object/vegetation page addressing explicit.

Each world manifest identifies a bounded coarse cover and its hierarchy roots.
Use a root forest where necessary; a grid anchored at zero cannot always represent
a world spanning negative and positive coordinates with one parent. Detailed node
metadata is paged and cached, not loaded for the entire world at startup.

Coverage is explicit. Missing source cells outside the authored terrain domain are
known absent; missing required records inside it are errors. Initially, a drawable
node must cover a complete square of terrain. Partially covered nodes are traversal
metadata and descend to covered children. They must not fill gaps or connect
separate islands with invented ground. Cook rejects a coarse cover that cannot fit
the configured limits. Arbitrary terrain holes and caves are a later extension.

### Node products

Each node descriptor contains:

- Key, child coverage, horizontal extent and conservative vertical bounds.
- Maximum geometric error in metres relative to the authoritative finest surface.
- Height/normal and material product references, format versions and fingerprints.
- Encoded, decoded and estimated GPU sizes for admission before allocation.

Use a `2^k + 1` endpoint-inclusive grid with a declared resolution per world.
Retain 33×33 samples for the current 8 m road fixture. Parent nodes keep the same
grid resolution while covering twice the width, so coarse nodes replace both
vertices and draw calls. A 33×33 grid has 2,048 triangles regardless of its extent.
This is a topology count, not a frame-time estimate.

Initial payloads store heights and normals; vertex/index data is derived off the
main thread. Index topology and edge-stitch patterns should be reused where the
renderer permits. A future shared-grid/height-texture implementation can replace
buffer construction without changing source ownership or hierarchy selection.

### Height precision and hierarchy construction

The former cooked heightfield quantized every cell against the world-wide height
range using `u16`. Expanding a range to 2,000 m produces about 3.05 cm between
representable heights, which can damage shallow road relief.

The first hierarchy implementation now uses canonical **f32 cooked heights**,
matching source precision, instead of widening that global 16-bit quantizer.
The compiler, CPU surface sampling, grass height upload and renderer share this format.
Shared endpoints must come from identical canonical samples; independent per-page
normalization is forbidden. Compact encodings can return later only with explicit
error bounds and cross-page agreement. Existing compression still applies.

Final surface queries must also match the mesh's piecewise-linear triangles.
The former CPU and grass GPU bilinear interpolation could disagree with those
triangles inside a nonplanar quad even when all four heights matched. Height and
interpolated-normal sampling now follow the same triangles across CPU, GPU, placement
and picking. Source resampling before final mesh construction
is a separate operation. Test a saddle-shaped quad, not only flat terrain and edges.

Construction order:

1. Read base heightfields and required neighbouring dependencies.
2. Evaluate roads/junctions once against base terrain, then finalize leaf heights,
   normals, ground weights, vegetation and object placement using that surface.
3. Build parent heights from nested samples of the finalized child grids, using
   the same triangle-diagonal convention at every level.
4. Compute a conservative error bound against the finest surface. Propagate child
   error plus child-to-parent deviation; do not measure only the retained samples.
5. Derive parent normals with cross-node neighbours and build composite materials.

Nested sampling deliberately preserves common vertices. Fine features that fall
between coarse samples are accounted for by the error bound and force refinement
when visible. Do not average road depth again or reapply relief at each level.
Bounds include descendant extrema and all geometry-transition positions.

## Selection, transitions and streaming

### Selection

Begin with the coarse cover. Traverse visible nodes and refine those whose
projected geometric error exceeds the profile threshold. Use conservative node
bounds and the actual world-render viewport, including render scale. Support both
perspective and orthographic editor views.

Start with a configurable refinement threshold of 2 rendered pixels and a collapse
threshold of 1 pixel. These are prototype values, not final quality defaults.
Selection must handle a camera inside or near a node without division by zero or
incorrectly accepting a coarse surface. Use deterministic priority ordering.

Screen-space error is subject to explicit patch, triangle, metadata, memory and
work limits. Under pressure, retain a coarser valid cover and report the quality
shortfall. Reserve capacity for required nearby terrain before refining distant
scenery. No algorithm may exceed its budget merely because the view contains a
large amount of terrain.

### Mountain summits and looking down

High viewpoints are a required use case for this hierarchy. Use the full camera
projection and three-dimensional node bounds when estimating error and proximity.
A valley floor hundreds of metres below the camera is distant even if its XZ cell
is nearby. Horizontal cell rings alone must not force that valley's finest terrain
or vegetation into residency. Nearby summit ground still receives protected detail.
Use conservative bounds for terrain that spans both elevations, then refine metadata
as needed; testing only a node's centre can discard a nearby cliff or ridge.

An elevated view may reveal much more terrain and many more objects with little
occlusion. The implementation must remain bounded in that case without assuming
that hills or trees will hide most of the world. Optional occlusion culling can
improve ordinary views later; it is not the mechanism that makes the summit fit.
Actual field of view and viewport changes must participate in selection too.

Looking down also tests representations beyond geometry: ground composites must
retain appropriately filtered roads, clearings and biome patterns, and forest
proxies must retain canopy area, height and gaps from elevated angles. Fine tracks
may disappear below pixel size; forcing all such features to remain sharp would
cause aliasing. Low-detail scenery must read as the same landscape while descending
from a summit into the valley.

### A valid active cover

For every visible authored area, exactly one active terrain resolution owns the
surface. A parent stays visible until all required child replacements, materials
and neighbour transitions are ready. Stage work over several frames, then switch
the replacement group together. Never remove a parent when a request is merely
queued, and never render overlapping parent and child surfaces as the final state.

Initial crack prevention uses a balanced quadtree: adjacent active patches differ
by at most one level. Precomputed edge-stitch index patterns connect fine edges to
the coarser neighbour's triangulation. Balance the **resident active cover**, not
just the desired tree; if dependencies cannot fit, keep/coarsen the parent.

Morph between the actual old and new triangulations for visible refinement/collapse.
The preview uses a common finer partition with canonical vertex collapse at each
endpoint, preserving the stitched triangles exactly. Shared edges and corners must use a common transition
target and factor derived from the active neighbours. Independent patch timers
must not open cracks. Normals and material transitions must also agree at borders.
Skirts may hide the outer authored boundary during prototyping; they are not the
acceptance solution for internal LOD seams.

Error/contact checks must account for stitched edges and morph positions, using
the relevant parent error bound where needed, rather than only the unmodified grid.

### Detailed consumers

Gameplay and vegetation sample authoritative leaf terrain, never whichever LOD
happens to be visible. Within the protected player area, keep exact leaf geometry.
Within the active grass/object-contact area, also constrain geometric deviation
by a small world-space contact tolerance; begin with 1 cm and validate it visually.
Refine further when necessary to prevent floating roots or buried grass.

Height/field source demand must cover the configured vegetation range in metres,
including required halos. The current fixed three-cell radius must not silently
truncate a larger grass range on the 8 m grid. Account for this demand separately
from the visible terrain mesh count. If the minimum required working set cannot
fit, report an invalid profile rather than silently dropping required data.

### Request lifecycle and budgets

Reuse the read-only database worker, asynchronous decoding and metered attachment.
Prioritize coarse coverage/readiness, protected nearby terrain, then the largest
visible error. Prefetch modestly along movement; bound speculative work and cancel
obsolete demand. Camera rotation should not continuously evict the nearby fields.

Extend admission to count metadata, height samples, meshes, material images, shared
assets, pending uploads and old/new replacement overlap. Deduplicate shared assets
in accounting. Budget limits must include pinned fallback products and staged
children, not only the currently drawn set. Reserve room to complete replacements
so admission cannot deadlock with every byte owned by old nodes.

The present limits (16 database requests, two attachments per frame, 64 MiB resident
decoded data and 256 MiB estimated resident GPU data) are starting constraints to
audit, not evidence that the new system fits. Make budgets explicit profile data,
add traversal and upload limits, and report measured peaks before changing them.

Pin a valid coarse cover for the active world. Load it before completing world entry
or a world-space transition; then refine. A teleport within that world immediately
has coarse scenery while its local detail loads. Gameplay readiness may still wait
for authoritative data. On failure, retain the last valid cover and expose the
error. Do not quietly render an empty horizon or accept a partial publication.

All jobs carry world, generation and request identity. Reject stale completions.
World changes release owned products. Publication stages a valid new cover before
replacing the old generation; unrelated generations cannot share a stitched edge.

## Distant terrain materials

Keep the existing detailed material nearby. Add cooked regional composite maps for
base color and low-frequency material response, with coarse surface normals. The
far shader uses these products with ordinary lighting and fog, without evaluating
the full surface stack or stochastic detail sampling for every pixel.

The material baker belongs to the asset/cooking boundary, not the pure environment
compiler. It consumes compiled ground weights and deterministic, preprocessed
surface texture data. Extend terrain texture preparation to provide the necessary
CPU-readable inputs; do not require a graphics device just to publish a world.
Retain the current renderer's supported-surface validation.

Bake and filter color in linear space, then encode it appropriately. Use canonical
world coordinates, cross-tile gutters and consistent mip generation. Filter and
renormalize normals correctly. Include the existing macro variation once, without
changing its phase when the floating origin moves. Fingerprints include material
settings, source texture hashes and baker versions.

Do not bake the current sun direction, exposure or dynamic shadows into albedo.
Day/night lighting must still work. Blend detailed and composite shading over a
bounded transition band on the same terrain geometry; changing material detail is
independent of changing geometry detail. Match average color and roughness so the
transition does not appear as a moving ring.

Initially these composites describe ground. They do not imply that distant grass
or tree canopies have been represented. Meadow coverage/color and forest proxies
are explicit subsequent products; avoid replacing a forest with flat green ground.

## Rocks, cliffs and distant scenery

Heightfields own the broad landform. Rock surface rules can describe exposed slopes;
meshes own overhangs, caves, ledges and distinctive cliff silhouettes. The initial
foundation permits opaque cliff meshes overlapping solid terrain. Cutting actual
terrain holes requires an explicit future source and rendering contract.

Cliff materials need suitable UVs or selective axis/triplanar projection; current
XZ ground projection stretches on vertical faces. Share material palettes and
ground-contact treatment where useful, while keeping mesh and terrain geometry
ownership explicit. Automatic cliff placement/generation is later authoring work.

For distant static scenery, cook hierarchical proxy groups from the same generated
and manual placements used nearby. Each group records its membership, bounds,
dependencies and replacement relationships. Use ordinary simplified meshes first;
forest clusters or impostors can be added where the real asset set justifies them.
Preserve canopy height, silhouette and large color patches.

Require elevated and near-overhead views of each proxy representation. Ordinary
vertical tree billboards are not sufficient for this contract. Start with simplified
3D canopy geometry; any later impostor solution must cover elevation angles as well
as horizontal viewing directions. This is why scenery acceptance cannot stop at a
forest skyline viewed from ground level.

A group proxy and its detailed members must not remain simultaneously visible as
duplicate scenery. Retain the proxy until its complete replacement group is ready.
Near mesh LOD selection continues within that group. Distant proxies do not create
individual gameplay entities, collision objects, or editor selection rows for all
of their members.

Shadow demand has its own range, LOD and budget. Do not expand detailed tree shadows
to the terrain horizon. Large terrain/cliffs outside the view can still shadow a
nearby area; later coarse caster or terrain-horizon data should cover that case.
The first terrain milestone retains current bounded shadows and reports distant
shadow omission as a limitation. Fog/aerial perspective supplies depth cues but
must not conceal holes or be required for acceptable clear-weather views.

## Cooking, invalidation and editor integration

### Bounded cooking

Introduce a streaming cook path rather than scaling the current all-in-memory
`ProjectDocument`/`RuntimeBuild` route to the full map. Read source in bounded spatial
batches from one consistent source snapshot, with certified halos. Write leaf
products to a temporary generation database. Build parent levels bottom-up by
reading bounded child groups and write products incrementally.

The final manifest is published only after coverage, dependencies, borders, bounds,
error values and formats validate. Interrupted or failed cooking leaves the
previous published generation usable. Never combine source revisions gathered
from different snapshots into one supposedly coherent generation.

Geometry edits invalidate affected leaves, normal/edge halos and their ancestors.
Ground-material edits invalidate material products and required filtering halos;
they do not rebuild unchanged geometry. Object edits invalidate the affected
scenery groups. Shared preset changes use reverse dependencies and bounded work
queues. Names and other nonvisual metadata do not invalidate render products.

### Editor behaviour

The editor uses the same distant renderer. Overview can display coarse terrain
without loading editable source or detailed placements for the visible region.
Selecting/focusing a remote location moves the logical camera and starts the normal
local tool working set. An approximate distant hit may support navigation; actual
painting, road edits and placement must wait for authoritative local surface data.

Applied unsaved edits overlay the published generation in revisioned preview
products. Rebuild affected hierarchy paths from changed leaves plus unchanged
cached siblings. Stage affected active nodes and their edge dependencies as one
coherent regional replacement. Keep the previous valid region while rebuilding;
never stitch newly edited heights against an incompatible stale border.

Preview jobs remain bounded and reject stale results. Keep controls, browser lists
and accepted imagery stable during refresh, following the recent inspector fixes.
Use a steady pending/error indicator without repeatedly hiding valid previews.
Save still persists source; Publish still changes the runtime generation.

Complete floating-origin support in the game as well as the editor. Logical node
keys and procedural sample positions survive rebases; render transforms, cameras,
actors, lights, picking and active terrain/vegetation/scenery move consistently.
World-aligned material coordinates must retain their phase and local precision.
Audit camera clipping and depth precision for long views rather than assuming a
larger far plane alone solves visibility.

## Code ownership

| Package | Responsibility |
| --- | --- |
| `world` | Node keys, descriptors, coverage, height/normal payloads, errors and validated format contracts; no Bevy dependency |
| `world_db` | Bounded source/hierarchy queries, node indexes, staged generation writer and manifest validation |
| `environment_compile` | Authoritative final leaf surface and existing ground/vegetation/object derivation; no camera LOD or texture decoding |
| `world_cook` | Bounded orchestration, hierarchy construction, material baking, dependency fingerprints and later scenery grouping |
| `terrain_render` | Mesh construction/stitch patterns, morphing, near/far materials and render resource lifetimes |
| `engine` | View-dependent selection, active-cover management, streaming budgets, origins and diagnostics |
| `app_editor` | Source editing, regional preview overrides, navigation and readiness presentation |

Extract focused modules from the existing large files as these responsibilities
are implemented. Keep source topology and hierarchy selection testable without a
window; no new broad framework is required for the first slice.

## Implementation sequence and acceptance

Slices 1 and 2 have the bounded data/cooking and functional opt-in geometry paths
recorded above. Broader production acceptance and performance measurements remain
open. Later slices remain pending; implement and validate them in order.

### 1. Data contracts, precision and bounded cooking

- Add typed hierarchy keys, coverage/descriptor contracts and bounded storage APIs.
- Replace coarse global height quantization through the shared terrain consumers.
- Align final-surface interpolation with rendered triangles on CPU and GPU.
- Add a streamed leaf/parent cooking path and a deterministic relief fixture.
- Begin with a 512 m landscape; extend to a 2 km landscape once cooking is bounded.
  Include negative coordinates, a steep ridge and shallow road/junction relief.
- Pass deterministic cook, shared-edge/normal, precision, sparse-coverage, truncated
  input, format rejection and interrupted-publication tests. Whole-project sample
  arrays must not be required in memory by the production cook path.

### 2. Coarse coverage and terrain LOD

- Draw the coarse cover and refine from projected error with bounded traversal.
- Implement active-cover handoff, edge stitching, contact protection and morphing.
- Finish game rebasing and verify editor perspective/orthographic views.
- Pass camera rotation, movement, teleport, delayed upload, eviction, rebase and
  world-switch and publication reload/rollback tests. No holes, duplicated ground,
  internal cracks or unbounded residency. Leaf heights, vegetation roots and nearby road relief still agree.
- Use a simple material while validating geometry; this slice alone does not claim
  finished scenery or final terrain shading cost.

### 3. Distant material products

- Bake composites with gutters/mips and integrate the cheap far shader.
- Validate biome boundaries, roads, broad color variation, lighting changes,
  material transitions and origin changes in both applications.
- Separate ground-paint invalidation from geometry invalidation. Record preparation
  time, material bytes and GPU pass cost against the detailed material reference.

### 4. Authoring preview and a usable landscape

- Connect applied unsaved changes to bounded regional hierarchy refresh.
- Verify edit, undo, save, publish and reload produce matching terrain/materials.
- Add one representative cliff asset and retain a reachable route from the near
  scene to the distant ridge. A mountain must remain the same landform when visited.
- Check grass/contact transitions and clear-weather views from ground level and
  an elevated lookout. General height-sculpting UI is not required for this fixture.

### 5. Distant vegetation and scenery

- Add ordinary mesh proxies for a representative forest/rock grouping, including
  correct membership and handoff to individual objects.
- Address meadow appearance beyond blade range using the vegetation representation
  contract; do not reopen the grass renderer wholesale as part of terrain LOD.
- Validate silhouette/color continuity and measure object, shadow and material cost
  separately. Revisit coarse distant shadows after the core view is working.

## Performance evidence and completion criteria

Add counters for active terrain patches/triangles by level, achieved pixel error,
budget-limited nodes, traversal work, request/decode/upload time, replacement backlog,
resident/staged bytes and proxy/object counts. Expose near terrain, distant terrain
and scenery toggles for attribution. Include shadow work in the report.

Use repeatable cameras: ground-level meadow, steep road/cliff, elevated wide view,
forest skyline, and a fixed travel/teleport path. Compare the same scene, viewport,
render scale, MSAA, lighting and frame cap. Record frame-time percentiles and stalls
alongside GPU pass timings; include cold loading and steady state. A temporarily
coarse cover under deliberate budget pressure is different from missing geometry.

The elevated case must include the highest playable summit looking across the
valley, steeply down and directly down, then a 360-degree rotation and a descent to
the valley floor. Test clear weather, representative sun directions and supported
FOV extremes. Include a valley that is close in XZ but far below the camera. Inspect
LOD/material boundaries, canopy coverage, distant-shadow limitations and replacement
backlog. Report summit cost separately; matching ground-level frame time is not
assumed. Terrain acceptance covers geometry/materials; full summit-scene acceptance
also requires the scenery work in slice 5.

Begin with short functional and cost measurements. Sustained thermal tests are
useful after choosing a stable configuration; they are not required for every
implementation slice. Do not extrapolate a Mac 120 fps result into a mainstream-PC
60 fps claim. Select and record an actual desktop baseline before claiming the
target is met; GPU milliseconds and memory budgets remain provisional until then.

Terrain foundation is complete after slices 1–4 pass their checks and measurements
show bounded cost while view distance increases. Complete open-world scenery also
requires slice 5 and representative content. Report unimplemented scenery/shadow
features explicitly rather than hiding them behind the terrain milestone.

## References and interpretation

- [NVIDIA: geometry clipmaps](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry)
  explains nested terrain resolutions and transitions. Yarra chooses hierarchical
  chunks to fit its page/cooking architecture; this is not a clipmap implementation.
- [Unreal: Landscape technical guide](https://dev.epicgames.com/documentation/en-us/unreal-engine/landscape-technical-guide-in-unreal-engine)
  describes terrain components, LOD and the CPU/draw cost of subdivision.
- [Unity: terrain settings](https://docs.unity3d.com/6000.0/Documentation/Manual/terrain-OtherSettings.html)
  documents geometric error control and distant composite terrain textures.
- [Unreal: World Partition HLOD](https://dev.epicgames.com/documentation/en-us/unreal-engine/world-partition---hierarchical-level-of-detail-in-unreal-engine)
  describes distant proxy geometry/materials that outlive detailed cell residency.
- [SideFX: heightfields and terrains](https://www.sidefx.com/docs/houdini/heightfields/index.html)
  describes procedural terrain authoring and heightfield limitations.
- [Unity: BillboardAsset](https://docs.unity.com/en-us/engine/6000.3/script-reference/unityengine/billboardasset)
  documents a billboard representation intended for approximately horizontal views,
  illustrating why elevated-view support must be an explicit scenery requirement.

The supplied Yōtei images are visual references for landform silhouettes, cliff
detail, canopy masses and atmospheric depth. They do not establish which rendering
algorithms that game uses. This specification is a Yarra design, not a reconstruction
of Yōtei's engine or a promise of equivalent performance.
