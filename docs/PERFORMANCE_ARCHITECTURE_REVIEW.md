# Performance architecture review — 2026-09-06

The rendering workload needs a substantial reduction before expanding the game. The evidence supports keeping the world/data foundation and revising the rendering policies. It does not establish that another project restart or an engine migration would solve the problem.

**Follow-up, 2026-09-07:** the [current-frame GPU breakdown](GROUND_GPU_BREAKDOWN.md)
confirms that ground fragment shading dominates and the prepared material reduces
its work substantially at native resolution with 4× MSAA. The earlier nearly
unchanged HUD durations did not justify rejecting that implementation. Memory
and visual acceptance remain open; the candidate stays opt-in.

This review covers the dirty `feat/vegetation` working tree at `5a1bf35`, compared with `main` at `c85de48`. It independently checks source code, the authoring/runtime databases, and existing capture/log artifacts. **No new live benchmark or matched `main` runtime comparison was performed.** Historical GPU timings below are identified as such; source inspection alone cannot establish a speedup or the complete cause of thermal deterioration. No production code was changed during this initial review. Subsequent implementation and validation are recorded in [Prepared ground](PREPARED_GROUND.md).

The [performance handoff](PERFORMANCE_HANDOFF.md) remains the record of earlier experiments and their limitations. The new database/source comparison results are recorded in [source-comparison.json](../tmp/performance-review/source-comparison.json).

**1. The expensive ground material already exists on `main`. This is now verified beyond the shader alone.**

`git diff main HEAD -- assets/shaders/terrain_material.wgsl` is empty. Both files have SHA-256 `10cdbddf4238a552a75ad62395eba4be5fb84e1cc2b8965bdb89f2141c7e50f7`. The working-tree changes add isolation variants and the stochastic-transform cache while retaining the original material computation.

The tracked `main` authoring database was extracted into a separate scratch file and opened read-only. Compared with the current source and runtime databases:

| Data checked | Result |
| --- | --- |
| Terrain surfaces: tiling, anti-tiling, normal strength, roughness | All rows identical |
| Terrain profiles: macro scales/contrast/strength, weight resolution | All rows identical |
| Texture-set metadata and layer assignments | All rows identical |
| Source cell surface slots | All 8,273 rows identical |
| Source cell weight pages | All 4,096 rows identical |
| Resolved Bevy / wgpu versions in `main` and HEAD | Both 0.19.1 / 29.0.4 |

Texture metadata equality does not establish that historical, externally generated texture files had identical bytes. Nevertheless, there is no identified new expensive ground-material feature or dependency upgrade in this branch.

The branch changes the surrounding workload: desktop terrain uses 33×33 heightfield meshes; the old ground used planes. Terrain and vegetation source residency now use a rotation-independent 7×7 shell. The sun is lower, fog is enabled, and the closest camera pitch changes from 18° to 10°. The current iOS path explicitly substitutes flat planes even for relief terrain, so the phone's ground-only failure cannot be blamed on the new heightfield triangles.

The remembered improvement on `main` remains plausible. Its exact cause remains unresolved, and establishing it is not required to proceed. The user explicitly rejected spending further time on a historical runtime comparison. Grass-covered ground is a different workload from ground filling the view: the old cards had different screen coverage and an explicit depth pass. Those differences could change how much expensive ground shading survives, but their effect has not been measured here. The source comparison is retained as context, not as a reason to pursue a historical investigation.

Sources: [terrain shader](../assets/shaders/terrain_material.wgsl), [terrain attachment](../crates/engine/src/world_streaming.rs), [atmosphere](../crates/atmosphere/src/lib.rs), database comparison artifact above.

**2. Ground has a high per-pixel cost and no material level of detail.**

For a two-surface page with anti-tiling and macro variation enabled, the selected material paths use three albedo samples and one packed normal/material sample per surface, one weight sample, and three macro samples: **12 logical texture samples before lighting and shadows**, plus blending, normal reconstruction and stochastic coordinate calculations. This is a source-level count, not a hardware sampler-transaction count or a claim about removal of the initial plain sample by the compiler. The sampler permits 8× anisotropy. Mipmaps reduce texture-detail cost; they do not remove the two material evaluations or the remaining fragment math.

There is no distance-based material simplification. The recent cache replaces stochastic hashes with buffer reads; it leaves the texture sampling, blending and lighting largely intact. Consequently it attacks only part of the cost.

Existing physical-phone evidence establishes that production ground is costly even with vegetation disabled. The ground-only log reaches about 33 ms GPU duration and 44 FPS after heating. Conversely, the separate lightweight baseline measured clear/no-UI around 1.0 ms and flat ground around 1.36 ms. These are different experiments and must not be subtracted to invent a production-material timing. They do establish that a basic ground draw is feasible and that expensive scene rendering, rather than the existence of a ground mesh, deserves attention.

**Recommended direction:** prototype an inexpensive *lit* terrain material with a documented sample budget and distance-based detail. Preserve the important color, blend and shadow cues, and compare close and grazing views. The existing flat and single-texture unlit modes are diagnostics, not finished visual alternatives. Choose whether anti-tiling/macro work belongs in authored textures, a bounded bake, or a near-detail shader from that comparison. A full virtual-texture/material-cache system is a larger commitment whose necessity has not yet been established.

Sources: [surface sampling and fragment material](../assets/shaders/terrain_material.wgsl), [sampler](../crates/terrain_render/src/lib.rs), [ground evidence and failed experiments](PERFORMANCE_HANDOFF.md).

**3. Grass lacks an inexpensive distant representation, and its draw work is already a major budget consumer.**

The current renderer retains procedural blades to a 96 m cutoff. Balanced density approaches a 30% far plateau, with a transition band that can keep additional roots alive while their widths contract. It reduces geometry and density, but never transitions the field to a different coverage representation. The shader still constructs curves/ribbons and evaluates substantial foliage lighting for distant grass. The production fragment path includes rounded normals, specular filtering/lobes, transmission and received shadows. The four topology/LOD draws share one shader variant; topology, lighting modes and diagnostic behavior are largely runtime branches.

The old implementation used textured clumps, a much simpler color/shadow fragment path, and an explicit alpha-tested grass depth pass followed by depth-equal color rendering. The current grass participates in the opaque color phase and has no grass prepass. Turning on the camera's `DepthPrepass` does not restore the former grass path. This is a consequential architectural difference, but reinstating a prepass is an experiment: it also adds geometry and attachment work, and its net value must be measured on the target GPU.

In the existing low-camera A17 Pro capture, grass vertex, fragment, generation and preparation work together account for about 68% of the frame. Terrain fragments account for about 15%. These percentages describe that view, not every scene. They show why a terrain-only optimization cannot recover the whole low-camera budget.

**Recommended direction:** budget detailed blades for the near field and compare cheaper clump/coverage representations for the middle and far field. The old renderer is useful as a measured reference and a source of representation ideas. The current data model can be retained. Validate the visual transition and shimmer with 4× MSAA available; a lower triangle count or fewer draw calls alone is insufficient evidence of success. Specialize expensive shader features by representation where useful, instead of carrying the complete diagnostic/general shading path through every production draw.

Sources: [density policy](../assets/shaders/vegetation_debug_compute.wgsl), [scheduler cutoff](../assets/shaders/vegetation_schedule_compute.wgsl), [grass drawing](../assets/shaders/vegetation_debug_draw.wgsl), [pipeline setup](../crates/vegetation_render/src/renderer.rs). Historical comparison: `git show main:assets/shaders/ground_cover.wgsl` and `git show main:crates/ground_cover/src/renderer.rs`.

**4. Moving-camera generation and streaming remain unnecessarily coupled to stable placement.**

The stationary reuse key includes the camera transform/projection. Ordinary movement rebuilds generated instances. The acceptance cache rejects known failures, but accepted candidates still enter the original evaluator and reconstruct placement/surface/coverage information before view-dependent decisions.

The scheduler produces a rectangular dispatch: its X dimension is the largest candidate count among visible fields and Y is the number of visible fields. Shorter fields launch excess lanes that return immediately. In default Balanced mode the scheduler explicitly disables its quarter-lattice shortcut. Density reduction therefore does not proportionately reduce dispatched work. Existing captures report roughly 3.33 million generation invocations on Mac and 4.13 million on iPhone; those include padding and are not counts of visible blades.

Additionally, a streamed-page change rebuilds the whole joined vegetation scene, repacks all source arrays and invalidates the acceptance cache. The Mac capture log records 51 source repacks reaching 49 resident pages and a final source upload of roughly 1.97 MB. That is evidence of coarse invalidation/startup churn, not proof of a stationary bottleneck. Continuous traversal deserves its own measurement.

**Recommended direction:** stable page-local placement records or another compact persistent representation, source invalidation per changed page, and small spatial batches for view culling/LOD. Measure savings against added storage and reads. The rejected compact-ID experiment already demonstrates that less compute can still lose at whole-frame level. Because drawing dominates the captured grass workload, fix the representation/draw budget alongside this work rather than expecting a placement cache to solve heat by itself.

Sources: [scene joining](../crates/app_game/src/main.rs), [packing and generation](../crates/vegetation_render/src/renderer.rs), [candidate cache](../crates/vegetation_render/src/renderer/candidate_cache.rs), [capture log](../tmp/mac-mask-cached-capture.log).

**5. The world foundation is useful, but “bounded” is not a rendering budget.**

The separation of authoring, cooking, immutable runtime data, asynchronous database/decode work, page attachment/removal, and floating origin is appropriate to keep. Vegetation also avoids one entity per blade. Inspection does not reveal a whole-world simulation loop or stationary page-growth mechanism explaining the ground-only GPU failure. Bevy's resource extraction is change-gated; the scene's derived `Clone` does not by itself mean the entire scene is copied every frame.

The 64 MiB decoded / 256 MiB estimated-GPU admission limits apply to streamed page accounting. They are not total process/GPU limits and do not include all render targets, MSAA/depth/shadow attachments, renderer instance/preparation caches or driver allocations. A bounded 344,064-instance arena can still be far too costly per frame. The existing Mac HUD reports around 359 MB graphics memory in the short cache comparison despite the 256 MiB page-estimate limit; that is compatible with the accounting scopes, not evidence that admission checks failed.

Forest scaling also needs validation. Static objects currently load handles for every LOD, use an individual world-asset root per object, and select LODs on the CPU every frame; changing LOD changes the scene asset at that root. Shared asset handles and Bevy batching help, but this is not yet evidence of an efficient dense forest. A representative forest patch should validate instance grouping, LOD transition work, foliage coverage and shadow cost before filling the world. Terrain geometry itself currently always requests LOD 0.

Sources: [streaming admission, attachment and LOD](../crates/engine/src/world_streaming.rs), [renderer capacities](../crates/vegetation_render/src/renderer.rs), [Mac comparison](../tmp/mac-mask-live-summary.json).

**6. Performance acceptance has to govern further work.**

The previous changes have useful correctness tests and some lower counters, but they have not established the required sustained phone performance. For example, blade preparation reduces arithmetic while increasing device-memory traffic; the latest candidate bitmask changes the measured Mac frame by only about 0.05 ms in one short comparison. Those results do not justify calling the thermal problem fixed.

The normal launch, interactive audit settings, and scripted repro are also different configurations. The repro selects 75%, 4× MSAA and no prepass; at the time of this review ordinary startup selected native resolution and MSAA off, with desktop prepass enabled. The subsequent prepared-ground iteration changes normal startup to 4× MSAA. Interactive audit selections are not a persistent device quality profile. Every comparison needs the actual configuration, source/build identity and camera recorded.

**User direction: do not spend further time benchmarking against `main` or reconstructing the historical regression.** Existing current-branch measurements already establish unacceptable costs and justify implementation work. Keep verification focused on the current scene: fixed cameras/settings, a short before/after check for each substantial change, and visual comparison. Reject changes whose benefit is negligible. Use longer thermal acceptance only after a candidate demonstrates a useful reduction in the current workload; short checks cannot establish sustained success.

Prioritize the inexpensive lit-ground prototype and near/middle/far grass comparison directly. Set provisional environment budgets below half the frame deadline to leave room for gameplay: approximately 6–8 ms at 60 Hz on the phone and 3–4 ms at 120 Hz on the Mac are starting targets to validate, not measured capabilities or guaranteed thermal limits. For promising candidates, use sustained runs long enough to pass the previously observed 1–2 minute deterioration, then a 10–15 minute acceptance run with camera movement. Record FPS, GPU time, thermal behavior and memory; record power where available. Test without phone mirroring or active GPU replay. Keep visual references and 4× MSAA comparisons in the acceptance criteria.

**7. Anti-aliasing should have a small set of supported modes and a separate temporal-integration plan.**

**User requirement: anti-aliasing is mandatory. The current 4× MSAA result is the minimum acceptable visual reference; insufficient AA looks unacceptable even at native resolution.** Include 4× MSAA in the normal optimization baseline and environment budget. Off and 2× remain diagnostic controls, not accepted production quality targets. A performance improvement obtained by going below that visual floor does not satisfy the task.

Retain **MSAA 4×** as the current accepted AA method. SMAA, FXAA or TAA may become production alternatives only if they meet the visual reference in motion, including wind, camera movement, low views and fine foliage. Lower cost alone is insufficient, and no alternative has yet demonstrated visual acceptance here. This is a proposed policy, not newly implemented support. Keep the selected method mutually exclusive and render scale separately configurable, apply AA to the world camera, and preserve full-resolution UI. Use the same policy in the game and editor and log/persist the actual selection. Benchmark renderer/material/representation changes with the accepted AA configuration active; use AA-off measurements only for diagnosis.

MSAA cost depends on edge density, attachment formats, sample count and render-pass lifetime. Four samples do not imply four complete shading executions for every pixel. Apple GPUs can resolve samples within tile memory and avoid storing the multisample surface. Our Bevy 0.19.1 color-attachment helper instead uses `StoreOp::Store` with a resolve target; the previous iPhone capture flagged 32.47 MiB of stored pre-resolve MSAA data. That is a concrete integration cost to consider. Removing it requires accounting for subsequent passes that consume the samples; it is not safe to change the store action globally. See [Apple's MSAA/render-pass explanation](https://developer.apple.com/videos/play/wwdc2020/10602/).

The existing short phone comparisons also show view-dependent cost: 4× barely changed duration in one cool overhead view, while the hot low-view comparison differed by about 3 ms. The slowdown already existed with AA off. These observations neither establish that MSAA is free nor make it the sole cause of overheating.

SMAA is a spatial image filter with lower bandwidth requirements than MSAA in its intended use, better edge reconstruction than FXAA and greater processing cost than FXAA. It does not accumulate previous frames; it cannot reliably recover grass that repeatedly disappears between pixel samples. Bevy's current implementation is the non-temporal variant. See [Bevy's SMAA documentation](https://docs.rs/bevy/latest/bevy/anti_alias/smaa/index.html). FXAA can soften fine detail, so it should not be assumed visually equivalent to the existing 4× result.

TAA can address temporal and shading aliasing, but its integration has prerequisites. The installed Bevy implementation requires depth, motion vectors, jitter and MSAA off. The custom vegetation path currently writes neither prepass depth nor motion vectors, and its raster position uses the unjittered custom camera matrix rather than Bevy's jittered view uniform. Enabling the camera component alone would therefore leave incompatible grass inputs. See [Bevy's TAA requirements](https://docs.rs/bevy/latest/bevy/anti_alias/taa/struct.TemporalAntiAliasing.html).

Before exposing TAA, vegetation needs matching depth/motion output for camera motion and wind deformation; rasterization must use the shared jitter while culling/LOD remain stable; history must reset or reject appropriately on cuts, streaming and representation changes. Previous motion must follow the same blade/root identity, not an atomic append slot whose ordering can change. Test ghost trails, thin-blade loss and blur in motion. The complete prepass/history cost must be included in any comparison. A temporal upscaler would have similar integration requirements and is a later decision.

AA does not replace filtering the scene itself. The grass representation, stable density/topology transitions, texture mips/alpha coverage, and specular filtering must reduce the sources of shimmer. In particular, moving to cheaper grass cards changes the AA problem: texture-cutout edges need suitable mip/coverage handling; polygon-edge MSAA alone does not provide that automatically. Evaluate AA alongside the new grass representation rather than assuming the current 4× appearance will transfer unchanged.

The immediate deliverable should be a small environment that meets an explicit sustained budget with useful headroom. The existing world/editor architecture can carry that work. More exact-output arithmetic caches or another complete restart should not substitute for demonstrating it.
