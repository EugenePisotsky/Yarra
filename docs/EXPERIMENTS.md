# Experiment ledger

Decisions and observations through 2026-09-21. Current behavior belongs in [architecture](ARCHITECTURE.md); reproduction and measurement rules belong in [performance](PERFORMANCE.md). This ledger replaces the individual experiment reports. **Retained means present/useful, not necessarily a proven frame-time or energy improvement.** Rejected trials should not be revived without a new hypothesis.

Most desktop measurements used M2 Max; early phone work used iPhone 15 Pro Max / A17 Pro. Scenes, pixels, density, clocks and measurement methods changed. Compare only matched runs within an entry. A counter reduction, Xcode replay duration, sampled GPU span, Metal HUD duration and delivered frame interval are different measurements. Temporal GPU markers are known to undercount. No result here establishes mainstream-PC 1440p/60 acceptance or sustained phone 60 FPS.

**Density correction:** September 15–16 default-fixture captures labelled “44 roots/m²” actually used **66**. The cooked catalog is authoritative. Historical 32 m grass fixtures are different from the current 8 m authored world. Later 96/128-density fixtures are explicit variants. Old report labels inside raw archives remain historical; use this correction when interpreting them.

## Foundations and early optimization — through September 8

These results were recorded in the former `RENDER_AUDIT.md`, `PERFORMANCE_HANDOFF.md` and ground-cover reports at Git revision `2afce93`. Many original captures lived in ignored `tmp/`; they are historical observations, not newly verified measurements.

| Approach | Decision and evidence |
| --- | --- |
| Original card/ribbon grass and its old editor/schema | **Superseded and removed** by compact V2 fields, GPU candidates and procedural blades. Do not reconstruct the old path from historical design proposals. |
| Phone diagnostic composition/UI, prepass (GP001) | **Simplified** default diagnostic overhead. Early Clear test: composite/direct differed by ~2.3 ms and visible debug UI by ~1.7 ms; Clear direct/hidden UI ~0.97 ms, Flat ~1.36 ms. In a grass workload composite/direct differed by only ~0.22 ms. These are workload-specific differences, not fixed pass costs or thermal savings. Phone prepass default became off. |
| Skip zero-weight terrain texture samples | **Rejected.** Calls 145.55M→136.90M (−5.9%), L1 reads 8.18→6.88 GB, but ALU effectively unchanged and registers 149→152. September 19 retry also slowed the measured shader. |
| Stochastic terrain lookup cache | Texture variant **replaced** by bounded buffer cache: 18 hashes→6 loads, shader ALU −12.6%, registers 149→142, texture-call count unchanged; ~1.44 MiB for 49 tables, 8 MiB cap. Retained structural reduction; no sustained thermal win established. Later prepared ground supersedes much of this work in the normal path. |
| Live scheduler count | **Correctness fix retained.** Stop treating retired capacity slots (`arrayLength`) as live work. Did not explain phone heat. |
| Stationary grass placement reuse | **Retained.** Eliminates 2,331,840 generation lanes in the stationary fixture; draws/vertices unchanged (103 / 2,075,253). Movement regenerates; wind still updates. No thermal fix. |
| Prepared blade shape/wind | **Retained.** VS plus preparation ALU −51% on Mac / −29.5% on A17, traffic +17.5% / +13.4%; historic arena ~18 MB. Replay 4.107→3.982 ms / 18.41→18.03 ms does not establish a sustained live gain. |
| Earlier candidate rejection; optional counters off | **Retained.** A17 generation ALU −5.76%, replay 18.03→18.02 ms. Disabling optional atomics reduced generation atomic bytes 49.7%, replay 18.02→17.82 ms. Counters now default off on all platforms; neither change proved a heat fix. |
| Compact accepted-ID arena | **Rejected/replaced.** ~32 MiB arena, generation ALU −20.4% but fragment ALU +9.6%; replay 4.58→4.80 ms and poor live behavior. The historical 39–42 FPS run remains unexplained, not dismissed as external interference. |
| Source acceptance bitmask | **Retained.** ~1 MiB, preserves dispatch; generation ALU −6.7%, one live comparison 3.978→3.927 ms. Multiset/frozen-path checks support correctness; 0.05 ms is not a meaningful established frame win. |
| Prepared ground (GP002) | **Retained/default.** M2 Max 2560×1440, 4× MSAA, grass/prepass/UI off: single-replay opaque durations overhead 1.309→0.775 ms, low camera 1.914→0.615 ms. Not fixed-clock live evidence. Later native ASTC 8×8 saved 32 MiB relative to the original extra ~73 MB. Keep 4K: a 2K trial only suggested 1–3% HUD improvement without accepted quality. |
| MSAA color store discard (GP003) | **Retained with consumer fallback.** Captured ground frame wrote 19.40 MB less (35.25%); readback images identical. Replay timing unreliable. Applies to disposable multisample color, not depth or memoryless allocation; consumers that load it must preserve it. |

## Grass appearance and canopy — September 9–16

Appearance acceptance and performance acceptance are separate. The former grass-density/improvement/shadow/lighting documents at `2afce93` retain the full iteration narrative; local studies/screenshots may require the original ignored `.editor/` or `tmp/` files.

| Trial | Decision / measured effect |
| --- | --- |
| Raised shadow sheet and clump proxy | **Rejected and removed September 9.** Blurry repeated patterns, zoom boundaries and poor overhead appearance. On/off means 4.220/4.199 ms and 4.053/4.018 ms were noise-sized, not proof of zero cost. No production physical grass-shadow caster resulted. |
| Pixel/contact shadow fixture | **Test only.** Pixel16 gave subtle blade contact; Pixel8 lost detail. Missing ground shadows and false edges remained. No runtime GPU measurement or accepted integration. |
| Static root coverage | **Art study only**, not directional occlusion. One/two extra ground samples; ~1.42 MiB in a 16 m fixture, CPU bake 1–3.4 ms. No GPU-cost result. |
| Procedural blade bands | First version too faint; second made regular static stripes; third used irregular wind advection. **Historical optional experiment; game controls removed during cleanup**; not an accepted physical shadow solution and not GPU-timed. |
| Density, length and curvature studies | Fuller paired/arched blades compared ~45 versus 60 candidates; length/layering and width trials changed local catalogs. Fixed curvature sampling showed visible knees and was rejected; the geometric direction-error metric did not match appearance. These studies do not establish a density performance target. |
| Longer near-detail range | **Continued as quality work September 13.** Close game 3.254→3.484 ms (+7.1%); top-down 3.371→3.637 ms (+7.9%). An observed quality cost, not an optimization. |
| Folded near geometry; far ribbons (GP004) | Shared near geometry retained (15 inputs / 13 triangles). Far retention 0.39 and single far ribbon rejected for sparse coverage/curvature; restored bent pair with 7/15 low/high inputs and 0.65 retention. Single/restored means 4.246/4.239 ms were indistinguishable. Later low-width compensation 1.30 replaces 1/0.65 (~15.5% less compensation), not a uniform coverage reduction. |
| Directional/clump lighting | Dark-stripe trial rejected. Sheet-normal correction (18% rounded body normal, gloss retained) remained appearance work without timing acceptance. CPU tuft-ray tests found direct self-shadow more useful than AO, but remained regular/angular. **Field-scale appearance is the gate**; tiny tuft fixtures alone were explicitly insufficient. |
| Field canopy | Bright flat field and dirty-root/bright-gap variants rejected. Broader root mask improved soil continuity without solving uniform lighting. Selected field integration remains the current artistic treatment; gradient quality unresolved. The old G catalog comparison was removed during cleanup. |
| Shared canopy preparation | **Async fix retained.** 49-page 224×224 raster: 876,096 cells / ~3.5 MB. Old render packing microbenchmark 39.40 ms versus 0.38 ms without this work. Separate jobs, 150 ms coalescing, stale-result rejection and no new jobs while disabled. This is not a measured whole-frame saving. |
| Shading early exits (GP005) | **Retained.** Short whole-frame comparison 4.912→4.851 ms; too small to claim a heat win. |
| Compact candidate scheduling / preparation priority (GP006) | **Reverted at user request.** 67–69% fewer fixed candidate slots but only ~0.06 ms whole-frame difference; complexity not justified. Zoom result inconclusive. |

Reference ideas from the Ghost of Tsushima grass talk—paired geometry, clumping, shared wind, stable distant representation, shadow/contact approximations—are design inputs, not evidence about this renderer or undocumented Ghost of Yōtei implementation.

## Controlled grass profiling — September 16

GP007 established the PC review/acceptance gap; GP008 added the snapshotting runner. GP012/GP014 were proposed follow-ups, not completed measurements. Raw directories below retain JSON, manifests and evidence archives; generated prose is consolidated here.

| ID / question | Outcome | Evidence |
| --- | --- | --- |
| GP009–010: windowed 1440p 60/120/120/60 | Short repeat exposed 120-FPS thermal/cadence limits. Corrected reporting: average ~60 can hide alternating 8.33/25 ms intervals and p95 25 ms. Later frame-pacing work addresses a distinct presentation problem. | [report](performance/20260916-190422/report.json) |
| GP011: grass on/off/off/on at 60 | All ~60 FPS, nominal p95 16.67 ms. Off used less power, but off visits themselves differed ~1.21 W combined; no precise attributable grass saving. | [analysis](performance/20260916-192957/analysis.json) |
| GP013: native GPU attribution | Three-second default/adaptive trace: main rendering dominated, scheduler small. No power counters. | [analysis](performance/20260916-gpu-attribution/analysis.json) |
| GP015–016: fullscreen baseline/smoke | Corrected 66-density, 2592×1626 internal / 3456×2168 output, 15 minutes targeting 120: 119.946 average, 119.897 final five minutes, CPU+GPU ~23.43 W. Transient pressure recovered ~6:10. User reported no loud fans in this run; not complete-game or phone acceptance. Smoke only established actual fullscreen pixels. | [analysis](performance/20260916-200703/analysis.json), [context](performance/20260916-200703/follow-up-context.json), [smoke](performance/20260916-200334-fullscreen-smoke/report.json) |
| GP017: actual density/capacity | Corrected 44→66 label. Explicit 66/72/96/128 variants had no sampled capacity drops; adaptive radius trades distance quality for bounded work. | [diagnostics](performance/20260916-density-preparation/density-diagnostics.json) |
| GP018: larger preparation arena at 96 | **Opt-in only.** +48 MiB; vertex ALU −42.7%, summed replay work −21.6%. No demonstrated energy improvement. | [comparison](performance/20260916-density-preparation/preparation-comparison.json), [validation](performance/20260916-density-preparation/preparation-validation.json) |
| GP019–020: enlarged/default 96 paired runs | Enlarged 15-minute run: 119.600 average, CPU+GPU ~25.13 W, worst second 77.31 FPS; smoothness not accepted. Default also had late dips (worst ~73 FPS) and final-five-minute GPU power ~1.53 W lower. **Do not promote the enlarged cache.** | [enlarged](performance/20260916-212946/analysis.json), [paired control](performance/20260916-220712/comparison.json) |
| GP021: early heat reanalysis | Moving minutes 1–4 ~25.1 W CPU+GPU; stationary warmup was a different workload. Reanalysis, no new run. | [analysis](performance/20260916-early-heat/analysis.json) |
| GP022: skip overwritten low-pair vertex work | **Retained.** Vertex ALU −13.74%, vertex count unchanged; 12 readback cases identical. Power unmeasured. | [analysis](performance/20260916-low-vertex/analysis.json) |
| GP023: clump light fragment→vertex | **Reverted.** Fragment ALU −6.06%, vertex +5.72%, total −1.52%; 15 images identical, timing overlapped. No power run. | [analysis](performance/20260916-clump-vertex/analysis.json) |

## Terrain and clouds — September 19–20

| Change | Decision / limits | Evidence |
| --- | --- | --- |
| Contact validation / streaming responsiveness | **Retained.** Debug dense-surface microbenchmark 1913.325→1.421 ms after removing repeated assertion scans; not a release-frame gain. Single release pair 59.77→59.91 FPS, worst frame 47.97→33.32 ms. | [comparison](performance/20260919-terrain-streaming/game-comparison.json) |
| Actor-priority budget and crack-free cover | **Retained correctness work.** Stress 450560→260096 triangles, zero blocked actor contact; hill fixtures used 1,048,576 normal / 262144 tight budgets. Ground-fit outliers fell 44/41→2. No timing claim. | [evidence](performance/20260919-terrain-budget/evidence.zip) |
| Near shader indexed lookup | **Retained.** Avoid copying a 304-byte record. Actual game 2592×1456, 4× MSAA, native/prepass: short 20-second mean GPU 8.51→5.92 ms, updates 117.35→120. Zero-weight retry 6.70→6.89 ms, removed. No power measurement. | [summary](performance/20260919-terrain-gpu/summary.json) |
| Close-ground first / direct fields / first probe | **Retained.** Short 40-second comparison 5.94→5.31 ms (~11%); zero-weight variant 5.47 ms, rejected. | [short summary](performance/20260919-terrain-headroom/short-summary.json) |
| Hierarchy ten-minute battery soak | **Unresolved sustained cost.** Minute 1 119.96 FPS / 5.57 ms, minute 6 110.99 / 9.13, final 116.94 / 7.20; total 116.15, worst second 92.54. Legacy return 119.95 / 4.58. Residency stable (256 grass pages versus legacy 49), no memory-growth evidence. Late partial power telemetry cannot identify one thermal cause. Short shader wins did not solve this. | [soak](performance/20260919-terrain-headroom/soak-summary.json), [partial power](performance/20260919-terrain-headroom/partial-power-summary.json) |
| Admission queue review / neighbor bypass | **Queue fix retained:** scan bounded candidates and count actual admissions instead of letting two blocked candidates prevent later fitting work. Decode limit is 16 pages, not aggregate bytes (sample largest 16: 411104 bytes). Neighbor-bypass timing 5.79 / 5.91 / return 5.89 ms; no gain, diagnostic removed. | [summary](performance/20260919-terrain-review/summary.json) |
| Cloud shadow reuse and lower-res direct blend | **Superseded for Balanced by cache below.** Nominal whole frame 7.03→6.11 ms, Off 5.19; subsequent High comparison ran hotter. Historical local `tmp/cloud-performance/comparison.json`, no tracked raw capture. | Git `2afce93`, former `CLOUD_PERFORMANCE.md` |
| Balanced 512² strip cache, at most 32 updates/s | **Retained/default.** Trace pixels ~−93%, observed whole frame 6.45→4.64 ms. Off measured later at 5.92 ms while hot, so not a controlled cloud-cost comparison. ~215-second session, no watts; heat unresolved. Local `tmp/cloud-performance/cached-comparison.json`. | Git `2afce93`, former `CLOUD_PERFORMANCE.md` |

## Upscaling and temporal grass — September 20–21

Primary [summary](performance/20260920-metalfx-investigation/summary.json) and [commands](performance/20260920-metalfx-investigation/commands.json): Release M2 Max, 3456×1942 output, grass off, bloom on, 120-FPS target, 15-second warmup / 10-second sample unless noted. 50% means 1728×971 internal. Clocks unlocked; system GPU watts are not process-exclusive power.

| Approach | Decision / observation |
| --- | --- |
| Native / Spatial / Temporal baseline | Native 100% + 4×: HUD 6.36 ms / GPU 24.2 W; Spatial 50%: 3.97 / 10.7; Temporal 50%: 7.07 / 21.9. Temporal with linear bypass 4.01 ms; no bloom 6.85. **Spatial remains Auto on supported devices; Temporal is a prototype.** |
| GPU timing validity | Temporal bloom on/off markers 6.63/3.93 ms versus HUD 7.07/6.85. **Marker undercount unresolved.** F1's sampled GPU span cannot settle total Temporal cost. No repeated scaler recreation, separate submission queue, CPU pixel copy or continuous reset was identified. |
| Native/compact input packing | Real input native ~2.31 ms versus compact ~2.282 (only ~0.03 ms). Flat input ~2.37 ms at large output versus ~1.44 at 720→1440. Packing added work without useful saving and was removed. No allocation/auto-exposure configuration fix established. |
| Shared motion attachment | Temporary ~25.6 MiB saving, Temporal 7.15→7.26 ms; grass-off test did not validate grass. **Reverted.** [Follow-up](performance/20260920-metalfx-investigation/followup-results.json), [patch](performance/20260920-metalfx-investigation/shared-motion-experiment.patch). |
| Temporal 33% | Short grass-off 5.49–5.53 ms versus native 6.30. Subsequent release gameplay heat/quality rejected by user. **Not an accepted preset or half-power result.** |
| Empty-frame / MSAA isolation | Stock Bevy ~0.79 ms, game Linear33 ~1.25, Spatial33 ~2.74; incomplete attribution. Native-only probe also reproduced Temporal cost. With grass: native no-MSAA 4.92, native4× 6.47, Temporal50 7.64 ms. [Empty](performance/20260920-metalfx-investigation/empty-frame/results.json), [native Temporal](performance/20260920-metalfx-investigation/temporal-empty/results.json), [MSAA](performance/20260920-metalfx-investigation/msaa-grass/results.json). |
| Packed HDR | RGBA16F 4.835 ms / 668 MHz / 2.16 W → RG11B10F 3.494 / 753 / 5.02; RGBA repeat 4.842 / 2.14. Clock/power difference prevents a free performance claim; reduced precision, no alpha/negative range. **Not adopted.** [Results](performance/20260920-metalfx-investigation/config-options/results.json). |
| Direct Linear composition | **Retained.** Removes ~25.6 MiB image and a separate pass; empty75% 1.43→1.42 ms is not a meaningful time win. [Results](performance/20260920-metalfx-investigation/linear-composition/results.json). |
| Temporal pose-buffer reuse | **Retained** stable-pose reuse; default previous-pose allocation ~19 MiB. No isolated whole-frame result. |
| Direct tone-map output | **Retained with fallback contract.** Full grass/wind/bloom, five-second warmup / ten-second sample: standard sampled medians 7.81/7.84 ms, direct 7.68/7.63. Marker limitations apply; native MetalFX command-buffer medians still ~2.9–3.1 ms. Readback max difference 1/255; restoring standard exact. Only temporal full-target views with no later LDR effects; unsupported conditions use original path. Not a Temporal regression fix. Historical details: `UPSCALING.md` at `2afce93`. |
| Strafe blur / motion validation | **Blur unresolved**, reproduced with wind off, stable roots/no resets, prepared/fallback and unlit grass. Reprojection p95 error ~0.001 input pixel. Analytic jitter/motion tests favored implemented signs/scales: static RMSE 0.006701 versus reversed jitter 0.038347; correct motion 0.076813 versus double/half/opposite ~0.16–0.17. These numerical checks do not establish visual quality. [Images](performance/20260921-temporal-grass/strafe.png); former camera-motion report at `2afce93`. |
| Reactive mask / more input pixels | R8 strength 0.35 did not help; 1.0 jagged. **Mask code removed.** 75% still blurred, 100% better but not a fix; exposure-hint changes prevent treating those captures as a controlled resolution study. |
| Exposure correction | **Retained correctness fix:** already camera-exposed HDR needs unit pre-exposure instead of a second exposure hint. Gray ~63–71→112–124. Not a blur or power fix. [Before](performance/20260921-temporal-grass/exposure-before.png), [after](performance/20260921-temporal-grass/exposure-after.png). |

## Presentation and input — September 21

| Change | Decision / evidence |
| --- | --- |
| Display link plus Metal minimum presentation interval | **Retained.** Physical 3456×1942, Linear50%, 4×, grass/bloom, 60-FPS target, 15-second warmup / ten-second sample: near-16.67-ms intervals timer 81.42%, display-link-only 84.30%, display link + Metal 99.65%; p95 25 / 25 / 16.67 ms. Patched timer control 41.69%, p95 25 ms; final two use the same executable. Normal Auto follow-up 60 seconds: 99.86%, p95 16.67 ms. Cadence result, not thermal acceptance. [Summary](performance/20260921-frame-pacing/pacing-summary.json), [manifest](performance/20260921-frame-pacing/manifest.json). Preserve the [wgpu patch](../third_party/wgpu-hal/YARRA_PATCH.md) until its upstream replacement is verified. |
| Runtime FPS changes | **Retained.** Follow-display / 60 / 30 / 120 transitions worked without hangs; final60 still had occasional p95 25-ms misses. Local `tmp/runtime-fps-20260921`; old runtime-FPS report at `2afce93`. |
| Trackpad clamp/smoothing | **Retained.** Per-frame aggregate clamp lost more distance at 60 than120. Per-event clamp plus interval-integrated exponential smoothing (rate20) passes identical 120-event streams grouped into60/120 updates. |
| Native input trace | **User-reported uneven feel remains unresolved.** 1315 native events all consumed with matching batches across1202 updates/20 seconds; p95 event→read15.85 ms. Clamp removed34/13518 horizontal pixels (~0.25%); no queue loss demonstrated. Render/main-clock difference median0.375 / p952.629 ms. Local `tmp/camera-input-1789991480-33519/analysis.json`, former input report at `2afce93`. Timestamp resampling/alternate smoothing are hypotheses, not proven fixes. |

## Open gates and maintenance

The remaining gates are sustained terrain/whole-game power, Temporal cost and motion quality, field-scale grass lighting, target-PC acceptance, and physical-phone heat/60-FPS delivery. Keep correctness references until their replacements pass the relevant gate. Existing counters often identify less work without demonstrating better delivered frames or lower power.

For each new experiment update one row or add a compact entry: date/ID, hypothesis, retained/rejected/unresolved decision, device/build and workload, before/after metric, limitation, evidence link. Do not create another standalone report. Raw evidence lives under `performance/`; prose removed during consolidation remains in Git at `2afce93`. New architecture proposals and cleanup work belong in [refactoring](REFACTORING.md), not this results ledger.
