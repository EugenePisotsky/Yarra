# Grass performance assessment, 2026-09-16

**Catalog correction (GP-017):** decoding the actual database proves the measured
baseline was **66 roots/m²**. Earlier 72-root labels came from the RON reference
and a hard-coded HUD label. The runtime payload hash and all recorded power/timing
results are unchanged. See [density diagnostics](GRASS_DENSITY_DIAGNOSTICS_20260916.md).

The target clarified in this investigation is **1440p, 60 fps, mainstream gaming PC, maximum production grass quality**. The current 66-root field is a reasonable candidate to test against that target. It is not a validated maximum, and the Mac's heat does not establish that it is too dense for a PC. No production renderer or quality settings were changed for this investigation.

The useful next investment is a measured grass budget at the target resolution, followed by reducing expensive distant drawing/shading if that budget is exceeded. On the Mac this must include power and frequency context: short HUD durations alone cannot establish efficiency, spare throughput or thermal headroom. The evidence does not justify another round of tiny scheduler optimizations, removing 4x MSAA without a visually acceptable replacement, or restarting the engine.

Follow-up tooling: [controlled profiling runner](GRASS_PROFILING_TOOLS.md) now provides explicit internal resolution, real-time routes, 60/120 fps caps, ordered comparisons, power collection and reports. The user completed the [first powered comparison](GRASS_POWER_BASELINE_20260916.md): 1440p60 used about 5–6 W CPU+GPU, the first 120 run about 20.6 W, and the repeated 120 run encountered thermal pressure and lower clocks. A separate presentation-pacing warning was also identified at 60 fps. The investigation results below remain historical evidence; use the follow-up report for the new measurements and their limits.

Fullscreen correction, September 16: the user clarified that normal play should be
fullscreen. The powered suites used a smaller window, although their world target
was genuinely 2560×1440. References below to normal 1920×1080 rendering describe
the earlier 2560×1440 physical window only. Fullscreen at the game's 75% scale uses
the actual full surface dimensions; the new fullscreen baseline is recorded as
GP-015/GP-016 in the [optimization log](GRASS_OPTIMIZATION_LOG.md).

The [15-minute fullscreen baseline](GRASS_FULLSCREEN_120_20260916.md) subsequently
held 119.946 fps at 2592×1626 world pixels, with 119.897 fps in the final five
minutes and 23.43 W average CPU+GPU power. Thermal pressure was elevated temporarily
around minutes 3–6, then nominal for the rest of the run. Fans were not loud per
the user; case heat was not checked. This establishes near-120 delivery at current
density for that route, and prevents treating the earlier windowed failure as an
absolute ceiling. No grass optimization explains the difference; its cause is not isolated.

The subsequent [grass on/off comparison](GRASS_ON_OFF_20260916.md) held 1440p60
with nominal pressure and better pacing using identical inputs. Grass-on used
4.68–4.91 W CPU+GPU; off used 2.60–3.81 W. The off-visit spread limits precise cost
attribution. Consult the [optimization log](GRASS_OPTIMIZATION_LOG.md) for the latest
conclusions. Draw attribution and a preparation-buffer experiment subsequently
completed as GP-017/018. The [96-root sustained candidate](GRASS_DENSITY96_SUSTAINED_20260916.md)
averages 119.600 fps but has initial thermal pressure and later short stalls;
it does not establish consistently smooth 120 or a mainstream-PC density budget.
The [matched default-buffer control](GRASS_PREPARATION_POWER_COMPARISON_20260916.md)
also stalls. Enlarged preparation reduces GPU duration but has higher observed
power in this pair; it remains opt-in rather than becoming a thermal optimization.

## Current implementation and configuration

Inspected HEAD: `355ee03`. The pre-existing working-tree change is `content/vegetation/canopy-look.ron`; it was preserved. An offline, locked release build completed successfully. Local hardware inspection identifies **MacBook Pro Mac14,6, M2 Max, 38 GPU cores, 12 CPU cores, 32 GB**. This is substantially different hardware from a base M2.

| Item | Current behavior and implication |
| --- | --- |
| Source density | `short_split_fill` is 66 roots/m², versus 44 in the comparison catalog: +50%. The selected species produce paired blades, so the nominal unfiltered maximum is approximately 132 blade forms/m². Coverage, grouping, visibility and LOD reduce actual emissions. |
| Production density | Balanced. Full Reference bypasses distance thinning and the paired-root retention reduction; it is a diagnostic ceiling, not an appropriate definition of a shipping Ultra preset. |
| Topology | Paired high: 15 topology vertex inputs / 13 submitted triangles per root. Paired low: 7 inputs / 3 triangles. These are topology counts, not measured shader invocations. |
| High-detail footprint | Capacity-derived radius, scaled by `HIGH_TOPOLOGY_DISTANCE_SCALE = 0.65`, with stable staggering and morphing. The gameplay focus is an equal-area ellipse centered 2 m ahead of the character, stretched 1.5x along the view direction. |
| Near density footprint | Separate from high geometry. Full near coverage through elliptical distance 12 m, fading to the ordinary density policy by 26 m. Thus shortening high geometry does not also eliminate the dense low-detail roots around it. |
| Distant density | Balanced targets 100%, 55%, 30% at projected root-spacing anchors 6, 2, 0.75 pixels. Away from the near focus, paired-root retention multiplies these by 0.65. Far target is therefore 19.5%, with extra transitional roots retained for width fading. This is already substantial thinning. |
| Range | Approximately 96 m, with conservative blade-reach margins. Grass remains procedural blades throughout this domain. The current canopy effect is shading, not a replacement far representation. |
| Submission | GPU field/frustum/range culling, per-candidate visibility, four indirect indexed draw bins, no entity or conventional vertex stream per blade. No terrain/object occlusion test in the grass scheduler/generator. |
| Placement | Reused while source/view/settings stay unchanged. Ordinary camera movement regenerates. A stable acceptance bitmask avoids some rejected-candidate work. Balanced still visits the full candidate lattice; the scheduler's quarter-lattice path is restricted to Authored mode. |
| Blade preparation | 131,072 physical blades, 128 bytes each; curves and root wind prepared every animated frame. Overflow falls back to vertex calculations rather than removing grass. |
| Allocation | 851,968 root records x 32 bytes = 26 MiB reserved instance storage. Preparation plus lookup/arguments is about 19.25 MiB. These are capacities, not proof that all records are processed every frame. |
| Drawing | Opaque, depth-writing, two-sided geometry; no alpha-card texture/discard path. The shader directly computes foliage lighting, shadow reception, broad GGX, nearby sheen, rounded normals, transmission and canopy shading. Thin-triangle rasterization, surviving fragments and memory traffic remain costs even without alpha cards. |
| Normal game | 75% of physical window dimensions for the world, native UI, 4x MSAA, prepared terrain, display VSync. Desktop depth prepass is enabled for participating scene geometry; the custom grass has no prepass draw. |
| Scripted benchmark | Same 75% world scale and 4x MSAA, but explicitly disables the depth prepass. In today's normal-sized Retina window this means 1920x1080 world rendering and 2560x1440 presentation. A 1440p window is not a native-1440p world test. |
| Shadows | Blade marks default Off in code, despite the older README saying Medium. Grass receives world shadows but is not drawn as an individual-blade shadow-map caster. Canopy treatment is an artistic approximation. |

Relevant implementation: the runtime/project database catalog is authoritative; [RON reference](../content/vegetation/field-current.ron) differs in density and colors, [generation/LOD](../assets/shaders/vegetation_debug_compute.wgsl), [scheduler](../assets/shaders/vegetation_schedule_compute.wgsl), [draw/material](../assets/shaders/vegetation_debug_draw.wgsl), [buffers/pipelines](../crates/vegetation_render/src/renderer.rs), [preparation](../crates/vegetation_render/src/renderer/blade_preparation.rs), [presentation](../crates/app_game/src/game_render.rs), [reproduction overrides](../crates/app_game/src/render_audit/repro.rs).

Current canopy settings reach full distance strength by **1 m**, rather than 20 m in the archived September 15 tests. Height is 0.26 m, blade amount 0.49, patchiness 0.24. The height and boundary early exits still work; the old near-distance early exit now rejects fewer eligible fragments. This identifies a changed workload, not a measured regression or explanation of the heat.

## What the measurements establish

The vegetation catalog payload in today's runtime is **byte-identical** to both September 15 archived runtime catalogs (`shading-perf-01` and `scheduling-preparation-01`). SHA-256: `d75b7fb8cb8113d357f93237bb2c5ea8c7a8f5983270e15ccbae04c359f6942c`. Some earlier reports used less grass, but those two recent investigations already measured the current 66-root catalog.

| Evidence | Result | Meaning |
| --- | --- | --- |
| September 15 shading changes | Low-camera whole-game mean 4.912 -> 4.851 ms; overhead repeat showed no meaningful improvement | Retained small optimization; not a solution to sustained heat. |
| September 15 compact scheduling/preparation | Low-walk mean 4.7052 -> 4.6440 ms; candidate dispatch slots fell 67-69% in fixed views | About 0.061 ms saved, because removed lanes mostly exited cheaply. Reverted at user request. |
| September 15 zoom series | Identical variants drifted by more than a millisecond, with Fair thermal state | Inconclusive comparisons; workload/clock drift matters. |
| Older blade preparation | Less vertex/preparation ALU, but more device-memory traffic; little demonstrated frame improvement | Enlarging the cache is not automatically an optimization. |
| Older 75%/4x sustained Mac run | About 119.6 fps for over 15 minutes, loud fans and thermal pressure | Meeting 120 fps and running hot can coexist. Different historical scene/resolution; not current acceptance. |
| Prepared terrain and MSAA storage | Verified reductions in shader/sample work and unnecessary attachment writes | Already implemented. Old production-ground costs must not be treated as today's baseline. |

Sources: [shading](GRASS_SHADING_PERFORMANCE.md), [reverted scheduling](GRASS_SCHEDULING_PREPARATION.md), [handoff](PERFORMANCE_HANDOFF.md), [ground breakdown](GROUND_GPU_BREAKDOWN.md), [MSAA storage](MSAA_COLOR_STORAGE.md).

Two new sequential runs used today's release binary, runtime and canopy settings, `low-walk`, Balanced density, wind, 4x MSAA, 1920x1080 world resolution, counters off. Both completed 3,600 frames, remained focused, reported Nominal thermal samples, and ended near 120 fps. No other Yarra/Xcode process was found before launching; unrelated desktop activity was not controlled.

| Run | Settled whole-game GPU mean | Median | HUD-sample p95 |
| --- | ---: | ---: | ---: |
| Current 1 | 4.915 ms | 4.91 ms | 5.31 ms |
| Current 2 | 4.838 ms | 4.81 ms | 5.45 ms |

The settled selection uses Metal HUD packet headers 900 through 3500, excluding startup and the frame-600 screenshot. HUD packets contain overlapping/correlated samples; their p95 is not an independently measured frame-time percentile or a confidence interval. The original benchmark summaries, which discard five startup packets, report means 4.909 and 4.821 ms. These are short whole-game durations, not isolated grass timings, fixed-clock throughput, sustained power measurements, or native-1440p results. They do not establish a gain or regression against a different day's build.

A separate fixed `grass-close` counter run confirms **5,945 high + 108,246 low = 114,191 paired roots**, 228,382 blade forms, 1,206,069 submitted indices / 402,023 triangles, approximately 846,897 topology inputs, and **zero capacity drops**. Low roots are 94.8% of emitted roots and account for about 81% of submitted triangles. Preparation handles 131,072 blades and falls back for 97,310. The scheduler reports 4,120,320 padded candidate lanes, which are not 4.1 million visible blades and are not relaunched on every stationary reuse frame.

Raw evidence, hashes, screenshots, settings and settled summaries: [local investigation directory](../tmp/grass-budget-20260916/). Source canopy contents were captured in `inspection.json`; the standard benchmark metadata does not itself hash that source file.

## Comparison with the Tsushima PDF

The [55-page local PDF](../content/gdc_2021_procedural_grass_in_got-2.pdf) was text-extracted and the relevant diagrams rendered and visually inspected. The [existing talk notes](GHOST_OF_TSUSHIMA_GRASS_TALK_NOTES.md) supply separately identified spoken explanation. The PDF does not provide a usable universal roots/m² limit or a grass GPU-time budget; several slides contain placeholders for demonstrations. Exact tile dimensions and a complete performance breakdown cannot be reconstructed from those slides alone.

| PDF evidence | Comparison with Yarra | Practical implication |
| --- | --- | --- |
| Page 11: distance/frustum culling, dropping empty candidates, **occlusion culling** | Yarra has the first categories but no equivalent grass occlusion test | Test conservative patch occlusion for terrain/buildings in a populated scene. It may save little in an exposed flat meadow and has its own cost. |
| Pages 22-23: compute, indirect args, drawing; alternating instance buffers and overlapping work | Yarra follows compute-to-indirect submission but prepares large shared arenas before drawing | Different scheduling/memory strategy. A possible later experiment, not evidence that four draws are excessive or that copying console overlap will work identically through wgpu/Metal. The four-tile/eight-tile numbers come from the spoken notes. |
| Pages 24, 26, 27: 15/7 vertices, nested density reduction, folded pairs | Yarra already uses these principles and the same paired vertex counts | There is no missing order-of-magnitude blade-topology trick here. Avoid treating another small vertex reduction as the main solution. |
| Page 26 diagram: retain one of four roots when merging four tiles | Yarra's far paired target is about 19.5%, plus its fade band; near override is different | Both thin heavily. Compare projected coverage and transition distances, not percentages in isolation. |
| Page 38: pixel shader outputs material data to G-buffers; texture-based gloss/color/vein data | Yarra directly evaluates its full lighting model in the grass fragment shader | Similar geometry does not imply similar shading cost. A cheaper distant material is worth measuring. A deferred rewrite also adds G-buffer/AA/bandwidth costs, so it is not a free win. |
| Pages 42-43: far-LOD texture on terrain, illustrated at landscape/field scale | Yarra has a shaded ground underlayer, but still draws blades to its range limit | A far coverage representation could preserve the appearance of density while removing blade work. The PDF does not specify all of its filtering or handoff details. |
| Pages 45-47: terrain shadow impostor and screen-space shadow examples | Current canopy shading is an approximation, without comparable directional detail | More convincing density does not require every grass blade to cast cascaded or ray-traced shadows. |

Yōtei's newer [GDC ambient-lighting presentation](https://media.gdcvault.com/gdc2026/Slides/Wohllaib_Eric_Ambient_Diffuse_Lighting_in_Ghost_of_Yotei.pdf), PDF pages 78-79, explicitly excludes procedural grass from its ray-tracing BVH and describes grass impostor colors and separate shadow impostors on terrain. That was independently checked from the downloaded PDF. It does **not** publish Yōtei's grass density or prove that its complete grass pipeline is unchanged from Tsushima.

Yōtei's appearance is a valid art target, but its displayed image/FPS does not specify its internal resolution, reconstruction, grass count, frame headroom or grass-only cost. We have not independently established a perfectly locked 60 fps in every current game mode/patch. Do not use that premise to infer a hardware multiplier.

## Translating 120 fps on the Mac to 1440p60

At 120 fps the frame interval is 8.33 ms; at 60 it is 16.67 ms. The same workload requires approximately twice as many frames' work per second at 120. A proper 60 fps cap should generally reduce average workload and heat when there is headroom. It does not halve the inherent work in each frame, and clocks/power do not scale linearly. This investigation did not change the existing display-VSync default or measure a matched 60 fps power run.

Resolution must be included:

| World rendering | Pixel positions/frame | Pixel positions/second |
| --- | ---: | ---: |
| 1920x1080 at 120 fps | 2.074 million | 248.8 million |
| 2560x1440 at 60 fps | 3.686 million | 221.2 million |

1440p60 has 77.8% more pixels per frame but 11.1% fewer per second than 1080p120. For unchanged geometry, per-second geometry work also halves. Therefore today's 120 fps behavior is compatible with a materially easier 60 fps thermal target even at a higher resolution. These pixel counts exclude MSAA samples, overdraw and other passes. Higher resolution can also retain more grass through projected-size LOD, so neither time nor grass population can be scaled by pixels alone.

Do not extrapolate today's approximately 4.9 ms HUD duration into a 1440p time or available grass budget. The user correctly highlighted that dynamic GPU performance states make that arithmetic misleading. Some work is resolution-independent, LOD changes, and the GPU may choose different clocks as demand changes. The short runs establish that the tested presentation target was met in those conditions; they do not quantify unused maximum throughput or sustainable efficiency.

The M2 Max is a capable development GPU, but there is no trustworthy single M2 Max-to-mainstream-PC conversion. Native shader/backend behavior, rasterization, memory architecture, drivers and sustained clocks vary by workload. TFLOPS, core counts and laptop temperature cannot settle that ranking. Treat this Mac as an additional test platform, not a conservative PC performance floor.

For concrete validation, use a named desktop configuration: for example RTX 3060 and RTX 4060 test points, with an AMD RX 6600/7600-family configuration for cross-vendor coverage. These are proposed test points, not equivalence claims or a claim that every one will meet native 1440p maximum quality. [Steam's August 2026 GPU survey](https://store.steampowered.com/hwsurvey/videocard/) shows a broad mix of generations and desktop/laptop parts; the RTX 3060 is the largest individual entry at only 3.92%. There is no single representative “average GPU.”

## Measuring this Mac without confusing clocks with efficiency

Dynamic voltage/frequency changes can make less work run at lower clocks with similar duration. Conversely, a cool or boosted GPU can hide an expensive workload until sustained heat reduces available performance. Milliseconds remain useful in context; FPS and a coarse Nominal thermal label do not hold that context constant. Fan onset is a delayed system-level symptom, not a measurement of grass cost.

The previous Mac evidence already illustrates this: one historical 75% run maintained roughly 119.7 fps at 300-360 seconds while averaging about 998 MHz and 88.9% GPU activity under thermal pressure. Those figures are not today's baseline, but they explain why FPS alone was insufficient.

| Question | Measurement | Tool/method |
| --- | --- | --- |
| Does an implementation do less work? | Vertex/fragment invocations, shader work, texture activity, memory traffic, occupancy and limiting hardware units | Matching Metal frame captures and counters; interpret their tradeoffs together |
| Is it cheaper to run at the target experience? | Same actual 60 fps and image quality, average CPU/GPU power estimates, approximate energy per presented frame, with clocks/activity alongside | `powermetrics` plus time-aligned game/presentation logs |
| Will it stay smooth after warming up? | Sustained presentation intervals, missed deadlines, CPU/GPU timing, power, thermal state and clock trend | A representative 15-20 minute run after bounded candidates show useful improvement |
| How much throughput headroom exists? | GPU-bound repeatable workload at controlled resolution/quality, uncapped throughput with performance-state and power context | Separate throughput test; do not use an uncapped stress run as the normal thermal target |

Locally verified on September 16: `powermetrics --help` lists `gpu_power`, `cpu_power` and `thermal`, and explicitly describes its watt figures as estimates suitable for same-device efficiency work rather than device-to-device comparisons. `xcrun xctrace list templates` includes Metal System Trace, Game Performance, Game Performance Overview and Power Profiler. Availability of a template does not guarantee every metric is supported on this machine. `metalperftrace` was not found in the selected toolchain.

A minimal power capture, run in a local terminal after administrator authentication, is:

```sh
sudo /usr/bin/powermetrics \
  --samplers gpu_power,cpu_power,thermal \
  --sample-rate 1000 --sample-count 120 \
  --output-file /tmp/yarra-power.txt
```

An unattended one-sample check returned `sudo: a password is required`; no new power samples were collected. Do not send an administrator password into chat. The older [power recorder](../tmp/record_mac_power.sh) and [analyzer](../tmp/analyze_mac_power.py) exist, but the recorder launches an older app wrapper and relies on manual settings; verify the wrapper/binary and add deterministic phase control before reusing it as an A/B benchmark.

For a bounded comparison, keep native internal resolution, camera path, wind speed in real time, lighting, quality, focus, power mode, charger/battery condition and background activity the same. Warm up assets before measuring. Compare baseline/candidate/candidate/baseline, discarding transitions, and repeat from comparable thermal starting states. Record an app-closed baseline to estimate unrelated system work, while recognizing that idle subtraction is only an approximation and display composition changes when the app opens. Isolate grass changes while leaving the ground/canopy state intact. Runs must use a real 60 fps render limiter; `PresentMode::AutoVsync` follows the current 120 Hz display and does not provide that limit. The original frame-count reproduction route is frame-driven; the follow-up timed profiling mode now supplies time-based camera/wind playback and an event-loop frame limiter.

Average subsystem watts divided by actual presented frames per second gives an approximate joules-per-frame proxy. For example, equal 60 fps with reliably lower combined CPU/GPU watts is a useful efficiency improvement even if HUD duration is unchanged. These are system subsystem estimates, not precise process- or grass-attributed energy; compare repeated runs on this Mac, not their watt values with PS5/PC. Lower frequency by itself is not success. Lower power caused by missing the FPS target is not success either.

For bottleneck diagnosis, profile identical captured work with the same induced GPU performance state and concurrent execution mode. Apple documents the [performance-state control](https://developer.apple.com/documentation/xcode/optimizing-gpu-performance); it improves comparability but is not a universal live-game frequency lock or immunity to thermal limits. [Per-pass counters](https://developer.apple.com/documentation/xcode/analyzing-apple-gpu-performance-using-counter-statistics) identify where work goes, but counter profiling can serialize passes, so summing its timings does not recreate live overlap. Requested state, actual conditions, identical geometry and repeated measurements all matter. Do not replace one misleading number with another.

The acceptance evidence should combine a useful reduction in the relevant work/bottleneck, lower power at the same target experience or better sustained headroom, and preserved appearance. Small changes must exceed repeat-run variation. Final mainstream-PC acceptance still requires that PC; a carefully measured Mac cannot certify another GPU/backend.

## What to improve, and what not to promise

1. **Measure the current grass contribution at native 1440p first.** Use identical camera, scene, world scale, MSAA, canopy/ground state and shadows with grass drawing enabled/disabled. Also obtain supported GPU pass timestamps or a capture. A whole-scene toggle that changes ground shading, UI or resolution is not an isolated grass cost; incremental differences also include changed visibility of the ground. Measure moving and fixed views, because placement reuse changes their costs.
2. **Prioritize the distant material and representation if drawing dominates.** About 95% of roots in today's close capture are low detail, yet they still use the general material and substantial curve/wind machinery. Test a simpler filtered distant lighting response and then a real far coverage handoff. The previous single widened ribbon and 0.39-retention pair were rejected visually and gave no convincing timing win; do not present them as untried fixes. Preserve silhouettes, openings, clump contrast and motion stability when evaluating a new representation.
3. **Evaluate both near-density reach and geometry reach.** The 0.65 high-detail change only affects a subset of work. Reducing the 12-to-26 m full-coverage override is a separate quality tradeoff. More source density also shrinks the capacity-derived high-detail radius; a density sweep can accidentally trade away geometry reach. Record or hold that reach when comparing.
4. **Profile preparation traffic before expanding its cache.** The current fixed view already exceeds its blade capacity. Larger records/caches exchange repeated arithmetic for bandwidth and storage; earlier preparation results did not demonstrate a meaningful thermal win. Compact or LOD-specific preparation is a hypothesis to test, not a promised saving.
5. **Use occlusion and persistent page placement where the scene justifies them.** Hills/buildings can hide whole patches; continuous traversal can benefit from less source repacking. Neither eliminates the cost of visible grass, and neither is established as the leading cost in the current open view.
6. **Keep 4x MSAA as the visual reference.** Removing it previously caused unacceptable shimmer. Four samples do not automatically mean four full fragment evaluations, and the unnecessary color store is already addressed. Temporal reconstruction could be a future option, but needs grass motion data and motion-quality validation, not just a new AA toggle.

Do not expect major gains from reducing draw calls further, rescheduling already-cheap empty lanes, reserving a larger instance arena, or shading everything unlit and calling the result equivalent. Current capacity fixes prevent missing grass; they do not reduce its rendering cost. No evidence here establishes that Bevy itself imposes an unavoidable grass-density ceiling.

## A usable production budget and decision rule

Suggested **initial engineering budgets**, to validate on the chosen PC at native 2560x1440 rather than treat as measured capabilities:

| Scope | Proposed sustained target |
| --- | --- |
| All grass work, including generation/preparation/draw | Approximately 2-3 ms GPU |
| Ground and grass together | Approximately 5-6 ms GPU |
| Representative complete game | Approximately 12-14 ms GPU at the sustained p95, leaving margin to the 16.67 ms deadline |

CPU/frame pacing needs its own budget; CPU and GPU durations generally overlap and should not simply be added. Streaming and shader-compilation hitches need separate tracking. These grass budgets leave room for characters, trees, their shadows, effects, lighting and future gameplay rather than allowing a meadow to consume the whole frame.

Keep **66 roots/m² near the player as the provisional art target**, with shipping density/geometry/material LOD. “Maximum quality” should still use LOD. There is currently no defensible global maximum root or triangle count for mainstream 1440p60. A mostly hidden or subpixel blade and a large screen-covering blade have different costs; distance, view angle, width, overlap and material are part of the limit.

The next bounded test should compare 44, 66, 72, 96 and 128 roots/m² at a fixed camera path and explicit near-detail reach, using the same ground/canopy treatment where density is the variable. Include a low view, overhead, fast rotation and traversal into fresh pages; keep timing runs separate from counters/captures. Record emitted roots/triangles, material and GPU-pass cost, frame pacing, capacity drops, actual internal resolution and memory. Select the highest visually useful density that stays within budget in a representative scene, then perform a 15-20 minute sustained 60 fps run. A shorter focused 120/60 comparison on the Mac can separately answer the heat question. This matrix and native-1440p PC acceptance have not yet been performed.

If the current field already fits, use additional budget on the world and better field appearance before increasing density everywhere. If it does not fit, reduce the cost of unresolved distant grass before lowering the nearby density that the player can actually see.
