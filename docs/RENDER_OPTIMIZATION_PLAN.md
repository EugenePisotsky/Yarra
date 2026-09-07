# Rendering optimization implementation plan — historical

**Reassessment pending, 2026-09-06.** Read [PERFORMANCE_HANDOFF.md](PERFORMANCE_HANDOFF.md)
before using this plan. It preserves earlier implementation decisions, not an approved
next sequence. Several changes reduced GPU work counters without demonstrating a
meaningful sustained iPhone frame-rate/heat improvement. The user requested a handoff
so another session can reassess the approach. Full material caching, further grass
architecture and engine migration remain unselected options. “Done” below means
implemented and checked for correctness, not that performance acceptance passed.

Targets: sustained 60 FPS on iPhone 15 Pro Max; display VSync (120 Hz on the
current Mac). Keep 4× MSAA available for grass shimmer. Measure GPU work and
power as well as FPS: staying at VSync does not imply spare thermal capacity.

Evidence and capture instructions live in [RENDER_AUDIT.md](RENDER_AUDIT.md).
The captured Mac frame attributes 44.64% to terrain fragments, 18.95% to grass
vertices, 10.51% to grass fragments, and 9.06% to grass generation. These are
one frame's shares, not portable iPhone timing estimates. Replay clocks varied;
the earlier captures cannot establish an absolute before/after speedup.

## Implementation order

1. **Done: reuse stationary grass placement.** Keep generated instances when
   inputs are unchanged; wind still animates. Movement and source changes rebuild.
   Verified that the stationary frame loses the generation dispatches without
   changing draw counts. Physical-device thermal improvement is not yet measured.
2. **Done: cache stochastic lattice transforms.** Each lattice
   vertex has a stable rotation and offset, yet the shader computes nine hashes
   per surface per fragment. Generate these small tables once on the GPU, using
   the same hash implementation as the reference shader. Keep source texture
   sampling, mip selection, material blending, normals, lighting and shadows.
   Bound memory and upload work; use the reference path until a table is ready
   and whenever a material cannot fit. The retained buffer implementation removes
   12.6% of main-pass fragment ALU instructions with essentially unchanged texture
   sample counts in the captured Mac view. Patterned GPU comparisons are byte
   identical; table payload in the scene is 1.44 MiB. Thermal benefit is unmeasured.
3. **Done: prepare shared grass curves.** Compute
   shape randomness, curve control points and root wind/gust response once per
   blade per frame. Vertex-specific curve evaluation, flutter, ribbon opening and
   shading remain in the draw shader. Preparation is separate from placement so
   stationary placement still reuses its instances while wind animates. A bounded
   131,072-blade buffer uses 17.31 MiB including the instance lookup and indirect
   arguments; overflow follows the original shader without removing grass. Native
   image comparisons cover wind, 4× MSAA, camera movement and forced overflow.
   The retained path cuts main-pass vertex-plus-preparation ALU by 51% versus its
   reference switch (about 50% versus the previous shader); total frame arithmetic
   falls 9.7%. Total device-memory traffic rises 17.5%, so an iPhone power/thermal
   A/B remains required. The Mac replay was 4.11 versus 3.98 ms; clock variation
   means this is not a validated sustained speedup. Enabled by default;
   `--grass-vertex-reference` disables preparation.
4. **Full terrain material cache.** Cache blended albedo, normal, roughness and
   AO, with lighting/shadows still dynamic. Requires a camera-dependent tile
   working set, mip/edge treatment, explicit memory budget, bounded updates and
   close-view fallback. Do not bake all resident 32 m pages at source resolution:
   the 1.6 m / 1024-pixel surface has 640 source texels/metre, or 20,480 texels
   across a page. Two RGBA8 maps at that density exceed 3 GiB per page before
   mips. A uniformly low-resolution page bake would silently blur close views.
   This was proposed for the ground-heavy view, but it has not been implemented or
   shown necessary. The subsequent physical iPhone close-view capture changed the
   immediate order; the latest handoff leaves the architectural decision open.
5. **Reassess remaining draw costs.** Capture after the changes above; only then
   select shadow filtering/cascades, grass shading or geometry as the next target.
   Keep each quality tradeoff explicit and independently measurable.

## Acceptance checks

- Cache/reference comparison at fixed camera, material, resolution and MSAA.
- Moving camera, changed terrain inputs, negative coordinates, page unload and
  reload, diagnostic transitions, and shader reload must not leave stale output.
- Memory and per-frame cache update work have hard bounds. Logs identify cache
  configuration, resident bytes, builds and reuse so performance runs are auditable.
- Native GPU validation plus Mac and iOS compilation. No shader compilation or
  validation failures, missing terrain, or changed grass population.
- Matched captures with one replay running at a time. Report eliminated work and
  any measured timing separately; do not infer speedup from thermally different
  sessions. Final on-device sustained test uses the same scene/settings and
  records thermal transitions, GPU time and FPS.

This plan records both implemented work and outstanding work. It does not claim
that the entire thermal problem is solved by any one cache.


### Latest device acceptance: log9 fails

At 75%/4× with continuous low-camera movement, iPhone reaches Fair at 54.534 s,
Serious at 74.809 s, and about 38 FPS / 31 ms HUD GPU duration near the end.
Both caches are enabled. Placement regenerates every moving frame. This is not
a matched before/after run, so the Mac arithmetic savings remain validated but
iPhone performance/power savings do not. Prioritize a physical-device capture
of this workload and a grass-preparation A/B before extending that cache.

### Physical iPhone capture changes the immediate priority

Completed the low-camera A17 Pro capture and preparation A/B. Grass is about
68% of the captured frame (vertex 28.62%, fragment 22.87%, generation 13.27%,
preparation 3.34%); terrain fragments are 14.90%. Preparation reduces combined
vertex/preparation ALU 29.5%, but adds 13.4% memory traffic and changes replay
time only from 18.41 to 18.03 ms. This does not establish a thermal improvement.

Two additional changes are implemented without reducing grass density:
early candidate rejection (generation ALU −5.76%, native comparison preserves
all accepted candidates) and default-off unreadable diagnostic atomics on iOS
(generation atomic write traffic approximately halves). These are modest
savings; replay after both is 17.82 ms, with clocks not controlled.

Next substantial work should address **moving-view grass**, rather than adding
more small arithmetic caches:

1. Separate stable candidate placement from camera-dependent culling/LOD.
   The captured frame launches 4.13 million generation invocations, repeating
   placement/coverage evaluation during every moving frame. A bounded cache
   needs explicit source invalidation, streaming lifetime, and population
   equivalence checks. The current stationary reuse cannot cover this case.
2. Reduce grass draw work and improve use of the prepared-blade budget. Check
   which LOD bins consume the bounded arena before changing its allocation
   priority. The low main-pass vertex occupancy and substantial fragment cost
   mean arithmetic reduction alone may not translate into throughput. Consider
   geometry/shader structure and a different distant-grass representation;
   keep any visual tradeoff explicit and compare images with 4× MSAA.
3. Investigate the opaque pass's stored pre-resolve MSAA attachment (32.47 MiB
   flagged by Xcode). Discard is only valid if later rendering does not need
   those samples; otherwise pass combination or a different render path is
   needed. Do not globally change store actions without checking transparency,
   transmission, UI, and custom passes.
4. Implement the bounded terrain material cache described above for the
   ground-heavy workload. The new close-view profile does not invalidate the
   earlier ground-only measurements.

The sustained iPhone acceptance target remains open. Detailed captures, counters,
validation and reproduction flags are recorded in RENDER_AUDIT.md.

### Grass candidate acceptance cache (2026-09-06)

The current implementation stores one source-stable acceptance bit per candidate,
after ownership, growth, terrain validity and coverage/competition tests. Generation
checks that bit before expensive evaluation. Original candidate indices and dispatch
shape are preserved; visibility, topology and population LOD still use the current
camera. Accepted roots retain the existing generation and draw calculations.

An initial compact-ID implementation reduced generation ALU but increased fragment
work in the matched Mac capture. It was replaced by the bitmask; see RENDER_AUDIT.md.

The mask arena is bounded to 1 MiB, plus entry/build-queue metadata. Nearest fields
get priority on source repack. Oversized/unallocated/unready fields, diagnostic
modes and the authored quarter-lattice path retain the reference evaluation.
Builds are limited to four fields and 262,144 padded lanes per frame. Camera/wind/LOD
changes reuse acceptance. Source revisions and compute shader reloads invalidate it,
including changes to peer competition and surface validity.

`--grass-candidate-reference` disables use/building for comparisons. Audit logs
include `candidate_cache`, bytes, cumulative builds and ready/planned field counts.
Source changes currently invalidate the whole acceptance cache. Preserving unchanged
fields across repacks is a possible follow-up if moving-world captures show rebuild
cost is material; the current bounds and reference fallback remain required.

Validation and performance results are recorded in RENDER_AUDIT.md. This iteration
is tested on macOS with display VSync; it does not establish iPhone thermal performance.
