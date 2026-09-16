# Grass performance and optimization log

**Catalog correction (GP-017):** decoding the actual database proves the measured
baseline was **66 roots/m²**. Earlier 72-root labels came from the RON reference
and a hard-coded HUD label. The runtime payload hash and all recorded power/timing
results are unchanged. See [density diagnostics](GRASS_DENSITY_DIAGNOSTICS_20260916.md).

Updated: **2026-09-16**. Start here for the current performance conclusions,
experiment outcomes and next tests. The detailed investigations linked below
retain their original conditions and limitations.

**Current priority, clarified after GP-020:** reduce the heat buildup in the first
few minutes at fullscreen 120 fps. Late pacing dips remain recorded but their
native trace is deferred. [GP-021 early-load review](GRASS_EARLY_HEAT_20260916.md)
finds about 25.1 W CPU+GPU in moving minutes 1–4 at 96 roots with default
preparation, versus 26.5 W with enlarged preparation. [GP-022](GRASS_LOW_VERTEX_20260916.md)
now implements the low-detail vertex shortcut: 13.74% fewer main-pass vertex ALU
instructions, identical output in 12 frozen comparisons, no extra buffers.
Retained as a bounded work reduction; repeated replay timings show no reliable
speedup and power/heat benefit remains unmeasured. Only short GPU validation was
performed; no new sustained run is requested.

## Target and current conclusions

- Target experience: **1440p, sustained 60 fps, maximum production grass quality
  on mainstream gaming PC hardware**, with room for a populated game. Exact PC
  test GPUs and the maximum affordable density remain unvalidated.
- Immediate development question, clarified after GP-011: **can this Mac sustain
  120 fps fullscreen for an extended period with denser grass and acceptable heat?**
  Showing that the current scene runs at 60 fps does not answer this. The user
  clarified that normal play is fullscreen. Use the normal 75% world scale as the
  initial Mac baseline and record actual pixels. In GP-015 the user reported fans
  were not loud; case heat was not checked.
- Art reference: Yōtei, with implementation guidance from the Tsushima grass
  presentation. Similar appearance does not establish identical density or cost.
- Development machine: **M2 Max, 38 GPU cores, 32 GiB**, not a base M2. Its power
  and timing results cannot be converted into a PC budget with one multiplier.
- Current measured baseline is 66 authored roots/m² with production
  density/geometry LOD and 4× MSAA. Full Reference density is a diagnostic mode,
  not the definition of maximum shipping quality.
- The first powered 1440p suite measured approximately **5–6 W CPU+GPU at average
  60 fps**, versus **20.6 W at the first 120 fps visit**. The second 120 visit
  encountered thermal pressure, lower clocks and approximately 103 fps.
- That failure is not an absolute hardware ceiling. The subsequent **fullscreen
  GP-015 held 119.946 fps over 15 measured minutes**, including 119.897 fps in the
  final five minutes, at 2592×1626 world / 3456×2168 presentation pixels. Its thermal
  pressure was elevated around minutes 3–6, then nominal for the final nearly nine
  minutes. Average CPU+GPU power was 23.43 W. Current density has a sustained
  fullscreen baseline. The subsequent 96-root candidate is recorded below.
- GP-019 measured **96 roots/m² with enlarged preparation** for 15 minutes:
  119.600 fps overall, 119.266 in the last five minutes, 25.13 W CPU+GPU overall.
  Initial pressure recovered by 7:05, matching the user's hot → louder fans →
  quieter/stabilized impression. Three later short stalls reached 84/94/77 fps
  while pressure was nominal. Near-120 capacity is demonstrated for most of the
  route; consistently smooth delivery and matched-density energy saving remain
  open. GP-020 supplies the matching default-preparation control.
- GP-020 confirms **the same binary/database/shaders/canopy**, changing only
  preparation capacity. Default/enlarged final-five-minute GPU power is
  **19.48 / 21.01 W**, CPU+GPU **23.84 / 25.38 W**, app fps **119.394 / 119.266**.
  Enlarged preparation reduces HUD GPU duration (6.44 → 4.90 ms) but has no
  demonstrated power saving. Both have late stalls roughly 64 seconds apart;
  default's worst one-second app window is 73 fps. User says end-of-run fans/heat
  felt mostly the same. Keep the production allocation unchanged. The subsequent
  user priority is early heat reduction (GP-021); late timing diagnosis is deferred.
  This is one pair, not a repeated
  energy-regression estimate.
- Both 60 visits in that first suite contain uneven HUD presentation intervals.
  The subsequent grass on/off suite, with matching inputs and no intentional
  setup changes reported by the user, has 16.67 ms interval p95 in all visits.
  The earlier pacing issue is intermittent and remains unexplained.
- The on/off suite stays nominal at 60 fps. Grass-on measures **4.68–4.91 W CPU+GPU**;
  grass-off measures **2.60–3.81 W**. Disabling grass lowers power in both comparisons,
  but the large off-visit spread prevents a precise grass-only attribution.
- The exact grass contribution, sustained full-scene budget, useful density ceiling,
  and mainstream-PC acceptance remain open. There is no demonstrated general
  solution to heat or iPhone performance.
- Measurement clarification after GP-015: the settled GPU-power estimate is
  consistent (19.21–19.61 W across final-five-minute 30-second groups; 19.41 W mean),
  versus 0.032 W before launch. This is useful energy evidence. What remains missing
  was fine-grained grass draw/limiter attribution (now GP-017) and a higher-density
  ceiling (still open after GP-019).
  The user normally closes other heavy workloads; focus is verified. Do not use
  unsupported background-app speculation to explain away the outcomes.
- GP-013 now has a direct native GPU trace: main opaque vertex/fragment stages
  average 2.203/1.985 ms per encoder, grass generation/finalization 0.516 ms,
  blade preparation 0.071 ms and scheduling 0.010 ms in the sampled interval.
  Main rendering combines grass/terrain/objects and stages can overlap. This
  establishes an initial breakdown, not grass-only draw cost or fixed-clock headroom.

Current implementation review: [assessment](GRASS_PC_PERFORMANCE_ASSESSMENT.md).
How to run tests: [profiling tools](GRASS_PROFILING_TOOLS.md).
Latest power measurements: [matched 96-root preparation comparison](GRASS_PREPARATION_POWER_COMPARISON_20260916.md).
Current interpretation and work priority: [early heat buildup](GRASS_EARLY_HEAT_20260916.md).
Enlarged candidate: [96-root fullscreen 120 for 15 minutes](GRASS_DENSITY96_SUSTAINED_20260916.md).
Original fullscreen baseline: [66-root results](GRASS_FULLSCREEN_120_20260916.md).
Earlier sessions: [grass on/off](GRASS_ON_OFF_20260916.md) and the
[windowed 60/120 baseline](GRASS_POWER_BASELINE_20260916.md).

## Experiment index

IDs stay stable. “Retained” describes the implementation decision; it does not
claim a measured sustained power improvement. Older rows summarize their linked
records and were not rerun for this log. Their timings must not be pooled with
today's different scene, density, resolution or performance state.

| ID / date | Experiment | Recorded outcome | Evidence and limits |
| --- | --- | --- | --- |
| GP-001 / through Sep 6 | Scheduler bounds, placement reuse, blade preparation, candidate rejection/caching; baseline isolation | Correctness fix and several changes retained; compact accepted-ID approach rejected | [Historical handoff](PERFORMANCE_HANDOFF.md). Reduced work in some stages, with memory/traffic tradeoffs. No demonstrated sustained iPhone solution. |
| GP-002 / Sep 7 onward | Prepared ground material | Retained, subsequently integrated as game default | [Ground breakdown](GROUND_GPU_BREAKDOWN.md), [integration](PREPARED_GROUND.md). Verified shader-work reduction; earlier near-equal live HUD duration was insufficient reason to reject it. |
| GP-003 / Sep 7 | Resolve/discard unneeded multisample color | Retained | [MSAA storage](MSAA_COLOR_STORAGE.md). About 19.40 MB fewer device writes in the matched captured ground frame, identical screenshots. Replay timing variation prevents a reliable live speedup claim. |
| GP-004 / Sep 14 | Reduced far paired retention and single far ribbon; restored bent low pair | Cheaper-looking alternatives rejected visually; 15/7 near/far topology and 0.65 paired retention restored | [Restoration](GRASS_FAR_LOD_RESTORE.md). Historical source density was 44 roots/m². Single/restored whole-game means 4.246/4.239 ms are within variation. Geometry counts alone did not predict useful performance. |
| GP-005 / Sep 15 | Canopy and narrow-sheen early exits | Retained | [Shading experiment](GRASS_SHADING_PERFORMANCE.md). Frozen-scene output equivalence verified; short low-camera HUD means 4.912 → 4.851 ms. No meaningful overhead-view or sustained power gain established. |
| GP-006 / Sep 15 | Compact candidate scheduling and preparation priority | Reverted at user's request | [Reverted experiment](GRASS_SCHEDULING_PREPARATION.md). 67–69% fewer candidate slots in fixed views, about 0.06 ms short whole-game difference; extra queue/pipeline complexity not justified. Zoom comparison inconclusive. |
| GP-007 / Sep 16 | Current implementation, configuration and Tsushima/Yōtei review | Investigation completed; performance ceiling unresolved | [Assessment](GRASS_PC_PERFORMANCE_ASSESSMENT.md). Current runtime catalog matches Sep 15 catalogs; those tests already had today's 66-root population. Canopy settings differ. Short 4.9 ms HUD runs provide no fixed-clock headroom estimate. |
| GP-008 / Sep 16 | Controlled profiling runner | Implemented and exercised | [Tools](GRASS_PROFILING_TOOLS.md). Explicit internal resolution, real-time routes, frame caps, warmup, input hashes, one finite privileged power collector, offline reports. Normal game defaults unchanged. |
| GP-009 / Sep 16 | Powered 1440p 60/120/120/60 comparison | Measured baseline; thermal limiting strongly supported at repeated 120 | [Results](GRASS_POWER_BASELINE_20260916.md), [preserved evidence](performance/20260916-190422/README.md). Same inputs throughout; short idle gaps retain thermal history. No grass-specific or PC acceptance claim. |
| GP-010 / Sep 16 | Detect pacing hidden by average FPS | Report correction implemented; presentation cause unresolved | [Pacing analysis](GRASS_POWER_BASELINE_20260916.md#presentation-pacing-and-report-correction). 60 fps HUD intervals cluster near 8.33/25 ms. Analyzer now warns on excessive interval p95; original report preserved. Renderer unchanged. |
| GP-011 / Sep 16 | Grass on/off/off/on at 1440p60 | Measured; reduced power with grass off, precise saving uncertain | [Results](GRASS_ON_OFF_20260916.md), [evidence](performance/20260916-192957/README.md). All visits approximately 60 fps, nominal pressure, 16.67 ms HUD interval p95. Off visits differ by 1.21 W combined power. No optimization or pacing fix was applied. |
| GP-013 / Sep 16 | Direct GPU intervals at matching fullscreen settings | Initial pass/stage breakdown measured; individual draw/limiter attribution open | [Native trace and evidence](performance/20260916-gpu-attribution/README.md). Three-second post-warmup trace, actual GPU execution intervals. Main rendering dominates the named stage intervals; scheduling is small. Default/adaptive performance state, no power collection or shader counters. |
| GP-015 / Sep 16 | Current-density fullscreen 120 fps, 15 measured minutes | Baseline completed; near-120 sustained despite temporary thermal pressure; larger density ceiling open | [Results](GRASS_FULLSCREEN_120_20260916.md), [evidence](performance/20260916-200703/README.md). 119.946 fps overall, 119.897 final five minutes, 23.43 W CPU+GPU. Pressure returned to nominal after 6:10. User says fans were not loud; case heat unknown. No grass optimization. |
| GP-016 / Sep 16 | Fullscreen profiling controls and resolution correction | Tooling verified; used for GP-015 | [Control check and evidence](performance/20260916-200334-fullscreen-smoke/README.md). Actual fullscreen surface 3456×2168, world 2592×1626. Brief 12-second measurement meets 120 fps; no power collector. Historical presets retain windowed 1440p. This is a measurement change, not a renderer optimization. |
| GP-017 / Sep 16 | Actual database catalog, per-draw attribution and isolated density variants | Baseline corrected to 66 roots/m²; grass/far vertex work isolated; numeric variants prepared | [Diagnostics](GRASS_DENSITY_DIAGNOSTICS_20260916.md), [evidence](performance/20260916-density-preparation/README.md). 66/72/96/128 sampled capacity drops zero; adaptive detail radius remains a quality tradeoff. |
| GP-018 / Sep 16 | Larger blade-preparation arena at 96 roots/m² | Work reduction validated; GP-019/020 show no energy win; remains opt-in | Same geometry/vertex counts; main-pass vertex ALU −42.7%, summed replay work −21.6%, +48 MiB. [Replay results](GRASS_DENSITY_DIAGNOSTICS_20260916.md). Default allocation unchanged. |
| GP-019 / Sep 16 | 96-root / enlarged-preparation fullscreen 120, 15 measured minutes | Mostly near-120; thermal recovery followed by short nominal-pressure stalls; smoothness not accepted | [Results and next control](GRASS_DENSITY96_SUSTAINED_20260916.md), [evidence](performance/20260916-212946/README.md). 119.600 fps, 25.13 W CPU+GPU; worst one-second window 77.31 fps. Cadence-window warning added; original report preserved. |
| GP-020 / Sep 16 | Matched 96-root / default-preparation sustained control | Mostly near-120 with recurring late dips; enlarged allocation not promoted for energy | [Comparison and decision](GRASS_PREPARATION_POWER_COMPARISON_20260916.md), [evidence](performance/20260916-220712/README.md). Default uses 1.53 W less final-five-minute GPU power in this pair. Timing trace subsequently deferred in favor of GP-021 heat work. |
| GP-021 / Sep 16 | Early heat buildup and revised optimization priority | Existing logs reanalyzed; low-detail vertex-path candidate selected (subsequently implemented in GP-022) | [Early heat review](GRASS_EARLY_HEAT_20260916.md), [analysis](performance/20260916-early-heat/analysis.json). Moving minutes 1–4 use ~25.1 W CPU+GPU with default arena. Stationary asset warmup is a different workload. No new power/trace run. |
| GP-022 / Sep 16 | Skip overwritten vertex work for fully low-detail paired grass | Retained; arithmetic saving verified, heat benefit open | [Implementation and evidence](GRASS_LOW_VERTEX_20260916.md). Main-pass VS ALU instructions −13.74%, vertices unchanged; 12 byte-identical shader A/B cases. Two profiles per capture at Medium do not demonstrate a reliable speedup. No power run. |

## GP-017 — actual catalog and individual grass draws

The database catalog is 66 roots/m²; earlier 72-root labels were incorrect.
The RON reference and hard-coded HUD had diverged from the actual runtime.
Input hashes and existing timing/power evidence are unchanged.

A matching fullscreen Metal capture attributes 64.88% of profiled replay work
to grass, split into 42.12% vertex and 22.76% fragment work. The paired low-detail
draw accounts for 62.21%. The old preparation arena falls back for about 96k
blade forms at baseline, 182k at 96 roots/m² and 272k at 128. Sampled root-capacity
drops are zero. See [draw/density diagnostics](GRASS_DENSITY_DIAGNOSTICS_20260916.md).
These are offline/sampled observations, not sustained capacity acceptance.

GP-018 completed an opt-in preparation-capacity comparison at 96 roots/m², retaining
geometry and shading. Normal game allocation remains 131,072 blades. Candidate
524,288 uses 48 MiB more arena storage. Matched replay falls 7.218 → 5.660 ms summed encoder time; main-pass vertex
ALU falls 42.7% with identical vertex counts. Preparation work/traffic increases.
Counter, screenshot/repeat and short preset checks completed. GP-019 subsequently
completed the frozen 96-root/524288-preparation 15-minute powered preset. It shows
near-120 average delivery with initial thermal pressure and three later short
stalls under nominal pressure. The user reported heat and a fan ramp followed by
quieter operation. GP-020 completed the matching 96-root/131072 preparation
control: GPU duration is longer but observed power is lower, and late stalls
recur. There is no demonstrated same-density energy benefit, so the larger arena
remains opt-in. Consistently smooth delivery remains open, but the user has
prioritized reducing early heat/power over diagnosing the brief late dips.

## GP-009 — first powered baseline

Question: how do 60 and 120 fps change power and sustained delivery at the same
1440p grass workload on this Mac?

Configuration: low-walk, Balanced density, 2560×1440 internal world pixels, windowed
1280×720 logical / 2560×1440 physical presentation, 4× MSAA,
wind on, counters off, repro depth prepass off. Thirty-second warmup plus two-minute
measurement per visit, 15-second gaps. AC/charging recorded at startup; Low Power
Mode off. Binary, database, canopy and shader hashes match across all four visits.

| Visit | App fps | GPU W | CPU + GPU W | GPU active MHz | Powermetrics pressure | HUD interval p95 |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| 01 — 60 | 60.00 | 3.65 | 5.18 | 702 | Nominal | 25.00 ms |
| 02 — 120 | 119.73 | 16.05 | 20.59 | 1,361 | Nominal | 8.34 ms |
| 03 — 120 | 103.35 | 9.33 | 12.23 | 813 | Heavy | 25.00 ms |
| 04 — 60 | 60.00 | 4.15 | 5.74 | 712 | Moderate → nominal | 25.00 ms |

Interpretation: the first 120 visit consumed about four times the first 60 visit's
CPU+GPU power. Its GPU duration was shorter, 5.16 versus 9.96 ms, alongside much
higher clocks. This directly demonstrates why duration alone cannot judge energy
efficiency. The repeated 120 visit's lower watts accompanied missed cadence and
thermal pressure. At final 60, pressure returned to nominal about 19 seconds into
measurement. The final `CHECK` includes that recovery and the pacing warning.

Limits: power estimates cover CPU/GPU subsystems, including other processes;
they are not isolated grass watts or total laptop power. HUD samples are correlated.
These two-minute measured windows are not a long sustained acceptance test. The
scene has little of the future game's content, and no PC GPU was measured.

Original next step: keep the current renderer as the comparison baseline and use
matched 60 fps power plus pacing and work counters; GP-011 completed the on/off
comparison. After the user's sustained-120 clarification, prioritize diagnosis at
that cap and its agreed resolution. Do not reduce grass density or claim an
optimization based on these whole-scene measurements alone.

## GP-011 — grass on/off

Same baseline inputs as GP-009, at 1440p60 with 30-second warmup and 120-second
measurement per visit. All four visits pass the individual-run checks. Combined
CPU+GPU power is 4.68 / 3.81 / 2.60 / 4.91 W in on/off/off/on order; GPU power is
2.97 / 2.20 / 0.92 / 3.18 W. Mean on/off combined power is 4.79 / 3.21 W, but paired
differences range from 0.86 to 2.30 W. Report that variation rather than assigning
a precise percentage of system cost to grass.

The earlier presentation pattern did not reproduce, without a code change or
intentional setup change reported by the user. All visits have nominal thermal
pressure and 16.67 ms HUD interval p95, with occasional longer intervals. GP-012
remains open. GPU clocks differ between on/off, so HUD-duration subtraction is not
an isolated grass timing. Terrain visibility changes when grass is removed.

Decision: keep the current baseline and move to matched pass/work attribution
before selecting an optimization. See the [detailed record](GRASS_ON_OFF_20260916.md)
for controls, thirty-second power windows and limits.

## Next tests and open questions

| ID | Status | Question / next action | Decision it should support |
| --- | --- | --- | --- |
| GP-012 | Open; not reproduced in GP-011 | Reproduce/trace uneven 60 fps presentation; consider limiter, render queue, compositor/display behavior and completion timing | Explain intermittent pacing before treating it as fixed. Same inputs produced different pacing across sessions. |
| GP-013 | Pass and grass draw attribution completed (GP-017) | Split the expensive main render pass into grass/terrain/other draws and inspect hardware limiters at matching fullscreen settings | Select one bounded optimization from measured expensive work. Compare at the same density and quality and preserve visual review. Direct GPU time is not energy or a universal capacity percentage. |
| GP-014 | Not yet performed; shipping validation | Native 1440p60 tests on named mainstream PC GPUs with a populated scene | Set a production grass budget and density/quality ceiling; Mac results alone cannot close this item. |
| GP-015 | Baseline and matched 96-root pair completed; density ceiling open | GP-022 removes redundant vertex arithmetic; early-load power benefit remains unmeasured. Late trace deferred; longer thermal acceptance only after an energy result | Find useful density/quality with acceptable heat/noise. Do not convert GPU active residency or HUD duration into a linear density allowance. |

The previously agreed grass-on/off command has completed as GP-011. No additional
long power run is required merely to repeat that first attribution question. A
small claimed optimization gain will need more repeatable power evidence, given
the off-visit spread. GPU pass/draw attribution completed as GP-017. GP-018/019's
matching default-preparation control completed as GP-020. No further long power
run is needed merely to confirm that both capacities have the late stalls.
The original [timing-capture proposal](GRASS_PREPARATION_POWER_COMPARISON_20260916.md#decision-and-next-diagnostic)
is now deferred: a short capture at the known late event still requires around
12 minutes of game runtime, which does not meet the user's no-long-runs constraint.
The user has prioritized early heat buildup instead. Follow
[GP-022](GRASS_LOW_VERTEX_20260916.md) for the implemented candidate and evidence;
do not claim its arithmetic saving proves heat reduction or fixes the separate pacing dips.

For GP-015, the known windowed 1440p120 failure is already enough to motivate
diagnosis; a longer identical windowed run is not required just to rediscover it.
Fullscreen is a different baseline and has now been measured. The previous description
of “normal 1080p” applied only to the 2560×1440 physical window. Normal rendering
uses 75% of each physical surface dimension, so fullscreen can substantially raise
world pixels and also changes presentation and aspect ratio. Do not infer its cost
from the window's logical size, display marketing resolution or pixel ratios alone.

The command `python3 tools/grass_profile.py suite tools/profiles/grass-fullscreen-120.json`
completed as session `20260916-200703`. Mode and actual world/surface dimensions
were confirmed. It remains the controlled repro (including its disabled depth
prepass), not full populated-game acceptance. See the
[detailed record](GRASS_FULLSCREEN_120_20260916.md) for 30-second analysis and
the temporary thermal event. Preserve both this outcome and the earlier failure;
the different conditions do not identify why their behavior differed.

Suggested density steps are 66 → 72 → 96 → 128 authored roots/m², stopping when cadence,
thermals or visual quality fails. These are experiment candidates, not promised
capacity. Cook separate databases from a copied authoring project and retain
Balanced thinning. `--density full` changes the thinning policy and is not a
numeric authored-density increase. Verify actual submitted roots, topology,
preparation fallback and capacity drops in separate counter runs.

Evaluate a continuous run in 30-second windows and compare its first and last five
minutes: application cadence, HUD presentation intervals, power, clocks and thermal
pressure. A whole-run 120 fps average or short HUD GPU duration is insufficient.
Fifteen to twenty minutes is an initial observation period, not proof of indefinite
thermal equilibrium; extend it if the trend is still changing. The current tool
records the needed timeline but does not certify equilibrium, fan noise or case
temperature. User comfort observations are a separate result. Nominal pressure
alone does not mean the laptop stays cool or quiet.

GP-012 can be investigated independently, but record any pacing implementation
change as a new baseline rather than silently mixing old/new runs. Future density
tests must record near-detail reach: increasing authored density can change the
capacity-derived high-detail radius and thereby alter a second variable.

## How to maintain this record

After each experiment, update its row and add a dated detailed record. Preserve
failed, reverted and inconclusive outcomes. Record:

1. **Question and one intended change.** Baseline/candidate IDs, status, date and
   whether this is an optimization, quality tradeoff, diagnostic or measurement fix.
2. **Conditions and provenance.** Hardware/backend, power mode/source, actual
   internal resolution, MSAA, scene/path, density/LOD, wind, instrumentation,
   warmup/measurement duration, order, code state and input hashes. A commit hash
   alone does not describe an uncommitted build.
3. **Evidence.** Per-visit results, clocks/thermal context, presentation distribution,
   relevant counters and images. Distinguish measured values from hypotheses and
   unverified budgets. Record invalid or contaminated runs instead of hiding them.
4. **Decision.** Retained, reverted, rejected, inconclusive or still planned; why;
   visual acceptance; complexity/memory cost; what remains unknown; next action.
5. **Preservation.** Keep a compact evidence bundle under `docs/performance/` for
   important sessions. Include raw logs, run manifests, input hashes, original and
   corrected summaries, and the analyzer used. Keep large binaries/databases/traces
   in the local experiment directory and explicitly identify that dependency.

Do not silently overwrite a conclusion when the analyzer or interpretation changes:
date the correction and preserve the original evidence. Do not add speedups across
different views/builds/devices or convert a whole-game HUD delta into isolated grass
cost. Historical plans and provisional budgets remain hypotheses until measured.

Use the older [handoff](PERFORMANCE_HANDOFF.md) for the September 5–6 raw-log map
and failed cache experiments, the [assessment](GRASS_PC_PERFORMANCE_ASSESSMENT.md)
for implementation/reference analysis, and the [improvement plan](GRASS_IMPROVEMENT_PLAN.md)
for the broader visual and architecture history.
