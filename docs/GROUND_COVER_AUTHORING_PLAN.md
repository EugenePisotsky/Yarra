# Ground-Cover Authoring and Publication Plan

## Status and purpose

This document specifies the planned authoring model for grass and other dense decorative ground
cover. [`GRASS.md`](GRASS.md) remains the record of the renderer that exists today. This plan adds
the source-data, editor, preview, and publication architecture needed to customize that renderer
without losing its scaling properties.

The first implementation is intentionally narrower than the complete model. We need editable grass
regions and customizable cards first. Individual geometric blades and horizontal litter are planned
extensions, not requirements for the first brush tool.

Implementation status:

- implemented: project schema 9 contains versioned card visuals, presets, organizational layers,
  regions, and region-owned cell masks;
- implemented: transactional schema 7→8 and 8→9 migrations preserve existing source content; the
  first converts every old layer into a preset, layer, and “Existing coverage” region, while the
  second adds optional procedural recipes without changing built-in visuals;
- implemented: the cooker resolves the new source records back into the unchanged runtime species,
  clusters, pages, and GPU renderer; the demo's pre-migration page checksum is retained;
- implemented: the pure bounded cell compiler is shared by the full cooker and the editor-derived
  page job; the existing meadow still produces its pre-extraction runtime payload checksum.
- implemented: accepted locally changed cell pages replace matching cooked grass through bounded,
  disposable editor-owned assets; empty derived pages suppress cooked coverage without allocating a
  GPU page, and remote overrides retain CPU intent without remaining GPU-resident;
- implemented: the World tree selects existing layers/regions; Paint and Erase project onto loaded
  flat source terrain, interpolate continuous strokes across cell boundaries, and retain only the
  changed byte rectangle per cell as one chronological undo/redo command per drag;
- implemented: brush changes use the bounded revision-checked dense save path and the exact derived
  page presentation; missing painted cell rows are created sparsely and unsaved creation is removed
  again by undo;
- implemented: schema-4 crash recovery stores dirty reusable preset/visual definitions and region
  catalog snapshots alongside dense `base`/`current`/`runtime` snapshots; schema-1 object-only,
  schema-2 object/dense, and schema-3 region journals remain readable, and the editor exposes
  explicit conflict resolution;
- implemented: stable-ID region creation and empty-region deletion share global undo/redo, overlay
  project queries immediately, compile through the exact derived preview, and save through a
  bounded transaction whose database-wide coverage check prevents deletion from cascading masks
  outside the loaded editor window;
- implemented: explicit background publication runs the shared full-project cooker, validates a
  complete staging database, atomically replaces the runtime file, reopens the exact published
  generation in the streamer, and only then advances editor runtime baselines/removes stale page
  overrides.
- implemented: existing presets, card visuals, and region assignments have stable-ID editor
  working sets, global undo/redo commands, mixed bounded revision-checked transactions, save-all
  ordering, crash recovery, and exact local preview invalidation; a registered floating Ground
  Cover window edits the renderer's existing density, tint, size, flattened-card, and wind fields;
- implemented: preset and visual duplication/deletion use reversible tombstones and bounded atomic
  lifecycle writes. Project-wide foreign-key dependencies block deletion with a typed diagnostic;
  Save orders visual-before-preset upserts, then region changes, then preset-before-visual
  deletions;
- implemented: a validated procedural blade recipe materializes the frozen built-in-v1 meadow
  settings into editable variants. The editor exposes variant/blade count, normalized blade
  height/width, spacing jitter, seed, lean, C/S curves, tint gradient, and physical card size with
  a generated mask preview. Cooking stores complete R8 mip chains and the renderer assigns stable
  per-visual atlas layers without changing the cluster/culling/indirect-draw architecture. The
  cooker rejects projects exceeding the explicit 256-layer runtime atlas budget;

## Outcome

An author can:

- create reusable ground-cover presets;
- choose or edit what is visible inside each card;
- control physical card dimensions and stable variation;
- choose whether cards remain world-oriented or respond to the camera;
- control rest tilt and visible geometric curvature independently from wind;
- paint multiple named regions, each using its own preset;
- undo, redo, save, preview, and eventually publish those changes;
- use the same region and streaming architecture for future individual blades and surface-aligned
  cover such as fallen leaves.

The game continues to load immutable cooked runtime generations. It never reads the editor working
set or mutable project database.

## Preserve the current renderer

The current grass implementation already solves the expensive part of the problem well. The new
authoring model must preserve these properties unless profiling justifies a replacement:

- coverage masks are stored instead of individual plant records;
- coverage is cooked into bounded clusters and streamed through normal world pages;
- the CPU uploads clusters and visual recipes, not expanded clumps;
- the GPU performs cluster culling, deterministic placement, density reduction, LOD selection, and
  indirect draw-list construction;
- placement is stable in world space and does not reshuffle as the camera moves;
- authored placement extents remain separate from conservative animated visibility bounds;
- geometry LOD remains nested so a surviving card does not rotate or move at a tier transition;
- wind and actor interaction remain world-anchored and independent of page lifetime;
- resolved camera zoom remains the single input for the existing field-scale view transition.

Customization therefore changes the **recipe used to expand a cluster**, not the ownership or
streaming model.

## Terminology and source model

The source model has four author-facing concepts and one derived runtime concept:

```text
visual definition       preset                 layer                  region
how one clump looks  <- how it is populated <- organization only <- painted coverage
        |                    |                                            |
        +--------------------+----------------------+---------------------+
                                                    |
                                      compiled ground-cover page
                                      clusters + visual dependencies
```

### Visual definition

A visual definition describes how one generated clump is drawn and responds visually. It is
reusable and independent from density or painted location.

The initial implemented visual family is `CardCluster`. It owns:

- one or more coverage-mask variants used inside the cards;
- bottom and top tint colors;
- minimum and maximum physical height and width;
- card layout and view-response settings;
- rest tilt, rest curvature, and stable shape variation;
- maximum wind and interaction displacement used both for animation and conservative bounds.

The data model reserves a typed visual-family discriminator. Future families may add
`RibbonBladeCluster` or `VolumetricBladeCluster`, but each family has its own validated settings
record. We will not create one large record in which most fields are irrelevant to most renderers.

### Preset

A preset is the reusable choice shown in the grass tool. It references one visual definition and
owns population settings such as base density and distribution variation.

The preset is the normal unit authors duplicate and tune. The editor may display the visual fields
inside the preset inspector so this does not feel like two unrelated objects. The separate visual
identity is still useful because several density presets can share one card design and because the
runtime atlas depends on visuals, not painted regions.

Initially a preset owns only:

- stable ID, key, display name, enabled state, and source revision;
- visual-definition reference;
- density per square metre;
- deterministic seed and placement/jitter parameters that the current generator actually supports.

Terrain-surface, slope, height, moisture, and noise filters can be added later to presets without
changing regions or masks. They are not part of the first brush implementation.

### Layer

A layer is an editor organizational group inside one world space. It owns a stable ID, display
name, enabled state, ordering, and source revision. It contains regions but does not force all of
them to share one visual.

Layers can later acquire explicit composition or exclusion policies. The first version has simple
semantics: coverage from regions using the same preset is combined by maximum coverage, while
regions using different presets are independent contributions. The Inspector warns when several
dense presets overlap. There is no priority/blend-mode matrix in version one.

### Region

A region is one editable painted area. It owns:

- stable ID, layer ID, preset ID, display name, enabled state, and source revision;
- a density multiplier for intentional local adjustment;
- sparse per-cell 8-bit coverage masks, each with its own source revision.

Full visual settings are not copied into a region. To make a visually distinct area, an author
duplicates a preset and assigns the new preset. This keeps region commands, conflict handling, and
publication dependencies small and understandable.

The footprint remains raster coverage rather than an editable vector polygon. Empty cells have no
row. Candidate hashing uses the preset seed and absolute world-grid coordinate, never the region
ID. Painting, splitting, or merging regions that use the same preset therefore does not move
surviving candidates.

### Compiled page

A compiled ground-cover page contains renderer-sized clusters and references to the runtime visual
recipes needed by those clusters. It contains no authoring region objects and no individual blade
identities.

## Card visual design

### What is painted inside a card

Version one treats card artwork as an 8-bit **coverage mask**. White pixels are covered, black
pixels are empty, and intermediate values preserve filtered edges. Bottom/top tint and lighting are
applied by the shader. This directly generalizes the current generated R8 clump texture without
requiring an additional material family.

A visual may contain several variants. Stable hashing chooses a variant for every card so streaming
and camera movement do not change it. The asset cooker generates coverage-preserving mip levels;
authors do not paint or import mips manually.

The card editor supports the first procedural source mode:

- switching the frozen built-in-v1 source to an initially identical editable recipe;
- controlling variant/blade count, blade height and width ranges, base jitter, seed, lean, primary
  curve, and S-curve;
- previewing each generated mask with the card's bottom/top tint gradient;
- cooking deterministic R8 variants with coverage-preserving mip levels.

Later direct-mask modes will support:

- importing a grayscale or alpha image;
- painting and erasing coverage in a bounded canvas;
- adding, duplicating, deleting, and reordering variants;
- previewing the alpha cutoff and distant mip levels;
- previewing one clump and a representative dense patch.

Canonical editable coverage should be revisioned source data, not a runtime GPU texture. The
recommended initial storage is one bounded blob per visual variant in the project database. An
imported image is decoded into that representation. Four 256-by-256 R8 variants are only 256 KiB,
remain transaction-friendly, and avoid coordinating an external file replacement with a database
transaction. Runtime texture arrays are derived publication artifacts.

The current procedural four-variant atlas is a versioned built-in visual source. Existing meadow
content continues to reference that exact frozen recipe/output. Selecting Generated materializes
the same four-variant, 34-blade recipe before any value is changed. An author can duplicate the
visual, materialize its recipe, and tune it without importing an image. Painting over generated
pixels remains a later conversion to the direct R8 source mode.

Optional colored cutout artwork can be introduced later as another material mode. It is not needed
to unlock authored silhouettes and must not complicate the first shader/atlas path.

### Size and stable variation

Height and width remain physical world-space ranges. Stable per-clump hashes choose dimensions
inside those ranges. Card dimensions are distinct from coverage-cluster dimensions and from mask
resolution.

The editor exposes useful physical controls but keeps topology engine-owned:

- height range;
- width range;
- shape-variation strength;
- optional flattened-card mix for an upright clump.

Authors do not directly enter vertex counts or arbitrary per-LOD card lists in version one. The
renderer selects a validated nested layout profile for the chosen visual family. This preserves
stable LOD and prevents one preset from accidentally multiplying geometry cost without limit.

### Rotation and camera response

Three different rotations must remain explicit:

1. **root yaw** is deterministic world-space variation for the clump;
2. **layout rotation** places the nested cards around that root;
3. **view response** optionally adjusts card width orientation toward the camera.

The initial author control for view response is a continuous `view-facing width` amount:

- `0` keeps the authored/world-seeded orientation;
- `1` provides full axial camera-facing width;
- intermediate values retain world structure while reducing edge-on disappearance.

Only the width axis responds. The authored root-to-tip centerline remains stable. This is a better
base for small isolated grass patches than rotating the complete rest shape toward the camera.

The exact current meadow behavior is retained as a named built-in field-billboard profile, including
its high-camera parallel-facing treatment. Migration does not silently change the existing demo.
New presets default to a milder hybrid response suitable for both fields and local patches.

Camera-dependent forward lean is a separate setting from width facing. It defaults to zero for new
visuals. The current meadow preset retains its existing zoom response explicitly. A camera move must
never alter rest tilt unless this response is enabled by the visual.

### Tilt and curvature

Rest shape, wind, and actor interaction are separate contributions:

- **rest tilt** is world-stable and describes the unanimated card;
- **rest curvature** bends its root-to-tip centerline using the card's vertical segments;
- **wind** adds coherent animated displacement;
- **interaction** adds bounded actor displacement while pinning the root.

The useful initial rest controls are root angle, tip angle/curvature, curve concentration, direction
variation, and a limited upright mix. These carry forward the successful concepts from the legacy
grass model without restoring its entire large inspector at once.

Curvature amount is author data; subdivision count is an engine quality/LOD decision. Near and mid
profiles may use enough segments to show the curve while far LOD becomes one flat quad. The profiles
must remain nested and the surviving root, dimensions, orientation, texture variant, and rest-shape
seed must not change between tiers.

Conservative bounds are derived from the largest possible height, width, rest tilt/curvature reach,
wind displacement, and interaction displacement. Those bounds are never reused as placement
extents.

## Geometry families

### Card clusters — implement first

The current card renderer remains the default and should serve most grass fields, flowers, weeds,
and similar dense cover. Custom coverage variants, view response, and curved segmented cards give it
substantially more range without abandoning the existing GPU expansion path.

### Individual ribbon blades — reserved, not initially implemented

An individual-blade visual still uses coverage masks, deterministic GPU reconstruction, streamed
clusters, and indirect draws. It means that each generated primitive represents one ribbon blade
rather than one painted clump card; it does **not** create editor records, Bevy entities, or physics
bodies per blade.

This path needs a measured reason to exist because it increases geometry and may increase visible
instance pressure. Before implementation it must define:

- representative near/mid/far geometry and a card fallback, if any;
- maximum blades per clump and quality scaling;
- overdraw versus vertex-cost measurements at the gameplay camera range;
- stable transition behavior between geometry representations.

The typed visual-family boundary lets us add it later without redesigning regions, presets, masks,
history, or publication.

### Volumetric blades — experimental future family

Three-sided or otherwise volumetric blades can improve very small hero patches and multi-angle
silhouettes, but the legacy implementation showed their substantially higher geometry cost. They
remain a separate experimental family with explicit budgets, not a checkbox that silently makes
every card expensive.

## Horizontal and surface-aligned ground cover

Fallen leaves, petals, needles, and similar litter should reuse card coverage, presets, regions,
cluster streaming, stable placement, atlas packing, and density LOD.

They use a `SurfaceScatter` card layout rather than an upright crossed-card layout:

- the card plane is built in the terrain tangent frame;
- yaw rotates stably around the terrain normal;
- a small height offset prevents z-fighting;
- slope alignment and normal offset are explicit;
- view-facing and upright flattened-card behavior are disabled;
- wind and actor response default to zero and can later use surface-appropriate behavior.

This is a layout of the existing card family, not a separate editor domain named “leaves.” It does
require cooked terrain normals/heights at generated positions; the current flat cell-height cooker
must be corrected before surface alignment is production-ready.

## Project database direction

The eventual source schema should represent the conceptual records directly:

- `ground_cover_visuals` with identity, visual family, and source revision;
- one backend-specific settings table for card visuals;
- `ground_cover_card_variants` with bounded coverage blobs and revisions;
- `ground_cover_presets` with visual reference and population settings;
- `ground_cover_layers` reduced to world-space organization;
- `ground_cover_regions` with preset reference and local metadata;
- `ground_cover_region_cell_masks` keyed by region, space, and cell.

Backend-specific tables keep validation strict and prevent a nullable universal settings table.
Stable IDs, foreign keys, bounded payloads, and optimistic source revisions follow the existing
object and dense-record transaction model.

Migration from the current schema is deterministic:

- each `ground_cover_species` becomes one card visual using the exact built-in atlas and current
  appearance/response values;
- each current layer becomes one preset plus one organizational layer;
- all masks belonging to that layer become one generated “Existing coverage” region;
- layer density and seed move to the preset without changing candidate hashes;
- migration validation compares compiled cluster counts, bounds, seeds, and visual recipes.

## Editor workflow

The World floating window eventually shows:

```text
World
  Terrain
  Grass
    Layer: Meadow
      Region: Main field          [Meadow long grass]
      Region: Path edge           [Short mixed grass]
    Layer: Ground litter
      Region: Forest floor        [Dry leaves]
  Visible assets
```

Selecting a layer or region changes the right Inspector. Activating Grass exposes a small viewport
toolbar with Select, Paint, Erase, Fill, and Smooth. The initial implementation may ship Select,
Paint, and Erase only.

The implemented slice can select an existing region or explicitly create a new empty region using
the selected region's layer/preset (falling back to the first enabled choices). A region remains
selectable and editable after save/load. Empty-region deletion is intentionally conservative: the
database rechecks every mask row transactionally, including cells outside the bounded editor query.
The viewport currently shows the brush footprint; region outlines and optional mask-cell/coverage
overlays remain future inspection aids and must not make generated clumps selectable.

### Coverage precision and cluster budget

The current source mask lattice and runtime cluster lattice are the same: the demo's 32-metre cell
and 16-by-16 mask produce 2-metre clusters. Increasing a mask from 16-by-16 to 64-by-64 would not be
a free editor-quality setting; it could produce sixteen times as many runtime clusters and would
also conflict with overlapping regions of the same preset that use another resolution.

The first brush-quality layer therefore stays on the existing lattice. Each circular stamp is
deterministically area-sampled inside every touched mask sample. The brush stores 8-bit fractional
coverage across a smooth outer falloff, and the compiler turns that into fractional cluster density.
This changes a hard square boundary into a stable irregular/dithered population boundary without
changing the runtime format, lattice resolution, or maximum cluster budget. A soft erase may retain
low-density boundary clusters that a binary erase would remove, but it cannot multiply the fixed
lattice. Hardness controls how much of the brush radius remains full strength, and the viewport
draws both the full-strength and outer radii.

If authored shapes later need reliable detail below one cluster, authoring resolution must be
decoupled from runtime tiling. A higher-resolution source mask would then compile into the fixed
cluster grid plus a bounded sub-cluster coverage summary; GPU candidate reconstruction would test
that summary before emitting a clump. The exact summary (for example a small fixed sub-mask versus a
compact analytic edge) must be chosen from representative visual and GPU profiling. It must not add
one runtime cluster per high-resolution authoring texel.

The Inspector responsibilities are:

- **layer:** name, enabled state, ordering, and region list summary;
- **region:** name, enabled state, preset, density multiplier, and coverage summary;
- **preset:** population settings plus its referenced visual;
- **card visual:** variants, size, orientation/view response, rest shape, color, wind, and
  interaction response.

Preset, card, and selected-region editing now live in a registered floating Ground Cover tool
window. It uses the same typed workspace/window registry and does not add permanent panels to the
shell.

## Commands, history, journal, and save

All mutations use the editor command architecture:

- one completed world-space brush stroke is one compact `PatchGroundCoverRegion` command;
- one completed card-canvas stroke is one compact `PatchGroundCoverCard` command;
- commands retain only changed rectangles and before/after bytes, not complete world masks;
- create/delete/rename/reassign operations are stable-ID catalog commands;
- dirty and selected region records remain pinned outside the current query window;
- undo/redo operates across selection changes and save checkpoints;
- the schema-4 crash journal includes dirty object, preset/visual catalog, region-catalog, and dense
  records;
- save uses bounded revision-checked project transactions and preserves local intent on conflict.

Save participates in the editor-wide save coordinator: one Save action drains reusable catalog,
region-catalog, object, and dense-mask batches until every domain is clean. Catalog upserts precede
regions so a region may safely adopt a new preset; region changes precede catalog deletions so an
old preset can be released safely. Within catalog batches, visuals precede preset upserts and preset
deletions precede visual deletions. Save & Publish uses the same drain before requesting a runtime
cook. The transactions remain bounded and typed internally; their batching is not exposed as
repeated user actions.

The dense working-set and revision-checked writer are keyed by region masks. Paint/Erase gestures
now use that seam directly and share the global command order with object edits, without retaining
compatibility with the removed layer-owned mask model. Dirty dense checkpoints and runtime
baselines are now restored from the atomic sidecar journal before normal reconciliation can evict
them. Conflict resolution clears stale history and refuses unsafe automatic shape rebasing.

## Exact editor preview

There must be one pure cell compiler used by both editor preview and publication:

```text
compile_ground_cover_cell(
    terrain input,
    overlapping region masks,
    region metadata,
    presets,
    visual definitions,
    compiler version,
) -> ground-cover page + visual dependencies + diagnostics
```

The editor schedules this compiler through the bounded derived-work coordinator. A dirty accepted
result temporarily replaces the cooked ground-cover presentation for the affected cell; unchanged
domains continue to come from the immutable runtime generation. Late results are rejected by input
revision.

The compiler and exact editor artifact are implemented. The artifact contains the runtime
`GroundCoverPage`, resolved species recipes, conservative maximum height, and compile diagnostics.
It rejects missing or incompatible dependencies and refuses a truncated source window rather than
publishing a partial result. Accepted artifacts for source/runtime-divergent cells are presented as
editor-owned ground-cover assets and suppress only the matching cooked page. The renderer follows
active page components rather than every asset in storage, so replacement does not mutate the
immutable runtime asset. Presentation is capped to the runtime index's 7×7 window; saved remote
changes remain pinned as CPU source/artifact state and are recreated when the camera returns.

The dense working set retains a separate runtime baseline. Undoing an unsaved edit back to that
baseline removes the override, while saving advances the project checkpoint without falsely making
the old cooked generation current. Successful generation adoption now advances `runtime` to the
source `base` used by the cooker. A command made while the background cook is running remains
divergent because its `current` value is newer than that base.

An artifact key includes relevant mask, region, preset, visual, card-variant, terrain, and compiler
revisions. A visual size/rest-shape change invalidates every bounded cell using it because bounds may
change. A later optimization may split structural and shading revisions, but version one should
prefer correctness over a more complicated invalidation graph.

Preview controls should make the risky cases easy to inspect:

- fixed low gameplay camera and high/overhead camera;
- orbit around a small isolated patch;
- wind paused and interaction disabled to reveal rest shape;
- LOD colors and individual near/mid/far isolation;
- coverage mip preview and overdraw/visible-instance diagnostics.

## Runtime publication

Publication remains an explicit boundary:

```text
editor working set
  -> save revision-checked project source
  -> validate source and dependencies
  -> cook bounded cells and visual atlases into a staging generation
  -> validate the immutable staging database
  -> atomically publish the generation
  -> editor adopts it when safe
  -> game loads it on its normal lifecycle
```

The first publication implementation deliberately invokes the complete shared cooker rather than
assembling the editor's bounded preview artifacts. This gives every generation one validated,
content-addressed snapshot across all cells and domains. It runs on a dedicated worker, so project
size affects publication time but not viewport residency or frame work. Publication is enabled only
when all editor source commands are saved and conflict-free. The staging file is validated by an
immutable reader before atomic replacement of the live path.

After replacement, the runtime streamer receives an exact expected-generation reload request. It
discards resident pages, reopens the path, verifies the generation ID, and only then resumes page
demand. Failure leaves the shell and logical camera operational and does not advance editor runtime
baselines. A successfully adopted generation retires obsolete failure diagnostics and clean
source-backed overrides while retaining accepted artifacts that may represent newer unsaved
commands; those commands remain overlays on the newly adopted checkpoint.

Custom card variants are packed into one or more runtime R8 texture arrays with
coverage-preserving mips. A runtime visual recipe stores its array set, base layer, variant count,
dimensions, response settings, and conservative reach. Stable instance hashes select only within
that assigned range.

The existing common atlas can remain a single bind group while the project fits one array. The
runtime representation must still include an atlas-set identity so the cooker can partition a
larger catalog later. Pages/draw work are grouped by compatible visual family and atlas set; an
unused future geometry family incurs no draw or dispatch cost.

Publication is never described as “saving into game state.” Mutable gameplay state and save files
remain overlays on immutable content. Future cut, burned, trampled, or seasonally suppressed cover
belongs in sparse gameplay overlays and does not mutate project or runtime databases.

## Delivery phases

### Phase 1 — source model and shared compiler

- implemented: add versioned visual, preset, layer, region, and region-mask source records;
- implemented: migrate the existing meadow without changing its deterministic page output;
- implemented: extract a bounded pure per-cell compiler from the current whole-project cooker;
- implemented: make editor derived-page generation and runtime publication consume the same
  compiler output;
- implemented: add dependency/revision validation, focused migration and database round-trip
  tests, compiler overlap/disabled/error tests, and editor/cooker integration regressions.

### Phase 2 — region editing vertical slice

- implemented: add the Grass layer/region tree and Inspector sections;
- implemented: select existing regions and Paint/Erase their masks with a continuous bounded brush;
- implemented: area-sample circular brush stamps and expose hardness/falloff so curved boundaries
  retain fractional density without refining the runtime cluster lattice;
- implemented: compact stroke and stable-ID region create/delete commands use global undo/redo,
  revision-checked bounded save, schema-4 crash recovery, and explicit
  compatible-rebase/database conflict choices;
- implemented: show accepted dirty cell previews through the current renderer;
- implemented: publish and live-adopt a validated immutable runtime generation; the game continues
  to read only that database through its normal startup lifecycle.

Phase 2 may use only the current built-in meadow visual. Region editing should not wait for the card
designer.

### Phase 3 — customizable card visuals

- implemented: update existing presets and card visuals through one floating Ground Cover window;
  region name/enabled/preset/density reassignment uses the same command stream;
- implemented: global undo/redo, bounded atomic mixed preset/visual writes, optimistic conflict
  handling, schema-4 recovery, save-all ordering, and exact bounded preview invalidation;
- implemented: expose the renderer's existing height/width, tint, flattened probability, wind,
  preset density/seed, and visual-reference fields without introducing a parallel editor model;
- implemented: duplicate and delete presets/visuals with reversible tombstones, explicit
  project-wide dependency rejection, and dependency-safe save ordering;
- implemented: editable procedural R8 card variants, mask preview, coverage-preserving cooker mip
  generation, bounded runtime atlas allocation, and stable per-species texture-layer selection;
- add direct R8 canvas painting/import, generated-to-painted conversion, and variant
  add/duplicate/delete/reorder tools;
- add world/view-facing response, camera lean, rest tilt, and rest curvature source/runtime fields;
- implemented: carry visual selection through GPU species data and stable texture-layer selection;
- derive correct conservative bounds from every authored displacement;
- add isolated-patch, camera-rotation, mip, nested-LOD, and dense-field performance tests.

### Phase 4 — surface scatter

- compile per-placement terrain height/normal rather than one flat cell height;
- implement the surface-tangent card layout, height offset, and slope response;
- add a fallen-leaves sample preset and region;
- validate z-fighting, slopes, streaming edges, and overhead density behavior.

### Phase 5 — measured blade experiment

- prototype individual ribbon blades behind a separate visual family;
- compare image quality, vertex cost, overdraw, culling pressure, and memory against improved cards;
- keep it only if representative small hero patches show a meaningful benefit;
- evaluate volumetric blades separately rather than bundling them into the ribbon result.

## Acceptance criteria

- Existing meadow content migrates deterministically and preserves the current optimized render
  path and appearance profile.
- Two disconnected named regions can use different presets in one layer.
- A saved region can be selected, extended, erased, undone, redone, saved, and published.
- Painting or splitting a region does not move surviving deterministic clumps.
- A world-fixed small patch does not rotate or change rest tilt when the camera orbits.
- The built-in field preset retains the current camera-facing/overhead behavior.
- Curved near cards become simpler at distance without surviving cards moving at LOD transitions.
- Custom card masks retain acceptable silhouette coverage through generated mip levels.
- Empty or disabled regions create no runtime clusters or renderer work.
- Dirty preview and the published runtime page are produced by the same cell compiler.
- Project size affects database/query/cook time, not the editor's steady-state working-set size.
- A representative dense project remains within explicit visible-instance, atlas, decoded-memory,
  GPU-memory, and frame-time budgets.

## Deliberate non-goals for the first implementation

- no individual authored blade identities;
- no arbitrary renderer graph or user-authored shader;
- no unlimited per-preset card/segment counts;
- no region polygons, boolean geometry, or complex blend-mode stack;
- no per-region copy of every visual setting;
- no grass collision or physics bodies;
- no shadows until representative rendering tests justify their quality and cost;
- no destructive/trampled gameplay persistence in the authoring database;
- no individual or volumetric blade backend before measurement.

This boundary gives us a small first product—presets plus painted regions using the current
renderer—while making custom cards, stable orientation, curvature, horizontal cover, and measured
blade rendering additive features rather than future rewrites.
