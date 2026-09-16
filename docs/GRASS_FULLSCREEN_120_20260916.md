# Fullscreen 120 fps sustained baseline — September 16, 2026

**Catalog correction (GP-017):** decoding the actual database proves the measured
baseline was **66 roots/m²**. Earlier 72-root labels came from the RON reference
and a hard-coded HUD label. The runtime payload hash and all recorded power/timing
results are unchanged. See [density diagnostics](GRASS_DENSITY_DIAGNOSTICS_20260916.md).

**GP-015 baseline completed; higher-density ceiling remains open.** This run held
approximately 120 fps for 15 measured minutes at current density, including the
final five minutes. Elevated thermal pressure was temporary and recovered during
the continuing workload. The user reported that fans were not loud; case heat was
not checked. This is stronger evidence for the intended fullscreen target than the
earlier short windowed comparison, and changes the current assessment.

Evidence: [preserved session](performance/20260916-200703/README.md),
[structured analysis](performance/20260916-200703/analysis.json).
History and next work: [optimization log](GRASS_OPTIMIZATION_LOG.md).

## Conditions and validity

- M2 Max, 38 GPU cores, 32 GiB; AC power, 99% and finishing charge at startup.
  Charging was not sampled continuously. Low Power Mode stayed off.
- Borderless fullscreen: **3456×2168 presentation surface, 2592×1626 world** at
  normal 75% scale. Monitor metadata reports 3456×2234 and 120 Hz; actual surface
  dimensions, not monitor dimensions, determine the world target.
- Current authored population: 66 roots/m²; Balanced production thinning,
  4× MSAA, low-walk repeating in real time, wind and UI on, prepared terrain on.
  Repro depth prepass remains off. This is not a populated gameplay acceptance test.
- One continuous run: 60-second warmup, **900.006 measured seconds**. Focus,
  resolution, settings and residency remained stable: 49 source pages, 196 work
  items, 49 active prepared terrain pages, source revision 51.
- Successful exit, no runtime errors, empty collector stderr, 898.858 seconds of
  required power telemetry (99.87% coverage). Counters were off; instance counts,
  preparation fallback and capacity drops were not independently measured.
- Binary matches the fullscreen tooling check. Database, canopy and all shader
  hashes also match both earlier powered suites. The executable changed to add
  fullscreen profiling and audit controls; no grass optimization was applied.
- Subsequent user clarification: they normally do not run other heavy workloads
  during tests and keep the game focused. Focus is independently confirmed by the
  logs. There is no evidence identifying a competing heavy app as a cause of the
  earlier difference. See [follow-up context](performance/20260916-200703/follow-up-context.json).

## Frame delivery, energy and clocks

| Measured period | App fps | GPU W | CPU+GPU W | Active GPU MHz | GPU active % | HUD interval p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Whole 15 minutes | 119.946 | 19.25 | 23.43 | 1,354 | 89.27 | 8.33 ms |
| First five minutes | 119.960 | 18.81 | 23.02 | 1,324 | 89.62 | 8.33 ms |
| Middle five minutes | 119.980 | 19.54 | 23.72 | 1,369 | 89.59 | 8.33 ms |
| Final five minutes | 119.897 | 19.41 | 23.54 | 1,370 | 88.60 | 8.33 ms |

The worst 30-second group was **119.45 fps**, around minutes 11:30–12:00, while
pressure was nominal. One approximately one-second app sample fell to 114.10 fps.
There were 53 app intervals longer than 12.5 ms out of 107,952 measured updates
(0.049%); maximum interval was 17.263 ms. Thus this was sustained near-120 delivery,
not literally every presentation locked to 8.33 ms.

HUD tail samples above 10.42 ms account for 0.045% of the reported samples. These
overlap and are correlated; neither this proportion nor the app late-update count
is an independently measured count of dropped presentations. The short whole-run
HUD GPU mean of 5.37 ms is not a density/headroom budget.

Five-minute and 30-second groups use whole app and power sample intervals within
each group, excluding boundary-crossing samples. Their coverage is retained in
`analysis.json`. Power is an estimate for CPU/GPU subsystems including other
processes, not total laptop power or isolated grass consumption.

### What the power measurement does establish

The final five minutes' 30-second GPU-power averages range from **19.21 to 19.61 W**;
the mean is 19.41 W. CPU power averages 4.13 W. In the app-closed interval before
launch, GPU power averaged 0.032 W and CPU power 0.302 W. Together with the user's
testing practice, this supports treating the running game and its presentation as
the dominant added GPU demand, while retaining the subsystem-estimate label.
The idle difference is context, not an exact per-process watt attribution.

These stable power estimates are useful even though the cause of different thermal
responses is unresolved. Apple's local `powermetrics` manual says its estimates may
be inaccurate but can help optimize energy efficiency; their absolute calibration
has not been validated here. Within-run consistency does not establish exact watts
or cross-session repeatability. GPU active residency is not a percentage of the
GPU's maximum useful throughput and cannot be converted into a blade allowance.

The missing measurement for optimization is the game's current **per-pass work
and bottleneck**, including generation/preparation, vertex/fragment work, terrain
and composition. Obtain a matching Metal trace/capture and counters, then compare
one change under matching conditions. Keep the sustained run as an acceptance
check. More temperature explanations cannot substitute for that GPU attribution.

Follow-up: [GP-013's native trace](performance/20260916-gpu-attribution/README.md)
now supplies initial direct GPU intervals: main opaque vertex/fragment stages
2.203/1.985 ms, grass generation/finalization 0.516 ms, preparation 0.071 ms,
scheduling 0.010 ms per observed encoder. Main rendering still combines grass and
terrain, so individual draw/limiter attribution remains next. These short diagnostic
timings are separate from the powered session; do not combine them into a synthetic
watts-per-pass result.

## Why the report says CHECK

Approximate transitions relative to the **start of measurement**, after warmup:

| Time | Powermetrics pressure |
| --- | --- |
| Start → 3:11 | Nominal |
| 3:11 → 3:13 | Moderate |
| 3:13 → 5:16 | Heavy |
| 5:16 → 6:10 | Moderate |
| 6:10 → 15:00 | Nominal |

Weighted measured coverage is about 720.0 seconds nominal, 56.6 seconds moderate,
and 122.2 seconds heavy. App-level thermal reporting separately changed from
nominal to fair around 3:12 and back to nominal around 6:11. The two APIs use
different labels; do not equate powermetrics “heavy” with app “serious”.

Clock averages dropped to about **1,149 MHz** in the 3:30–4:00 group (lowest single
power sample 1,109 MHz), then recovered to roughly 1,370 MHz. The same 30-second
group still delivered 119.967 fps. Thermal pressure/clock reduction occurred, but
did not cause the sustained FPS collapse seen in the earlier windowed run.
The last nearly nine minutes remained nominal with stable clocks and power.

The automatic warning is correct: it flags any elevated state in the measured
window and does not claim the final state stayed elevated. Do not suppress it or
relabel the historical report. The workload appears to have settled for the latter
part of this test; that is not proof of indefinite equilibrium or a cool case.

User response: “not sure about the heat I did not touch it but fans weren't loud”.
Record acceptable reported fan noise separately from unmeasured surface heat.

## Interpretation and next decision

Current grass **can sustain approximately 120 fps in the intended fullscreen
configuration for this 15-minute route**. A strict requirement of no elevated
thermal pressure at any point was not met. Low fan noise was reported; case
temperature remains unknown. No higher authored density has yet been tested.

### Why this was better than the smaller-window test

The measured difference is the **severity and recovery of the clock reduction**,
not a grass optimization. In GP-009 the failed 120 visit averaged 813 MHz and
103.35 fps. Here the weakest 30-second clock group was 1,149 MHz at 119.967 fps,
followed by recovery to about 1,370 MHz. Both runs encountered thermal pressure.

The short suite did not observe the same duration of continuing 120 fps load:

| Timing from first 120 app start, including warmup | Earlier windowed suite | Fullscreen run |
| --- | --- | --- |
| Heavy pressure first reported | 3:15 | 4:13 |
| End of 120 workload | 5:15, after two visits separated by 15 seconds idle | 16:00, uninterrupted |
| Pressure returned to nominal | 6:20, after switching to 60 fps | 7:10, while still at 120 fps |

The older run may have recovered if 120 fps had continued; it stopped before the
new run's recovery point. That is a hypothesis, not a reconstruction of the
unmeasured remainder. It does not by itself explain why the old dip was deeper.
Different cooling response and starting thermal conditions are plausible. The
old suite had already run a 60 fps visit, whereas this session has a different
workload history. Startup charging was 46% in the old session versus 99% finishing
charge here; its contribution to heat was not measured. No fan-speed, temperature
or per-process background-load record is available to choose among these causes.

Fullscreen changes composition, surface size, aspect ratio and visible work;
its effect cannot be isolated from these sessions. In fact this run's stable GPU
power is around 19.4 W versus about 16 W in the earlier successful 120 visit, so
the measurements do not show fullscreen made the GPU workload cheaper. More pixels
also do not guarantee more visible roots; counters were disabled.

The previous windowed 120 fps failure remains valid evidence for that session,
but cannot be treated as the Mac's absolute density ceiling. These are not a
controlled windowed/fullscreen A/B: surface, aspect ratio, internal pixels, thermal
history, charging state and profiler build differ. Recovery is consistent with a
changing cooling/power response, but fans and temperature were not instrumented,
so its cause—and the reason the earlier run behaved differently—remain unresolved.
Do not claim fullscreen caused an optimization or that charging caused the old drop.

A causal window-mode test would use the same current binary, world target, aspect
ratio, path, starting conditions and a sufficiently long continuous run in each
mode, reversing run order. Log charging throughout and add fan/temperature telemetry
if available. That is separate from validating normal fullscreen play and need not
block the next higher-density test. The present artifacts establish the outcomes,
not a unique cause for their difference.

Keep this as the current fullscreen baseline. The next density candidate is
**96 authored roots/m² (+45.5% versus 66)** in an isolated cooked database. Review
its appearance and emitted-work/capacity counters before the same sustained test,
then test 128 only if warranted. Record the density-dependent near-detail radius:
the authored density increase does not guarantee 33% more emitted blades or work.
Recheck 66 around candidate comparisons to expose session drift.

GPU active residency near 89% and short HUD times do not imply a proportional
number of extra roots will fit. Pass-level diagnosis remains the route to selecting
an optimization for more density or less energy. Native mainstream-PC 1440p60
acceptance and representative full-scene headroom are still unmeasured.
