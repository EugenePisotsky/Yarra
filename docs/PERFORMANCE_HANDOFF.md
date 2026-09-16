# Rendering performance investigation — handoff, 2026-09-06

**Current entry point, 2026-09-16:** use the
[grass optimization log](GRASS_OPTIMIZATION_LOG.md) for the current 1440p60
mainstream-PC target, Mac power results, pacing caveat and next tests. This handoff
preserves the earlier iPhone/Mac investigation and its dated scope; its 120 fps Mac
testing preference is historical, not a restriction on the new controlled 60/120 tests.

**Start here before continuing performance work. The iPhone frame-rate/heat problem is unresolved.**
Planning update, 2026-09-08: the user reiterated sustained performance as the central constraint
for grass and shadow work, with iPhone as a longer-term target. The user then clarified that current
iPhone performance is already unacceptable: defer phone tests and broader grass optimization. The
immediate shadow task must add only small measured cost on Mac, without making the existing renderer
materially worse. Neither phone acceptance nor fixing the baseline blocks this bounded experiment. The
[grass performance priority](GRASS_IMPROVEMENT_PLAN.md#performance-priority---2026-09-08) records the
bounded shadow experiment and Mac comparison requirements. Newer prepared-ground and MSAA-storage
results are linked there. **Restart, 2026-09-09:** both the raised sheet and clump-mask shadow
experiments were rejected visually and removed. Their small reported timing differences are
historical evidence, not acceptance. [The restart plan](GRASS_SHADOW_RESTART.md) now requires
small-patch visual validation from above and in motion before selecting and integrating a new
representation. Neither failed experiment established low power consumption or iPhone viability.
The historical measurements and session scope below remain dated evidence,
not a current-build baseline or a prohibition on newly requested planning work.

We found real costs, fixed a correctness bug, and reduced several GPU work counters.
We have **not demonstrated a meaningful sustained iPhone performance or thermal improvement**.
The user's assessment is that the game still feels unacceptably heavy and the time spent
has not produced a useful result. Do not describe the implemented optimizations as a solved
problem, or ask the user to repeat the same exploratory switches without a new reason.

This document consolidates the investigation, including failed experiments, corrected
interpretations, and inconvenient results. The [chronological audit](RENDER_AUDIT.md)
contains more detail. The [old implementation plan](RENDER_OPTIMIZATION_PLAN.md) records
earlier decisions; its proposed next steps are **not a commitment to continue that approach**.

## Scope, user priorities, and state being handed over

- iPhone 15 Pro Max / A17 Pro: sustained **60 FPS**. Tests were Release, both plugged
  and unplugged. A 120 Hz display label does not mean this app was rendering at 120;
  observed cool-device presentation was about 60.
- Current Mac / M2 Max: **display VSync, approximately 120 FPS**, not an artificial 60 cap.
  The latest instruction is to do further testing on macOS for now, not iOS.
- Grass plus ground should leave room for an actual game. A scene that only briefly
  meets VSync before heating/throttling is not acceptable evidence of completion.
- The user finds 50% resolution visually acceptable except for significant grass shimmer.
  Shimmer occurs with camera movement and animated grass. **4× MSAA looks good**;
  keep its visual benefit in view when considering tradeoffs.
- “iPhone still looks terrible” in the log9 discussion meant **frame rate and heat**,
  not a newly reported visual artifact.
- Latest request: write down what happened for another session to reassess. No new
  optimization, profiling run, renderer rewrite, or engine migration was requested in
  this documentation turn.

Workspace: `/Users/eugenepisotsky/Dev/Sources/Hobby/YarraProject`.
Branch: `feat/vegetation`. HEAD when documenting:
`5a1bf35150a5b81a327ba96f606280e3f5f15c2b`.
**The investigated implementation is in a large uncommitted working tree, not that commit.**
It includes both earlier work and changes from this investigation. Preserve it; do not
reset it or assume every dirty file belongs to a single experiment. Resolved dependencies
are Bevy 0.19.1 / wgpu 29.0.4; the scene uses `assets/generated/demo.runtime.sqlite`.

## What the evidence establishes

1. **Production ground is expensive in ground-filled views.** It is a layered procedural
   material with filtering, anti-tiling, normals, lighting and shadows, not a single
   picture. In one Mac frame terrain fragments account for 44.64% of captured GPU time.
   A basic flat ground draw is much cheaper. This does not by itself explain why the
   chosen material consumes so much of the practical phone budget.
2. **Grass can dominate a different view.** In the physical iPhone low-camera capture,
   grass vertex, fragment, generation and preparation work together account for about
   68%; terrain fragments account for 14.90%. Neither result gives a universal ranking.
3. **The initially high “empty” GPU floor was partly our measurement scene.** Removing
   the composite path and game UI reduced the Clear baseline from about 5 ms to about
   1 ms. That did not make the production scene cheap, and those savings cannot be
   subtracted as constants from a heavy frame.
4. **Thermal deterioration is real, but a single underlying defect has not been found.**
   iPhone logs record Fair/Serious transitions and later missed presentation targets.
   Mac power traces show substantial system GPU activity and reduced clocks under
   thermal pressure. No evidence establishes that one hidden renderer bug explains it all.
5. **Lower resolution does work.** Logged physical render dimensions change. A long Mac
   run at 75%/4× maintained approximately 120 FPS, with loud fans and thermal pressure.
   iPhone 50% did not give the user a convincing FPS/heat improvement. These observations
   are compatible; they are different workloads/devices, not proof scaling is broken.
6. **Less arithmetic has not translated into a convincing whole-frame result.** Several
   caches exchange ALU work for memory traffic, retain most texture/draw work, or help
   only while stationary. The latest small grass mask saves compute work but its live
   whole-frame difference is around 0.05 ms in one Mac comparison.

## User-supplied iPhone runs, in order

Names matter: **`log4.txt` and `logs4.txt` are different files**. Do not merge them.
GPU numbers below are Metal HUD durations unless explicitly called replay counters.
Thermal labels describe coarse device states, not fixed GPU clocks.

| Evidence | Experiment and observation | What it does / does not establish |
| --- | --- | --- |
| [logs.txt](../tmp/logs.txt) | Stationary grass only. Early approximately 10.56 ms / 59.99 FPS; later 14.94 ms while still at 60; final approximately 23.56 ms / 50.18 FPS. | Establishes slowdown without camera movement. No initial configuration/build marker or thermal-state readings, so exact settings and thermal attribution are incomplete. |
| [logs2.txt](../tmp/logs2.txt) | Full → Frozen draw → Compute only → Schedule only → Disabled → Full. Full before Frozen: 21.31 ms / 56.21 FPS. Frozen initially 13.50 ms / 60, later 22.28 ms / 57.39. Compute only 8.86 ms; Schedule only 7.56; Disabled 7.47, each around 60. | Drawing matters substantially; freezing placement alone cannot solve grass slowdown. The user saw Serious early and throughout, even when FPS recovered. These sequential windows do not have controlled clocks or isolate the shared floor. |
| [logs3.txt](../tmp/logs3.txt) | Ground only enabled at 17.037 s; Fair at 52.196 s; Serious at 72.283 s. Late Ground: 33.23 ms / 44.29 FPS; Clear: 5.78 ms / 60; Ground return: 32.34 ms / 44.93. | Strong evidence the production ground workload matters, even with grass disabled. Camera, source revision and resident work were stable. Ground was expensive independently of the earlier grass finding. |
| [log4.txt](../tmp/log4.txt) | Fair, short holds: Production 11.67 → Surface unlit 10.51 → Single texture 8.60 → Flat 7.74 → Production 11.77 ms. All about 60 FPS. | Distinguishes material and lighting work. Does **not** establish sustained performance: the user explicitly says the holds were short and it would degrade if left running. The remaining Flat duration was not all the ground's cost. |
| [logs4.txt](../tmp/logs4.txt) | Flat: shadows/prepass on 7.60 → shadows off 6.48 → prepass off 5.23 ms. Clear with both off 4.69. Flat at 50% 4.67; 100% returns 5.16/5.30. | Shadows/prepass and common rendering matter. Flat adds only about 0.55 ms in the relevant comparison. The remaining floor needed another isolation test. |
| [log5.txt](../tmp/log5.txt) | Automated 160 s lightweight baseline. Clear composite/UI 5.07 → direct/UI 2.70 → direct/no UI 1.05; Flat direct/no UI 1.36; Clear return 0.97; UI return 2.74; composite/UI return 5.03 ms. Nominal throughout. | Reproducible debug UI/composite cost in Clear; basic Flat increment about 0.39 ms. This resolved the apparent expensive empty baseline, **not** production-ground or grass thermal performance. |
| [log6.txt](../tmp/log6.txt) | Grass Full: Direct 10.12 → Composite 10.34 → Direct 10.12 ms. | Corrects the earlier implication that Direct would generally save approximately 2.3 ms. Here the observed difference is only 0.22–0.23 ms. The user's observation that switching seemed to do little was accurate. |
| [log7.txt](../tmp/log7.txt) | Ground+grass, 50% (1398×645). Nominal top-down: AA off 10.20 → 2× 10.53 ms, about 60. Serious at 104.355 s while still on 2×; 4× first enabled at 114.240 s. Hot low view: 4× 25.10 ms / 47.43 FPS, then Off 22.01 ms / 52.06. | Degradation predates 4×. The approximately 3.09 ms difference is a short hot comparison, not a general MSAA price. Off still misses 60. Camera/thermal changes prevent pooling the whole run. |
| [log8.txt](../tmp/log8.txt) | Cold top-down repeat at 50%: Off 10.259 → 4× 10.331 → Off 10.345 → 4× 10.343 ms, all approximately 60. Two 2× visits last only 0.587 and 0.250 s. Last checkpoint 46.883 s. | 4× barely changes duration in this view. The two 2× visits are not settled measurements. Short unchanged GPU time does not imply zero AA energy cost or sustained thermal success. |
| [log9.txt](../tmp/log9.txt) | After terrain lattice caching and blade preparation: 75% (2097×967), 4×, Gaussian shadows, no prepass, ground+grass, wind. Nominal top-down 11.31 ms / 60; moving low view 18.58 ms initially still 60; Fair 54.534 s; Serious 74.809 s; late approximately 31.01 ms / 38 FPS. | **Failed practical acceptance.** Both caches were active and bounded. Camera height changed 12.354 → 1.730 m, so this is not a matched before/after against top-down log8. It provides no convincing iPhone benefit from those changes. |

The ground-only screenshots also show fragment dominance. The four files in Downloads
are `Screenshot 2026-09-05 at 05.24.24.png`, `05.25.04.png`, `05.25.29.png`, and
`05.25.52.png` (each with the full `Screenshot 2026-09-05 at ` prefix).
Nominal shows GPU 11.36 ms, encoder 11.38 ms, fragments 10.93 ms and vertices 0.28 ms.
Fair shows GPU 14.65 ms and fragments 14.16 ms. The final Serious screenshot shows
GPU 45.54 ms, encoder 31.10 ms, fragments 29.11 ms and 31.99 FPS. These are different
metrics/windows; do not treat encoder and overall GPU durations as interchangeable.
The screenshots show **API Validation Enabled and GPU Frame Capture Enabled**, despite
Release mode. Instrumentation was subsequently disabled for ordinary performance runs.
Do not precisely align screenshot times to logs3 without establishing the same run.

The later Downloads screenshots `IMG_1228.PNG`, `IMG_1229.PNG`, `IMG_1230.PNG` accompany
log5: Clear/no UI shows 1.18 ms; Clear with UI returned shows 2.87 ms; the final image
shows **restored grass after the test**, 10.31 ms. That last image is not an expensive
Clear result. This was one source of confusion about what the baseline had established.

## Ground: what was inspected and what was tried

### Actual material and basic checks

The cooked overworld uses two terrain surface layers with stochastic anti-tiling.
For the usual path, intended per-fragment sampling is six albedo samples (three per
layer), two packed normal/AO/roughness samples, one weight sample and three macro
samples: approximately **12 logical texture calls**, followed by PBR/shadows and
output processing. Both layers are evaluated even where one weight is zero. An
initial plain albedo sample is overwritten by anti-tiling; do not count it as a
confirmed extra hardware access without inspecting compiled output.

Terrain PBR uses Gaussian directional shadow filtering by default; the custom grass
shader uses hardware 2×2 filtering. The light uses three 2048² shadow cascades.
Three cascades do not mean every fragment samples all three. Sampling uses up to
8× anisotropy; logical texture calls are not the physical tap count or bandwidth.

Basic asset checks found iOS ASTC textures, 1024² resolution, all 11 mip levels, and
about 3.67 MiB for the shared terrain set. Maps are already packed. No explicit HDR
camera, SSAO, bloom, screen-space reflections or atmosphere raymarching was found in
the inspected setup. Missing mips, an uncompressed giant ground image, and a collection
of unexplained post-effects are **not established explanations**.

The native iPhone target is 2796×1290, about 3.61 million pixels, despite the small
physical screen and the desktop 1280×720 window declaration. At 60 FPS that is about
216 million output pixel positions per second before overdraw and extra passes.
50% scales both dimensions (25% of native pixels); 75% means 56.25% of pixels.
Scaling the 3D target does not uniformly scale shadow maps, CPU work, UI or all grass
work. This explains why screen size alone is not a compute budget, not why this
particular implementation must be as expensive as it is.

### Ground experiments

| Attempt | Result | Current status |
| --- | --- | --- |
| Flat / one-texture / surface-unlit modes | Much cheaper than production in log4. Flat itself is cheap once shared costs are removed in log5. | Diagnostic modes retained. No replacement production material selected. |
| Disable shadows and depth prepass | Flat comparison falls 7.60 → 6.48 → 5.23 ms. Custom grass has no depth-prepass draw; enabling that pass does not pre-render this grass. | iOS defaults to prepass off; toggle retained. Gaussian / hardware 2×2 / off comparisons are available. No general sustained shadow-setting win claimed. |
| Skip a surface at **exactly zero weight** | Main-pass texture calls 145.55M → 136.90M (−5.9%); texture L1 reads 8.18 → 6.88 GB (−15.8%); fragment ALU 7.539 → 7.554B, effectively unchanged; registers 149 → 152. | **Rejected and removed.** Archived source: `tmp/terrain-material-layer-skip-experiment.wgsl`. Do not propose this as an untried obvious fix. |
| Cache stochastic transforms in a texture | Fragment ALU −12.3%, texture accesses +14.9%. | **Replaced**, not the current cache. Archive: `tmp/terrain-transform-texture-implementation/`; trace named `terrain-transform-cached.gputrace` belongs to this rejected version. |
| Cache stochastic transforms in a storage buffer | Replaces 18 fragment hashes with six buffer loads for two surfaces. Main-pass ALU 7.552 → 6.598B (−12.6%); texture calls essentially unchanged at 145.406M; texture L1 bytes +0.7%; terrain registers 149 → 142. | **Retained**, bounded and output-validated. This is only a transform cache, not a baked material. No sustained iPhone improvement demonstrated. |
| Cache/bake complete blended material | Discussed as a larger possible reduction of repeated texture/blend work. | **Not implemented or benchmarked.** No proof its complexity is justified. |

The retained buffer cache uses 1,514,016 bytes (1.44 MiB) for 49 tables in the scene.
It caps payload at 8 MiB plus a 16-byte fallback, builds at most eight tables per
frame, and bounds each lattice axis to 128 nodes. Driver allocation/metadata and
in-flight replacement overlap are additional. Unready/unsupported/over-budget
materials use the original path. Native comparisons with patterned textures,
negative coordinates, macro changes and 4× MSAA were byte-identical. Movement
replaced pages without a continuously growing table set.

Timing warning: the first baseline/layer-skip/stationary-placement captures reported
4.21 / 9.96 / 9.27 ms, but **unchanged work also slowed by more than 2×**; multiple
replays were open during parts of that investigation. Those are invalid speedup
comparisons. Later transform reference/texture/buffer replays were 4.54 / 4.50 /
3.91 ms, but unchanged grass vertex timing also fell. The 3.91 result is **not a
verified 14% whole-frame gain**, even with one replay at a time.

The full material-cache idea would need view-dependent tile resolution, mips/edges,
an explicit memory/update budget, invalidation and close-view fallback. Baking every
32 m page at the 1.6 m / 1024-pixel source density means 20,480² texels per page;
two RGBA8 maps exceed 3 GiB per page before mips. A uniformly low-resolution bake
would silently blur nearby ground. No such system has been built here.

## Grass: implementation attempts and their actual outcomes

### 1. Scheduler bounds bug — fixed, not the demonstrated thermal cause

The grow-only work-item allocation was bounded with `arrayLength(work_items)`
(capacity), not the live record count. A shrinking resident set could schedule
retired records in the final dispatch group. An explicit live count now bounds it.
The native regression uses a retained 64-record allocation with live counts 64,
5 and 0, and checks operation with telemetry disabled. This was a real correctness
fix. The stationary thermal logs do not establish it as their cause.

### 2. Reuse placement while stationary — retained

Full reuses generated instances/indirect arguments when source, camera/projection,
settings and compiled pipelines are unchanged. Wind phase is excluded from the key
so blades keep animating; wind strength remains because it affects bounds.
Movement, source changes, shader reloads and isolation-mode transitions regenerate.

One Mac capture removed 2,331,840 generation lanes per stationary frame, with
103 draw calls and 2,075,253 submitted vertices unchanged. Command buffers fell
56 → 50, compute encoders 16 → 14 and dispatch calls 69 → 65. Movement/return and
Full/Frozen/Compute/Schedule/Disabled transitions were exercised.

**Limit:** continuous movement still regenerates. Frozen draw had already degraded
in logs2, so this cannot be the whole fix. In log9 movement accumulated 2,372
generation dispatches while reuse stayed at 972. No sustained phone win is established.

### 3. Shared blade shape/wind preparation — retained, substantial bandwidth tradeoff

Computes shape randomness, curve control points and root wind/gust response once per
blade per frame. Per-vertex curve evaluation, flutter, ribbon opening and shading
remain. It is separate from placement because wind continues during stationary reuse.
The bounded 131,072-blade arena plus lookup/arguments uses 18,153,484 bytes (17.31 MiB)
at the scene's capacity. Overflow uses the original vertex math; it does not drop grass.

| Measurement | Mac capture | A17 Pro low-view capture |
| --- | ---: | ---: |
| Vertex + preparation ALU reduction | 51% | 29.5% |
| Whole-frame device-memory traffic change | +17.5%, about 25.1 MB/frame | +13.4%, about 36.6 MB/frame |
| Observed reference → prepared replay | 4.107 → 3.982 ms | 18.41 → 18.03 ms |

Mac total-frame ALU fell 9.7%. Neither timing pair establishes a sustained thermal
benefit; replay clocks were uncontrolled and the older repro had independent real-time
wind phases. Native image checks cover camera movement, wind, 4× and forced overflow.
The current allocation order is bins 0,1,2,3. Prioritizing 0,2,1,3 was discussed,
**not implemented or validated**. `--grass-vertex-reference` disables preparation.

### 4. Earlier rejection of ineligible candidates — retained, small whole-frame effect

Production generation stops once ownership/growth/surface/coverage/visibility rules
reject a candidate, avoiding later calculations. Diagnostics retain full evaluation.
The native test compared 90,086 accepted and 425,464 rejected candidates across 21
cases with no eligibility or accepted-output mismatch.

A17 generation ALU fell 1,773,567,328 → 1,671,404,896 (5.76%); replay was 18.03 →
18.02 ms. This is less work, **not an observed meaningful frame-time win**.
`--grass-placement-reference` restores the old evaluation order; despite its name,
it does **not** disable stationary-placement reuse.

### 5. Disable unused diagnostic atomics on iOS — retained

Optional global candidate/eligibility counters were still written even though iOS
could not read them back. They now default off on iOS, while required instance-append
atomics remain. Generation atomic bytes written fell 17,670,592 → 8,893,952 (49.7%).
These are hardware transaction counters, not allocated-buffer sizes. Replay changed
18.02 → 17.82 ms with the usual clock/wind limitations; it is not a proven heat fix.
The Metal HUD and Xcode hardware counters are separate from these application atomics.

### 6. Compact accepted candidate IDs — tried and rejected on Mac

The first moving-view source cache compacted accepted IDs into a bounded 32 MiB arena
and dispatched fewer generation lanes. Native population checks passed. The valid
v4 capture pair had matching camera, wind, viewport and 2,084,772 total vertices:

| Metric | Reference | Compact IDs |
| --- | ---: | ---: |
| Generation invocations | 3,331,298 | 2,598,498 |
| Generation ALU | 1,320,699,904 | 1,051,805,696 |
| Generation time | 0.561 ms | 0.513 ms |
| Main opaque fragment invocations | 3,221,504 | 3,295,616 |
| Main opaque fragment ALU | 5,276,733,184 | 5,785,079,552 |
| Whole-frame replay | 4.58 ms | 4.80 ms |

Compute ALU fell 20.4%, but fragment ALU rose 9.6% with changed append/draw order;
the whole frame did not improve. This implementation was replaced. Do not report its
compute reduction as a delivered speedup. Its traces remain for explaining the decision.

A separate live run of that build was approximately **39–42 FPS while Nominal**,
matching the user's report of extreme heaviness. Replay had been stopped; compilation
started only after that game was terminated. Idle Xcode and other desktop processes
were still open. A later snapshot found Spotlight/WindowServer activity, but did not
prove the earlier cause. Do not dismiss this as definitely an external process issue.

### 7. Source-stable acceptance bitmask — current implementation, small result

The replacement stores one acceptance bit per original candidate and preserves the
original dispatch layout/indexing. Rejected candidates skip expensive evaluation;
accepted candidates still use existing generation and drawing. The mask arena is
bounded to 1 MiB plus metadata (1,052,688 bytes in the measured scene), with four
fields / 262,144 padded build lanes per frame. All 196 resident fields fit in this run.
Source revisions or shader reloads invalidate it; camera/wind/LOD changes reuse it.
Unready, unallocated, oversized, diagnostic and authored quarter-lattice cases fall
back to original evaluation. Source repacks currently invalidate the whole cache.

| Final matched Mac counter | Reference | Bitmask |
| --- | ---: | ---: |
| Generation invocations | 3,331,298 | 3,331,298 |
| Generation ALU | 1,325,285,376 | 1,236,937,216 |
| Generation time | 0.588 ms | 0.571 ms |
| Main opaque vertex invocations | 1,001,882 | 1,001,882 |
| Main opaque fragment invocations | 3,221,472 | 3,220,992 |
| Main opaque fragment ALU | 5,275,989,504 | 5,280,000,000 |
| Whole-frame replay | 4.72 ms | 4.63 ms |

Generation ALU falls 6.7%, with effectively unchanged fragment work (+0.08% ALU).
After fully quitting Xcode and finishing builds/tests, isolated one-instance live
comparisons at 75% (1920×1080), 4×, moving low view and counters off gave:

| Live HUD windows 20–115, after startup | Reference | Bitmask |
| --- | ---: | ---: |
| FPS | 120.02 | 120.02 |
| Mean GPU duration | 3.978 ms | 3.927 ms |
| GPU duration p95 | 4.92 ms | 4.78 ms |

Both roughly one-minute runs were Fair in settled samples. **A single approximately
0.05 ms mean difference is not a meaningful demonstrated whole-frame improvement.**
The earlier 40 FPS problem was not reproduced under these conditions, but its cause
was not isolated. There was no new power capture: administrator authentication had
expired. No iPhone build/run of this latest bitmask iteration was performed.

Native validation requires bit-identical per-bin multisets of 32-byte instances,
matching draw counts and zero capacity drops. Fixture expensive evaluations decrease
24,550 → 10,859 (55.8%); this is **not a reduction of dispatched production lanes**.
Images have mean byte error 0–0.002682 and maximum 12 in the mask run. An earlier
compact-ID test failed a stricter maximum-pixel check (maximum 18). Exact population
checks were added: atomic append order can change equal-depth MSAA winners at blade
intersections despite identical instances. The image criterion now also requires mean
error <0.01 and <0.1% of channels differing by more than two. Record this tolerance
change rather than presenting all images as byte-identical. Diagnostic images are exact.

`--grass-candidate-reference` disables this cache's building/use, but retains allocated
buffers. This does not turn off the other grass optimizations.

## Mac CPU, power and sustained runs

CPU traces were collected before the later frame captures. The available summaries
estimate about 115.20% CPU for a 20.67 s Full run and 74.23% for a 20.37 s Clear run;
an earlier 25.6 s CPU trace estimates 141.19%. **100% means one core**, not the whole
Mac. Sample weights are not watts or frame-critical-path duration, and inclusive
stack percentages overlap. Wait/synchronization symbols do not prove an active spin
loop or identify the phone's heat source. No CPU root cause was established.

Code inspection found possible CPU churn: rebuilding demand/residency statistics,
allocating/sorting vegetation signatures and cloning a catalog before an unchanged
early return, and rewriting some unchanged transforms. These have not been shown to
dominate. Large packing/uploads are revision-gated, and stationary source counters
settle. There is no established full-scene re-upload on every stationary frame.

| Power capture | Observation | Limits / correction |
| --- | --- | --- |
| `mac-power-20260905-103131.txt` | 120 samples / 124.41 s. Focus was lost just as sampling began, and complete game intervals averaged 60.001 FPS. System GPU 3.71 W and CPU 1.58 W overall; thermal state recovered Fair → Nominal. | **Not the requested focused 120 FPS baseline.** Bevy's unfocused default uses a 1/60 s reactive timer. The decline in power was not an optimization result. Earlier traces lacked focus logging. |
| `mac-power-20260905-104216.txt` | Focused, 100%, 4×, 3456×1936. 120 samples / 124.18 s, approximately 119.78 FPS; system GPU 25.40 W, CPU 4.68 W; GPU 90.2% active at 1376 MHz, close to listed maximum 1398. | Target was 6.69M pixels versus 3.69M previously, and FPS doubled; this is not an AA-only comparison. Power sampling ended around 2:15 after startup. Fair came at 3:19, then game windows dropped to 114.03, 98.72 and 80.26 FPS by 4:10. **No power/clock samples cover that decline**, and camera changed. |
| `mac-power-20260905-105136.txt` | 75% (2592×1452 from 3456×1936), 4×, focused, ground+grass. Settled game log spans 15m34s, average 119.56 FPS, final five minutes 119.80. Fans loud. | Demonstrated sustained Mac VSync in this configuration, with occasional dips; not a quiet/low-power result or iPhone result. Power sampling covers only the first 372.49 s, not the full game run. |

For comparable Nominal 70–130 s windows in the last two runs, system GPU power was
24.00 W at 100% versus 15.53 W at 75% (about 35% lower), with CPU approximately
4.8 W in both. Camera route, population and render path differed, so this is not a
pure scale A/B. It does support reduced workload in the 75% Mac configuration.

The settled 75% power window averaged GPU 15.06 W plus CPU 4.54 W. Foundation
reported Fair at 160.287 s; powermetrics separately reported Moderate around 159.8 s
and Heavy from about 185.8 s. Do not equate these two interfaces' labels. At 300–360 s,
GPU frequency averaged 998 MHz, activity 88.9%, power 12.64 W, while the game still
averaged 119.73 FPS. Some dips aligned with lower clocks and near-100% GPU activity.
This shows that reduced clocks and thermal pressure can coexist with stable VSync.

All watt figures are **whole-system CPU/GPU subsystem estimates**, not total laptop
power or Yarra-attributed watts. Per-process GPU time was zero/unusable. WindowServer,
computer-use services, powermetrics and transient Spotlight activity were present.
There was no matched app-closed power baseline to attribute their shares. Mac watts
must not be transferred to the phone. Fan RPM was not recorded.

`tmp/record_mac_power.sh` was created to request local `sudo -v`, launch the Release
game and record approximately six minutes by default. The user supplied local admin
authentication on September 5; it had expired for the September 6 comparisons.
An earlier heavyweight Metal System Trace was also attempted; the later useful
shader attribution came from single-frame Xcode GPU captures. Do not treat profiling
with all these tools running as an ordinary game-performance baseline.

## Frame attribution, capture problems and misleading results

The early stationary Mac frame (2560×1440, AA off, Gaussian shadows, prepass/UI/
counters on) attributed 44.64% to terrain fragments, 18.95% to grass vertices,
10.51% to grass fragments and 9.06% to grass generation. Terrain vertex share was
only 0.27%. Main opaque average overdraw was 1.0014, with 3,691,456 fragment
invocations for 3,686,400 pixels. That frame does not support massive hidden-ground
overdraw as its explanation. ALU, texture-read and filtering limiters overlap;
their percentages are not additive time shares.

The physical A17 low view (2097×967, 4×, no prepass) attributed grass vertex 28.62%,
grass fragment 22.87%, generation 13.27%, preparation 3.34%, terrain fragment 14.90%
and UI fragment 6.18%. Generation launched 4,130,690 invocations; main-pass vertex
invocations were 1,054,694. Grass vertex used 142 registers with no spills and
5.84% measured main-pass vertex occupancy; generation used 150 registers and spilled
16 bytes. These identify work worth understanding, not a self-sufficient explanation
of the throughput limit. Changing the camera changed which work dominated.

Problems encountered and how to interpret preserved evidence:

- **Release was not necessarily uninstrumented.** Early screenshots show Metal API
  validation and frame capture enabled. A shared **Yarra Performance** Xcode scheme
  was added for Release with those diagnostics/checkers disabled. It is a scheme
  inside the existing project, not another game/application. Its initial disabled
  debugger meant the Xcode console was missing expected logs; LLDB attachment was
  restored. Debugger attachment and rendering diagnostics are separate switches.
- **Logs and input initially obscured the experiment.** Settings switches, elapsed
  phase, camera, actual resolution, focus, thermal/Low Power state and cache/allocation
  counts were added. HUD touch interception initially blocked movement; input capture
  was restricted to the panel and tested through release at 3× display scale.
  The scripted repro intentionally overrides the camera, so its control lock is a
  different situation from a normal manual run.
- **Programmatic iPhone capture required fixes.** Locked phones blocked replay.
  Internal trace symlinks prevented `devicectl` copies; the exporter now materializes
  them after capture. `iphone-low-prepared.gputrace` is incomplete; use `prepared2`.
  Capture-only iOS runs now exit after saving, because the event loop otherwise left
  a stopped black window. Xcode replay can also own the GPU/show a black phone screen;
  that is not by itself evidence the normal game has a rendering regression.
- **Mirroring is a confound.** The last two A17 counter captures were acquired with
  iPhone Mirroring active. A later 86 s live mirrored run reached Fair at 43.478 s,
  Serious at 58.678 s and about 32.8 FPS / 30.6 ms late; it was stopped before the
  planned three minutes. Do not compare these thermal times to unmirrored log9.
  Mirroring was useful for visual inspection, not a controlled sustained benchmark.
- **Wind and frame numbers made early candidate-cache comparisons invalid.** Repro
  version 2 synchronized camera but used real-time wind. Version 3 synchronizes
  camera and wind, but the earlier v3 traces still captured different geometry counts
  because the trigger used render-loop frame 600. The current trigger uses extracted
  application frame 600 (usually render frame 601). The accepted v4 compact-ID and
  final mask pairs match camera `(2.513,1.730,-5.895)`, 1920×1080 and wind phase 5.
- **Xcode “Medium” is not a clock lock.** It attempts that state subject to thermal
  conditions. Multiple replay sessions and changing clocks invalidated early absolute
  comparisons. Stop replay, finish builds, use one live game, and keep focus/viewport
  fixed before interpreting a new timing pair. Fully quitting Xcode preceded the
  later successful live Mac checks, but no experiment isolated it as the 40 FPS cause.
- **Application GPU-stat readback is unavailable on iOS.** Zeros did not mean zero
  instances, no preparation or no fallback. Capacity metadata was corrected to report
  344,064 instance capacity / 11,010,048 bytes independently of unavailable readback.
- **Memory growth was observed but no leak was identified.** In the first log process
  memory rose approximately 647.94 → 772.94 MB while graphics rose only 325.42 →
  326.75 MB. In logs2, process memory grew 27.34 MB during Disabled while graphics
  stayed at 333.47 MB. Later instance/source/cache capacities were bounded. These
  observations neither prove a leak nor rule out unrelated CPU allocations. Graphics
  and app HUD memory categories overlap; do not add them as separate totals.
- **Startup noise is not the established late-frame cause.** Early logs included
  shader compilation, a 3.384 s presentation stall near a Thread Performance Checker
  QoS warning involving indirect-buffer/render extraction work, and drawable-after-
  present warnings. None was shown to cause the steady later thermal deterioration.

## Measurement rules learned the hard way

1. Use settings-matched, settled windows. Record actual render dimensions, camera,
   wind phase, AA, focus, shadows, prepass, UI, counters and build/reference flags.
   Comparing two visually different views is attribution, not a clean optimization A/B.
2. Metal HUD GPU duration is not necessarily exclusive GPU busy time; submissions,
   overlap and idle intervals can affect it. Encoder/stage durations and presentation
   intervals are different metrics. Do not subtract a Clear whole-frame duration
   from a heavy frame and call the remainder exact material cost.
3. A nominal 16.67 ms iPhone or 8.33 ms Mac presentation budget is not a sustainable
   power budget. FPS pinned to VSync hides headroom, and coarse thermal labels hide
   clock differences. No fixed “10 ms is safe on this phone” threshold was established.
4. Collapse only exactly duplicated consecutive Metal HUD **packets**. The initial
   file has 576 lines / 288 unique packets, each emitted twice. Do not deduplicate
   identical timing pairs inside a packet. HUD frame identifiers were not a reliable
   wall-clock axis; align audit timestamps/line boundaries conservatively.
5. Xcode CSV exports in this locale contain unquoted decimal commas in percentage
   cells. Existing parsers repair only split percentage cells and assert header width.
   Do not naively zip split rows to columns. Check counter units; the Mac exported
   GPU-time values used here are nanoseconds converted to milliseconds.
6. Brief 2× visits, compile/setting transitions and app-loop FPS spikes during changes
   are not steady presentation measurements. MSAA sample count does not multiply
   every shader invocation by the same factor. Unchanged cool-view duration does
   not establish unchanged energy use.
7. Power samples and game logs have different coverage. Some logs outlast recordings
   by minutes. Whole-system power is not per-app attribution; inclusive CPU sample
   weights are not additive costs. No current power result isolates iPhone CPU heat.
8. Passing shader, population, image and lifetime tests demonstrates correctness of
   the tested cases. It does **not** establish useful performance, energy efficiency,
   or headroom for a larger game. Preserve failed/invalid results with their labels.

## Current code, switches and validation state

Main implementation locations:

| Area | Files |
| --- | --- |
| Audit controls / logging / scripted route | [render_audit.rs](../crates/app_game/src/render_audit.rs), [render_audit/](../crates/app_game/src/render_audit/), [main.rs](../crates/app_game/src/main.rs) |
| Programmatic GPU capture | [metal_capture.rs](../crates/app_game/src/metal_capture.rs) |
| Game camera, input and streaming changes | [engine/src/lib.rs](../crates/engine/src/lib.rs), [world_streaming.rs](../crates/engine/src/world_streaming.rs) |
| Ground shader and cache | [terrain_material.wgsl](../assets/shaders/terrain_material.wgsl), [terrain_stochastic.wgsl](../assets/shaders/terrain_stochastic.wgsl), [terrain_stochastic_cache.wgsl](../assets/shaders/terrain_stochastic_cache.wgsl), [stochastic_cache.rs](../crates/terrain_render/src/stochastic_cache.rs), [terrain lib.rs](../crates/terrain_render/src/lib.rs), [gpu_tests.rs](../crates/terrain_render/src/gpu_tests.rs) |
| Grass generation / drawing / scheduling | [vegetation_debug_compute.wgsl](../assets/shaders/vegetation_debug_compute.wgsl), [vegetation_debug_draw.wgsl](../assets/shaders/vegetation_debug_draw.wgsl), [vegetation_schedule_compute.wgsl](../assets/shaders/vegetation_schedule_compute.wgsl), [renderer.rs](../crates/vegetation_render/src/renderer.rs) |
| Blade preparation and candidate cache | [vegetation_blade.wgsl](../assets/shaders/vegetation_blade.wgsl), [vegetation_prepare_blades.wgsl](../assets/shaders/vegetation_prepare_blades.wgsl), [renderer/](../crates/vegetation_render/src/renderer/) |
| iOS launch/profile setup | [ios/README.md](../ios/README.md), [shared schemes](../ios/Yarra/Yarra.xcodeproj/xcshareddata/xcschemes/) |

Other dirty files include Cargo manifests/lockfile, vegetation renderer configuration,
and `crates/app_editor/src/vegetation_authoring.rs`; an untracked earlier recording
exists under `content/`. Inspect `git status` rather than treating the table as a
complete change manifest. This handoff does not commit or remove any of that work.

Current manual `--render-audit` defaults: ground+grass, production shading, Gaussian
PBR shadows, Full grass, wind on, 100% Direct, AA off, UI on and controls unlocked.
Depth prepass and optional grass diagnostic atomics default **on on Mac, off on iOS**.
The last restored Mac instance was manually set to 75%/4×, prepass/counters off;
**that is not the fresh-launch default**. A click-to-move smoke check streamed source
revision 52 → 68, returned to 196 ready mask fields, and showed about 120 FPS /
3.1 ms overhead-camera GPU duration. It was not a sustained test. Process state may
have changed since that check; the documentation turn did not launch/stop the game.

| Flag/control | Meaning |
| --- | --- |
| `--render-audit` | Debug menu with scene/grass/material isolation, shadows, prepass, resolution, AA, wind, optional counters, control lock and Direct/Composite. Settings/periodic audit logs are emitted. |
| `--render-repro low-walk` | Enables audit and overrides camera every frame. 75% Composite, 4×, no prepass, controls locked, optional atomics off unless `--grass-counters`. Current version 3 uses frame-based camera/wind: 60 Hz iOS / 120 Hz desktop, 300 warmup frames and a 600-frame out-and-back route. |
| `--terrain-procedural` | Disables only the stochastic transform cache. There is no implemented full material cache to disable. |
| `--grass-vertex-reference` | Disables shared blade preparation. |
| `--grass-placement-reference` | Disables early rejection, **not** stationary reuse. |
| `--grass-candidate-reference` | Disables source acceptance mask building/use. Other caches remain enabled. |
| `--grass-counters` | Enables optional application grass atomics in the scripted repro; independent of Metal HUD/Xcode counters. |
| `--render-console` | Adds iOS stderr audit logging for `devicectl --console`; repro implies it. Ordinary iOS launches retain OSLog. |
| `--metal-capture PATH` + `MTL_CAPTURE_ENABLED=1` | Explicit single-frame capture and exit; destination must not exist. Current capture uses extracted application frame 600. On iOS, see exporter instructions in the audit/README. |
| Baseline test button | Automated 160 s Clear/Flat, UI and Direct/Composite isolation. Restores the prior scene afterwards. This is not a sustained full-scene thermal test. |

At non-native scale the audit renders 3D to an image then composites it with native
UI. Direct is native size. There is no universal flag that returns every subsystem
to the original investigation baseline. Use the individual reference flags deliberately.
Restart without `--render-repro` for manual camera/character testing; changing an
on-screen control lock does not remove the scripted camera system.

Commands for a later session, **not run as part of writing this document**:

```sh
cd /Users/eugenepisotsky/Dev/Sources/Hobby/YarraProject
cargo build --offline --locked --release -p yarra-app-game

# Close the previous game before launching another copy. Focus the game window.
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 \
  ./target/release/yarra-app-game --render-audit > tmp/mac-test.log 2>&1

# A separate controlled moving-view run; optional reference flags go on this command.
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 \
  ./target/release/yarra-app-game --render-repro low-walk > tmp/mac-repro.log 2>&1
```

Pick new output filenames if preserving an existing run. For power, inspect
`tmp/record_mac_power.sh` before using it: it launches its own normal audit instance,
does not attach to the existing one, and requires local administrator authentication.
Do not run builds/native GPU tests/replay concurrently with a live performance test.

Latest recorded checks: app 9 tests passed; vegetation 17 passed with four native
tests ignored by default; all four native Metal tests passed when explicitly run;
Mac Release build passed. Earlier terrain validation included five ordinary tests
and a native patterned-render comparison, and earlier changes built/ran on iOS.
**The latest acceptance-mask iteration has not been validated on iOS.** Workspace-wide
strict Clippy was not clean because of existing argument-count, type-complexity,
enum-size and style warnings; unrelated refactoring was not performed. No runtime
test was rerun for this documentation-only handoff.

## Evidence inventory and preservation

**Most raw evidence is ignored `tmp/` content; screenshots are in Downloads. A fresh
checkout or `git clean` will not preserve it.** This document and the chronological
audit preserve conclusions, but replay needs the actual trace directories. Do not
delete/clean these artifacts as an incidental build-cleanup step. Some `.gputrace`
directories are large. The paths below are relative to the project unless stated.

| Artifact group | Paths / use |
| --- | --- |
| Original user logs | `tmp/logs.txt`, `logs2.txt`, `logs3.txt`, `log4.txt`, `logs4.txt`, `log5.txt` through `log9.txt`; linked individually above. |
| Parsed phone results | `tmp/metal-analysis/`, `metal-analysis2/`, `metal-analysis3/`, `metal-analysis4/`, `metal-analysis-passes/`, `metal-analysis5/` through `metal-analysis9/`. Inspect each summary's input filename; suffixes do not always mirror raw-log spelling. |
| Phone parsers | `tmp/analyze_metal_log.py`, `analyze_grass_modes.py`, `analyze_ground_modes.py`, `analyze_ground_shader_modes.py`, `analyze_ground_passes.py`, `analyze_baseline.py`, `analyze_render_paths.py`, `analyze_msaa.py`, `analyze_msaa_cool.py`. |
| Mac CPU traces | `tmp/mac-cpu-probe.trace`, `mac-cpu-time.trace`, `mac-release-full.trace`, `mac-release-clear.trace`, adjacent exports/summary JSON and `analyze_cpu_trace.py`. |
| Mac power | Three dated `tmp/mac-power-20260905-{103131,104216,105136}.txt` captures above, matching `-game.log`, `-summary.json`, `-samples.csv` and analysis text; `analyze_mac_power.py`, `record_mac_power.sh`. |
| First Mac attribution / rejected zero-weight branch | `tmp/metal-frame-{baseline,layer-skip,placement-cache}.gputrace`, `*-counters.csv`, `*-counters.json`, `*-game.log`. Work counters useful; absolute timings invalid for speedup claims. |
| Ground transform variants | `tmp/terrain-transform-{reference,cached,buffer}.gputrace`, matching ` Counters.csv`, `-counters.json`, `-game.log`. **cached = rejected texture cache; buffer = retained version.** |
| Rejected ground source | `tmp/terrain-material-layer-skip-experiment.wgsl`; `tmp/terrain-transform-texture-implementation/`. |
| Mac blade preparation | `tmp/grass-{reference,prepared}.gputrace`, matching counters and `grass-preparation-comparison.{json,txt}`. |
| A17 shader/preparation/rejection/atomics | `tmp/iphone-low-{reference,prepared2,rejection,no-counters}.gputrace`, matching ` Counters.csv`, `-counters.json`, `iphone-preparation-comparison.json`. The original `prepared` capture is incomplete. |
| Mirrored phone run / restoration | `tmp/iphone-low-sustained.log`, `iphone-low-sustained-summary.json`, `iphone-restored-game.log`. Mirrored timing is excluded from matched thermal comparisons. |
| Invalid early compact-ID captures | `tmp/mac-candidates-{reference,cached}.gputrace` (wind mismatch), `mac-candidates-{reference,cached}-v3.gputrace` (geometry mismatch). |
| Valid rejected compact-ID tradeoff | `tmp/mac-candidates-{reference,cached}-v4.gputrace`, matching ` Counters.csv` / `-counters.json`. `tmp/mac-candidates-cached-live.log` preserves the poor 39–42 FPS live run. |
| Current mask capture/live comparison | `tmp/mac-mask-{reference,cached}.gputrace`, matching ` Counters.csv` / `-counters.json`; [counter comparison](../tmp/candidate-mask-counter-comparison.txt), [live summary](../tmp/mac-mask-live-summary.json), `mac-mask-{reference,cached}-live.log`. |
| Latest validation | `tmp/candidate-mask-tests.log`, `candidate-mask-native-test.log`, `candidate-mask-other-native-tests.log`, `candidate-mask-release-build.log`. Earlier `candidate-cache-native-test.log` preserves the initial image-tolerance failure; `candidate-cache-native-test2.log` its revised population/image check. |
| Earlier validation | `tmp/render-improvements-tests.log`, `render-improvements-native-tests.log`, `terrain-cache-tests.log`, `terrain-buffer-tests.log`, `blade-preparation-tests.log`, `blade-preparation-final-native-test.log`, `rejection-tests.log`, `vegetation-cache-tests.log`. |
| Last manual Mac smoke check | `tmp/mac-grass-restored.log`; `tmp/YarraReleaseProfile.app` is the local launch wrapper for the Release executable. |
| Screenshots | `/Users/eugenepisotsky/Downloads/` filenames listed in the iPhone section. The user's earlier recording is under `content/`. |

The table uses braces and shared suffixes as naming shorthand, not literal filenames.
Some analysis scripts evolved with the investigation; preserve their generated summaries
as well as inputs. Detailed test names, individual captures and exact counter tables
are in [RENDER_AUDIT.md](RENDER_AUDIT.md).

## What remains unknown, and what has not been tried

- No matched, long, unmirrored iPhone run demonstrates the net benefit of the retained
  changes. No measured app-attributed phone power breakdown identifies CPU versus
  GPU/bandwidth contributions to heat. The current thermal problem remains open.
- No clean, agreed representative workload and sustainable GPU/power budget has been
  accepted as the project's visual baseline. Different cameras, resolutions and
  instrumentation repeatedly changed the comparison. The latest scripted route makes
  single-frame comparisons more repeatable, but it is not a complete gameplay test.
- No full terrain material cache, terrain visual simplification, new grass shader/
  representation LOD, prepared-bin priority change, or final MSAA attachment-store
  optimization has been delivered. These are hypotheses with quality/engineering costs.
- Xcode flagged 32.47 MiB of stored pre-resolve MSAA data in an iPhone opaque pass.
  Avoiding that store requires proving later passes do not need the samples, or
  restructuring passes. Blindly discarding them may break transparency/transmission/
  UI/custom rendering. This is an investigation lead, not a known safe fix.
- The roughly 40 FPS Mac run has not been causally explained. Later isolated runs
  recovered 120 FPS; that does not prove either the code or Xcode was solely responsible.
- Several caches are functionally validated but have no demonstrated meaningful net
  thermal benefit. Their maintenance and bandwidth costs should remain open to review;
  “already implemented” is not evidence they should all be retained indefinitely.

The engine discussion did **not** establish that Bevy is incapable of this game,
that switching engines will fix it, or that this custom material must be kept.
Examples discussed were Unity's distant terrain basemap, Unreal's per-component
unused-layer pruning, and runtime virtual texturing. They illustrate different
ways to avoid/reuse work, not a measured comparison with this project. A conventional
multi-layer terrain shader can also make many texture calls, so “12 samples” alone
is not a sufficient diagnosis. Porting this shader to another engine would not
automatically provide a configured terrain cache or cheaper grass representation.

Reference links from that discussion, if a later session researches the alternatives:
[Unity basemap distance](https://docs.unity3d.com/6000.0/Documentation/ScriptReference/Terrain-basemapDistance.html),
[Unity URP terrain implementation](https://github.com/Unity-Technologies/Graphics/blob/master/Packages/com.unity.render-pipelines.universal/Shaders/Terrain/TerrainLitPasses.hlsl),
[Unreal landscape materials](https://dev.epicgames.com/documentation/en-us/unreal-engine/landscape-materials-in-unreal-engine),
[Unreal runtime virtual texturing](https://dev.epicgames.com/documentation/en-us/unreal-engine/runtime-virtual-texturing-in-unreal-engine).
No competing-engine representative scene was built or benchmarked here.

The last discussion acknowledged that this had become renderer R&D before much game
development, with no convincing user-visible return. Possible directions were a
simpler affordable baseline in Bevy, or a bounded representative trial of an established
terrain/vegetation solution elsewhere before considering migration. **The user has
not selected either direction.** A suggested 20% whole-frame improvement threshold
was a proposal for a future substantial change, not an achieved result or an agreed
requirement. The next session should reassess the scope and evidence before extending
the old implementation plan.

## Suggested opening brief for the next session

> Read `docs/PERFORMANCE_HANDOFF.md` first, then the relevant evidence in
> `docs/RENDER_AUDIT.md`. The iPhone 60 FPS/heat problem remains unresolved after
> extensive isolation tests and several caches. Ground dominates some views; grass
> dominates the physical phone low view. Some arithmetic reductions added bandwidth,
> and the latest grass mask gave only about 0.05 ms in one Mac live pair. Do not repeat
> already-rejected zero-weight branching or compact candidate IDs as new ideas, or
> treat old replay timings as reliable gains. Preserve the dirty tree and ignored
> `tmp/` artifacts. Current testing scope is macOS at display VSync (120), with iPhone
> sustained 60 still the ultimate requirement. Reassess whether the rendering approach
> and complexity are appropriate before proposing more work. No engine migration or
> further renderer architecture has been selected.
