# Distant world and terrain rendering

Status: data contracts and bounded production cooking implemented, 2026-09-18. Terrain hierarchy
products and precision changes exist; camera-driven LOD rendering, distant materials
and scenery proxies remain **planned**. This is not a measured performance claim.

## Implementation checkpoint

Implemented:

- Canonical f32 cooked heights and matching triangle interpolation in terrain CPU
  queries, vegetation CPU queries and the grass compute shader. Runtime schema is
  **17**, page payload is **8**, terrain-node payload is **1**; project schema **22**
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
results, not render timings; the new nodes are not drawn yet.

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

**Next is slice 2:** active terrain cover, projected-error selection, transitions and
the shared game/editor renderer. View distance is still unchanged. The saved normal
authoring world is preserved; synthetic fixtures run in temporary test files.

Validation: world/database/cooker/environment/vegetation tests and all 123 editor
tests passed, along with the native Metal interpolation regression, workspace
compilation and focused Clippy with warnings denied. SQLite query plans confirm
primary-key range seeks for both hierarchy metadata and staged leaf iteration.
The default runtime was recooked by the streamed path as generation `5db3ff451dffc61a`: overworld
256 leaves / 340 nodes / 4 roots; interior 81 leaves / 119 nodes / 21 roots.
SQLite integrity passed, and the source project checksum stayed unchanged. Every
runtime product table matches the previous cooker (only generation metadata changed).
The current scene's maximum input batch was 9,801 height samples / 126,750 mask bytes /
1 manual object / 1 road span; peak encoded/decoded cell output was 26,389 / 46,510
bytes. The cooker/database suite passed 53 tests plus the separate 2 km acceptance;
all 123 editor tests passed. Core Clippy with warnings denied and workspace checks passed.

The desktop target is 60 fps at 1440p on mainstream gaming GPUs. Record the actual
internal render resolution as well as display resolution: current game defaults
render the world at 75% scale with 4× MSAA. A MacBook Pro M2 remains a development
and comparison device, not proof of performance on that target class.

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
| `engine::world_streaming` queries a 7×7 cell neighbourhood and requests terrain pages at `lod: 0` | Add a spatial hierarchy whose traversal does not depend on the local cell index. The current authoring world's 8 m cells make that query only 56 m across. |
| `terrain_render::build_heightfield_mesh` creates one mesh per chunk, one vertex per height sample and two triangles per grid quad | Select coarser geometry for larger areas; avoid retaining one draw per tiny source cell throughout the visible world. |
| Terrain textures have offline mip chains; the renderer includes prepared-ground and reference paths | Add regional composites and a distinct cheap distant material path. |
| Source heightfields and the road compiler produce a shared final terrain surface | Preserve that authority and build the hierarchy after relief evaluation. |
| Generated and manual objects use ordinary static meshes with object LODs | Add aggregate distant scenery products without loading every placement. |
| Database requests, decoding, attachment and residency already have limits | Extend accounting and scheduling to hierarchy metadata, fallback nodes and staged replacements. |
| The editor supports a floating origin; the game configuration currently disables rebasing | Complete game integration before treating long-distance travel as supported. |
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

Add geometry morphing from child positions to the parent triangle surface for
visible refinement/collapse. Shared edges and corners must use a common transition
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

Slice 1 is implemented and checked as recorded above. Later slices remain pending;
implement and validate them in order.

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
  world-switch tests. No holes, duplicated ground, internal cracks or unbounded
  residency. Leaf heights, vegetation roots and nearby road relief still agree.
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
