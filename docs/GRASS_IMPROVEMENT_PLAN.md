# Grass System Improvement Plan

## Status and purpose

This is the living investigation, decision log, and work-in-progress plan for Yarra's grass and
general ground-cover renderer. It describes what is promising, what is currently weak, what we need
to measure, and which ideas should be tested rather than assumed.

This document is not a claim that every idea below should be implemented, and it does not replace:

- [`GROUND_COVER_ARCHITECTURE.md`](GROUND_COVER_ARCHITECTURE.md), the accepted V2 architecture and
  current implementation direction;
- [`GHOST_OF_TSUSHIMA_GRASS_TALK_NOTES.md`](GHOST_OF_TSUSHIMA_GRASS_TALK_NOTES.md), the evolving
  slide/video companion whose observations inform some experiments below.

The fixed-buffer measurements below are a 2026-08-30 historical snapshot of the deleted renderer,
not current V2 performance data.

## Performance priority - 2026-09-08

The user's central requirement is low added cost, with iPhone as a longer-term target.
The minimal current scene is already too expensive and heats the Mac despite a field that appears
less dense than the Yotei reference. Shadow work must fit an affordable renderer; visual completion
alone does not justify shipping more GPU work. **iPhone testing is explicitly deferred:** the current
grass renderer is already unacceptably slow there, and another phone test would not help this task.
Broader grass optimization is later work, not a prerequisite for the shadow prototype. The immediate
goal is useful shadow approximation with small measured overhead on the current Mac renderer, without
multiplying its already excessive workload. Sustained 60 FPS on iPhone 15 Pro Max / A17 Pro remains
a longer-term goal after broader optimization, with room for the rest of the game.
Mac validation retains display VSync; reaching that limit is not evidence of low power consumption.

Read [PERFORMANCE_HANDOFF.md](PERFORMANCE_HANDOFF.md), then the newer
[ground measurements](GROUND_GPU_BREAKDOWN.md), [prepared-ground updates](PREPARED_GROUND.md), and
[MSAA storage results](MSAA_COLOR_STORAGE.md). Historical iPhone captures attributed about 68% of
one low-camera frame to grass. Curve preparation reduced arithmetic but increased memory traffic
without establishing sustained thermal improvement. Later ground/storage changes mean those timings
are not a current-build baseline. Do not repeat rejected cache experiments or claim that either
grass or the thermal problem is solved.

### Restart and implementation constraints — 2026-09-09

Both shadow implementations failed visual review and were removed. The user explicitly requested a
restart. [GRASS_SHADOW_RESTART.md](GRASS_SHADOW_RESTART.md) supersedes the earlier prescribed canopy
band/grid/dither architecture and the assumption that broad darkening alone is a useful endpoint.
No replacement representation is selected. [The retired report](GRASS_SHADOW_PROXY.md) records the
failed work without treating its timings or tests as visual acceptance.

- Establish the required blade detail and ground occlusion separately on a small patch of actual
  grass, including sparse boundaries. Inspect gameplay overhead and vertical top-down first, then
  near, grazing, orbit, zoom and wind. Independent stamps and blurred field noise are rejected.
- Preserve ordinary object-shadow reception and existing authored root color/AO during the reset.
  Diagnose along-blade AO versus actual height before choosing an additional occlusion model.
- Prefer work within existing draws when it produces the required signal, but count per-fragment
  cost over real grass overdraw. A material-only effect does not fulfil ground-shadow casting.
- Do not add a full-field blade caster, larger maps, more cascades, more receiver samples, higher
  grass density, a depth prepass, temporal resources or screen-space tracing to rescue quality.
- A bounded offline patch reference is permitted to answer what geometric/visibility information
  a useful approximation must retain. It is not a production full-field shadow path.
- Validate appearance before integrating another cache, field-wide caster or controls. Then measure
  a short matched Mac comparison; extend testing only for a promising candidate.
- Keep the existing AA quality, render settings, source lifetime and MSAA storage policy. iPhone
  tests and broader grass optimization remain deferred.

### Cost and acceptance gates

Evaluate incremental cost on Mac for this task. The previously proposed A17 timing gate is withdrawn;
there is no phone benchmark or full-renderer optimization prerequisite. Declare justified resource and
work bounds for the chosen prototype before implementation, including any page/cascade draws,
maximum update work, buffers, metadata, scratch resources and peak memory. Use the Mac baseline to
set a small overhead allowance and report both absolute and relative cost. Arbitrary unmeasured
millisecond limits are not evidence that an architecture is affordable.

Compare enabled/disabled using the same build, population, camera path, wind, actual render
dimensions, AA, receiver settings, and content. Inspect total affected GPU work: proxy rasterization,
shadow-map writes, main-pass reception, and any changed attachment load/store behavior. A coarse mesh
can still fill large portions of every cascade; low triangle count does not establish low cost.
Measure CPU submission and streaming spikes as well. Preserve the current MSAA storage saving.

Use repeated matched Mac measurements and settled live comparisons, including moving/streaming
workloads. Record
frame-time distribution, missed deadlines, thermal state and, when available, energy/power data;
coarse thermal labels and profiler replay timings alone are insufficient. A noisy or unavailable
measurement leaves the cost conclusion open. Reject configurations that materially increase total
frame cost, bandwidth, or streaming spikes; a several-fold slowdown is categorically unacceptable.

Keep the prototype switchable for the comparison. Accept the shadow increment on useful visuals and
small measured overhead relative to the current Mac baseline; do not require the already-poor base
renderer to meet its future iPhone target first. If the increment is expensive, simplify or reject
the shadow approach rather than expanding this task into a renderer overhaul. Mac results establish
only that comparison, not future iPhone performance. Revisit phone validation during the later broader
optimization effort. Passing the proxy visual test must not automatically schedule screen-space detail.

## Implementation history

Direction update, 2026-08-31: the phases and experiments below remain useful workstreams, but all
implementation follows the V2 contracts in [`GROUND_COVER_ARCHITECTURE.md`](GROUND_COVER_ARCHITECTURE.md).
The old cluster/ribbon runtime, compiler, authoring UI, and schemas have been removed.

Scheduler/LOD checkpoint, 2026-08-31: V2 now compacts view-visible resident field work into a GPU
queue, writes indirect candidate-dispatch dimensions, and emits accepted candidates into four
bounded bins: single-high, single-low, split-high, and split-low. The split family temporarily
approximates broad leaves with two leaves. Species parameters drive high/low sections, authored
projected-size thresholds, stable nested low-LOD density, longitudinal redistribution, height/width distributions,
complete cubic silhouette variants, lateral curve/camber, pair spread, taper, rounded normals, clump-level color variation,
roughness, transmission, and root-to-tip AO. Geometry now uses a separate 32-byte record; parent and
outcome data remain in a 64-byte diagnostic-only buffer.

Curve-authoring checkpoint, 2026-09-01: the previous independent tilt and handle sliders could
produce combinations that neither endpoint preview represented. Each ribbon variant now owns its
normalized tip plus both cubic handles, and one stable silhouette coordinate interpolates the whole
curve. The editor exposes two direct-manipulation charts for those minimum/maximum endpoints and
draws the real high-LOD sample positions after longitudinal redistribution. Handle vectors are
prepacked per species; this adds no vertices, instances, per-blade data, or draw calls and removes
one independent random interpolation path from the vertex shader.

Edge-on-fullness checkpoint, 2026-09-01: both the fixed 1.24 world-width multiplier and the later
minimum projected-pixel-width correction were rejected. The former could not recover a ribbon whose
side vector projected to almost zero; the latter produced uniformly thin screen-space filaments on
close, curved grass. Ribbons now transport their width axis along the local Bezier tangent and author
a bounded maximum view-opening angle. The unoriented width line follows the camera-facing width line
until it reaches that bound: the correction is zero for a face-on ribbon and grows continuously
toward edge-on views. Lighting retains the physical rounded normal, so the correction cannot darken
the material merely because its silhouette opens. Broad leaves bypass the opening. This changes
neither instances, topology, draw calls, nor per-blade storage and removes the extra clip-space
projection work from the rejected path.

Lighting experiment checkpoint, 2026-09-01: V2 now defaults to an exposure-aware two-scale foliage
response and retains the former empirical response behind the `L` comparison toggle. Nearby ribbons
reconstruct an analytic cylindrical cross-section from their physical normal, width axis, and
interpolated width coordinate. A separate narrow waxy sheen lets the sun/view half-vector select one
side of that cross-section instead of presenting a static central brightness mask. Unresolved blades
blend to a shallow, group-stable normal tied to terrain relief. GGX roughness grows with screen-space
normal variance, direct energy follows the camera's physical exposure, and the special
gloss/transmission response is concentrated toward the upper blade and low sun. Far gloss retains a
restrained floor instead of disappearing completely. This is an experiment pending fixed-camera,
moving-sun, and animated temporal acceptance rather than a finished material decision.

Height-driven topology checkpoint, 2026-09-01: ribbon topology allocation is now a stable physical
species property rather than a species-wide or camera-driven choice. Each root generates one height
from its retained seed and group coherence. Roots at or below an authored world-space threshold fold
the fixed strip budget into two short blades; taller roots spend the same budget on one more finely
sampled cubic curve. Geometry LOD remains an independent high/low choice, so a distant tall blade can
still retain the single curved form. The editor exposes the height-distribution bias, pairing toggle,
threshold, and expected single/paired mix. No vertices, instances, or per-root bytes were added.

Overhead-LOD correction, 2026-08-31: the first projected-size metric measured only the segment from
the root to `root + surface_normal * maximum_height`. That segment collapses toward zero in an
overhead view even though a bent or tilted blade still has a large horizontal footprint. This
incorrectly selected low topology, quarter-density work, far rejection, and draw-side density
collapse. Scheduling, candidate classification, and vertex morphing now all measure a conservative
authored envelope containing maximum height and maximum horizontal reach. The same bounds also
expand page visibility. A regression test requires the full-envelope metric in all three stages so
they cannot silently diverge again. Geometry and population LOD are now separate: topology follows
that blade envelope, while stable nested root retention follows the projected screen area of one
authored density cell. Consequently an overhead camera may use cheaper low blade geometry without
discarding roots that remain visibly separated on the ground. The 32-byte instance record keeps the
per-view population target in eight bits without increasing the arena.

The first fixed-camera `P` captures on the M2 Max put the full workload around 3.7-4.2 ms, with the
schedule-only scene around 3.6 ms and the vegetation compute contribution much smaller than the
remaining raster/fragment cost. These are overlay-level comparisons rather than exact pass timings,
but they reject the earlier assumption that candidate compute dominates. Further density work must
therefore report submitted topology inputs, indices, and screen coverage together; adding geometry
to hide ground without reducing overdraw is not acceptable.

Architecture correction, 2026-08-31: the first count/plan/emit implementation is rejected. It
classified candidates twice, rendered non-indexed triangle lists, doubled geometry for paired short
grass, retained excessive low-LOD population, and reserved 18 MiB of instances. The replacement
classifies and emits once, finalizes indexed indirect arguments in one invocation, keeps paired
topology within the single topology's unique-input budget, uses stable quarter-density low/far
subsets in the ribbon fixture, and initially reserved 6.50 MiB. A later measured full-density
reference required a 10.50 MiB arena. Height-driven allocation now rebalances its unchanged total
low capacity between single and split bins according to the expected authored height mix. Source
buffers grow and update in place instead of being recreated on each residency revision. No
compatibility requirement preserves the rejected prototype.

Fully low-LOD work items now dispatch the nested quarter-lattice itself when supported. Uniform
growth selects one world-aligned member per deterministic 2x2 block; divisible parent/child growth
selects one child per deterministic group of four. Work items crossing the transition keep the full
lattice. This removes the specific waste of evaluating four distant candidates and rejecting three
after terrain/competition work while retaining the same roots used by the visual fade.

The V2 debug HUD samples a 144-byte GPU readback asynchronously at low frequency. It reports source
repacks and actual reallocations, last upload and retained capacity bytes, source and scheduled work,
single-pass candidate evaluations, eligible/emitted instances, exact submitted index and topology
input counts, and capacity drops. Any drop is labeled a budget violation. The readback never waits
for the device and production rendering does not depend on CPU-visible counts. Named schedule,
generate, finalize, and draw spans also report built-in GPU time on backends with timestamp-query
support and CPU recording time everywhere. Metal requires a Metal/Xcode capture for exact per-pass
GPU milliseconds; the HUD says this explicitly instead of substituting CPU time.

## Goals

The renderer should eventually support fields containing combinations of:

- tall and short grasses;
- species with visibly different blade shape, color, and motion;
- low ground cover;
- flowers and other sparse accents;
- fully artist-authored decorative assets placed procedurally through compact GPU instances;
- near procedural geometry with a convincing transition to cheaper distant representations.

It should retain bounded memory, deterministic streaming, stable visuals, useful artist control, and
predictable performance. Individual blades should remain anonymous decorative samples rather than
database objects or entities. Broad grass-shadow density and useful local blade detail should also be
possible without rendering every blade through the full pipeline once for every shadow-casting light.

The target is not to reproduce Ghost of Tsushima's system verbatim. Their solutions are valuable
evidence, but Yarra's global batching, Metal/WGPU execution model, content model, and intended mix of
ground-cover families may justify different choices.

## Removed legacy Yarra pipeline

The following hybrid ribbon/card experiment was removed on 2026-08-31. It remains here only as a
baseline for understanding measurements and rejected constraints:

1. A 32 m source cell contains a 16x16 coverage mask, yielding up to 256 two-metre clusters.
2. At the current density of five carriers per square metre, a fully covered 2 m cluster contributes
   twenty deterministic carrier candidates.
3. Near and middle carriers can expand to as many as twelve independent ribbon-blade instances. Far
   carriers become textured card instances.
4. A first compute dispatch, `count_ribbon_candidates`, traverses the clusters and calculates exact
   near/middle demand.
5. A second compute dispatch, `cull`, repeats visibility, projected-size, density, and LOD decisions,
   then writes near, middle, and far instance arrays.
6. A one-invocation `finalize` dispatch turns the accumulated counts into indirect arguments and
   clamps them to capacity.
7. The three global indirect lists are rendered in the alpha-tested depth prepass and color pass.

Former implementation entry points were `crates/ground_cover`, `crates/ground_cover_compile`,
`assets/shaders/ground_cover*.wgsl`, and their world/database/cooker/editor integrations. They no
longer exist.

### Historical fixed GPU allocation

`VisibleInstanceGpu` was 96 bytes (six `vec4`s). Each near/middle/far list reserved 131,072
instances, so the three instance buffers alone reserved 36 MiB. The 256-layer, 256x256 R8 card atlas
with full mip chains was about 21.33 MiB, and the receiver-shadow visibility volume was about
0.14 MiB. The known fixed minimum is therefore roughly 57.47 MiB before other renderer resources.

The old on-screen "estimated GPU" residency figure described streamed data and did not expose this
fixed allocation. V2 now reports its fixed instance arena and retained source capacity separately.

### Current shadow status

The removed grass renderer had a prefiltered directional receiver-visibility volume. V2 now follows
the strongest world directional light and receives its ordinary CSM shadows through Bevy's mesh-view
bindings. The authored AO profile also controls how much ambient body survives in shadow, so dense
roots darken more strongly than exposed tips. The old cached receiver volume has not been carried
forward. Both [shadow proxy experiments](GRASS_SHADOW_PROXY.md) were rejected and removed.
There is currently no grass caster. [The restart](GRASS_SHADOW_RESTART.md) separates existing root
shading, fine inter-blade occlusion, grass-to-ground casting and ordinary object-shadow reception.
The representation must be re-evaluated without requiring reuse of the failed source reduction.

## What already works well

These properties are worth preserving unless measurements prove otherwise:

- **Coverage, not identities.** Streaming compact coverage and regenerating anonymous instances is
  the right scaling model for dense decorative vegetation.
- **Deterministic placement.** Stable hashes and low-discrepancy retention prevent ordinary camera
  motion from reshuffling the whole field.
- **Shared bounded compiler.** Cooker and editor-derived pages resolve source data through the same
  bounded cell compiler.
- **GPU-driven visibility.** Culling, expansion, counts, and indirect arguments remain on the GPU;
  rendering does not require visible-count readback. The low-frequency debug readback is optional
  instrumentation rather than a scheduling dependency.
- **Global batching.** Yarra currently renders one indirect list per representation tier rather than
  one draw per tile. This may be a substantial strength and should not be discarded merely to match
  another engine's architecture.
- **Procedural indexed generation.** Ribbons use a small static index buffer plus vertex/instance IDs
  and no stored vertex stream. This preserves shared section vertices and makes submitted topology
  work explicit.
- **Stable near/middle roots.** In the absence of capacity pressure, the same retained candidates
  underpin the neighboring ribbon LODs.
- **Separation of authored and culling bounds.** The content footprint is not forced to equal the
  conservative animated render bound.
- **Shared streamed terrain surface.** Terrain rendering, vegetation placement, and character
  grounding consume the same floating-origin-aware height/normal data.
- **Observable fixed budgets.** The HUD distinguishes retained source capacity, the fixed 10.50 MiB
  instance arena, submitted topology, and capacity violations from streamed world residency.

## Current problems and risks

### 1. The current visual result is still too expensive for its coverage

The corrected renderer no longer repeats classification, but the reference view can still schedule
roughly a million candidate lanes before stable density rejection. Fully low-detail uniform and
divisible parent/child work now use the quarter lattice directly; transition work and other density
profiles still traverse their full candidate domain. The reported M2 Max captures range from about
4 ms to 6.7 ms total GPU time while counters and memory stay stable. That does not prove a software
leak, but it is already too costly for the field density and material quality shown.

We need per-pass timestamps and fixed-camera captures before attributing the variation to thermal
state. Independently of that diagnosis, candidate work, submitted indices, overdraw, and fragment
cost must all earn their budget.

### 2. Source streaming still repacks whole residency snapshots

GPU source buffers are persistent and grow geometrically, so ordinary residency revisions no longer
allocate new buffers. The CPU still repacks and uploads the full resident source snapshot whenever
the revision changes. A bounded persistent page-slot table with dirty-range uploads is the remaining
streaming boundary. Its win must be measured separately from frame-local candidate scheduling.

### 3. Geometry LOD needs visual acceptance, not only structural invariants

High geometry converges onto exact low-section samples, and low population is a stable subset of high
population. Tests enforce the nested one-of-four rank. The density transition no longer scales blade
height: rejected high-detail candidates keep their complete centreline and contract only in width,
which removes the literal camera-following growth wave. The capacity-driven topology radius is also
staggered by the existing stable LOD rank, spreading the remaining curve simplification instead of
forming one coherent ring. We still need moving-camera evidence that shape, density, color, and
perceived volume cross the boundary without popping. Fragment cost may remain high even after
reducing vertex and root counts.

The visible transition sphere is not itself evidence that a radial/projected-size policy is wrong;
the reference games can expose a similar boundary. The acceptance failure is that Yarra currently
loses too much perceived coverage outside it, especially overhead. Low geometry, low population,
projected width, and material response must be tuned as one coverage contract so that the boundary
does not read as a bald ring even when its location can still be found in a diagnostic view.

The current single-ribbon topology also submits 18 unique high inputs and eight low inputs, versus
the talk's 15/7. This comes from an eight-section maximum and separate left/right inputs at the
zero-width tip. It is a conservative implementation choice, not a Bezier requirement. It is not an
accepted reason to spend more vertices: the next topology pass must benchmark a shared-tip 15/7
layout, and the folded short-grass layout must remain within that same budget, before any section
count is increased.

### 4. Ribbon curve authoring now preserves the two independent handles

The renderer evaluates a cubic Bezier with root, two interior controls, and tip. The earlier
prototype drove both controls from one `bend` sample and then coupled their lengths through a
provisional tilt-smoothness value. That contract has been replaced by two complete authored curve
variants. Each variant owns a root tangent, tip tangent, root handle length, and tip handle length;
one stable per-blade coordinate interpolates the complete variants.

This is visible in dense long grass: the target is not one uniformly bowed silhouette with random
tilt. A field needs a controlled distribution of broad arches, nearly straight blades, curvature
concentrated near the root or tip, and tips that flatten or continue downward. Independent random
blades will not repair a curve family that cannot express those silhouettes.

The 15/7 mesh vertices from the Ghost talk remain topology samples, not independent bend points.
Yarra preserves its fixed indexed topology and Bezier/derivative evaluation. Source angles and
normalized handle lengths are packed into root/tip handle vectors once per species, avoiding new
per-vertex trigonometry. High/low samples and the artist-controlled longitudinal remap approximate
the same curve. The remaining work is visual acceptance and tuning of curve families, not adding
instances, sections, buffers per blade, or draw calls.

### 5. Capacity is observable and high-topology admission is bounded

Every bin exposes eligible, emitted, and dropped counts; any drop is labeled `Budget: VIOLATION`.
Within the supported profiles the arena must never overflow. Atomic append order is not a fair
admission policy: it produced page-like holes when the tall split population exceeded its high bin.
V2 now derives a camera-local high-detail radius from the worst overlapping authored root density,
the topology-bin capacity, and explicit safety headroom. The outer annulus uses the existing exact
high-to-low morph; roots outside it enter low topology rather than disappearing. The radius is
world-space stable under zoom and shared by classification and draw reconstruction. Low-bin drops
remain invalid and require either profile rejection or a similarly deterministic budget design.

### 6. The topology families are not yet a finished species renderer

Species data controls current ribbon and two-leaf procedural shapes, but broad leaves are deliberately
restricted to the exact two-leaf topology the shader can honor. Flowers, stems/heads, sparse authored
meshes, and any eventual distant representation remain separate families to design. No requirement
preserves the removed card system; a far representation must win on continuity, cost, and authoring
clarity before it is adopted.

### 7. Strong coherent wind is implemented; interaction remains missing

V2 now deforms every procedural topology from one CPU/GPU-sampleable travelling-wave field. Shaped
gust pulses move the field coherently, while a substantial hashed sine offsets phase by blade and by
position along its length. Independent amplitude reduces with distance but keeps a small floor to
avoid lockstep clumps. The cubic and longitudinal detail are evaluated consistently across topology
LODs, and visibility/LOD bounds include maximum wind reach. Species-specific stiffness, interaction
displacement, floating-origin-stable field coordinates, and the full isolation views remain future
work.

### 8. Visibility integration is view-bounded but still incomplete

The CPU keeps a conservative three-cell residency shell and the GPU performs a conservative
eight-corner frustum test before scheduling. This fixed the rotation-dependent holes. Occlusion,
shadow views, reflections, and other view families are not yet represented, and adding them must not
multiply full candidate generation blindly.

### 9. Verification has counters and named spans but lacks a repeatable capture protocol

Existing tests successfully validate compilation, shader parsing, and several static invariants, but
we lack automated or repeatable evidence for:

- LOD continuity;
- capacity fairness;
- per-blade motion distribution;
- overdraw and alpha-test cost;
- temporal specular stability;
- repeatable pass-level GPU timings and overdraw;
- long-traversal traces that correlate thermal state, residency revisions, uploads, and workload.

### 10. Grass shadow casting: restart after two rejected experiments

Both the raised dithered-depth sheet and the clump-mask caster failed visual review. The latter's
small measured Mac difference and passing bias regression did not establish a convincing result
from above. Both have been removed. Follow [the restart plan](GRASS_SHADOW_RESTART.md), beginning
with a small actual-grass patch and separate inspection of AO, sun visibility and final shading.
Do not continue tuning either implementation or interpret the historical measurements as acceptance.

## What the Ghost talk contributes

| Talk technique | Current Yarra state | Working position |
| --- | --- | --- |
| Generate instances, accumulate count, then finalize indirect args | Present; one classify-and-emit pass traverses a GPU-compacted visible work queue, then one invocation finalizes indexed arguments | Preserve; do not reintroduce duplicate classification |
| Eight-tile scratch buffer, four-tile compute/graphics overlap | Not copied literally; Yarra now has bounded topology/LOD arenas and GPU indirect work scheduling | Finish persistent page slots, then benchmark ring/ping-pong only if global bins still lose |
| No vertex streams; derive topology from IDs | Present | Preserve |
| 15-/7-vertex high/low blades | Current single topology uses 18/8 unique inputs because it has an eight-section ceiling and duplicates the zero-width tip | Benchmark and prefer a shared-tip 15/7 layout; do not spend extra vertices without measured visual value |
| Artist-controlled redistribution of vertices along the curve | Present as a per-species longitudinal exponent | Tune against representative curvature rather than adding vertices first |
| High LOD morphs toward low LOD | Implemented by converging high vertices onto exact low-section samples; the budget boundary is stable-rank staggered | Tune transition width from moving-camera captures |
| High LOD fades three of four blades before larger low tiles | Implemented as species-authored stable nested density; physical tile-size coupling is unnecessary | Tune density fractions per species rather than hard-coding three of four |
| Fold one strip into two short blades | Implemented per stable root height: short roots use the split unit while tall roots of the same species keep the full single curve | Tune the authored height bias/threshold; evaluate asymmetric tip topology only if needed |
| Cubic Bezier shape and derivative normal | Cubic position and derivative normals use two complete variants with independent root/tip tangents and handle lengths | Preserve the fixed evaluation and topology; tune the curve family and sampling distribution against reference captures |
| Unified CPU/GPU 2D wind field | Implemented as a shared analytic broad-wave/gust function with a public CPU sample and matching GPU evaluation | Keep the mathematical definition synchronized; move sampling to absolute field coordinates before large-world rebasing is finalized |
| Per-blade phase variation | Implemented as substantial blade-identity and longitudinal phase variation over coherent gusts, with a restrained far-distance amplitude floor | Add group-only/root-only/combined isolation views and species stiffness controls |
| Rounded normals | Present as an analytic/stable approximation | Preserve and make species-adjustable |
| Edge-on view-space thickening | Implemented as local tangent-frame transport plus a bounded, species-authored view-opening angle | Tune from grazing captures; reject visible billboarding, highlight rotation, or an excessive fragment increase |
| Distance blend toward a clump normal | Yarra mostly uses a stable up-dominated clump normal at all distances | Recover useful near detail, then explicitly blend to the stable field with distance |
| Reduce distant gloss as normal variance becomes subpixel | Partly approximated through roughness logic | Add a controlled distance/footprint response and verify temporal stability |
| Blade/clump material profiles and textures | Current bottom/top color and analytic shading are simpler | Add compact per-species curves/LUTs only after geometry/species unification |
| Authored blade AO instead of temporal SSAO | Root-to-tip AO is authored analytically; V2 does not write grass velocity | Preserve unless a measured need justifies velocity and temporal cost |
| Artist-authored field assets through a minimal GPU instance stream | Missing as a distinct family; current V2 supports procedural ribbons/two-leaf units only | Add a sparse decorative-asset family sharing deterministic coverage sampling, streaming, culling, and indirect drawing |
| Keep only a 3x3 neighborhood of generated authored assets | No equivalent authored-asset cache yet | Preserve the bounded-residency principle; derive Yarra's actual neighborhood from cell size, asset bounds, and view range rather than copying 3x3 literally |
| Full grass pipeline in shadow maps for rare cases | Visible blades remain absent from caster passes; the opt-in canopy is separate | Build only as a reference/optional exceptional mode if budgets allow, not as the default |
| Grass shadow impostor | Both experiments rejected and removed; see [restart](GRASS_SHADOW_RESTART.md) | Establish correspondence and view consistency on a small actual-grass patch before selecting a replacement |
| Short-range screen-space blade shadows | Missing | Investigate as the fine-detail complement to a broad proxy, with explicit off-screen and disocclusion limits |

## Working decisions

These are provisional design positions. Change them when an experiment provides better evidence.

### Preserve

- Anonymous deterministic coverage rather than per-blade persistent state.
- GPU indirect rendering and procedural topology.
- Global batching as the baseline comparison.
- A cheap distant field representation as a requirement; its topology is intentionally undecided.
- Stable nested hashes/ranks for density transitions and capacity selection.
- Stable non-temporal ambient/self-occlusion for the grass field.

### Adopt as design direction

- One authored species definition must drive every representation it uses, material response, bounds,
  and wind response. Cards are optional, not an architectural requirement.
- Shape LOD and density LOD need separate, continuous transitions.
- Material detail must be filtered toward a shared field response as it becomes subpixel.
- Fixed memory, demanded/emitted counts, and overflow must be visible in diagnostics.
- Mixed cover should be a composition of typed visual families, not a pile of indistinguishable card
  presets.
- Dense continuous grass should support stable analytic Voronoi clumps. Clump identity and
  centre-relative coordinates may coherently affect placement, height, facing, material, and later
  motion, while each channel retains an independent artist-controlled weight.
- Non-interactive authored field assets should use a compact, deterministic GPU instance stream;
  heavyweight game objects remain appropriate only when gameplay identity is required.
- Default grass casting should combine a broad field-density proxy with optional short-range detail,
  rather than multiplying full blade rendering across lights.

### Experiment before committing

- Compact global lists versus bounded compute/graphics ping-pong batches.
- Count-scan-fill versus atomics/reservation with one classification pass.
- Per-carrier versus per-blade compute workgroup layout on Apple GPUs.
- Maximum view-opening angles and continuous angular response for the local tangent-frame correction.
- Derivative/animated near normals followed by distance blending versus stable normals everywhere.
- Alternative split topology budgets only if the implemented two-leaf unit fails visually.
- Texture profiles versus analytic curves/LUTs for vein, gloss, translucency, AO, and clump color.
- Voronoi clump spacing, feature-point jitter, root attraction, centre falloff, shared direction,
  radial/tangential direction, and residual per-blade variation. Compare against both independent
  random blades and explicit parent/child tufts rather than assuming one clump model fits all grass.
- Terrain-proxy topology, density encoding, world-stable dithering, and filtering for broad grass
  shadows.
- Screen-space shadow tracing as the local-detail layer over the broad proxy.

### Defer unless evidence changes

- Per-blade database identity or entity state.
- A grass velocity buffer solely to feed temporal SSAO.
- Blind adoption of one draw call per tile.
- More density as the first response to a thin-looking field.
- Full per-blade shadow rendering for every light, or other default costs that scale with the full
  visual population. A measured reference/rare exceptional mode is still useful.

## Proposed work plan

The phase numbers express broad dependencies. The 2026-09-09 [shadow restart](GRASS_SHADOW_RESTART.md)
supersedes the earlier sequence of field-wide proxy implementation followed by visual tuning.
Establish a convincing small-patch signal across views, declare a candidate's complete cost, then
measure it in the current scene. Broader grass optimization and iPhone testing are later work.

### Phase 0 - establish evidence (in progress)

Do this before further density or topology tuning. The asynchronous workload/memory counters are
implemented. Named pass spans are implemented; built-in GPU values are available on WGPU backends
with timestamp-query support, while Metal exposes the corresponding CPU recording spans and requires
Metal/Xcode capture for exact pass GPU time. Repeatable scenes, camera transforms, captures, and
comparison records remain.

- Preserve the named visible-work scheduling, classify/emit, finalization, and indexed-color spans.
  Add future shadow/depth passes to the timing set only when they exist.
- On Metal, use the `P` workload cycle with a fixed camera: full, frozen draw from the immediately
  preceding full frame, compute without draw, and scheduler only. Record several settled samples per
  mode from the same view; do not treat CPU recording spans as GPU execution time.
- Preserve the implemented per-bin eligible/emitted/drop counters, scheduled work, candidate lanes,
  candidate evaluations, submitted indices, and unique topology inputs.
- Preserve separate reporting for the fixed 6.50 MiB instance arena, retained source capacity,
  source uploads/reallocations, and streamed residency.
- Add debug views for LOD tier, shape-morph weight, density-retention rank, capacity rejection,
  candidate domains, clump identity, and any future distant-representation transition.
- When wind is implemented, add group component only, per-root component only, frozen phase color,
  displacement magnitude, and direction-field modes with it.
- Capture repeatable close, grazing, overhead, fast-camera, and rainy/glossy scenarios at fixed
  resolution and camera transforms.
- Measure depth-prepass savings and fragment overdraw instead of assuming the prepass is a net win.
- Add fixed-light shadow scenes with field boundaries, slopes, several grass heights, long low-sun
  shadows, camera motion, and a visible non-terrain receiver.
- Keep separate counters/timings for receiving opaque-object shadows and casting grass shadows so one
  optimization cannot be mistaken for the other.

Exit criterion: a change to density, topology, memory, wind, or shadows can be compared using
repeatable visual captures, GPU timing, instance counts, overflow, and memory.

### Phase 1 - unify species and make LOD continuous (structurally implemented)

- Preserve the implemented runtime geometry profiles for height/width ranges, facing distribution,
  complete cubic Bezier silhouettes, longitudinal remap, topology family, normal rounding,
  material response, and conservative bounds. Add wind fields only with the wind implementation.
- Keep high geometry converging on exact low-section samples.
- Keep the stable nested density transition and direct quarter-lattice scheduling for fully low work.
- Keep the two-leaf topology species-controlled and within the single topology's unique-input budget.
- Preserve the implemented page-seam-safe analytic Voronoi clump option for continuous populations.
  It computes the nearest and second-nearest jittered feature points from a world-space 3x3
  neighborhood per candidate and stores no entities, clump records, or page-local identities.
- Preserve the implemented separation between root distribution and rest-orientation control.
  Uniform/stratified roots can blend group-shared, radial, tangential, field-flow, and residual
  random directions with independent angular jitter.
- Tune clump spacing, density retention, attraction, orientation, and per-species shape coherence
  against fixed overhead and grazing reference captures. Keep the derived roots-per-clump cost visible
  while tuning; do not add a second independent count control to analytic Voronoi populations.
- Validate the implemented transitions visually at fixed boundaries; add a different distant
  representation only after this procedural baseline is measured.

Exit criterion: camera movement through all LOD boundaries produces no obvious shape, density, color,
or motion pop in the representative scenes.

### Phase 2 - reduce compute, memory, and bandwidth (current priority)

- Preserve the implemented 32-byte procedural instance and compact indexed species table; profile
  reconstruction cost before packing further.
- Preserve one candidate per lane, one classify/emit pass, and one finalize invocation.
- Convert full-residency source repacks into bounded persistent page slots with dirty uploads if the
  source upload diagnostics show a meaningful traversal cost.
- Extend direct reduced-lattice scheduling to other provably nested density fractions only when the
  mapping is exact and retains the same roots.
- Compare the current bounded append path against count/scan/reservation or a small reusable batch
  ring only with pass timings. Do not add another classification pass.
- Keep a global-list version as the baseline. A tiled double buffer wins only if its lower memory or
  overlap offsets extra dispatches, synchronization, and draw calls on the actual target hardware.
- Preserve density-derived deterministic high-topology admission and reject profiles that can still
  exceed a low-bin budget; do not accept atomic overflow as a quality mode.

Exit criterion: the selected design has measured wins in representative scenes, bounded worst-case
memory, and no new spatial bias or temporal instability.

### Phase 3 - rethink grass shadows after the rejected implementations

1. Preserve ordinary shadow reception and root shading; keep the retired casters out of the build.
2. Establish reference behavior on a small actual-grass patch. Separate AO, inter-blade sun visibility
   and grass-to-ground casting; include sparse gaps and bent blades.
3. Compare candidate representations from gameplay overhead, vertical top-down, near and grazing
   views, then in camera/light/wind motion. An inexpensive but visually wrong result fails.
4. Investigate a material-local illusion first for blade detail, while explicitly leaving ground
   casting unsolved. Any geometry/data approximation must share the visible grass's source definition
   or demonstrate plausible correspondence. Do not reintroduce independent masks as a solution.
5. For a promising candidate, count complete work and run a short matched current-scene Mac cost
   check before adding field-wide integration or controls. Preserve existing AA and render quality.

Exit criterion: a visually useful and view-consistent approximation with small demonstrated added
cost. No replacement is selected yet. The reference and candidate criteria are detailed in
[GRASS_SHADOW_RESTART.md](GRASS_SHADOW_RESTART.md). iPhone testing remains deferred.

### Phase 4 - improve wind, fullness, and material filtering

- Tune the implemented two-dimensional travelling-wave/gust field through its shared CPU/GPU
  mathematical definition. Layered scrolling noise remains optional rather than required.
- Use Phase 0 diagnostics to decide whether current motion needs wider per-blade phase, less group
  dominance, longitudinal phase, or more orientation variation.
- Tune the implemented local-frame view opening from fixed grazing captures so it improves coverage
  without making silhouettes or highlights follow the camera or materially increasing fragment cost.
- Test animated/derivative normals in the near field, then blend toward a species/clump normal as the
  blade becomes unresolved.
- Increase roughness/reduce gloss based on distance or projected footprint and validate under motion,
  grazing light, and rain-like settings.
- Add compact root-to-tip profiles for color, translucency, AO, and width; add clump-scale color
  variation. Use textures only where curves or a small LUT cannot express the needed art direction.

Exit criterion: wind reads as one coherent field without lockstep blades, the field remains full from
grazing angles, and mid/far highlights are temporally stable.

### Phase 5 - generalize ground-cover composition

- Introduce explicit geometry families such as tall ribbon, short split ribbon, card cluster,
  stem-and-head flower, and very low surface scatter.
- Add an authored-mesh asset family for pampas-like plants, lilies, flowers, and similar decoration.
  Its generated records should contain only transform, visual/mesh identity, bounds/culling data, and
  any minimal material variation needed by the draw path.
- Generate those records deterministically when a streamed tile/cell becomes resident by testing
  candidate positions against compatible ground-cover types. Reuse a generic growth/foliage instance
  stream if Yarra develops one rather than building a grass-only asset path.
- Bound authored-asset residency around the view and release generated records with their source
  cells. Ghost's nearest 3x3 tiles are a useful example, not a fixed Yarra constant.
- Let a preset contain a weighted mixture or structured layers of species while retaining deterministic
  placement and a single coverage authoring workflow.
- Distinguish dense base cover from sparse accents so flowers do not multiply with grass density.
- Extend the implemented shared terrain height/normal surface from the V2 diagnostic into every
  production representation family.
- Add conservative occlusion and multi-view support when a real use case and measurements justify it.
- Define representation-specific budgets so a few flowers cannot evict the grass layer unpredictably.

Exit criterion: one authored region can produce controlled mixed vegetation whose species retain
their own shape, material, density, LOD, and motion behavior.

## Initial experiment matrix

| Experiment | Hypothesis | Primary evidence | Failure signal |
| --- | --- | --- | --- |
| High-to-low Bezier morph | Matching the low shape before the switch removes curvature pops | Fixed-camera-boundary captures and image difference | Silhouette still jumps or blade length visibly breathes |
| Stable one-of-four density nesting | A nested rank can reproduce the talk's density transition without larger physical tiles | Retained-root debug view and moving-camera capture | Roots reshuffle, holes form, or far density changes abruptly |
| Short split topology | Two short blades per strip improve fullness at similar vertex cost | Grazing/overhead captures plus fragment and vertex timings | Odd topology is visible, overdraw erases the gain, or motion looks coupled |
| Wider/isolated blade phase | Current sameness comes from constrained phase/shared group motion | Group-only, blade-only, and combined captures | Extra phase reads as noise without improving natural variation |
| View-space thickening | Projected widening improves fullness more efficiently than more blades | Pixel coverage, overdraw, and grazing-angle captures | Close blades inflate, silhouettes swim, or fragment cost rises too much |
| Near derivative normal + distance blend | Local curvature can improve close lighting without restoring distant glitter | Animated specular captures at multiple distances | Shimmer returns or the normal transition becomes visible |
| 32-byte instance follow-up | Further packing may reduce bandwidth, but reconstruction already has a cost | Memory bandwidth/timing and visual parity | Reconstruction costs more than bandwidth saved or packing artifacts appear |
| Reduced candidate lattices | Entirely low-detail fields should schedule retained roots directly | Candidate lanes/evaluations and identical-root debug capture | Transition roots change or remapping/edge handling costs erase the win |
| Bounded batch ring | Overlap and smaller buffers can beat global allocation | GPU timeline, peak memory, draw/dispatch cost | Serialization remains or extra submissions cost more than memory saved |
| Dithered terrain/proxy grass shadow | First implementation rejected and removed | Historical tests/timings do not establish visual acceptance | Blurry depth-noise pattern and moving boundary failed the reference |
| Screen-space shadow detail | Short rays can restore visible blade contact shadows over the broad proxy | Slide-45/46-style boundary scene, motion, disocclusion, and off-screen tests | Halos, depth leaks, unstable noise, or missing off-screen detail are more distracting than the benefit |
| GPU-authored asset stream | Sparse flowers/pampas can enrich fields without entities or full blade density | Instance memory, tile load cost, cull/draw timing, and streaming churn | Asset records dominate residency, pop at cell boundaries, or need gameplay state |

## Open questions

- A17 Pro sustained 60 FPS is the established device target; what whole-game CPU/GPU headroom and
  content envelope must remain after ground and vegetation? Declare these before production
  acceptance; 16.67 ms is the whole frame deadline, not the grass budget.
- Does the target WGPU/Metal scheduling actually overlap these compute and graphics passes in one
  command stream, or would Ghost's double-buffer cadence require different submission boundaries?
- Should a mixed preset choose one species per deterministic root, layer independent sparse
  populations, or support both models?
- How much root-to-tip material control do artists need before textures are justified?
- Is the desired field direction driven by wind, terrain, authored flow maps, growth patterns, or a
  controlled combination? The current smooth direction field may be solving several concepts at once.
- Which flowers require geometry near the camera, and which can remain cards at every distance?
- Should sparse authored assets share one generic engine-level growth/foliage stream, or remain a
  ground-cover representation family?
- What Yarra residency radius replaces Ghost's 3x3 tile cache once cell size, largest asset bounds,
  camera speed, and preload margin are considered?
- Do gameplay systems need the same wind sample for projectiles, particles, cloth, or audio? That
  determines how important an exactly shared CPU/GPU function is.
- Which lights actually need grass casting? If only the directional sun matters, the default shadow
  design can be much narrower than a general per-light solution.
- Can Yarra inject a separate proxy caster into Bevy's directional shadow views without duplicating
  the grass compute pipeline, and can the same representation cover all cascades consistently?
- Should proxy height/density come directly from coverage/species metadata, a coarse generated height
  field, or terrain vertices? This determines both quality and coupling to terrain tessellation.
- Bevy 0.19.1 has contact shadows; is there ever sufficient measured headroom to justify integrating
  custom grass depth and sampling? This is deferred, not a prerequisite for the proxy.
- On slide 38, does the talk say translucency increases toward the tip? The physical explanation and
  the live wording should be reconciled before copying the profile.

## Decision log

- **2026-09-09 restart:** Both grass casters failed visual review and are removed. Reconsider the
  representation on a small actual-grass patch, with gameplay overhead and vertical views first.
  Earlier canopy-grid, dither and broad-only endpoint decisions are superseded. No new caster is
  selected; optimization and the deferral of iPhone testing remain binding.


Add dated entries here when experiments turn provisional positions into decisions.

- **2026-09-08, clarified:** Current iPhone grass performance is already unacceptable; defer phone
  tests and broader optimization. The immediate task is a bounded sun-only canopy experiment with
  small added cost measured on Mac. Withdraw the proposed A17 timing gate and unmeasured numerical
  allowances. Do not make solving baseline performance a prerequisite for shadow work. Retain
  existing shadow resources and defer full-blade/screen-space shadows; reject any approach that
  materially worsens the current renderer. iPhone remains a longer-term target.
- **2026-08-30:** Treat the Ghost talk as evidence and a source of experiments, not a target
  architecture.
- **2026-08-30:** Preserve anonymous deterministic coverage and GPU-driven indirect rendering.
- **2026-08-30:** Do not replace global batching with per-tile draws until a bounded batching prototype
  wins on the target hardware.
- **2026-08-30:** Make near geometry and far cards representations of the same authored species before
  expanding to mixed grass and flowers.
- **2026-08-30:** Instrument timing, memory, population, overflow, and LOD behavior before optimizing
  the core compute pipeline.
- **2026-08-30:** Treat Yarra's receiver-shadow visibility volume and Ghost's grass-casting terrain
  impostor as different systems; the former does not solve the latter.
- **2026-08-30:** Pursue grass shadows as a two-scale investigation: broad proxy density first,
  optional short-range screen-space blade detail second, with full-blade shadow rendering retained
  only as a reference or rare mode.
- **2026-08-30:** Plan non-interactive flowers and pampas-like assets as bounded GPU-instanced field
  decoration rather than full game objects.
- **2026-08-31:** Design V2 around independent species, population/growth, and spatial-field axes;
  current clusters, card recipes, page-local species, fixed instance lists, and hard-coded shaders
  are replaceable implementation details.
- **2026-08-31:** Use one de-duplicated GPU catalog, streamed population fields, one candidate per
  compute lane, stable budget resolution, compact emission, and topology/LOD draw bins whose count is
  independent of page and species count.
- **2026-08-31:** Treat parent/child growth patterns and exclusive population groups as required by
  the supplied dry-tuft and mixed-understory references, not optional polish.
- **2026-08-31:** Make terrain relief an endpoint-inclusive streamed data contract, not a visual-only
  shader displacement. Share world-space quantization across pages, cook neighbor-aware edge
  normals, and expose one resident CPU surface to terrain rendering and vegetation placement.
- **2026-08-31:** Use the rolling 33x33 demo heightfield as an acceptance fixture, not a
  procedural-world design decision.
- **2026-08-31:** Remove the legacy grass pipeline and make `--vegetation-v2-debug` the sole
  vegetation validation path. Outline nearby streamed terrain pages so surface seams can be judged
  before production blade geometry is introduced.
- **2026-08-31:** Retain parent coordinates and placement outcome metadata only in the diagnostic
  instance contract. Cycle between accepted species, parent links, and candidate outcomes without
  making that 64-byte record the production emitted-instance format; the production format remains
  a later compact, topology-specific design.
- **2026-08-31:** Ground opted-in characters, click destinations, and target indicators through the
  same floating-origin-aware resident terrain query used by vegetation. Do not add a general physics
  dependency only to sample height; evaluate Avian or another controller when slope limits, steps,
  falling, collision shapes, and dynamic-body interaction enter scope.
- **2026-08-31:** Implement the first topology checkpoint with one single-ribbon and one two-blade
  indirect bin. Generate cubic Bezier positions and derivative normals without vertex streams, use
  species-controlled longitudinal redistribution, taper, pair separation, rounded normals, and
  modest bounded edge-on opening, and keep all placement diagnostics available behind `X`.
- **2026-08-31:** Treat the reported frame-to-frame disappearing grass as capacity instability, not
  LOD or intentional variation. Replace atomic-race overflow with deterministic bucketed seed-rank
  admission, split the 64-byte diagnostic record from a 32-byte procedural record, and bias the fixed
  arena toward the more common split topology. Repeated count/emit classification is accepted only
  as a measurable intermediate cost.
- **2026-08-31:** Build fullness from layered populations: retain taller curved parent/child ribbons
  for silhouette, and add an independent dense short population whose render unit contains two
  widely spread blades. This applies the talk's short-grass doubling idea explicitly instead of
  doubling every species or hiding sparse placement with arbitrary width inflation.
- **2026-08-31:** Stop density tuning until work is view-bounded. Compact visible page-field work on
  the GPU, write indirect candidate-dispatch dimensions, split single/split geometry into authored
  projected-size high/low bins, prioritize capacity by distance and stable seed, converge high
  sections onto low samples, and collapse high-only density members before the transition. Keep
  persistent GPU page slots as the next streaming boundary rather than calling whole-scene source
  reuploads complete.
- **2026-08-31:** Fix rotation-dependent rectangular holes by separating compact source residency
  from render visibility. Keep terrain relief and vegetation fields resident in a bounded three-cell
  shell around the viewpoint (7×7 pages for the 32 m demo), matching the procedural range; let the
  GPU queue perform camera culling. Keep gameplay pages on their smaller one-cell proximity shell.
  Replace the queue's projected page-centre/radius approximation with a conservative eight-corner
  homogeneous side-plane test so a visible page edge cannot be rejected with its off-screen centre.
- **2026-08-31:** Diagnose long-traversal cost with asynchronous counters before attributing it to
  throttling or changing density. Read back only 144 bytes approximately four times per second and
  expose source repacks, scheduled work, candidate evaluations, per-bin eligible/emitted counts,
  and capacity drops. Never block or make rendering depend on the readback.
- **2026-08-31:** Reject the intermediate count/plan/emit renderer after measurements exposed
  duplicated classification, non-indexed expansion, doubled paired geometry, excessive low-LOD
  population, and an 18 MiB arena. Replace it with classify-once direct append, one-invocation
  finalization, fixed indexed topology budgets, a 32-byte instance, and a 6.50 MiB arena.
- **2026-08-31:** Treat every capacity drop as a profile/budget violation. Atomic overflow is not a
  production admission policy; accepted profiles must fit or gain a deterministic bounded admission
  design justified by measurements.
- **2026-09-01:** Use the full-density LOD mode as a visual and cost ceiling, not a production
  admission policy. A measured reference emitted 131,072 split-low instances and dropped another
  22,078, producing camera-dependent empty roads. Preserve the resulting 10.50 MiB total arena and
  exact capacity telemetry; height-driven topology allocation now restores 32,768 high entries for
  both topology classes and partitions one 278,528-entry low arena from the authored single/split
  height mix, with a 32,768-entry minimum for either class. This avoids page-shaped loss when an
  editor threshold moves demand between bins without increasing memory. Compare authored, balanced, and full modes
  with zero drops. The selected balanced production curve is now the default. It keeps 100% density
  through six-pixel projected root spacing,
  tapers to 55% at two pixels and 30% at 0.75 pixels, and contracts retiring blade width
  through a stable rank band without changing height.
- **2026-08-31:** Do not preserve the removed cards or their editor workflow. A distant
  representation remains required, but cards, reduced procedural roots, a field proxy, or another
  representation must compete on measured continuity and cost within the V2 species contract.
- **2026-08-31:** Schedule the exact nested quarter lattice for fully low-detail uniform and divisible
  parent/child work. Avoid spending four candidate evaluations merely to reject three after terrain
  and competition sampling.
- **2026-08-31:** Define procedural LOD from the full authored blade envelope. Maximum horizontal
  reach participates in work-item scheduling, per-candidate classification, and draw-side morphing;
  a surface-normal height segment alone is not a valid projected-size metric for overhead views.
- **2026-08-31:** Decouple topology LOD from population LOD. Use projected blade envelope for shape
  complexity and projected authored root-cell area for stable density retention; low-poly blades do
  not imply that visibly separated roots may be removed.
- **2026-08-31:** Add Ghost's slide-16 Voronoi clumping to the evidence and V2 direction. Independent
  per-blade randomness is not a natural-field model. Continuous populations need deterministic
  world-space clump identity and centre-relative influence, distinct from explicit parent/child
  tufts and configurable independently for placement, height, facing, material, and later motion.
- **2026-08-31:** Use one grouping interface for short grass, tall grass, broad leaves, and later
  flowers. Root placement, grouping source, rest orientation, and species response are separate
  contracts. `None`, explicit `Parent`, and analytic `Voronoi` all produce the same stable group key,
  centre, distance, direction, boundary influence, and density-retention sample. Exact child counts
  belong only to parent/child placement; expected Voronoi membership is derived from maximum root
  density and clump spacing. Preserve the 32-byte emitted instance by carrying a compact group
  variant rather than storing per-clump runtime records.
- **2026-08-31:** Fix tall split-grass budget instability without enlarging the arena. Derive each
  expensive topology's high-detail radius from maximum overlapping authored density and fixed bin
  capacity, reserve stochastic headroom, morph through a stable outer annulus, and route all farther
  roots to low topology. Atomic invocation order must never choose the visible survivors. Clamp
  runtime culling reach to the actual cubic-Bezier control hull plus width and wind displacement so
  an undersized editor bound cannot cause view-edge page loss.
