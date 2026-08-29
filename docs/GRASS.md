# Grass and Ground Cover

This document records the current implementation and the reasoning behind it. It is intentionally
limited to decisions that exist in the code today; it is not a roadmap for a complete vegetation
system.

The planned authoring, customization, and runtime-publication model is specified separately in
[`GROUND_COVER_AUTHORING_PLAN.md`](GROUND_COVER_AUTHORING_PLAN.md).

## Scope

Grass is decorative ground cover, not a collection of gameplay objects. Individual blades do not
have database identities, Bevy entities, inventories, or interaction state. A future harvestable
plant or destructible bush should be a gameplay object layered over ground cover rather than a
special blade.

The implementation lives in the dedicated `ground_cover` crate so that the same storage and render
path can later support other dense decorative fields such as small flowers or low plants.

## Data flow

The system has four stages:

1. The project SQLite database stores visuals, presets, layers, regions, and per-cell coverage
   masks.
2. The shared bounded cell compiler resolves those records into compact ground-cover clusters; both
   the world cooker and editor-derived page jobs call this exact function.
3. The existing page streamer loads ground-cover pages with the terrain cells around the camera.
4. A GPU compute pass culls clusters and expands visible coverage into indirect draw lists.

### Project database

`ground_cover_visuals` and the family-specific `ground_cover_card_visuals` record the reusable
appearance and motion limits shared by many painted areas. The currently implemented visual family
is the existing card-cluster renderer, with:

- bottom and top colors;
- minimum and maximum card height;
- minimum and maximum card width;
- probability of the optional nearly-flat card;
- maximum wind displacement.

Card visuals may retain the frozen built-in-v1 artwork or own a procedural blade recipe. A recipe
controls variant and blade counts, normalized blade height/width, spacing jitter, seed, lean, and
C/S silhouette curves. Bottom/top tint remains separate from R8 coverage. Cooking expands the
recipe into complete 256×256 coverage-preserving mip chains; the runtime atlas assigns each visual
a bounded layer range, and stable per-instance hashing selects one of its variants.

`ground_cover_presets` associates a visual with density and a deterministic seed.
`ground_cover_layers` are world-space organizational groups. `ground_cover_regions` are named,
enabled painted areas that reference a preset and may apply a density multiplier.
`ground_cover_region_cell_masks` stores an authoring-resolution coverage byte for every mask sample
in a cell. Coverage scales preset density from zero through full density. Regions using the same
preset share its absolute candidate grid and combine by maximum effective coverage.

This separation is deliberate: changing a visual or preset does not duplicate data across every
cell, splitting a region does not reshuffle surviving candidates, and the editor can paint coverage
without creating rows for individual clumps.

### Cooked runtime pages

Every non-empty coverage sample becomes one `GroundCoverCluster`. A cluster stores:

- its species ID;
- cell-local center;
- authored coverage extents;
- conservative visibility extents;
- density per square metre;
- a stable seed.

Coverage extents define where instances may be placed. Visibility extents are larger because they
also include maximum card and wind reach. These concepts must remain separate: using conservative
bounds for placement creates overlaps, while using authored bounds for culling makes moving cards
disappear near the frustum edge.

The demo uses 32-metre cells, a 16-by-16 mask, and therefore 2-metre clusters. Its current meadow
species uses 0.55–0.78 m height, 0.7–1.4 m width, 20% flattened-card probability, 0.22 m maximum wind
displacement, and five placements per square metre.

The editor integrates a circular brush over each mask sample and stores fractional coverage at the
boundary. Brush hardness controls the full-strength inner radius and a smooth outer falloff. This
softens curved Paint/Erase edges through stable density variation without changing mask resolution,
the maximum cluster budget, cooked data format, or the runtime renderer. A soft boundary can retain
a few low-density clusters that a binary erase would remove, but it never refines or multiplies the
fixed cluster lattice. It cannot encode an exact silhouette smaller than a 2-metre cluster; that is
a separate representation concern rather than a reason to multiply the current cluster grid
blindly.

Runtime pages contain clusters and dependencies on the species they use. The streamer treats ground
cover as its own page domain; unloading a cell also releases its ground-cover asset and GPU buffers.

## GPU rendering

The CPU uploads clusters and species, not expanded grass instances. For every resident page, a
compute dispatch processes clusters in 64-thread workgroups:

1. Run a lightweight candidate pass that counts exact visible near and middle ribbon demand.
2. Reject the cluster against the camera frustum using its conservative bounds.
3. Estimate conservative cluster screen coverage, then correct projected size at each generated
   root so a whole authored rectangle never changes visibility or LOD together.
4. Reduce density for small projected coverage using a rotated low-discrepancy rank and reject only
   deeply subpixel coverage.
5. Reconstruct stable generation tiles from the cluster seed using a jittered grid.
6. Classify surviving tiles into near, mid, or far buffers and expand ribbon tiers into blades.
   When demand exceeds a tier's fixed buffer, reduce every carrier to the same nested
   low-discrepancy subset rather than dropping later streamed pages.
7. Draw the three buffers with indirect draws.

Each LOD buffer currently has a capacity of 131,072 visible instances. Stable hashing and ranked
retention are important: camera motion may change LOD or retained density, but it must not reshuffle
every clump or randomly erase an entire authored coverage sample.

### Procedural cards

One instance represents a dense clump card rather than one blade. The renderer generates a
256-by-256, four-layer R8 texture array in code. Each layer contains 34 varied blade silhouettes and
coverage-preserving mip levels. This is a placeholder authoring path that gives dense coverage
without requiring a painted asset while the renderer is being established.

### Independent ribbon blades

The experimental near and middle tiers do not subdivide a rendered card into a hidden tuft. A
card-density sample is used only as a deterministic generation tile, and low-discrepancy roots fill
that tile independently. Each resulting visible GPU instance represents exactly one blade.

A continuous world-space clump field supplies a coherent facing, color variation, and response to
nearby roots without changing their positions or exposing generation-tile boundaries. It blends
unit directions instead of numeric angles, avoiding artificial full-circle turns at the angle seam. The high
ribbon is a native 15-vertex triangle strip
with seven cross-sections plus a tip; the low ribbon is a 7-vertex strip with three cross-sections
plus a tip. Both evaluate the same cubic Bézier definition, derive normals from its tangent, and
round those normals across the width. The initial material keeps a stable color response rather than
animating those normals through an unvalidated leaf-lighting approximation. Far coverage still uses
the optimized procedural cards. `B` switches between the ribbon experiment and the all-card path
for direct image-quality and performance comparison. Within the normal budget, near and middle
geometry retain the same root set and differ only in strip resolution. Under exceptional load, a
measured global tier budget keeps a smaller nested root subset in every carrier and compensates its
width modestly. This preserves field-wide coverage without unbounded memory or draw cost.

The cards are opaque with alpha testing, write depth, and use no face culling. They currently use a
simple color gradient rather than the standard PBR material. Grass does not cast shadows, but the
current experiment lets it receive the normal directional cascaded shadow map through the optimized
path described below.

### Experimental alpha depth prepass

Directly sampling the cascaded shadow map in the original single-pass grass fragment shader was
prohibitively expensive. Alpha-tested rectangles overlap heavily, so many fragments performed a
filtered shadow lookup before only the nearest surviving blade became visible. Caster count was not
the bottleneck; receiver overdraw was.

Ground-cover views now enable Bevy's depth prepass. Grass joins its alpha-mask phase using the exact
same generated vertices, wind, interaction, LOD dither, artwork sample, and cutoff as the color pass.
The prepass performs only that cutout test and writes the nearest depth. The color pipeline disables
depth writes and requires exact depth equality, so hidden overlapping cards are rejected before the
real directional-shadow lookup. Grass remains absent from all shadow-caster passes.

The color pass uses Bevy's inexpensive hardware 2-by-2 comparison filter. Cards retain an upright
receiver normal while ribbons use their derivative-based rounded normal. Consequently characters
and trees retain their real silhouettes, and moving the authoritative directional light updates
their grass shadows normally. Press `U` in the demo to toggle accelerated sun motion and stress both
cascade stability and receiver performance.

This is still an experiment: it must be measured in the representative dense field at native display
resolution. Tree wind may also require a simplified shadow-caster LOD later, but that concern is
independent from receiving shadows efficiently on grass.

### Nested geometry LOD

The tiers are nested so changing LOD removes detail without moving the surviving card:

| Tier | Cards | Vertical segments per card |
| --- | ---: | ---: |
| Near | 3 | 2 |
| Mid | 2 | 2 |
| Far | 1 | 1 |

Ribbon zero is shared by every tier, ribbon one by near and mid, and ribbon two only by near. Shared
ribbons keep the same width, height, tilt, texture choice, and orientation. This invariant was added
after earlier tier-specific geometry visibly rotated and shifted grass as the camera moved.

The shared base card faces the camera. At high zoom, point-facing orientation blends mostly toward a
distant virtual camera so the overhead field does not form obvious concentric circles. Cards lean in
the camera-forward direction; normalized camera zoom changes only the amount of lean, not its
direction. A subset of the third near ribbon may be almost flat to break up the vertical-card pattern.

## Density and geometry LOD

Visibility/density and geometry detail use different projected sizes:

- Visibility and density use the maximum possible extent so wide or flattened cards do not vanish
  early.
- Geometry LOD uses representative average dimensions. It emphasizes projected height in
  third-person and projected width overhead.

Current density retention is evaluated per generated root rather than once at the cluster centre:

- below 0.75 px: rejected;
- 0.75–3 px: 10–16%;
- 3–6 px: 16–45%;
- 6–18 px: 45–100%;
- above 18 px: full density.

Current geometry transitions are:

- near: 48–72 px in third-person, gradually changing to 36–54 px overhead;
- far: 10–20 px;
- between those ranges: mid.

A stable per-root selector spatially dithers the transition instead of producing one exact circular
distance ring. Retention uses a low-discrepancy rank, so sparse far coverage remains distributed and
cannot disappear in random rectangular blocks.

The playable camera currently stops at 17.6 metres, normalized zoom 0.68 on the original 24-metre
camera curve. Ground cover keeps its normal near, mid, and far classification throughout that range;
there is no top-down override that forces every retained clump into one geometry tier.

Normalized camera distance is the only input for the third-person-to-overhead visual transition.
Grass must not independently infer the mode from camera pitch or maintain another transition timer;
those earlier approaches caused field-wide shape changes at different points in the zoom path.

## Wind

`GroundCoverWind` describes one coherent world-space wind field: direction, base strength, gust
strength, spatial scale, speed, and elapsed time. Species limit their own maximum displacement.

Wind phases are derived from world position and stable instance values, so adjacent pages participate
in the same moving field and streaming a page out and back in does not reset its motion.

Ribbon wind preserves the authored rest direction and Bézier arc as its baseline. It applies a
bounded three-dimensional tip displacement dominated by the world wind direction, with smaller
cross-wind and vertical components. This makes a ribbon visibly sweep forward/back, left/right,
and up/down without freely rotating it or replacing its resting curvature. World-space group waves
keep nearby blades coherent while each blade receives a stable phase and response variation, so a
clump does not move as one rigid sheet.

The richer field is evaluated once per visible blade during GPU expansion and packed into the
existing visible-instance record. Every 7/15-vertex strip then reuses that displacement rather than
repeating trigonometric work per vertex. The current demonstration profile uses a 4.2-radian phase
speed with stronger base drive and gusts so the constrained motion is clearly visible.

## Actor interaction

Presented actors opt into decorative interaction with `GroundCoverInteractor`. The current
character profile defines a 0.72-metre radius, 0.48-metre maximum displacement, and 1.2-second
recovery. These controls belong to the visual interaction source; they do not create physics bodies
or identities for grass clumps.

The main world records movement as short world-space capsules. One capsule remains live beneath each
actor, while recently released capsules carry normalized recovery age. Sampling uses fixed time
intervals rather than one point per rendered frame, and each capsule spans the distance travelled in
that interval. This prevents a fast actor from stepping over gaps in the field. Movement longer than
five metres in one frame is treated as a teleport and resets contact instead of bending a line across
an area transition.

At most 16 stamps are uploaded. Active actors are selected first by explicit priority, then the
newest released capsules fill the remaining slots. The GPU evaluates these stamps once for each
reconstructed clump during the existing culling/expansion pass and stores one world-space tip
displacement with the visible instance. The vertex shader applies that displacement quadratically
over card height, pinning the root. Every card and every geometry LOD of the clump therefore shares
the same response.

This bounded capsule field is deliberately an initial backend, not a promise that 16 stamps will
serve a crowded final scene. It is cheap for the current player-focused test and independent of
streamed page lifetime. If representative gameplay proves that many simultaneous actors need long
trails, the public interactor concept can feed a low-resolution world-space deformation texture
instead. That decision should follow a measured scene; recreating per-blade or per-page history is
not an acceptable scaling path.

## Debugging

Press `G` to cycle:

1. `normal`;
2. `LOD colors` — near red, mid yellow, far blue;
3. `far only`;
4. `far disabled`.

At maximum zoom, LOD colors should still show the normal near, mid, and far classification. Use this
view to check whether moving transition boundaries remain acceptable under the reduced camera range.

`far only` and `far disabled` do not disable the common compute dispatch. They are useful for visual
isolation, but small timing differences in the macOS Metal HUD are not reliable measurements of the
total grass cost. macOS may change GPU clocks between otherwise similar frames.

## Important invariants

Future changes should preserve these unless measurements justify replacing them:

- Store coverage, not individual blades.
- Stream clusters through the normal world-page system.
- Expand and cull instances on the GPU.
- Keep placement deterministic in world space.
- Keep interaction world-anchored and independent of clump/page lifetime.
- Keep authored coverage extents separate from conservative culling bounds.
- Keep LOD geometry nested so surviving cards never move when tiers change.
- Use resolved camera zoom as the single view-transition input.
- Keep expensive grass shading behind the shared alpha-tested depth prepass. The depth and color
  paths must generate identical geometry and apply identical cutout/dither decisions.
- Keep grass out of shadow-caster passes unless representative measurements justify changing that.
- Do not add grass shadow casting or per-blade interaction without testing them in a representative
  scene.

## Current limitations

- The cooked clusters currently assume each source cell has one flat height.
- The bounded editor can paint region-owned masks and edit existing presets, card visuals, and
  region assignments through stable-ID working sets. Presets and visuals can be duplicated or
  dependency-safely deleted. Procedural card artwork can be created entirely in the editor and is
  visible through a per-variant mask preview and the normal derived world preview; direct canvas
  painting and image import are not implemented yet.
- Exact runtime-format pages produced by editor-derived jobs replace matching cooked pages through
  bounded editor-owned assets while source and runtime revisions differ. Empty results suppress the
  cooked page without allocating an empty GPU asset.
- Explicit editor publication runs the shared full cooker into a validated staging database,
  atomically replaces the immutable runtime file, and asks the streamer to reopen the exact
  content-addressed generation before clean derived overrides are retired.
- The current artwork sources are procedural. Direct painted/imported masks are still planned.
- Grass has no terrain lighting integration, shadow casting, or collision. Directional-shadow
  reception through the alpha/depth prepass remains experimental.
- Interaction response is currently a fixed-capacity actor field; it has not been tested with a
  crowded scene or authored per-species response.
- Visible-instance capacity is fixed rather than quality-scaled.
- The current constants are initial tuning for the demo meadow, not permanent engine defaults.

Relevant implementation files are `crates/ground_cover/src/lib.rs`,
`crates/ground_cover/src/renderer.rs`, `assets/shaders/ground_cover_cull.wgsl`,
`assets/shaders/ground_cover.wgsl`, `crates/ground_cover_compile/src/lib.rs`, and the ground-cover
sections of the world, database, cooker, editor, and streaming crates.
