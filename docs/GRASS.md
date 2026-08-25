# Grass and Ground Cover

This document records the current implementation and the reasoning behind it. It is intentionally
limited to decisions that exist in the code today; it is not a roadmap for a complete vegetation
system.

## Scope

Grass is decorative ground cover, not a collection of gameplay objects. Individual blades do not
have database identities, Bevy entities, inventories, or interaction state. A future harvestable
plant or destructible bush should be a gameplay object layered over ground cover rather than a
special blade.

The implementation lives in the dedicated `ground_cover` crate so that the same storage and render
path can later support other dense decorative fields such as small flowers or low plants.

## Data flow

The system has four stages:

1. The project SQLite database stores species, layers, and per-cell coverage masks.
2. The world cooker converts occupied mask samples into compact ground-cover clusters.
3. The existing page streamer loads ground-cover pages with the terrain cells around the camera.
4. A GPU compute pass culls clusters and expands visible coverage into indirect draw lists.

### Project database

`ground_cover_species` contains appearance and motion limits shared by many fields:

- bottom and top colors;
- minimum and maximum card height;
- minimum and maximum card width;
- probability of the optional nearly-flat card;
- maximum wind displacement.

`ground_cover_layers` associates a species with a world space, density, and deterministic seed.
`ground_cover_cell_masks` stores an authoring-resolution coverage byte for every mask sample in a
cell. Coverage scales layer density from zero through full density.

This separation is deliberate: changing a species does not duplicate data across every cell, and
the editor can paint coverage without creating rows for individual clumps.

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

Runtime pages contain clusters and dependencies on the species they use. The streamer treats ground
cover as its own page domain; unloading a cell also releases its ground-cover asset and GPU buffers.

## GPU rendering

The CPU uploads clusters and species, not expanded grass instances. For every resident page, a
compute dispatch processes clusters in 64-thread workgroups:

1. Reject the cluster against the camera frustum using its conservative bounds.
2. Estimate its screen-space size.
3. Reduce density for small projected coverage and reject subpixel coverage.
4. Reconstruct stable clump positions from the cluster seed using a jittered grid.
5. Classify surviving clumps into near, mid, or far buffers.
6. Draw the three buffers with indirect draws.

Each LOD buffer currently has a capacity of 131,072 visible instances. Stable hashing is important:
camera motion may change LOD or retained density, but it must not reshuffle every clump in a cluster.

### Procedural cards

One instance represents a dense clump card rather than one blade. The renderer generates a
256-by-256, four-layer R8 texture array in code. Each layer contains 34 varied blade silhouettes and
coverage-preserving mip levels. This is a placeholder authoring path that gives dense coverage
without requiring a painted asset while the renderer is being established.

The cards are opaque with alpha testing, write depth, and use no face culling. They currently use a
simple color gradient rather than the standard PBR material. Grass does not currently cast shadows.
Both choices are intentional until lighting and shadow quality can be tested in a representative
scene.

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

Current density retention is:

- below 2 px: rejected;
- 2–3 px: up to 12%;
- 3–6 px: 12–45%;
- 6–18 px: 45–100%;
- above 18 px: full density.

Current geometry transitions are:

- near: 48–72 px in third-person, gradually changing to 36–54 px overhead;
- far: 10–20 px;
- between those ranges: mid.

A stable per-clump selector spatially dithers the transition instead of producing one exact circular
distance ring.

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
- Do not add grass shadows or per-blade interaction without testing them in a representative scene.

## Current limitations

- The cooked clusters currently assume each source cell has one flat height.
- Coverage masks are populated by demo generation; there is no editor painting tool yet.
- The clump atlas is procedural and not artist-authored.
- Grass has no terrain lighting integration, shadow casting, or collision.
- Interaction response is currently a fixed-capacity actor field; it has not been tested with a
  crowded scene or authored per-species response.
- Visible-instance capacity is fixed rather than quality-scaled.
- The current constants are initial tuning for the demo meadow, not permanent engine defaults.

Relevant implementation files are `crates/ground_cover/src/lib.rs`,
`crates/ground_cover/src/renderer.rs`, `assets/shaders/ground_cover_cull.wgsl`,
`assets/shaders/ground_cover.wgsl`, and the ground-cover sections of the world, database, cooker, and
streaming crates.
