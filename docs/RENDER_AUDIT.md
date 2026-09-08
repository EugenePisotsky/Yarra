# Rendering performance audit — 2026-09-05–06

**Start with [PERFORMANCE_HANDOFF.md](PERFORMANCE_HANDOFF.md) for the current status,
failed experiments, corrected conclusions, and artifact map.** The sustained iPhone
frame-rate/heat problem remains unresolved. This file is a chronological record:
early findings, defaults, capture instructions and proposed next steps describe the
code at that point and may be superseded below. In particular, stationary placement
reuse is now implemented, iOS diagnostic atomics default off, and programmatic
capture now supports iOS and uses extracted application frame 600. Reduced arithmetic
and passing correctness tests do not establish a sustained performance improvement.

**Repeated AA comparison (`log8.txt`): 4x has essentially unchanged GPU
duration in the fixed, high-camera Nominal view.** At 50%, the sequence is Off
10.26 ms → 4x 10.33 ms → Off 10.34 ms → 4x 10.34 ms, all 59.99 FPS. The two 2x
visits last only 0.587 and 0.250 seconds and are not settled measurements. This
reinforces that log7's hot/low-view difference is not a general 3 ms MSAA cost.
Unchanged duration does not establish unchanged energy use or sustained thermal
performance, and this log does not explain the difference between those views.

**Earlier combined-scene run (`log7.txt`): 4x MSAA has an additional cost, but the
thermal slowdown starts before it is enabled.** At 50% resolution, the stable
Nominal high-camera comparison is AA off 10.20 ms / 60 FPS and 2x 10.53 ms /
60 FPS. The phone reaches Serious at 104.355 s while still using 2x, then slows
with an unchanged low camera before 4x is first enabled at 114.240 s. In that
same low view, settled 4x is 25.10 ms / 47.43 FPS and a subsequent AA-off return
is 22.01 ms / 52.06 FPS. The observed 3.09 ms / 14% difference is a short hot
comparison, not a fixed AA cost or a thermal-controlled benchmark. AA off still
misses 60 FPS there. The user finds 4x visually good. See the detailed run below.

**Render-path correction (`log6.txt`): the difference is workload-dependent.**
Current with grass Full measures Direct 10.12 ms → Composite 10.34 ms → Direct
10.12 ms. The observed difference is only 0.22–0.23 ms, not the 2.3 ms seen in
Clear. Do not treat the Clear difference as a fixed pass cost or a demonstrated
thermal improvement. The reason for the different response needs per-pass GPU
timing, not subtraction of whole-frame durations from different workloads.

**Earlier Clear comparison (`log5.txt`): the measured empty-scene baseline was
strongly affected by debug rendering.** Clear falls from 5.07 ms with composite/UI to 2.70 ms with
direct/UI, then 0.97 ms with direct/no UI. Flat ground with direct/no UI is
1.36 ms. Return measurements reproduce the composite and UI costs. The audit
now defaults to Direct at native resolution; scaled rendering still uses the
explicit Composite path. This resolves the shared baseline question, not the
original production-ground thermal slowdown. See the automated comparison below.

The target is sustained **60 FPS** on iPhone 15 Pro Max. The ground-only capture
(`logs3.txt`) shows warmed ground around 44–45 FPS, while Clear returns to 60 at
the same reported Serious thermal state. The material comparison
(`log4.txt`) separates production (11.67 ms), surface unlit (10.51 ms), one texture
(8.60 ms), and flat color (7.74 ms), all at Fair and 60 FPS. The production return
is 11.77 ms. These short holds do not show sustained thermal performance; the
user confirms production would slow down again if held longer.
The subsequent pass comparison (`logs4.txt`, a different file) reduces Flat
from 7.60 to 6.48 ms by disabling shadows, then to 5.23 ms by disabling the
depth prepass. Clear with both disabled is 4.69 ms. The basic ground draw adds
only about 0.55 ms in this comparison; the subsequent `log5.txt` comparison
attributes most shared cost to audit/UI rendering. The iOS camera defaults to prepass off, with the audit
toggle retained. Sustained production performance after that change is untested.
See the comparisons below; grass-only findings do not establish a global
optimization priority.

The reported grass-only
build holds 60 for several minutes in Release, both connected and disconnected,
then degrades as the device heats. Those observations are consistent with thermal
pressure, but neither touch temperature nor the FPS label identifies the limiting
processor. The subsequently supplied stationary grass-only Metal HUD capture is
analyzed below; it records GPU duration and memory, but no thermal-state readings.

The analysis below uses the current working tree (including the existing iOS
experiments), Bevy 0.19.1 / wgpu 29.0.4, and the local cooked runtime database.

## Findings and priority

1. **The ground is a substantial fullscreen shader workload.** Every overworld
   page in the cooked database has two material layers, both with anti-tiling.
   The shader combines three albedo samples per layer, one packed normal/AO/
   roughness sample per layer, three macro samples, and one blend-weight sample:
   12 intended texture samples before PBR, shadows, fog, and tonemapping. There
   is also an initial plain albedo sample overwritten by anti-tiling; whether
   the compiler eliminates it needs shader inspection. Both layers are evaluated
   even where a blend weight is zero. Surface sampling uses up to 8× anisotropy;
   shader sample calls are not equivalent to individual physical texture taps.
   See `assets/shaders/terrain_material.wgsl` and
   `crates/terrain_render/src/lib.rs`.

2. **Ground and grass use different shadow filters.** The camera has no explicit
   `ShadowFilteringMethod`, so ordinary PBR terrain uses Bevy's default Gaussian
   directional filter, with nine comparison samples. Grass explicitly compiles
   with `SHADOW_FILTER_METHOD_HARDWARE_2X2`. A mostly ground-filled image can thus
   cost more in shadow reception than grass, independently of mesh complexity.
   The light also renders three cascades, using Bevy's default 2048² shadow-map
   size. Three cascades do not mean every fragment samples all three: cascade
   selection/blending and shadow-map rendering are separate costs.

3. **The 1280×720 window declaration is not an iPhone render budget.** iOS
   switches to the fullscreen device surface; there was no separate 3D render
   resolution. Native 2796×1290 is 3.61 million pixels, approximately 3.91× 720p.
   At 60 FPS that is about 216 million screen pixels per second, before overdraw,
   shadow maps, and other passes. Confirm the actual size in the new audit panel.
   `AutoVsync` and the plist's high-refresh opt-in do not impose a 60 FPS limit.
   The observed current rate is 60; do not assume 120 from the display's maximum.

4. **Grass placement is regenerated every frame, including when stationary.**
   The scheduler, candidate generator, and finalizer run each frame. Generation
   performs grouping, terrain interpolation, occupancy, species selection,
   several projections, and LOD decisions before many candidates are rejected.
   Balanced production mode does not use the scheduler's direct quarter-lattice
   dispatch: that path is restricted to authored density. Drawing fewer roots
   therefore does not imply proportionally less candidate-generation work.
   Wind is applied in the vertex shader; it does not by itself require rebuilding
   static root placement. Splitting persistent roots from view-dependent LOD is
   a possible later optimization, contingent on measurements.
   Four indirect draw calls can still contain hundreds of thousands of procedural
   instances and millions of vertex invocations; a small draw-call count is not a
   reliable measure of grass cost. The current arena capacity is 344,064 instances,
   which is an upper bound, not a measured visible-instance count on the phone.

5. **Diagnostic atomics remain in the production grass compute path.** Every
   evaluated candidate increments the same global counter, and every eligible
   candidate increments another counter, in addition to the required draw-append
   atomic. iOS readback and render timing diagnostics were already disabled, but
   the shader still performed these writes. Their cost is unmeasured; the new
   counter switch isolates it without disabling the required dispatch/draw
   counters. The default remains on for comparable measurements.

6. **A real scheduler bounds defect was found and fixed.** Resident buffers grow
   geometrically and retain their capacity. The scheduler dispatched a rounded
   number of 64-thread groups but checked `arrayLength(work_items)`, which reflects
   allocation capacity, rather than the current live count. After residency
   shrinks, spare threads in the final group could schedule stale page records.
   Those records can refer to retired species/coverage/surface data and add ghost
   work. The shader now checks an explicit live count. This is a correctness fix,
   not evidence that it caused the stationary thermal slowdown. An explicit GPU
   regression test exercises a retained 64-record allocation with live counts of
   64, 5, and 0, and verifies dispatch still works with telemetry disabled.

7. **The current visually sparse scenes are not empty-engine baselines.** Hiding
   terrain suppresses its draws but still loads terrain materials and textures.
   The character, its animation, streamed objects, sun/shadows, depth prepass,
   UI, and streaming remain. The custom grass draw only participates in the
   opaque pass, so the depth prepass does not pre-render the grass itself.
   Depth prepasses can help with occlusion but also add passes and memory traffic;
   their value must be measured on this mobile scene.

8. **Lower-priority CPU churn exists, but is not yet a demonstrated bottleneck.**
   Page demand and residency statistics are rebuilt each frame. Vegetation scene
   conformance allocates/sorts page signatures and clones the catalog before its
   unchanged-scene early return. Some unchanged transforms are written every
   frame. In contrast, large vegetation packing/uploads are revision-gated, and
   Bevy resource extraction clones the scene only when it changes. The source
   repack counter should settle while stationary; there is no basis here to
   claim repeated full-scene upload or an unbounded leak.

The texture assets pass basic checks: iOS selects ASTC files, each texture is
1024² with all 11 mip levels, and the shared terrain set's recorded GPU footprint
is about 3.67 MiB. MSAA is already off. No explicit HDR camera, SSAO, bloom,
screen-space reflections, or atmosphere raymarching was found in the game setup.

## Stationary grass-only capture: `tmp/logs.txt`

The user supplied this iPhone 15 Pro Max / A17 Pro capture from the default
grass-only scene, without moving the camera. The log does not include settings
changes or a startup audit-settings marker, so it cannot independently establish
whether the audit composite was active or identify the exact build revision.

The 576 HUD lines contain 288 distinct packets, each repeated exactly twice.
Calculations remove duplicate packets but preserve all timing pairs inside each
packet. Many timing pairs also repeat. Header frame counters advance by roughly
half the number of timing pairs, rather than matching that number. Together with
the duplicate HUD metric-registration warnings at lines 14–18, this makes absolute
time/frame reconstruction unreliable. The chart therefore uses **report order**,
not seconds. These anomalies do not by themselves prove duplicate rendering.
Line 19 also warns about accessing a drawable texture after presentation. Check
whether this warning persists with HUD disabled before attributing it to the
game/wgpu presentation path; the log alone cannot identify its source.

Selected windows, excluding shader startup and transition periods:

| Phase (report numbers) | Mean interval-derived FPS | Mean reported GPU ms | GPU p95 ms |
| --- | ---: | ---: | ---: |
| Early steady state (20–47) | 59.99 | 10.56 | 11.36 |
| Later, still at 60 (55–160) | 59.99 | 14.94 | 15.28 |
| Degrading (181–210) | 56.35 | 21.36 | 24.09 |
| Final portion (241–288) | 50.18 | 23.56 | 26.32 |

GPU duration starts rising around report 48 (source line 220), well before FPS
changes. The first late-run missed presentation intervals occur at report 163
(line 450); the final phase has 24.3% of intervals above 16.68 ms. The step-like
timing increases with a stationary view support thermal/frequency pressure as a
hypothesis. No thermal state, GPU clock, or encoder-busy-time data was captured,
so this does not establish thermal throttling or attribute cost to a shader.
Standard HUD GPU duration can include idle gaps between encoders; it is not a
measurement of pure shader execution time. See
[Apple's GPU timing explanation](https://developer.apple.com/documentation/xcode/monitoring-your-metal-apps-graphics-performance/).

Two other observations require separate investigation:

- **Process memory continues growing:** from 647.94 MB at report 20 to 772.94 MB
  at the end (+125 MB). Graphics allocations move only from 325.42 to 326.75 MB
  (+1.33 MB). This warrants an Allocations/VM Tracker recording during stationary
  play. It does not establish a leak, identify an owner, or exclude driver/HUD/
  diagnostic retention. These memory measures must not be added together.
- **There is a separate startup stall:** report 12 (line 148) contains a 3384.09 ms
  presentation interval, immediately after a Thread Performance Checker warning
  (line 112). Its backtrace includes `write_indirect_parameters_buffers`, Bevy
  task-pool scoping, and `renderer_extract`: a User-interactive thread waits on
  Default-QoS work. This is a concrete CPU scheduling concern, but the single
  early warning does not establish the cause of the later sustained slowdown.
  Shader compilation warnings also occur only near startup in this capture.

For the next run, disable Thread Performance Checker for measurement or launch
through Profile/Instruments, following
[Apple's profiling guidance](https://developer.apple.com/documentation/xcode/diagnosing-performance-issues-early).
Enable Metal HUD through one configuration path and check whether the duplicate
warnings/packets persist; no cause for the duplication has been established.
Keep console timestamps and record the displayed thermal state and audit settings.

At the warmed, stationary view, collect Full → Frozen draw → Full (Reset baseline)
→ Clear, with 15–20 HUD packets per setting after pipelines settle. Full/Frozen
isolates vegetation placement compute; Clear measures the common engine/render/UI
floor. If Frozen has a substantial benefit, prioritize persistent grass roots
and generation counters. If it does not, compare frozen grass at 100% and 50%
resolution to investigate draw/pixel cost before changing placement logic.

Local artifacts (under ignored `tmp/`):

- [`metal-analysis/timeline.png`](../tmp/metal-analysis/timeline.png)
- [`metal-analysis/reports.csv`](../tmp/metal-analysis/reports.csv)
- [`metal-analysis/summary.json`](../tmp/metal-analysis/summary.json)
- [`analyze_metal_log.py`](../tmp/analyze_metal_log.py), which preserves the source
  and records its SHA-256 for reproducibility.

## Grass mode comparisons: `tmp/logs2.txt`

This capture contains five `RENDER_AUDIT` markers, at source lines 418, 489, 650,
753, and 902: Full → Frozen draw → Compute only → Schedule only → Disabled → Full.
All markers retain Current scene, production shading, shadows, depth prepass,
100% resolution, counters, and wind. Controls are locked from Frozen onward.
There are 920 HUD lines / 460 distinct packets / 27,366 retained timing pairs;
each packet is duplicated twice. The same HUD registration warnings and header
frame-count discrepancy remain, so the chart still uses packet order.

The user reports that the on-screen thermal state became **Serious** early and
stayed there through the end, including when FPS recovered after mode changes.
The CSV itself still contains no thermal values. This establishes reported system
thermal pressure during the test; it does not provide temperature, GPU frequency,
or the exact time of the initial state transition. Apple's definition of Serious
includes performance-reducing thermal mitigation; the category is not a fixed GPU
clock. See [Apple's thermal-state documentation](https://developer.apple.com/documentation/foundation/processinfo/thermalstate-swift.enum/serious).

The important comparisons are short windows around mode changes and whether each
mode stays stable, rather than one average over the whole run:

| Window | Reports | Mean FPS | Mean reported GPU ms |
| --- | --- | ---: | ---: |
| Full immediately before Frozen | 172–181 | 56.21 | 21.31 |
| Frozen, first 10 packets after excluding 3 transition packets | 185–194 | 59.99 | 13.50 |
| Frozen, last 10 packets | 207–216 | 57.39 | 22.28 |
| Compute only, excluding first 3 packets | 220–296 | 59.96 | 8.86 |
| Schedule only, excluding first 3 packets | 300–347 | 59.99 | 7.56 |
| Disabled, excluding first 3 packets | 351–421 | 59.99 | 7.47 |

**Within the grass-only experiment, drawing is the stronger priority.** Disabling generation
initially reduces reported GPU duration by about 7.8 ms and restores 60 FPS, so
generation is not free. But Frozen draw still degrades while the generated roots
remain fixed. Compute only restores 60 FPS and stays near 8–10 ms after dropping
the grass draw, despite Serious remaining on screen. In the code, Frozen skips
the scheduler/generator/finalizer, while Compute only retains those passes and
removes the vegetation draw from the opaque phase. This argues against placement
compute alone explaining the sustained failure. It does not yet distinguish
vertex deformation, geometry/rasterization, fragment lighting, or overdraw.

The observed total-duration difference from Compute only to Schedule only is
about 1.3 ms, and Schedule only to Disabled is about 0.09 ms. Treat those as
sequential comparisons, not isolated pass timings: clock/thermal state can evolve,
and command-buffer GPU duration can include idle periods. The scheduler is not a
promising first optimization based on this capture. Persistent roots may still
reduce energy, but that change alone would not explain or cure Frozen's slowdown.

**There is also a roughly 7.5 ms reported floor with grass GPU work disabled.**
Disabled retains the engine, character/other meshes, shadows, prepass, the native
render target, and the audit UI composite. CPU-side vegetation preparation and
small configuration uploads are not disabled by that switch either. This is not
a minimal Bevy baseline or proof that a blank draw consumes 7.5 ms of GPU busy
time. A Clear comparison and encoder timings can separate the remaining work.

**Memory growth continues without grass GPU work:** during Disabled alone,
process memory rises from 788.02 to 815.36 MB (+27.34 MB), while graphics memory
stays exactly 333.47 MB. This narrows the investigation away from grass GPU
scheduling/generation/drawing as a necessary cause. CPU preparation, other engine
systems, allocations retained by the allocator/driver, and diagnostics remain
possible owners. An allocation profile is required to identify the cause.

Returning to Full initially produces about 10.5 ms / 60 FPS again, then includes
a distinct transient spike (maximum reported GPU duration 77.87 ms; report 450
averages 19.16 FPS) before returning to roughly 21–27 ms at the end. There is no
settings marker at that spike. Do not summarize this nonstationary segment as a
single stable Full cost or attribute the spike without further tracing. The
separate startup priority-inversion warning also recurs, followed by a 4317.64 ms
presentation interval near the beginning of the file.

Next, keep Frozen draw and compare **production vs unlit at 100%**, then restore
production and compare **100% vs 50%** resolution. The unlit branch exits the
fragment shader before lighting and shadow reception; the resolution test retains
the frozen roots. Wind on/off can then isolate procedural deformation. These
comparisons distinguish pixel/lighting cost from geometry/vertex cost before a
renderer redesign. Keep the 60 FPS target; use GPU duration and thermal behavior
to evaluate margin even when both modes reach that target.

Local artifacts:

- [`metal-analysis2/timeline.png`](../tmp/metal-analysis2/timeline.png)
- [`metal-analysis2/reports.csv`](../tmp/metal-analysis2/reports.csv)
- [`metal-analysis2/segments.json`](../tmp/metal-analysis2/segments.json)
- [`analyze_grass_modes.py`](../tmp/analyze_grass_modes.py)

## Ground comparison: `tmp/logs3.txt` and four screenshots

This capture has 452 HUD lines, 226 distinct packets, and 31 timestamped audit
records. Ground is enabled at app time 17.037 seconds. Fair is first logged at
52.196 seconds (35.2 seconds later), Serious at 72.283 seconds (55.2 seconds
later). The 77.335–82.387-second application window falls to 43.15 FPS. The
camera, source revision 52, 49 vegetation pages, 196 source work items, 52 repacks,
34 source reallocations, and 354,304 source-capacity bytes stay fixed throughout.
There is no evidence here of growing page residency or repeated grass uploads
causing the stationary slowdown. This does not measure all CPU activity.

Late windows reduce transition contamination:

| Phase | HUD reports | Mean interval FPS | Mean HUD GPU duration |
| --- | --- | ---: | ---: |
| First ground segment, last 10 packets | 162–171 | 44.29 | 33.23 ms |
| Clear, excluding first 3 packets | 182–199 | 59.99 | 5.78 ms |
| Ground again, last 10 packets | 217–226 | 44.93 | 32.34 ms |

The app independently reports Clear at 59.80/59.99 FPS, followed by Ground at
45.15/43.50 FPS. These are different averaging windows from the HUD table. There
are only a few packets of Grass/Current between the long ground segment and
Clear, insufficient for sustained grass comparisons. All these late records say
Serious, but the thermal category does not imply constant GPU clocks. The second
ground segment also contains a major transient: one packet averages 206.94 ms GPU
duration. It is excluded from the last-ten comparison. It is not proof of a
shader recompile or a particular stall source.

The screenshots appear to be from a separate capture (their timestamps/control
lock differ); do not align them to exact log timestamps. They show the same
Ground / production / Gaussian / prepass / native-resolution settings:

| Screenshot thermal state | FPS | HUD GPU | Encoder GPU | Fragment GPU | Vertex GPU | Compute GPU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Nominal | 59.99 | 11.36 ms | 11.38 ms | 10.93 ms | 0.28 ms | 0.18 ms |
| Fair | 59.99 | 14.65 ms | 14.68 ms | 14.16 ms | 0.32 ms | 0.17 ms |
| Serious, earlier | 52.45 | 25.57 ms | 19.65 ms | 18.43 ms | 0.43 ms | 0.34 ms |
| Serious, later | 31.99 | 45.54 ms | 31.10 ms | 29.11 ms | 0.54 ms | 0.70 ms |

Fragment work dominates these encoder readings; a geometry or grass-compute
optimization is unlikely to remove the ground-only bottleneck. The aggregate
includes all fragment passes, including UI/compositing, and is not a terrain-only
timer. The large gap between HUD GPU duration and encoder time in the last image
also matters: GPU duration can include idle gaps. However, encoder time alone is
already far beyond the 16.67 ms target. Clear's remaining 5.8 ms includes the
common rendering/UI floor and cannot be subtracted as a clock-normalized pass
cost. A longer cold-start Clear run remains useful for checking that floor's
sustained thermal behavior.

All screenshots also show **API Validation Enabled** and **GPU Frame Capture
Enabled** despite a Release build. Apple documents measurable CPU overhead for
[API validation](https://developer.apple.com/documentation/xcode/validating-your-apps-metal-api-usage/)
and [capture support](https://developer.apple.com/documentation/xcode/capturing-a-metal-workload-in-xcode/).
These should be disabled for measurements, but their presence does not explain
the large fragment cost by itself. API validation is distinct from shader
validation; the screenshots do not establish that shader validation is enabled.

The production terrain is not yet the one-texture baseline: two anti-tiled
layers, packed normal/material maps, macro variation, and PBR/shadow reception
cover 3.61 million pixels. Full mip chains, 1024² compressed runtime textures,
and tiny vertex times argue against blaming texture dimensions or polygon count
first. We need the material comparisons below to attribute the fragment cost.

Artifacts: [timeline](../tmp/metal-analysis3/timeline.png),
[summary and event records](../tmp/metal-analysis3/summary.json),
[packet CSV](../tmp/metal-analysis3/reports.csv),
[analysis script](../tmp/analyze_ground_modes.py). Packet order is retained;
batched HUD measurements are not assigned invented wall-clock timestamps.

## Ground shader comparison: `tmp/log4.txt`

The capture contains 232 HUD lines / 116 distinct packets and 18 audit records.
Every thermal reading is Fair, Low Power Mode is off, and every ground-mode
interval is 60 FPS. Camera pose, native 2796×1290 resolution, source revision,
page counts, repacks, reallocations, and capacities remain fixed once streaming
settles. The iOS capacity correction is visible: 344,064 instances / 11,010,048
bytes of capacity, independently of unavailable GPU placement readback.

Means below exclude the first three packets of each mode to avoid asynchronous
HUD batches and shader/material transitions:

| Mode | Retained packets | Mean GPU duration | GPU p95 | Mean interval FPS |
| --- | --- | ---: | ---: | ---: |
| Production | 15–32 | 11.67 ms | 11.98 ms | 59.99 |
| Surface unlit | 36–53 | 10.51 ms | 12.09 ms | 59.99 |
| One texture | 57–70 | 8.60 ms | 8.93 ms | 59.99 |
| Flat unlit | 74–94 | 7.74 ms | 8.03 ms | 59.99 |
| Production again | 98–116 | 11.77 ms | 12.04 ms | 59.99 |

The production return differs by only 0.11 ms (about 0.9%). Using the last ten
packets instead gives 11.74 / 10.55 / 8.60 / 7.74 / 11.87 ms, preserving the
conclusion. Surface unlit oscillates more than the other modes; the log cannot
identify whether this comes from clock changes, idle gaps, or another cause.

Observed differences, not isolated pass timers:

- Removing PBR/receiver lighting, packed normal/material sampling, and
  post-lighting processing reduces GPU duration by about **1.15 ms**.
- Replacing blended, anti-tiled, macro-varied albedo with one layer/sample
  reduces it by a further **1.92 ms**.
- Removing that remaining texture sample reduces it by **0.86 ms**.
- About **7.74 ms remains with no terrain texture sampling or lighting**.

Thus ordinary texture sampling does not explain most of the reported ground
cost in this run, and PBR alone is also a minority of the measured total.
Terrain albedo processing is worth optimizing, but simplifying it to one texture
would still leave roughly 8.6 ms of reported whole-frame duration here. These
observations do not establish that Flat's remaining time is spent in the ground
fragment shader, or that the phone cannot afford an ordinary textured plane.

Flat changes only the material fragment path. The current terrain entities are
still eligible shadow casters, the sun still has three cascades, the depth
prepass remains enabled, and the main pass still rasterizes/writes the ground.
The audit also retains its offscreen 3D image, second camera, full-screen
composite, UI, and engine work. There is only a 0.665-second Clear segment (one
mixed packet) in this log, so **there is no settled same-run Clear baseline**.
Do not subtract the 5.8 ms Clear value from logs3 as if clocks/instrumentation
were identical.

Apple distinguishes command-buffer duration from encoder activity: the former
can include idle gaps, while encoder timing tracks work within individual
stages. The console CSV has the former, without the screenshots' vertex,
fragment, compute, or encoder timings. Therefore 7.74 ms is not established GPU
busy time. See [Apple's encoder timing explanation](https://developer.apple.com/documentation/xcode/monitoring-your-metal-apps-graphics-performance/).

This is a useful material comparison, not a sustained thermal test. The first
Production segment lasts 10.52 seconds; the return has at least 10.09 seconds of
timestamped observation. Lighter modes occupy most of the intervening time.
There are no Serious samples. This run does not establish that the previous
thermal collapse is fixed, nor that disabling Xcode diagnostics fixed it.
It uses the performance scheme verified in the preceding session; the log itself
does not report the validation/capture settings.

The duplicate HUD registration/packet behavior and drawable-after-present
warning persist with that scheme. The first eight application frames still span
5.675 seconds during startup, without a Thread Performance Checker warning.
Startup delays and this warning need separate attribution; neither is proof of
the sustained material bottleneck. Memory stays near 639 MB for most material
holds, with a brief rise near the production return; this shorter run is
insufficient to settle the earlier memory-growth concern. The unused-variable
compiler warning while entering Flat is expected for a constant-color shader;
compilation succeeds.

**Follow-up comparison requested at this stage (now recorded in `logs4.txt`):**
choose Ground / Flat / 100%, hold
each state for around 20 seconds, and apply one change at a time:

1. Flat with the existing shadows and prepass enabled.
2. Shadows off (cycle past 2×2); this tests shadow-map work without receiver shading.
3. Depth prepass off, leaving shadows off.
4. Resolution 50%, leaving both passes off.
5. Restore 100%, then select Clear and hold it.

The return to 100% before Clear preserves the comparable render-target size.
For Flat and Clear, also capture the HUD's **Encoder GPU** and stage timings;
if available, enable **Top Labeled Encoders**. Bevy's locally resolved source
labels passes such as `main_opaque_pass_3d` and `ui`, allowing that HUD view to
identify which passes dominate. If encoder activity is much lower than logged
GPU duration, CPU/submission gaps need investigation before treating all of the
duration as shader cost. If encoder activity remains high, the pass names guide
the next optimization. The material analysis does not require another build.

Artifacts: [timeline](../tmp/metal-analysis4/timeline.png),
[summary and audit events](../tmp/metal-analysis4/summary.json),
[packet CSV](../tmp/metal-analysis4/reports.csv),
[analysis script](../tmp/analyze_ground_shader_modes.py). The source log is
unchanged; its SHA-256 and assertions for fixed settings are included in the
analysis output/script.

## Ground passes and Clear: `tmp/logs4.txt`

This is distinct from the earlier `tmp/log4.txt`. The new capture contains 440
HUD lines / 220 distinct packets and 40 audit records. All thermal readings are
Nominal and Low Power Mode is off. After startup, all recorded HUD intervals
are 16.67 ms. The extra button-finding switches are retained in the analysis;
sub-five-second modes are excluded from the comparisons below, as are the
first three HUD packets after each retained switch. The camera, terrain source,
residency, repacks, and buffer capacities remain fixed after loading settles.

| Scene / settings | Hold duration | Retained packets | Mean GPU duration | GPU p95 |
| --- | ---: | --- | ---: | ---: |
| Flat, shadows on, prepass on, 100% | 10.99 s | 39–57 | 7.60 ms | 8.07 ms |
| Flat, shadows off, prepass on, 100% | 12.02 s | 62–82 | 6.48 ms | 7.01 ms |
| Flat, shadows off, prepass off, 100% | 9.99 s | 86–102 | 5.23 ms | 5.68 ms |
| Flat, both off, 50% | 14.79 s | 108–133 | 4.67 ms | 5.06 ms |
| Clear, both off, 100% | 15.05 s | 173–199 | 4.69 ms | 5.20 ms |
| Clear, both off, 50% | at least 10.05 s | 204–220 | 4.11 ms | 4.69 ms |

Two subsequent returns to Flat / both off / 100% give **5.16 and 5.30 ms**,
consistent with the first 5.23 ms hold. Last-ten-packet means for the six rows
are 7.65 / 6.43 / 5.29 / 4.63 / 4.72 / 4.15 ms, preserving the conclusions.
Brief Production/Surface/Single visits are not usable production comparisons.
Nominal and 60 FPS during these short holds do not establish sustainability.

The observed differences reveal several separate costs:

- **Shadow-map work:** disabling sun shadows in Flat saves about **1.12 ms**.
  Flat does not evaluate receiver lighting, but terrain entities are still
  shadow casters and the sun renders three cascades. This comparison primarily
  targets that additional pass work, not Gaussian receiver filtering. An empty
  scene's shadow maps can cost time even though the displayed material is unlit.
- **Depth prepass:** disabling it saves a further **1.25 ms**. Locally resolved
  Bevy 0.19.1 `bevy_core_pipeline/src/prepass/node.rs` also issues a full-size
  `copy_texture_to_texture` from the view depth texture to a sampleable prepass
  depth texture after its forward prepass. The switch removes both rendering
  and this copy; the log does not isolate their individual costs.
- **Basic ground draw:** with both passes off, Flat exceeds Clear by only
  **0.55 ms** at native size and **0.56 ms** at half width/height. Returning to
  native Flat produces a difference of about 0.48–0.62 ms versus Clear.
  This is not a measurement of the production material, which is more expensive.
- **Shared baseline:** changing 100% to 50% saves **0.56 ms in Flat** and
  **0.57 ms in Clear**. That near-equal saving cannot primarily be attributed to
  ground fragments. The remaining 4.69 / 4.11 ms Clear duration includes the 3D
  camera/render targets, audit composite, native UI, and potential command-buffer
  idle gaps. It is not an isolated measurement of unavoidable Bevy GPU work.

The two pass switches reduce Flat's measured duration by **2.36 ms / 31%** in
this run. Graphics allocation also falls from about 332.5 MB to 285 MB as those
passes are disabled; this is correlated whole-process reporting, not a precise
allocation breakdown. Later resolution changes release and recreate render
targets. Entity/mesh/image counts and source allocations remain stable, so this
capture does not show continuous scene or source-buffer growth.

### Applied change and remaining investigation

The game camera now defaults to **Depth prepass off on iOS**. Audit startup and
Reset use the same engine constant, so the audit does not silently re-enable it.
The current application enables no screen-space effects or occlusion-culling
feature that consumes prepass depth; the custom grass draw uses the main depth
attachment and does not sample prepass depth. Main-pass depth testing remains
enabled. Desktop/editor behavior is unchanged. The existing toggle can reproduce
earlier measurements, all of which started with prepass on.

The 1.25 ms saving was measured in Flat, not a long production run; it must not
be extrapolated to every material/view or represented as a thermal fix. Dense
overdraw or future depth-dependent effects may change the prepass tradeoff.
Global shadows stay enabled for production. Selectively excluding truly flat
ground from shadow casting, while retaining character/tree shadows and terrain
shadow reception, is a separate candidate requiring visual verification; real
heightfield terrain can need to cast shadows.

The next useful attribution is the **Clear baseline's actual GPU work**:
capture Encoder GPU / Top Labeled Encoders (or a Metal frame capture) and compare
the audit path with the normal camera path. Native UI and the audit composite
remain even when the 3D image shrinks. Encoder measurements can distinguish busy
passes from idle gaps in command-buffer duration. This is a more useful next
step than another unlit-material sweep. A sustained Ground / Production run with
prepass off is still needed to measure the effect on thermal degradation.

Startup still has only eight app frames over 5.208 seconds, before comparisons
begin. Duplicate Metal HUD registration/messages and the drawable-after-present
warning persist. Neither is established as the cause of steady rendering cost.
Shader warnings during brief Production transitions report successful compilation
with unused variables, not a rendering failure.

Artifacts: [comparison chart](../tmp/metal-analysis-passes/comparison.png),
[summary and every audit event](../tmp/metal-analysis-passes/summary.json),
[packet CSV](../tmp/metal-analysis-passes/reports.csv),
[analysis script](../tmp/analyze_ground_passes.py). The script verifies the source
hash is unchanged, packet/event counts, fixed camera/residency, selected report
ranges, and observed thermal/frame-interval values. HUD batches use console
order rather than invented wall-clock timestamps.

## Automated baseline: `tmp/log5.txt` and IMG_1228–1230

The test completes all eight phases in **160.835 seconds**, then restores Current,
grass Full, Gaussian shadows, prepass off, composite rendering, and the UI. The
capture contains 660 HUD lines / 330 distinct packets and 37 audit records.
Thermal state stays Nominal, Low Power Mode stays off, and camera/residency/source
allocations remain fixed after loading. These are intentionally light phases;
their thermal behavior does not establish sustained production performance.

The phase means below exclude the first three packets after each switch. Return
comparisons make the result substantially stronger than subtracting measurements
from separate runs with different thermal histories.

| Phase | Retained packets | Mean HUD GPU duration | GPU p95 |
| --- | --- | ---: | ---: |
| Clear, composite + UI | 54–89 | 5.07 ms | 5.41 ms |
| Clear, direct + UI | 93–129 | 2.70 ms | 2.81 ms |
| Clear, direct, no UI (includes pause) | 133–158 | 1.05 ms | 1.11 ms |
| Flat ground, direct, no UI | 162–198 | 1.36 ms | 1.69 ms |
| Clear, direct, no UI — return | 202–238 | 0.97 ms | 1.07 ms |
| Clear, direct + UI — return | 242–274 | 2.74 ms | 2.84 ms |
| Clear, composite + UI — return | 278–314 | 5.03 ms | 5.44 ms |

In Clear, Composite increases reported GPU duration by **2.37 ms initially /
2.29 ms on return**. This is a whole-frame difference, not a fixed isolated pass
cost; `log6.txt` below finds only 0.22–0.23 ms with grass Full. The difference
includes the offscreen target, extra camera/pass work, and full-screen image
composite, not just one fragment shader. UI drawing adds **1.77 ms** using the
uninterrupted return comparisons. It includes both the engine's large debug text
and the audit controls; this test does not separate those two overlays. The
UI's CPU-side updates continue while hidden, so this is not evidence that text
rebuilding on the CPU accounts for that difference.

Together these account for a **4.06 ms difference from the 5.03 ms Clear return**. The earlier
4.7–5 ms Clear baseline is therefore not an unavoidable cost of an empty Bevy
scene. The audit path changes this lightweight frame's measured duration
substantially. That does not establish an additive 4.06 ms of GPU busy time in
other scenes; budget the actual workload with matched settings. The original heating observations
predate the audit, so this finding cannot explain all of the original problem.

Flat ground adds only **0.39 ms** above the direct/no-UI Clear return. That is
an untextured material with shadow maps and prepass disabled. It does not measure
the complete terrain material, prove the phone will sustain every scene, or
justify treating the real terrain shader/shadow work as free. Do not derive a
production-ground budget by subtracting this result from earlier Fair/Serious
captures. The appropriate next workload is production ground with direct
rendering and reduced/hidden diagnostic UI, followed by a sustained measurement.

### Screenshots and pauses

The PNG capture timestamps, interpreted in the session's Europe/Kiev timezone,
agree with the visible phase labels. Their precision is approximately one second:

| Screenshot | Capture / phase | HUD GPU | Encoder GPU | Vertex | Fragment | Compute | Blit |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| IMG_1228 | 06:26:58, Clear/direct/no UI | 1.18 | 1.95 | 0.31 | 1.09 | 0.44 | 0.16 |
| IMG_1229 | 06:28:03, Clear/direct/UI return, 17.6 s into phase | 2.87 | 3.34 | 0.35 | 2.79 | 0.15 | 0.15 |
| IMG_1230 | 06:28:30, restored Current, 3.8 s after completion | 10.31 | 10.36 | 4.01 | 5.06 | 1.65 | 1.55 |

All times are milliseconds; every screenshot shows Nominal and about 60 FPS.
The first two support a substantial fragment-stage contribution from UI drawing
(1.09 versus 2.79 ms), although these are individual observations, not synchronized
pass-level differences. The last screenshot is indeed after completion: grass
and composite rendering are restored. It is not an expensive Clear measurement.
The Metal HUD's green **Direct** label describes its own presentation metric;
use the game's `render_path` record to distinguish our Composite and Direct paths.
In IMG_1230, the game's button and log explicitly say composite.

Do not sum these stage numbers into a frame budget or require Encoder GPU to
equal the HUD's GPU field. They are distinct measurements/aggregations; Apple's
[encoder timing documentation](https://developer.apple.com/documentation/xcode/monitoring-your-metal-apps-graphics-performance/)
also notes that command-buffer timing can include gaps between encoders. Here
the encoder values independently confirm that the blank screen has far less
rendering work than the debug overlays/restored scene.

Three isolated long intervals, **5.89 / 2.91 / 3.00 seconds**, occur around the
three screenshot captures. They are consistent with capture-related interruption,
but the log does not prove its mechanism. They are not the progressive thermal
collapse: surrounding GPU times remain low/stable and thermal stays Nominal.
The first no-UI phase's aggregate interval FPS is consequently 40.95; its median
GPU duration is 0.99 ms. The clean no-UI return is the primary baseline. Raw pause
packets remain in the JSON/CSV rather than being silently deleted. The last
restored Current segment is similarly affected, so its roughly 40 FPS aggregate
must not be presented as renewed sustained throttling. Startup also still has
eight app frames over 7.405 seconds, separately from these screenshot events.

### Applied correction

At this stage, audit startup/Reset defaulted to **Render: direct at 100%**. Composite remains
available explicitly and is selected automatically for 75%/50% rendering. A
1×1 placeholder avoids allocating a full-size unused audit image until Composite
is requested. The baseline sequence still explicitly tests both paths, then
restores the new Direct default when started from it. Normal gameplay without
the audit already used the direct path; this change bypasses the extra audit
rendering path in future native-resolution measurements. It does not guarantee
a fixed reduction in frame duration or energy use. It is not a new production-renderer
optimization, and the debug UI still has a measurable cost while visible.

**Update, 2026-09-08:** normal game startup and audit Reset now share **75% world
resolution / 4× MSAA** defaults. The scaled world target and native-resolution UI
live in `game_render.rs`, independently of the audit plugin. Audit controls can
still select Direct at 100%; scripted ground comparisons explicitly retain their
native-resolution settings.

Artifacts: [comparison chart](../tmp/metal-analysis5/comparison.png),
[full summary and screenshot alignment](../tmp/metal-analysis5/summary.json),
[packet CSV](../tmp/metal-analysis5/reports.csv),
[analysis script](../tmp/analyze_baseline.py). The script checks every phase,
fixed settings, duplicate counts, pause packets, completion, and source SHA-256.

## Render-path correction with grass Full: `tmp/log6.txt`

The user's observation is confirmed. This capture holds Current / grass Full,
Gaussian shadows, prepass off, native 2796×1290 resolution, and visible UI while
switching Direct → Composite → Direct. Camera pose, source residency, uploads,
and capacities stay fixed after loading; all nine thermal readings are Nominal.
There are 142 HUD lines / 71 distinct packets, each repeated twice. No packet
has a present interval above 50 ms, and the run remains approximately 60 FPS.

| Phase | Retained packets | Mean HUD GPU duration | GPU p95 |
| --- | --- | ---: | ---: |
| Direct initially, after streaming settles | 12–21 | 10.124 ms | 11.75 ms |
| Composite | 25–46 | 10.344 ms | 11.63 ms |
| Direct return | 50–71 | 10.116 ms | 11.67 ms |

Initial Direct uses packets after the settled audit sample at 10.642 seconds,
avoiding startup/streaming. Later phases discard the first three packets after
the switch. Last-ten-packet means are 10.124 / 10.394 / 10.169 ms; the conclusion
does not depend on the chosen steady window. Composite is about **0.22–0.23 ms**
slower using the main windows, approximately 2% of the reported duration. Direct
returns within 0.009 ms of its initial mean. The near-identical p95 values also
give no evidence of a large improvement to the slow frames in this short run.

The switch code changes the main camera target/order, active UI camera, and
composite visibility. Graphics allocation changes with those switches, consistent
with the path changing. The custom grass renderer remains attached to the same
Camera3d; the second Camera2d does not become a grass view. No differing camera
or source setting in the log explains away the user's comparison. The log does
not expose actual GPU instance counts or encoder timings, however.

**Correction to the previous interpretation:** the roughly 2.3 ms Clear result
was overgeneralized when presented as the cost of the composite path. It remains
a reproducible comparison for Clear, but is not a fixed number to subtract from
grass or production ground. Likewise the 1.77 ms UI comparison was made in Clear
and needs validation before being counted as a fixed saving in a rendered scene.
The direct default is still useful for avoiding an extra camera/target and
keeping native-resolution measurements closer to the normal game; it is not a
demonstrated fix for the original heating.

The mechanism behind the workload dependence is **not established**. Standard
HUD GPU time describes command-buffer durations; Apple notes that idle periods
between encoders can inflate it relative to active encoder work. GPU scheduling,
overlap, and operating-frequency changes are possible contributors, not findings
from this log. Nominal thermal state is not a GPU-frequency measurement. See
[Apple's explanation](https://developer.apple.com/documentation/xcode/monitoring-your-metal-apps-graphics-performance/).

The next useful diagnostic is an encoder timeline / Metal GPU capture of the
actual rendered workload, including production ground. It should identify the
duration and work of the terrain/grass, UI, and output passes and any waits, so
optimizations target actual work rather than a supposed constant Clear baseline.
No renderer change is justified solely by this log beyond retaining the simpler
Direct default already implemented.

Artifacts: [summary and audit records](../tmp/metal-analysis6/summary.json),
[packet CSV](../tmp/metal-analysis6/reports.csv),
[analysis script](../tmp/analyze_render_paths.py). The script checks fixed
settings, source stability, packet ranges/counts, and unchanged source SHA-256.

## Running the comparisons

The shared Xcode scheme now enables `--render-audit` for Run. Uncheck that argument
to remove the profiling panel and extra composite pass. On desktop:

```sh
cargo run --release -p yarra-app-game -- --render-audit
```

For phone measurements, select **Yarra Performance** in Xcode. This additional
shared scheme runs Release with LLDB attached to retain the Xcode console, while
API validation, GPU capture, Main Thread Checker, and Thread Performance Checker
are disabled. The original Yarra scheme
remains available for validation/capture. The Metal HUD's validation/capture
badges should be absent in a performance run. Enable the HUD through the device's
Developer settings if needed.

The initial performance scheme disabled debugger attachment, which removed the
console from this Xcode run workflow. Attachment is now enabled for collecting
the comparison logs. A later measurement without the debugger can quantify its
overhead once independent log collection is available; it is not required for
the current material comparisons.

Gameplay controls work by default. The panel captures pointer interactions within
its bounds, including touches released outside after starting on it. Use **Lock
controls** to freeze the player and camera for comparisons; Reset baseline unlocks
them. The scene cycle is Current → Clear → Ground → Grass → Current.
Current preserves the pre-existing visibility setup; on iOS that now means visible,
flat terrain with production grass. Ground and Grass hide other mesh visuals,
including the character and trees. Clear hides mesh visuals and skips all grass
GPU work. **Clear still runs the engine, streaming, animation, and UI**; it is a
control for scene rendering, not a separate minimal Bevy executable.

The optional **Render: composite** path renders 3D into an image and composites
it behind the native-sized UI. This adds a common composite cost, even at 100%.
**Render: direct** is the default and restores the game camera's original window target/order,
disables the second camera and composite image draw, and routes UI to the game
camera. Direct uses native resolution. Selecting a scaled resolution switches
back to composite. The retained offscreen image is not drawn in Direct; memory
retention does not imply ongoing offscreen GPU rendering. Compare these paths
and validate final optimizations again without the audit flag.
The Gaussian/2×2 switch affects PBR meshes; grass keeps its existing hardware 2×2
filter. Shadows off disables the sun shadow maps for both paths.
The percentage changes actual image dimensions, not logical UI/DPI scale. At
75% there are 56.25% as many pixels; at 50%, 25% as many.
**AA** cycles off → 2x MSAA → 4x MSAA → off independently of resolution. It changes
the 3D camera; the Composite UI camera stays single-sampled. Startup and Reset
leave AA off. `msaa_samples` logs the main-world camera component, while
`requested_msaa_samples` logs the selected setting; 1 means off. This is an
edge-coverage experiment for animated grass at reduced resolution. It does not
establish that specular shimmer is fixed or that sustained GPU cost is acceptable.
The automatic baseline forces AA off and restores the saved AA setting afterward.
Depth prepass defaults to off on iOS and on for desktop. Reset restores this
platform default; enable it explicitly when reproducing the older captures.

### Repeated Nominal AA comparison: `tmp/log8.txt`

The log contains 214 lines, 178 raw HUD packets, 89 distinct packets (all logged
twice), and 16 audit records. After loading, source revision 52, 49 pages, 196
work items, 51 repacks, 30 reallocations, source/instance capacities, and asset
counts stay fixed. Every camera checkpoint matches the original high view;
thermal is Nominal and Low Power Mode is off throughout. Current renders ground
and grass with production shading, Gaussian shadows, prepass off, and UI/wind/
counters on. Rendering stays at 1398x645 / Composite from 6.541 seconds onward.

| Setting | Distinct packets, first 3 excluded | Mean HUD GPU duration | p95 | Interval FPS |
| --- | --- | --- | --- | --- |
| Off | 7–14 | 10.259 ms | 11.700 ms | 59.988 |
| 4x | 20–33 | 10.331 ms | 11.590 ms | 59.988 |
| Off return | 37–43 | 10.345 ms | 11.696 ms | 59.988 |
| 4x return | 48–89 | 10.343 ms | 11.620 ms | 59.988 |

The first observed 4x-minus-Off difference is 0.072 ms; the return comparison is
-0.001 ms. No practically meaningful duration increase is established in this
view. Logged requested and applied camera sample counts match, and graphics
memory rises during 4x holds and falls on the Off return; these observations
are consistent with the toggle taking effect, not a disabled control.
The sustained AA setting here is 4x: 2x is only visited at 12.110–12.697 s and
26.463–26.713 s. No settled 2x or native-resolution comparison is available.
The last audit checkpoint is at 46.883 seconds, so this is not a sustained
thermal result. Nominal does not imply fixed GPU clocks or equal energy use.

MSAA samples primitive coverage multiple times without multiplying every vertex
operation or fragment shader invocation by the sample count. Apple describes
pixel shading once per triangle per pixel and tile-based resolve in
[Harness Apple GPUs with Metal](https://developer.apple.com/videos/play/wwdc2020/10602/).
That is general implementation context, not proof of which work dominates this
game. Per-pass/encoder timing is still needed to explain the near-constant
10.3 ms duration here and the hot close-view behavior in log7.

Reproduce with `tmp/analyze_msaa_cool.py`; full phase boundaries, short visits,
source hash, and packet windows are in `tmp/metal-analysis8/summary.json` and
`reports.csv`. Only identical consecutive HUD packets are collapsed. Internal
timing pairs remain intact; no HUD elapsed timestamps are invented.

### Combined ground/grass MSAA run: `tmp/log7.txt`

The run contains 702 lines, 644 raw HUD packets, 322 distinct packets (each logged
twice), and 38 audit records. Identical consecutive packets are collapsed; equal
timing pairs inside each packet are retained. All settings requests match the
logged camera MSAA component. The scene remains Current, grass Full, production
shading, Gaussian shadows, prepass off, UI/counters/wind on. From 9.524 seconds
onward, 3D remains 1398x645 (50%) through Composite, with a 2796x1290 surface.
There is no settled, comparable native-resolution hold in this run.

Camera movement prevents pooling all measurements by AA setting. The most useful
windows are:

| Context | AA | Distinct HUD packets | Mean GPU duration | Interval FPS |
| --- | --- | --- | --- | --- |
| Fixed high camera, Nominal | Off | 46–112 | 10.20 ms | 59.96 |
| Same high camera, Nominal | 2x | 116–132 | 10.53 ms | 59.99 |
| Fixed low camera, first Serious interval | 2x | 195–204 | 13.38 ms | 59.99 |
| Same low camera, later, before 4x | 2x | 205–214 | 19.60 ms | 56.02 |
| Same low camera, settled 4x tail | 4x | 224–230 | 25.10 ms | 47.43 |
| Same low camera, AA-off return | Off | 234–239 | 22.01 ms | 52.06 |
| Different high camera, Serious | 4x | 262–271 | 20.30 ms | 59.59 |
| Final low camera, Serious | 4x | 312–322 | 24.64 ms | 47.74 |

The high-camera pose is repeated at 29.630–73.123 s. The low-camera pose is
repeated at 99.304–128.127 s. Poses are sampled checkpoints, not continuous
camera-motion telemetry. Initial 2x and the hot Off return exclude the first
three packets after each switch. The first 4x switch has a longer transient:
even excluding three packets leaves 27.87 ms / 43.92 FPS. Its last seven packets
after the 119.296 s sample provide the separately reported 25.10 ms tail.
The later hot 2x visit lasts only 0.917 s and is not a settled comparison. Camera
movement starts again during the following 4x interval, so it is not an equivalent
4x return benchmark.

Fair is first logged at 79.141 s; Serious at 104.355 s, both with 2x enabled.
First 4x is at 114.240 s, about ten seconds after Serious. The low-view slowdown
already occurs with 2x held fixed; disabling AA afterward does not restore 60.
Using the last 17 Off packets before the cool switch gives a 2x difference of
0.34 ms (3.35%). The hot 4x-tail versus Off-return difference is 3.09 ms (14.03%).
These are observed whole-frame differences with thermal drift and GPU scheduling
still uncontrolled. Serious is not evidence of fixed GPU frequency. Reported
GPU duration can include idle periods and is not a sum of isolated pass costs.

The source remains revision 52, 49 pages, 196 work items, 51 repacks, and 34
reallocations after loading; source/instance capacities and asset counts are
stable. No render-validation errors or >50 ms HUD intervals are present. The
startup application-frame sample includes a 6.622 s loading delay before the
HUD packet stream, so it is excluded. There is no evidence here that continually
growing source buffers caused the later slowdown.

The remaining performance investigation needs pass/encoder attribution in the
hot close view. 4x is a viable visual-quality candidate, not yet a sustained
60 FPS configuration. Reproduce with `tmp/analyze_msaa.py`; all packet windows,
audit checkpoints, and limitations are retained in
`tmp/metal-analysis7/summary.json`, `reports.csv`, and `timeline.png`.

### Automated baseline comparison

Tap **Baseline test (160s)** once after the scene loads. It saves the current
audit settings, locks gameplay controls, selects native resolution, disables
grass/shadows/prepass, and runs these 20-second phases:

| Time after starting | Scene | Render path | Game UI drawing |
| --- | --- | --- | --- |
| 0–20 s | Clear (warmup) | Composite | On |
| 20–40 s | Clear | Composite | On |
| 40–60 s | Clear | Direct | On |
| 60–80 s | Clear | Direct | Off |
| 80–100 s | Flat ground | Direct | Off |
| 100–120 s | Clear (return) | Direct | Off |
| 120–140 s | Clear (return) | Direct | On |
| 140–160 s | Clear (return) | Composite | On |

The previous settings and UI visibility are restored at completion. The same
button cancels while it is visible; other audit controls are ignored during the
sequence. Timers use real time, independent of game time clamping. Stalls can
lengthen the sequence; keep the app foregrounded. The test is for attributing
baseline work, not proving production thermal sustainability.

Hidden UI phases hide the current root UI nodes, including the engine performance
text and audit panel. CPU-side engine/UI systems and status updates keep running;
this isolates UI drawing rather than removing all UI CPU work. The Metal HUD is
external to Bevy UI and remains visible. A screenshot around 65–75 seconds
captures Encoder GPU/stage times for Clear without game UI. Save all console
output in `tmp/logs5.txt` for the automatic phase analysis.

Each audit record now includes `render_path=composite|direct`, `ui=on|off`, and
`baseline_phase`. `RENDER_BASELINE event=start|complete|cancel` marks the run.
Automatic phase changes also emit regular audit change records; root hiding does
not interrupt logging. Use the three-packet transition exclusion as before.

Validation: desktop and iOS compilation, plus tests for real-time phase timing,
completion/cancellation restoration, direct camera/UI routing, restoration of
the scaled image target, and preservation of originally hidden UI roots.
A native desktop run completed all eight phases in 160.1 seconds with the
camera fixed, no reported render errors, and the original scene, grass, shadows,
prepass, UI, and controls restored. The blank UI phase and restored scene were
visually checked. This is functional validation, not iPhone performance evidence.
Its log and checked phase summary are in `tmp/baseline-smoke-stderr.log` and
`tmp/baseline-smoke-summary.json`.

In **Scene: Ground**, the shading button now cycles through four separately
compiled fragment variants, keeping the same terrain mesh and texture resources:

| Ground mode | Work retained |
| --- | --- |
| Production | Existing complete terrain material and PBR path |
| Surface unlit | Blended albedo, stochastic tiling, macro variation; no normal/material sampling, PBR, fog, or post-lighting processing |
| One texture | One sample call from the first base-color array layer, using the existing sampler and mip chain; no blending, macro variation, or lighting |
| Flat unlit | Constant fragment color, no texture sampling or lighting |

Depth prepass and shadow-map passes remain controlled separately in all four
modes. One texture still uses the existing anisotropic sampler, so it is one
shader sample call, not necessarily one physical texture tap. The former Ground
“unlit” mode swapped in a flat StandardMaterial; it did **not** preserve terrain
textures. The new ground shader choice is independent of the grass unlit switch.

For a material-isolation run, keep the camera and native resolution fixed, select Ground,
and cycle Production → Surface unlit → One texture → Flat unlit → Production,
holding each for about 20 seconds after compilation settles, then select Clear.
Save the whole log. The return to Production helps expose thermal/order effects.
If the large cost disappears at Surface unlit, follow with Gaussian → 2×2 →
shadows off in Production. If it persists until One texture, investigate terrain
blending/anti-tiling/macro sampling. If even Flat remains expensive, inspect
shared passes and compare 100% → 50% resolution before tuning texture content.

The audit also writes versioned `RENDER_AUDIT` records directly from the app:

- `event=startup` identifies the initial settings even if no buttons are pressed.
- `event=change` records applied audit/grass settings changes after Update.
- `event=sample` repeats a snapshot about every five seconds.
- `event=power` records thermal/Low Power Mode changes, polled at most once a
  second except for immediate settings changes.

Every record includes `unix_ms`, real `elapsed_s`, `main_frame`, interval-based
`app_fps_window`, the window duration, thermal state, Low Power Mode, the full
settings, render/surface dimensions, camera pose, entity/mesh/image counts, grass
source revision/page/work-item counts, repacks, reallocations, and capacities.
Ground shader (`ground_shader`) and terrain macro state (`terrain_macro`) are
also recorded, and the keyboard macro switch creates a change marker.
Real elapsed time continues through game-time clamping and the control lock;
`main_frame` is Bevy's application frame counter, not Metal's drawable counter.
`app_fps_window` measures application update frequency including waiting, not CPU
busy time. `last_source_upload_bytes` describes the last revision's upload, not a
per-frame transfer. Source snapshots may lag the settings marker by a frame.

Thermal and Low Power Mode come from `NSProcessInfo` on Apple platforms. Other
platforms report `unavailable`. GPU placement counts remain `gpu_readback=unavailable`
on iOS while the existing readback restriction is active; zero is not substituted.
Capacity fields are allocation limits, not live rendered-instance counts. Logging
uses existing CPU-side diagnostics and does not add GPU readback or timestamps.
In logs3, grass instance capacity incorrectly remains zero because that metadata
was only published by GPU readback. Allocation metadata is now published from
the CPU source preparation path, including on iOS; live placement counts still
remain unavailable there.

Keep both `RENDER_AUDIT` and `metal-HUD` lines in exported logs. Only startup/change
records define experiment boundaries; sample/power records describe the current
experiment. Exclude transition batches after switches because Metal logs batches
asynchronously. The native thermal readings require rebuilding the app; earlier
captures analyzed above contain only the original switch records.

Use Apple's Metal Performance HUD for GPU milliseconds, frame interval, and
thermal state, and Instruments' Game Performance template when CPU/GPU attribution
is unclear. On a development-enabled iPhone the HUD is available in Developer
settings. The built-in Bevy frame time includes waiting/presentation; it is not
CPU busy time. The panel does not restore iOS GPU buffer mapping, which was
disabled during the existing Metal corruption investigation.

Start with these comparisons, changing one control at a time:

| Comparison | Hold constant | What a substantial GPU-time reduction suggests |
| --- | --- | --- |
| Current → Clear | Resolution and pass settings | Scene draw/compute work rather than the common rendering/UI/CPU floor |
| Ground production → surface unlit → one texture → flat | Resolution, geometry, prepass | Separate lighting/material maps, terrain albedo blending, and basic texturing |
| Ground Gaussian → hardware 2×2 → shadows off | Production shading, resolution | Shadow filtering/reception and then shadow-map rendering |
| Ground 100% → 75% → 50% | Production shading and shadows | Pixel shading or framebuffer bandwidth |
| Grass full → frozen draw | Stable resident pages and camera | Candidate scheduling/generation contribution |
| Grass full, counters on → off | All other controls | Diagnostic atomic contention |
| Grass frozen, production → unlit | Resolution and wind | Grass fragment lighting |
| Grass frozen, wind on → off | Resolution and shading | Procedural vertex deformation, with some coverage changes |
| Prepass on → off | Repeat on Ground and Grass | Whether that extra pass pays for itself |

Frozen draw must be entered directly after Full has populated the buffers and
streaming has settled. Entering it automatically locks controls; unlocking
controls restores Full so the buffers can follow the camera. It retains roots
and LOD selections; it does not freeze vertex wind. For a resolution test with
grass, use frozen draw and wind off to
keep the instance population stable. In full mode, resolution also changes
screen-space LOD/density, so that test measures the combined effect. Even in
frozen mode, screen-space ribbon width compensation can affect coverage.

Allow a newly selected shader/material pipeline to compile before reading timing.
For thermal comparisons, use the same brightness and starting thermal state,
avoid screen recording, and do not compare a cool first run with a hot later run.
Record cold and sustained results separately. At a stable 60 FPS, compare GPU
milliseconds and thermal state rather than declaring every 60-FPS mode equally
cheap. A frame that barely fits 16.67 ms has little room for throttling or future
gameplay; the desired margin should be determined by sustained device tests.

Suggested result columns: scene, shading, shadows, prepass, resolution, grass
mode, counters, wind, seconds since change, FPS, GPU ms, thermal state.

## Mac power capture: `tmp/mac-power-20260905-103131.txt`

Recorded on the M2 Max using the Release build and `powermetrics` tasks,
CPU power, GPU power, and thermal samplers. The capture contains 120 samples
covering 124.41 seconds, with sample timestamps from 10:31:47 to 10:33:51 +0300.
Analysis is reproducible with:

```sh
python3 tmp/analyze_mac_power.py tmp/mac-power-20260905-103131.txt
```

The parser saves adjacent `-summary.json` and `-samples.csv` files. Averages are
weighted by each sample's reported duration. CPU percentages use the
sample-normalized CPU-time column; 100% represents one CPU core.

### Focus changed the workload

The game initially reports 114.20 FPS with `focused=true`. At elapsed 10.067
seconds, just as power sampling starts, it reports `focused=false`. All 24
complete game-log intervals inside the capture have unfocused endpoints and
average 60.001 FPS. The stationary camera, 2560×1440 render target, and settings
remain unchanged: ground plus grass, production shading, Gaussian shadows,
prepass on, MSAA off, direct rendering, UI and GPU counters on. Camera movement
appears after the power recording has finished.

The resolved Bevy 0.19.1 `WinitSettings::game()` default uses continuous updates
while focused and a reactive low-power 1/60-second timer while unfocused.
There is no project override. Thus this capture measures the background 60 FPS
workload, not the requested foreground 120 Hz VSync workload. The exact reason
for losing focus was not recorded. Earlier Mac traces did not log focus, so
this is a possible confound for them, not proof of their focus state.

### Measured power and CPU activity

| Metric | Entire capture | First 30 samples (31.09 s) | Last 60 samples (62.23 s) |
| --- | ---: | ---: | ---: |
| System CPU power estimate | 1.58 W | 2.13 W | 1.35 W |
| System GPU power estimate | 3.71 W | 6.35 W | 2.47 W |
| CPU + GPU + ANE power estimate | 5.28 W | 8.48 W | 3.82 W |
| GPU active residency | 70.1% | 71.5% | 69.0% |
| GPU active frequency | 579 MHz | 913 MHz | 467 MHz |
| Yarra CPU usage | 68.2% | 81.0% | 64.2% |
| All tasks CPU usage | 160.3% | 185.6% | 147.0% |

ANE power is zero throughout. These are subsystem estimates for the whole Mac,
not Yarra-attributed watts or total laptop power. GPU Power occurs twice in
each sample; the parser uses the first value, alongside CPU/ANE/Combined, and
does not add the duplicate GPU section. The process GPU-time column is zero
for every process in every sample despite substantial GPU activity, so that
column cannot provide attribution here. Energy Impact scores are not watts.

Thermal state improves from Fair at launch to Nominal at elapsed 34.280 seconds
(about 24 seconds into power recording), then stays Nominal. The power log
agrees: 23 Moderate samples followed by 97 Nominal samples. This run therefore
does not reproduce progressive thermal deterioration. The early-to-late power
decline is not an optimization result: focus/FPS changed at the beginning,
background activity changed, and clocks settled during the recording.

During the final 60 samples, WindowServer averages 31.1% of one CPU core,
SkyComputerUseService 9.4%, and powermetrics 3.7%, in addition to Yarra's 64.2%.
There is transient Spotlight/import activity at the beginning. The capture
does not establish how much GPU power each of these processes contributes,
or whether WindowServer work is attributable to the game or other windows.
No background services were stopped as part of this analysis.

The GPU consumes more of the reported CPU/GPU power than the CPU in this run,
and Yarra has measurable CPU work even at 60 FPS. Neither fact identifies an
individual shader, establishes a CPU spin loop, or explains the iPhone's
thermal behavior. Mac watts must not be transferred to the iPhone.

The next controlled comparison should keep the Mac game focused at the same
120 Hz VSync target and use full scene → Clear → full scene at the same
resolution, UI, and camera. Record power and process CPU through every phase,
check actual FPS/focus rather than only requested present mode, and include
an app-closed baseline for the other system activity. This distinguishes the
scene's contribution from common renderer/CPU work and unrelated system load.

## Focused Mac MSAA 4× capture: `tmp/mac-power-20260905-104216.txt`

This run holds focus throughout, including during the eventual FPS decline.
MSAA changes from off to 2× at elapsed 6.702 seconds, then 4× at 7.427 seconds,
before the first power sample. The 120 power samples cover 124.18 seconds,
10:42:31–10:44:34 +0300, ending about 134.64 seconds after game startup.
All power samples are at 4× MSAA, 100% scale, direct rendering, and a
3456×1936 target. The complete game-log intervals inside the recording average
119.78 FPS. Ground and grass are both enabled, with production shading,
Gaussian shadows, depth prepass, UI, and grass GPU counters on.

The target is larger than the previous run's 2560×1440: 6.69 versus 3.69
million pixels, or 81.5% more pixels per frame. Combined with 120 instead of
60 FPS, that is 3.63× as many output pixels per second, before considering
MSAA and camera-dependent workload. This is not a controlled off-versus-4×
MSAA comparison; neither the power increase nor its ratio can be assigned
to MSAA alone.

| Metric | Stationary top-down (20–55 s) | Lower camera / movement (70–130 s) |
| --- | ---: | ---: |
| System GPU power estimate | 27.32 W | 24.00 W |
| System CPU power estimate | 4.43 W | 4.83 W |
| GPU active residency | 95.7% | 86.5% |
| GPU active frequency | 1381 MHz | 1372 MHz |
| Game FPS | 119.64 | 119.82 |
| Yarra CPU usage (100% = one core) | 111.5% | 114.5% |
| Sampled grass instances | 76,809 | 144,694–146,471 |

Power averages use samples wholly inside the named windows, conservatively
allowing for one-second header precision. FPS uses complete game intervals
inside those windows. The lower-camera section includes pauses and changing
views, so it is not a pure movement-cost experiment. It does establish that
the substantial GPU load is already present in the stationary top-down view;
more generated grass instances do not necessarily mean a more expensive view.

Across the whole recording, system GPU power averages 25.40 W and system CPU
power 4.68 W. GPU active time averages 90.2% at 1376 MHz; the highest listed
GPU frequency state is 1398 MHz. This is substantial GPU activity near the
top available clock state. Yarra averages 113.3% CPU, WindowServer 46.6%, and
SkyComputerUseService 10.7%. The same limitations apply as in the first run:
these are whole-system subsystem estimates, per-process GPU time remains
zero/unusable, and there is no app-closed or Clear power baseline to isolate
the game's GPU watts. CPU usage does not establish the frame critical path.

### The game log outlasts the power recording

- At about 2:15 after startup, power sampling ends. All 120 samples are Nominal.
- At 3:19 (`elapsed_s=199.360`), the game reports Fair, still at about 120 FPS.
- Subsequent five-second windows show 114.03 FPS at 3:34, 98.72 at 3:50,
  and 80.26 at 4:10. The game remains focused with MSAA 4×, the same render
  resolution and present mode, and Low Power Mode off.
- The last record precedes window destruction at 10:46:32. The run contains
  no Serious thermal-state event.

The timing is consistent with thermal limits contributing to the slowdown,
but GPU power/clock samples during the decline are missing and camera position
also changes. This capture cannot directly prove a GPU clock reduction during
the FPS decline. The native thermal reader only logs state; it does not apply
an app-side frame cap or quality change.

This shifts the investigation toward the cost of the GPU workload at the
actual resolution and frame rate. It still does not identify a specific
render pass or show that MSAA, terrain, or grass alone is the cause. A matching
stationary 4× → off → 4× comparison at the same resolution/focus would isolate
the AA contribution; a full → Clear → full comparison would measure how much
system power follows scene rendering. Mac results must still be validated on
the iPhone rather than transferring watt estimates between devices.

The recorder now defaults to 360 samples (approximately six minutes), with an
optional positive sample-count argument. This allows subsequent recordings
to continue through the observed three-to-four-minute slowdown. No game
rendering settings or pacing were changed by this analysis. The parser now
includes resolution and MSAA in its aligned sample CSV and settings summary.

## Sustained Mac 75% / MSAA 4×: `tmp/mac-power-20260905-105136.txt`

The game log spans almost 16 minutes. After short initial visits to 75%, 50%,
and 100%, it settles at 75% at elapsed 19.595 seconds and stays there with
4× MSAA, focus true, VSync, ground plus grass, Gaussian shadows, prepass, UI,
and grass GPU counters enabled. The native surface remains 3456×1936 while
3D renders at 2592×1452 through the composite path. This is 56.25% of the
native 3D pixel count, a 43.75% reduction. It is a resolution scale, not a
uniform 25% reduction in every quality setting or GPU workload.

From elapsed 24.604 to 958.977 seconds (15 minutes 34 seconds), the 186 complete
log intervals average 119.56 FPS. The final five minutes average 119.80 FPS.
There is no progressive decline like the previous 100% / 4× run. Five
approximately five-second windows average below 115 FPS, with a minimum of
105.02; most windows are close to 120. The values describe window averages,
not a frame-time distribution or individual-frame lows.

The 360 power samples cover 372.49 seconds (10:51:50–10:58:01 +0300), ending
at elapsed 382.81 seconds. Power and clock conclusions cover only the first
six minutes, while the game log confirms sustained FPS much longer.

### Comparison while both runs were Nominal

Using elapsed 70–130 seconds in each capture avoids the initial setting
switches and compares both runs at approximately 120 FPS and Nominal thermal
pressure. Only samples wholly inside each window are included, allowing for
the power log's one-second timestamp precision.

| Metric | Previous 100% / 4× run | Current 75% / 4× run |
| --- | ---: | ---: |
| Render size | 3456×1936 | 2592×1452 |
| FPS | 119.82 | 119.86 |
| System GPU power estimate | 24.00 W | 15.53 W |
| System CPU power estimate | 4.83 W | 4.75 W |
| GPU active residency | 86.5% | 75.0% |
| GPU active frequency | 1372 MHz | 1210 MHz |
| Yarra CPU usage (100% = one core) | 114.5% | 119.3% |
| Mean sampled grass instances | 145,267 | 130,697 |

The observed GPU power difference is about 35%, while CPU power is similar.
The route, terrain view, grass population, and render path differ, so this is
not an isolated measurement of resolution or fragment shading cost. Full-mode
grass LOD also responds to scaled resolution. Nevertheless, the combination
of lower GPU power and sustained FPS supports resolution scaling as an
effective workload reduction in this Mac configuration. It does not measure
MSAA cost, which remains at 4× in both compared windows.

### Stable FPS still involved thermal pressure

The settled 75% region at elapsed 30–380 seconds averages 15.06 W GPU plus
4.54 W CPU, about 19.60 W for those reported subsystems. These are estimates
for the entire Mac, not total laptop power or app-attributed watts. As before,
per-process GPU times are all zero and cannot identify the GPU cost of Yarra,
WindowServer, or the other active processes. No fan RPM was recorded.

The app's Foundation thermal state changes to Fair at elapsed 160.287 seconds
and remains Fair through the final game record. Powermetrics separately
reports Moderate around 159.8 seconds and Heavy from about 185.8 seconds to
the end of its capture. Preserve these labels independently; the capture
does not establish a one-to-one mapping between the two thermal interfaces.

In elapsed 300–360 seconds, powermetrics reports Heavy pressure, GPU active
frequency averaging 998 MHz, GPU activity 88.9%, and GPU power 12.64 W. The
game still averages 119.73 FPS. Reduced GPU clocks can coexist with the same
VSync frame rate when the workload still fits the frame budget.

The occasional dips provide additional evidence of limited GPU headroom:

- Around elapsed 276–285 seconds, GPU frequency falls to roughly 791–957 MHz
  with activity near 99–100%. The corresponding game windows average 109.47
  and 110.30 FPS.
- Around elapsed 369–372 seconds, GPU frequency falls as low as 731 MHz at
  approximately 100% activity; the game window at 371.303 seconds averages
  113.42 FPS. Another busy period at 379–383 seconds aligns with a 112.66 FPS
  window. The subsequent 105.02 FPS window extends beyond power recording.

These are whole-system GPU observations under Heavy thermal pressure and
changing views, so they do not identify a specific pass or prove every dip
has only one cause. They do show that the GPU was busy at reduced clocks
during captured slowdowns, rather than simply inferring load from FPS.

The user's report of audible fans is consistent with the remaining power
load and the app's Fair thermal state. Apple's [Mac thermal-state guide](https://developer.apple.com/library/archive/documentation/Performance/Conceptual/power_efficiency_guidelines_osx/RespondToThermalStateChanges.html)
explicitly notes that fans may become audible in Fair. This run demonstrates
sustained performance with active cooling; it does not establish quiet or
low-power operation, or sustained iPhone performance.

### Allocation counters during traversal

The grass instance-buffer capacity stays at 11,010,048 bytes after startup.
The source-buffer reallocation count reaches 35 and its capacity reaches
4,556,800 bytes at elapsed 130.171 seconds, then both remain unchanged for
the rest of the run. Source repacks continue as the player traverses streamed
pages. These counters do not suggest continually growing grass buffers;
they are not a complete application-memory or leak measurement.

Keep 75% / 4× as a useful Mac baseline for subsequent GPU pass comparisons.
The remaining question is which rendering work consumes the GPU budget and
power, not whether the whole scene can ever sustain 120 FPS. Resolution
scaling is now supported by a long gameplay run; terrain, shadow, grass,
and AA contributions still require controlled measurements or a GPU frame
capture. This analysis changes no game settings or rendering code.

## Verification

- All four terrain variants compile and draw through the real Bevy/Metal pipeline
  in an explicit offscreen GPU test; the iOS target check and app/terrain unit
  tests pass. This checks shader/bind-group validity, not iPhone performance.

- The game compiles for `aarch64-apple-ios` using the locked dependencies.
- Input regression tests cover camera pan/pinch, gesture consumption while
  locked or captured, and HUD touch capture through release at 3× display scale.
- The 14 ordinary vegetation tests pass, including shader parsing/validation and
  CPU/GPU layout checks. The additional explicit GPU regression test passes on
  the Mac's Metal backend.
- The desktop audit was launched and its clear, ground, grass, material restore,
  shadow, prepass, resolution, frozen draw, counter, wind, and compute-only
  controls were exercised visually. No GPU validation errors were reported.
- Desktop FPS during this functional check is not an iPhone performance result.
  No physical-device thermal improvement is claimed by this change.
- Strict workspace-dependency Clippy is not clean: existing renderer/engine code
  reports argument-count, type-complexity, enum-size, and style warnings. These
  were not expanded into unrelated refactoring during this audit.

## References

- [Apple: iPhone 15 Pro Max specifications](https://support.apple.com/en-ie/111828)
- [Apple: improving game graphics performance and settings](https://developer.apple.com/documentation/metal/improving-your-games-graphics-performance-and-settings)
- [Apple: analyzing Metal app performance](https://developer.apple.com/documentation/xcode/analyzing-the-performance-of-your-metal-app/)
- [Apple: Metal Performance HUD metrics](https://developer.apple.com/documentation/xcode/understanding-metal-performance-hud-metrics)
- [Apple: enabling and using Metal Performance HUD](https://developer.apple.com/videos/play/tech-talks/110339/)

Bevy default values and extraction behavior were checked against the locally
resolved 0.19.1 dependency source, rather than assuming defaults from another
Bevy version.

## 2026-09-05: Metal frame attribution and stationary placement reuse

Captured the release Mac app programmatically at render frame 600, then replayed
and profiled the `.gputrace` in Xcode 26.0.1. This is a single-frame GPU capture,
not the earlier heavyweight Metal System Trace. Inputs: Current (terrain and
grass), production materials, 2560×1440, native/direct, MSAA off, Gaussian PBR
shadows, depth prepass on, UI/counters/wind on, default stationary camera.
These percentages describe this Mac frame; they are not iPhone measurements.

Artifacts in `tmp/`:

- `metal-frame-baseline.gputrace`, `metal-frame-baseline-counters.csv` and `.json`.
- `metal-frame-layer-skip.gputrace` and matching counters: rejected experiment.
- `metal-frame-placement-cache.gputrace` and matching counters: retained change.
- Matching `*-game.log` files record settings and placement counts.
- `placement-cache-movement-game.log` records movement and diagnostic transitions.

The baseline shader table gives a useful attribution:

| Shader/work | Xcode share of captured GPU time |
| --- | ---: |
| Terrain fragment (`opaque_mesh_pipeline`) | 44.64% |
| Grass vertex (`vegetation-v2 placement debug`) | 18.95% |
| Grass fragment | 10.51% |
| Grass generation encoder, including finalization | 9.06% |
| UI fragment | 1.51% |
| Terrain vertex | 0.27% |

The main opaque encoder is 75.66% in total. Its average pixel overdraw is
1.0014, with 3,691,456 fragment invocations for a 3,686,400-pixel target. Main-pass
limiters include ALU (63.41%), texture reads (47.74%) and texture filtering
(37.41%); these are overlapping utilization/limiter signals, not additive time
shares. Texture cache miss rate is 4.67%. This points to shader arithmetic and
filtering as useful targets rather than a large hidden-ground overdraw problem.

Terrain's shader evaluates two surfaces, each with stochastic three-way albedo
sampling and a normal/material sample, then the weight map and three macro
samples, followed by PBR, shadow filtering, fog, and tone processing. Grass
spends more in its procedural vertex shader than in its fragment shader in this
view. The source reconstructs blade shape and deformation at every vertex.

### Retained change: reuse identical grass placement

`Full` now keeps its generated instances, indirect draw arguments, and telemetry
when source revision, camera uniforms other than wind phase, diagnostic inputs,
and compiled compute pipeline IDs are unchanged. Wind phase still reaches the
draw shader every frame. Wind strength stays in the key because generation uses
it for conservative bounds. Camera/projection changes, residency/source edits,
settings changes, and compute shader reloads force regeneration. No density,
geometry, lighting, resolution, or MSAA defaults change.

`ComputeOnly` and `ScheduleOnly` always run their isolated workload. Leaving any
isolation mode invalidates the cache; in particular, returning from ScheduleOnly
must rebuild after its cleared draw arguments. Inactive/unready frames also
invalidate cached output. The log now includes cumulative
`generation_dispatches` and `generation_reuses` on both Mac and iOS. Existing GPU
readback counts describe the most recently generated placement, not fresh work
on every reused frame.

Verification:

- 15 vegetation-render tests pass, one native scheduler test remains explicitly
  ignored. The new test covers wind phase reuse and invalidation by camera,
  projection, wind strength, source revision, configuration, and pipeline reload.
- Mac and `aarch64-apple-ios` game checks pass; release game builds.
- Stationary capture: dispatches remain 50 while reuses grow 239 → 547.
  Xcode shows **103 draw calls and 2,075,253 vertices in both captures**, but
  command buffers fall 56 → 50, compute encoders 16 → 14, and dispatch calls
  69 → 65. Exported encoder counters confirm no vegetation scheduler/generator
  encoder in the cached frame. The removed generation dispatched 2,331,840
  candidate lanes per frame in the original stationary scene.
- Movement: camera moves from `(9.449, 12.354, 9.449)` to
  `(5.429, 12.354, 12.818)` and dispatches increase 50 → 205. After stopping,
  dispatches stay 205 while reuses increase 2,208 → 2,739.
- Exercised Full → Frozen → ComputeOnly → ScheduleOnly → Disabled → Full.
  Returning to Full regenerates once (243 → 244), then reuses increase
  6,278 → 7,484 with dispatches staying 244. Grass visually returns correctly.
  Those final focused five-second windows report approximately 120 FPS.

This eliminates repeated placement computation while stationary; it does not
remove the larger grass draw cost, and it is not a measured sustained iPhone
thermal fix.

### Terrain experiment and next substantial target

Tried skipping a terrain layer at exactly zero blend weight, with derivatives
computed before the nonuniform branch. It compiled and rendered all four terrain
variants. Main-pass texture sample calls fell 145.55M → 136.90M (5.9%) and texture
L1 read traffic fell 8.18GB → 6.88GB (15.8%), but fragment ALU instructions were
essentially unchanged (7.539B → 7.554B) and terrain register allocation rose
149 → 152. The experiment was removed from the active shader; its source is
preserved in `tmp/terrain-material-layer-skip-experiment.wgsl`.

Replay milliseconds are **not a trustworthy before/after result from these three
runs**: the baseline reports 4.21ms, the branch experiment 9.96ms, and the cache
capture 9.27ms, while unchanged compute/UI work also slows by more than 2× between
the first two. Xcode calls the mode “Medium” but explicitly says it only attempts
that state unless thermally throttled. Multiple replay sessions were open during
parts of the investigation; all sessions started here were subsequently stopped.
Use the work counters and eliminated passes as evidence, not a claimed FPS or
wattage improvement from these raw durations.

The next substantial terrain optimization should move static material evaluation
out of the per-frame fragment path: cache/bake blended albedo, normal and material
values when terrain pages load/change, then retain dynamic lighting/shadow
reception at draw time. A bounded cache with explicit texel density and mip
handling is needed to avoid trading shader cost for excessive memory or blurred
close views. That architectural change is not implemented here. The small
zero-weight branch alone is not supported as the major reduction needed.

### Repeating a Metal frame capture

On macOS, launch with `MTL_CAPTURE_ENABLED=1` and
`--metal-capture /absolute/path/name.gputrace`; the path must not already exist.
The release app records one render graph frame after 600 render frames, saves it,
and exits. Add `--render-audit` for paired configuration logs. Open the trace in
Xcode, select “Profile after replay”, then Replay. Performance → Shaders gives
attribution; Performance → Share → Export Encoder Counters saves counters.
Stop the GPU replay when finished. The capture hook installs no systems unless
the flag is present, and is excluded from iOS builds.

Xcode's CSV export under this locale leaves decimal commas in percentage cells
unquoted. The adjacent normalized JSON files merge only split percentage cells
and assert the resulting column count matches the header; do not naively zip
raw CSV cells to headers.

## 2026-09-05: Terrain stochastic transform cache

Implementation order is now tracked in
[RENDER_OPTIMIZATION_PLAN.md](RENDER_OPTIMIZATION_PLAN.md). The first terrain
cache stores the stable random offsets and quarter-turns of the triangular
anti-tiling lattice. It does not bake colour or replace the original surface
textures. For the normal two-surface path this replaces 18 fragment hash
evaluations with six storage-buffer lookup loads; the three filtered albedo
samples per surface, their derivatives, packed material samples, weights, macro
samples and dynamic PBR/shadows remain as before.

`terrain_stochastic.wgsl` contains the shared hash implementation used by both
the reference fragment path and the one-time compute builder. Each resident
material owns a storage buffer containing two layers of float4 entries. Lattice
bounds include a guard at page edges and support negative coordinates. Table
payload is capped at 8 MiB overall (plus one 16-byte fallback buffer), each side
at 128 nodes, and builds/allocations at eight tables per frame. GPU driver
allocation granularity and metadata are additional to the
reported payload; recently replaced GPU resources may overlap briefly while
submitted work completes. Excessively dense/large materials and over-budget
materials retain the original shader. There are no mipmaps on these integer lookup
tables; all source material texture mipmaps and anisotropy remain intact.

Tables become eligible for drawing after their GPU build has been encoded
before the camera passes. Readiness chooses a separate material shader variant,
so the cached fragment does not retain a runtime branch containing the hashes.
Changed lattice bounds/layer IDs replace tables; page removal frees their buffer
assets. Source texture, weight, roughness, normal and macro edits remain dynamic
because those values were never baked. A tile-size edit can reuse a table if its
existing lattice bounds still exactly describe the required lookup: entries
depend on integer lattice coordinates and surface layer, not metres per tile.
The render cache also tracks GPU buffer and compiled compute pipeline IDs.

Enabled by default in the terrain plugin. `--terrain-procedural` disables it in
the game for a matched reference capture. `TerrainCacheSettings` exposes enable
and memory budget controls to code. `RENDER_AUDIT` records `terrain_cache`,
`terrain_cache_tables`, `terrain_cache_ready`, `terrain_cache_bytes`, cumulative
`terrain_cache_builds`, and `terrain_cache_reused_frames` (frames needing no
table build while at least one table is ready).

Validation so far:

- Five CPU tests pass: mesh topology plus lattice edge coverage, invalid/fine
  inputs, allocation rate, memory cap, page removal, budget reduction, and
  invalidating a ready table on a layer edit.
- Explicit native GPU test compiles/draws all terrain diagnostics and compares
  cached/reference output with patterned textures at negative coordinates.
  Three configurations, including changed repeat rates, macro enablement and 4× MSAA,
  produce byte-identical 128×128 render target readbacks (max/mean error zero).
  Steady tables do not rebuild; disabling frees the table payload.
- Mac release build, editor and iOS game checks, and terrain Clippy pass.
- Visual/movement run (`tmp/terrain-buffer-visual-game.log`): camera moves from
  `(9.449, 12.354, 9.449)` to `(4.924, 12.352, 13.767)`. Builds rise 49 → 56
  as seven terrain pages are replaced, then stop. Residency returns to 49 tables
  and the same 1,514,016-byte payload. After stopping, reuse counts grow
  3,973 → 9,393 without further builds. Terrain/grass remain visible and the
  sampled windows stay near 120 FPS. Game and replay workloads were closed.
- Sustained phone temperature/power is not yet measured for this change.


### Matched frame work counters

Release Mac, 2560×1440, default camera, MSAA off, terrain + grass, Gaussian
shadows, prepass/UI/wind on. Both frames have 103 draws, 2,075,253 submitted
vertices, 621,129 main-pass vertex invocations, and 68,758 grass instances.
The cache builds 49 tables (1,514,016 bytes = 1.44 MiB payload) and then reuses
those tables. Xcode reports about 1.54 MiB additional allocated buffers. There
is no terrain cache compute encoder in the settled frame.

| Main opaque pass counter | Reference | Buffer cache | Change |
| --- | ---: | ---: | ---: |
| Fragment ALU instructions | 7,552,179,712 | 6,598,225,152 | −12.6% |
| Texture sample calls | 145,406,976 | 145,406,656 | Essentially unchanged |
| Texture L1 bytes read | 8,164,239,616 | 8,222,147,264 | +0.7% |
| Terrain fragment registers | 149 | 142 | −7 registers |

Counts refer to the whole main opaque pass, including grass; register counts
refer specifically to terrain. Small invocation/traffic differences remain
because grass wind phase differs between launches. Resolution, camera,
population and topology match.

The first implementation used texture lookups. It removed 12.3% of fragment ALU
but added 14.9% texture accesses; this was replaced by the buffer implementation.
Its source is preserved in `tmp/terrain-transform-texture-implementation/`.
The retained buffer path removes the extra texture calls.

Single replay times were 4.54 ms reference, 4.50 ms texture cache, and 3.91 ms
buffer cache (main pass 3.97 / 3.81 / 3.32 ms). These are observations, **not a
verified 14% whole-frame speedup**: unchanged grass vertex time also falls from
about 0.80 to 0.70 ms. Only one replay was active at a time, but performance
state/clock differences still confound cross-replay timing. The defensible
result is the eliminated arithmetic, unchanged texture sample count, preserved
output and bounded steady cache. Sustained iPhone power/temperature remains to
be checked; this small cache alone is not the full material-baking optimization.

Artifacts: `tmp/terrain-transform-reference.gputrace`,
`tmp/terrain-transform-cached.gputrace` (texture experiment),
`tmp/terrain-transform-buffer.gputrace` (retained implementation), their
`* Counters.csv`, normalized `*-counters.json`, and `*-game.log` files.
All replay workloads were stopped after exporting counters.


## Shared grass curve and wind preparation (2026-09-05)

Implemented after the terrain lattice cache. The original draw shader reconstructed
random shape, surface frame, Bezier controls and root wind/gust response for every
vertex. These values are now computed once per accepted blade before drawing.
Per-row curve evaluation/derivative, longitudinal flutter, view opening, width
compensation and fragment shading remain in the draw shader. Wind appearance,
mesh topology and population density are preserved.

The preparation is a separate pass from placement: a still camera reuses placement
while wind updates shared curves each frame. If wind is off and inputs are unchanged,
preparation also reuses its results. Camera, source, density/debug configuration,
generation and compiled-pipeline changes invalidate it. Full/Frozen draw use the
prepared data; compute-only, scheduling, disabled and vertex-only diagnostic modes
do not execute preparation. Diagnostic counters are copied after both passes.

A fixed lookup for 344,064 possible instances points into 131,072 prepared blades,
128 bytes each. With the indirect dispatch buffer the allocation is 18,153,484 bytes
(17.31 MiB). Overflow uses the original calculation for the entire render unit,
including both blades of a pair; it never removes blades. The draw shader also uses
the original calculation until preparation pipelines are ready. Shared WGSL code
keeps both calculations identical. The setup dispatch and preparation dispatch use
separate bind groups so indirect arguments are not simultaneously writable storage.

`VegetationBladePreparation.enabled` controls the path; game argument
`--grass-vertex-reference` disables it for A/B tests. New self-contained audit fields:
`blade_preparation`, `blade_preparation_bytes`, `blade_preparation_dispatches`,
`blade_preparation_reuses`, `sampled_prepared_blades`, and
`sampled_preparation_fallback_blades`. The last two follow the existing asynchronous
GPU sample; zeros are not evidence of no work when `gpu_readback=unavailable`.

### Release workload comparison

Captures: `tmp/grass-reference.gputrace` and `tmp/grass-prepared.gputrace`, with
matching ` Counters.csv`, `-counters.json` and `-game.log` artifacts. Reproduction
script/result: `tmp/compare_grass_preparation.py` and
`tmp/grass-preparation-comparison.{json,txt}`. Both used terrain caching, the default
camera, 2560×1440, full grass, Gaussian shadows, depth prepass and 1× sampling.
Wind was running in both, but wall-clock phase was not pinned; small fragment-work
variation is expected. One Xcode replay ran at a time; both reported Medium state.

| Counter | Reference | Prepared |
| --- | ---: | ---: |
| Main-pass vertex invocations | 621,129 | 621,129 |
| Main-pass vertex ALU instructions | 1,605,825,280 | 732,198,656 |
| Added preparation compute ALU | 0 | 54,123,776 |
| Vertex + preparation ALU | 1,605,825,280 | 786,322,432 |
| Whole-frame ALU (all stages) | 8,450,051,840 | 7,628,242,944 |
| Whole-frame device bytes read | 70,522,816 | 88,283,776 |
| Whole-frame device bytes written | 72,665,920 | 79,981,184 |
| Whole-frame texture samples | 155,306,240 | 155,668,160 |
| Replay GPU time | 4.107 ms | 3.982 ms |

The controlled switch removes **51.0% of vertex-plus-preparation arithmetic**;
against the pre-refactor shader's 1,561,568,768 main-pass vertex instructions the
reduction is **49.6%**. Whole-frame arithmetic falls **9.7%**. Memory read+write
traffic increases **17.5%** (~25.1 MB/frame). This is a compute-for-bandwidth tradeoff,
not a free cache. The measured preparation encoder is ~0.058 ms in this replay.
The 3.1% total replay timing difference must not be presented as a verified live
speedup or thermal result: replay clocks have varied throughout this investigation.

The scene still submits 68,758 render units, 1,021,572 grass indices and 103 total
draw calls. All 107,049 blades fit; fallback count is zero. Grass vertex shader
registers are 136 in both switch states (144 in the previous shader); preparation
uses 188 registers with no spills. Terrain remains the largest shader cost.

### Validation

- 28 regular tests pass across app_game, terrain_render and vegetation_render.
- All three native Metal tests pass: terrain image equivalence, grass preparation
  image equivalence and the live-record scheduler regression.
- Grass readback comparisons use the actual Bevy draw/compute pipelines at 256²:
  wind off is byte-identical; animated wind and a moved camera with 4× MSAA have
  maximum byte error 1 and mean errors 0.000004/0.000008. A three-blade buffer forces
  6,787 blades into fallback and is byte-identical. Tests also cover unchanged-data
  reuse, source replacement and Full/Frozen/Compute/Schedule/Disabled transitions.
- Terrain comparisons remain byte-identical in all three variants, including 4× MSAA.
- Mac game/editor checks, iOS game check, release build and diff whitespace check pass.
  Clippy completes with the existing renderer style warnings (needless update,
  argument count, derivable Default and constant assertions); no new warning remains.
- Interactive release run uses both terrain and grass, 75% resolution and 4× MSAA.
  Movement updates placement and terrain tables, then stationary placement reuses.
  Terrain builds go from 49 to 56 with the resident table payload unchanged. With
  wind off, preparation dispatches stay at 20,887 while reuse counts increase;
  stationary 4× MSAA samples are around 118–120 FPS and thermal remains nominal.
  This is a smoke test, not a sustained thermal benchmark. The detailed run is in
  `tmp/grass-prepared-visual-game.log`.

There was also an interval of 286–309 main-loop FPS while switching through 2× MSAA
at 75%, despite the window reporting AutoVsync; it returned to ~118–120 with 4×.
These are application-loop counts, not measured presented frames. Exclude setting
transitions from timing comparisons; this observation needs a presented-frame
check before attributing it to a pacing bug or thermal load.

### Remaining work

The bounded terrain **material** cache remains the next large rendering change;
the lattice cache only removes hashes and does not remove the repeated material
texture sampling/blending. Its working-set, mip and close-view constraints remain
listed in RENDER_OPTIMIZATION_PLAN.md. On-device sustained power/thermal comparison
of the combined changes is still outstanding; the Mac arithmetic reduction does
not establish the iPhone benefit, especially with the added memory traffic.


## iPhone log9: sustained target still fails (2026-09-05)

The user reports frame rate and heat, not new visual artifacts. Source:
`tmp/log9.txt`; packet/interval analysis: `tmp/metal-analysis9/summary.json`.
Exact consecutive duplicate HUD packets are collapsed (288 raw → 144 unique,
8,035 timing pairs retained). Audit line positions bracket the comparisons;
HUD report order must not be labelled as a reconstructed wall-clock timeline.
The interval summary omits the first three HUD reports after each audit.

Both optimizations are active: the terrain lattice cache has 49 ready tables,
1,514,016 bytes; blade preparation has 18,153,484 bytes. Terrain builds increase
only when streaming new pages (49 → 77), not every frame. iOS GPU readback is
unavailable: zero sampled blade/fallback counters do not imply zero blades or
zero preparation overflow.

| Workload / audit interval | GPU mean | HUD interval FPS |
| --- | ---: | ---: |
| Top-down, 75%, 4×, Nominal; 10.289–15.324 s | 11.31 ms | 59.99 |
| Low camera, moving, Nominal; 30.428–35.446 s | 18.58 ms | 59.99 |
| Moving, approaching Fair; 50.516–54.534 s | 19.77 ms | 55.25 |
| Moving, Fair; 64.653–69.711 s | 30.83 ms | 38.27 |
| After Serious at 74.809 s | 31.01 ms | 38.00 |

GPU duration and frame interval are different HUD metrics; do not subtract GPU
duration from the vsync budget as though it were an exclusive, non-overlapping
pass duration. The thermal transition is logged at Fair 54.534 s and Serious
74.809 s. Main-loop FPS corroborates the late drop. The OS thermal category is
coarse and cannot establish constant clocks within an interval.

The 3D target is **2097×967** on a 2796×1290 surface: resolution scaling is
working. The run uses Gaussian shadows, no depth prepass, 4× MSAA from 10.289 s,
both terrain and grass, and wind. Camera height drops from 12.354 m to 1.730 m
and then movement continues. Between 30.428 and 74.809 s, placement dispatches
increase by 2,372 while reuse stays at 972. Stationary placement reuse cannot
save this moving workload. Fixed buffers/residency do not show an accumulating
allocation explaining the slowdown.

This is a failed sustained-performance acceptance run. It is not a matched
before/after proof that either optimization helps or hurts iPhone: log8 used
50% resolution and a stationary top-down camera and was shorter; 75% renders
2.25 times the pixels of 50%. The previous Mac work-counter comparisons also
used a top-down view. Next profiling must reproduce low-camera motion and
measure the extra memory traffic of grass preparation on the physical phone.

The diagnostic `--render-repro low-walk` selects 75%/4×/prepass off and overrides
the camera after gameplay input. It warms up the close view for 300 frames,
then follows a continuous ~10 m out-and-back route over 600 frames around the
idle actor. It approximates the logged view, not the user's exact input replay;
wind retains real time. It leaves `--grass-vertex-reference` and
`--terrain-procedural` independent. `--metal-capture NAME.gputrace` on iOS saves
a single frame after 600 render frames to `Documents/MetalCaptures` in the app
container, then exits. Capture support uses Apple's public
[programmatic Metal capture API](https://developer.apple.com/documentation/xcode/capturing-a-metal-workload-programmatically).
Capture instrumentation must be disabled for sustained timing/power runs.

## Physical A17 Pro close-view captures (2026-09-05)

The `low-walk` capture changes the priority inferred from the earlier top-down
Mac frame. At 2097×967, 4× MSAA, Gaussian shadows and no depth prepass, the
prepared-grass iPhone frame reports these shader shares:

| Shader | Share |
| --- | ---: |
| Grass vertex | 28.62% |
| Grass fragment | 22.87% |
| Terrain fragment | 14.90% |
| Grass generation | 13.27% |
| UI fragment | 6.18% |
| Grass preparation | 3.34% |

Grass accounts for about 68% in this close view. Terrain remains expensive, but
optimizing terrain alone cannot address most of this frame. Generation launches
4,130,690 kernel invocations; main-pass vertex invocations are 1,054,694.
The grass vertex shader uses 142 registers with no spills and measured main-pass
vertex occupancy is 5.84%. Generation uses 150 registers with 16 bytes spilled.
These counters identify substantial remaining work; they do not individually
prove the cause of limited throughput.

### Preparation A/B on iPhone

Artifacts: `tmp/iphone-low-{reference,prepared2}.gputrace`, their ` Counters.csv`
exports and `tmp/iphone-preparation-comparison.json`. The incomplete original
`iphone-low-prepared.gputrace` is not usable. Both valid captures use the same
frame-based route and endpoint, with independent real-time wind phases.

| Counter | Reference | Prepared |
| --- | ---: | ---: |
| Main-pass vertex ALU | 3,056,861,712 | 2,080,618,416 |
| Preparation ALU | 0 | 74,507,808 |
| Main-pass vertex invocations | 1,054,694 | 1,054,694 |
| Whole-frame device reads | 160,719,488 B | 180,962,048 B |
| Whole-frame device writes | 112,224,832 B | 128,616,384 B |
| Replay GPU time, Medium state | 18.41 ms | 18.03 ms |

Preparation removes 29.5% of vertex-plus-preparation arithmetic in this view,
less than the earlier Mac view, while adding 13.4% (~36.6 MB/frame) of device
memory traffic. The 2.1% replay-time difference is modest and does not establish
a sustained thermal benefit. Keep the reference switch available.

### Additional changes and checks

- Production grass generation now rejects candidates as soon as ownership,
  growth, surface validity, coverage or visibility rules make them ineligible.
  It skips subsequent calculation rather than changing population or LOD rules.
  Diagnostic modes retain the complete evaluation. `--grass-placement-reference`
  selects the old evaluation order for comparisons.
- A native Metal comparison evaluates both paths in the actual production WGSL:
  90,086 accepted and 425,464 rejected candidates across 21 cases, with zero
  eligibility or accepted-output mismatches. Cases cover camera poses, density,
  invalid surfaces, partial growth and diagnostic modes.
- Optional grass diagnostic atomics now default off on iOS, where their GPU
  readback is unavailable. Required instance-append atomics remain. The audit
  toggle and repro argument `--grass-counters` can explicitly enable diagnostics.
  This does not disable the Metal HUD or Xcode's hardware counters.

Capture `iphone-low-rejection` retains diagnostic counters to isolate the first
change. Generation ALU falls from 1,773,567,328 to 1,671,404,896 (5.76%), while
kernel invocations and main-pass vertex invocations stay unchanged. Replay time
is 18.02 ms versus 18.03 ms: this is a small work reduction, not a demonstrated
frame-time improvement.

Capture `iphone-low-no-counters` then disables the diagnostic atomics. Generation
device atomic bytes written fall from 17,670,592 to 8,893,952 (~49.7%); these are
hardware transaction counters, not the size of allocated buffers. Replay time
is 17.82 ms. All three reported Medium performance state, which is not a lock
on clocks. Wind phase causes small fragment-count differences. The last two
captures were acquired with iPhone Mirroring connected; their short live HUD
logs must not be used as matched before/after timing against the earlier captures.

The capture also flags 32.47 MiB of stored pre-resolve MSAA data in the opaque
pass. Bevy's pass stores the multisampled attachment for possible later passes.
Avoiding this store requires proving later passes do not need it, or combining
those passes; blindly discarding it would break transparency or other rendering.
This is a separate bandwidth opportunity to investigate, not an implemented fix.

Regular tests: app_game 9 passed; vegetation_render 16 passed, 3 native tests
ignored by default. The new native candidate comparison passes separately.
The iOS Release build and installation succeed.

### Reproduction and console details

`--render-repro low-walk` version 2 defaults diagnostic counters off and logs
the full configuration. `--render-console` adds stderr logging on iOS so
`devicectl --console` receives audit settings and thermal transitions together
with `metal-HUD`; repro enables this automatically. Normal launches retain the
existing OSLog behavior.

The iOS capture exporter materializes internal trace file symlinks before
transfer because `devicectl device copy` rejects them. Captures use Apple's
public API and require `MTL_CAPTURE_ENABLED=1` for the explicit capture launch.
After saving, capture-only iOS runs exit directly: the iOS event loop otherwise
leaves a stopped black window after Bevy's exit event. Xcode replay can also
show a black phone screen and must be stopped before restoring the live game.

### Mirrored longer run: exclude from performance comparisons

`tmp/iphone-low-sustained.log` and `tmp/iphone-low-sustained-summary.json` record
an 86-second run of the new build, with capture instrumentation disabled but
**iPhone Mirroring active**. It starts Nominal, reaches Fair at 43.478 s and
Serious at 58.678 s. Late HUD intervals are about 32.8 FPS / 30.6 ms GPU duration.
The planned three-minute run was stopped once it degraded.

The user correctly identified Mirroring as additional work that can affect the
result. This run does not establish whether the changes improve or worsen
unmirrored performance; do not compare its thermal transition times with log9.
An unmirrored sustained acceptance run remains outstanding. Use Mirroring for
visual inspection and UI interactions, disconnect it for the performance run,
and run on the physical unlocked phone with matching display/settings conditions.

Xcode replay was stopped and the normal game restored without the scripted
camera or capture arguments. HUD interactions set 75% / 4× MSAA with controls
unlocked, confirmed in `tmp/iphone-restored-game.log`. The live game is visible
again. Tap/scroll attempts through Mirroring did not demonstrate camera movement;
physical multitouch control is not validated by this check.

## Grass candidate acceptance cache — Mac iteration (2026-09-06)

Per the user's direction, this iteration is tested on macOS. The current cache
stores one bit per original candidate after source-stable ownership, growth,
surface validity and coverage/competition checks. Generation keeps the original
lattice dispatch and checks the bit before expensive evaluation. View-dependent
visibility/LOD and accepted-candidate generation/drawing remain unchanged. This
avoids reevaluating candidates that source data always rejects without reducing
density, topology budgets or blade shading.

The mask arena is bounded to 1 MiB plus entry/build-queue metadata. Every allocated
candidate has a bit; unallocated, oversized and not-yet-ready fields use the reference
path. Visual diagnostic modes and authored quarter-lattice scheduling also use
the original path. Builds are limited to 262,144 padded lanes and four fields/frame.
Each 64-lane build workgroup writes two complete 32-bit words, including zero bits;
no stale acceptance survives source changes. Camera, wind and LOD changes reuse
acceptance; source revisions and compute shader reloads invalidate it. Source
revision changes currently rebuild all selected fields, so continuously changing
streaming coverage needs monitoring.

`--grass-candidate-reference` disables use/building for A/B tests. Logged fields:
`candidate_cache`, `candidate_cache_bytes`, `candidate_cache_builds`,
`candidate_cache_ready`, `candidate_cache_planned`. The reference switch retains
allocated buffers but does not read/build them for generation.

### Native validation

- App tests: 9 passed. Vegetation tests: 17 passed, four native tests ignored
  by default. The bitmask comparison passes explicitly on native Metal.
- The cache comparison checks **bit-identical per-bin multisets of generated
  32-byte instances**, matching draw counts and zero capacity drops. It covers
  moved cameras, wind phases, 4× MSAA, Balanced/FullReference/Authored density,
  page unload/reload, changed peer coverage/surface validity, forced fallback,
  and diagnostic-mode transitions.
- Fixture evaluations decrease from 24,550 to 10,859 (55.8%). The altered sparse
  field decreases from 12,275 to 2,250; forced fallback restores the original count.
  These count expensive evaluations, not dispatched lanes (unchanged with bitmasks).
- Images have mean absolute byte errors 0–0.002682, maximum 12 in the mask run.
  Atomic instance appends can change equal-depth MSAA winners at intersecting
  blades despite identical instance data. The image check requires mean error
  below 0.01 and fewer than 0.1% of channels differing by more than two, in addition
  to exact instance multisets. Diagnostic images are byte-identical.
- Artifacts: `tmp/candidate-mask-tests.log`, `tmp/candidate-mask-native-test.log`.

### Rejected compact-ID experiment and capture repeatability

The first implementation compacted accepted IDs into a 32 MiB bounded arena and
reduced generation dispatch size. Native population checks passed, but the matched
`mac-candidates-{reference,cached}-v4.gputrace` profiles showed a tradeoff:

| Metric | Reference | Compact IDs |
| --- | ---: | ---: |
| Total vertices | 2,084,772 | 2,084,772 |
| Generation kernel invocations | 3,331,298 | 2,598,498 |
| Generation ALU instructions | 1,320,699,904 | 1,051,805,696 |
| Generation time | 0.561 ms | 0.513 ms |
| Main opaque fragment invocations | 3,221,504 | 3,295,616 |
| Main opaque fragment ALU | 5,276,733,184 | 5,785,079,552 |
| Whole frame replay | 4.58 ms | 4.80 ms |

Both were profiled at Xcode's Medium state. Generation ALU fell 20.4%, but fragment
ALU rose 9.6% with changed append/draw order; total replay did not improve. This
motivated preserving the original candidate layout with the much smaller bitmask.
Do not report the compact-ID compute reduction as a whole-frame speedup.

Repro version 3 synchronizes wind to the camera's frame counter (120 Hz desktop,
60 Hz iOS); it does not change normal real-time wind. The capture trigger now uses
extracted application frame 600 instead of render-loop count 600. Capture logs
record the actual rendered camera/viewport/wind. The accepted v4 pair both records
main_frame=600, render_frame=601, camera=(2.513,1.730,-5.895), viewport=1920×1080,
wind_phase=5. Version-2 captures had different wind phases; the earlier v3 pair
had differing geometry counts. Neither pair is used for final timing claims.

### Live compact-ID run: not a successful performance result

`tmp/mac-candidates-cached-live.log` (75%, 4× MSAA, scripted low moving camera,
no capture instrumentation) records roughly 39–42 FPS after startup while thermal
state remains Nominal. This confirms the user's observation. Xcode replay had been
stopped before launch; compilation began after this game was terminated. An idle
Xcode and other desktop processes remained open, so external contention is not
ruled out. A later process snapshot found Spotlight/WindowServer activity, but it
does not prove the cause of the earlier slowdown. Xcode was fully quit before the
next live comparisons. This run is not evidence of a sustained improvement.

### Isolated live bitmask comparison

Xcode was fully quit, builds/native tests had finished, and only one game instance
ran at a time. Both release runs used the scripted low camera, 75% (1920×1080),
4× MSAA, VSync, wind, ground+grass and no capture instrumentation. Optional grass
atomics were off. Comparing Metal HUD report windows 20–115 (after startup):

| Metric | Reference | Bitmask |
| --- | ---: | ---: |
| FPS | 120.02 | 120.02 |
| Mean GPU duration | 3.978 ms | 3.927 ms |
| GPU duration p95 | 4.92 ms | 4.78 ms |

The ~0.05 ms difference is too small to establish a meaningful whole-frame win
from one pair. Both maintained VSync for the roughly one-minute checks, with Fair
thermal state in the settled samples. The 40 FPS slowdown was not reproduced
under these isolated conditions; this does not establish which earlier desktop
process or profiling state caused it. macOS power counters were unavailable because
the prior administrator authentication had expired (`sudo -n` refused access).

Artifacts: `tmp/mac-mask-{reference,cached}-live.log` and
`tmp/mac-mask-live-summary.json`. The bitmask allocation is 1,052,688 bytes including
metadata, and all 196 resident fields fit. The three other native Metal regression
tests also pass (`tmp/candidate-mask-other-native-tests.log`). No iPhone run was
performed in this iteration, and sustained thermal improvement remains unproven.

### Final matched bitmask counters

`tmp/mac-mask-{reference,cached}.gputrace` both record application frame 600,
render frame 601, the same camera, 1920×1080 viewport and wind phase 5. Xcode
reports 2,084,772 total vertices for both, at Medium performance state:

| Metric | Reference | Bitmask |
| --- | ---: | ---: |
| Generation kernel invocations | 3,331,298 | 3,331,298 |
| Generation ALU instructions | 1,325,285,376 | 1,236,937,216 |
| Generation time | 0.588 ms | 0.571 ms |
| Main opaque vertex invocations | 1,001,882 | 1,001,882 |
| Main opaque fragment invocations | 3,221,472 | 3,220,992 |
| Main opaque fragment ALU | 5,275,989,504 | 5,280,000,000 |
| Whole-frame replay | 4.72 ms | 4.63 ms |

Generation ALU decreases 6.7%; vertex work matches and fragment work is essentially
unchanged (+0.08% ALU). This avoids the compact-ID experiment's larger fragment-work
regression. It is a small compute reduction, not a demonstrated fix for heat or a
large whole-frame gain. Artifact: `tmp/candidate-mask-counter-comparison.txt` and
both exported `mac-mask-* Counters.csv` files.

Xcode was fully quit again after profiling. The normal release game was restored
through `tmp/YarraReleaseProfile.app`, with ground+grass, manual controls, 75%,
4× MSAA, prepass off, diagnostic counters off and the HUD visible. Its log is
`tmp/mac-grass-restored.log`. No build or GPU replay is left running alongside it.

A click-to-move smoke check moved the character and following camera and streamed
new pages (source revision 52→68). The cache rebuilt to 196/196 ready fields and
the settled live log/screenshot returned to 120 FPS, Nominal, about 3.1 ms GPU at
the overhead camera. This is a short functional check, not a sustained benchmark.
