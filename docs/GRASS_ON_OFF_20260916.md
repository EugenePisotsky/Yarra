# Grass on/off at 1440p60 — September 16, 2026

Experiment **GP-011** in the [optimization log](GRASS_OPTIMIZATION_LOG.md).
Session: `20260916-192957`. All four visits maintained approximately 60 app fps,
reported nominal thermal pressure, and had a 16.67 ms HUD presentation-interval
p95. Disabling grass reduced reported GPU and combined CPU+GPU power in both
comparisons. The two grass-off visits differ substantially in power, so the size
of the saving is not yet repeatable enough to assign a precise grass-only wattage.

The earlier 8.33/25 ms presentation pattern did not reproduce. No game code changed
between these suites, and the user reported no intentional display/refresh,
power-setting or background-app changes. This leaves that issue intermittent and
unexplained; it is not a confirmed fix.

Evidence: [preserved archive](performance/20260916-192957/README.md),
[structured analysis](performance/20260916-192957/analysis.json),
[local original session](../tmp/grass-profiles/20260916-192957/).

## Controls and checks

- Same M2 Max / 32 GiB machine and macOS version as the
  [earlier 60/120 suite](GRASS_POWER_BASELINE_20260916.md).
- Binary, database, canopy and every archived shader hash match across all four
  visits **and** the earlier suite. The authored population and renderer were not
  optimized between these measurements.
- 2560×1440 internal rendering and presentation surface, 4× MSAA, Balanced density,
  low-walk in real time, wind on, Gaussian world shadows, UI on, prepared terrain
  on, repro prepass off and counters off. Grass enabled/disabled is the intended
  independent variable.
- Windowed at 1280×720 logical pixels, not fullscreen. The user subsequently
  clarified that fullscreen is the intended play mode. Its normal 75% world scale
  must be evaluated against the actual fullscreen surface, not this window's size.
- Order: on/off/off/on. Each visit has 30 seconds of warmup and 120 seconds of
  measurement, with 15-second app-closed gaps. These gaps do not reset temperature.
- All games exit successfully; collector stderr is empty. Power coverage is
  98.5–99.4% of each measurement window. All 655 collected power samples report
  nominal pressure, including warmup and gaps. Focus/settings checks pass.
- All visits have 49 source pages, 196 source work items and 49 active prepared
  terrain pages. Source revision is stable within each visit (50 in the first
  off visit, 51 elsewhere); input hashes and resident counts agree.
- AC power and charging at 83% battery were recorded at startup, versus 46% at
  the earlier suite's startup. Low Power Mode remains off. User-reported unchanged
  setup does not establish constant temperature, charging demand, display timing
  or background system activity. Those are not continuously measured here.

## Results

| Visit | App fps | GPU W | CPU W | CPU + GPU W | GPU active MHz | GPU active residency | HUD GPU ms | HUD interval p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 01 — grass on | 60.00 | 2.97 | 1.71 | 4.68 | 714 | 73.8% | 9.74 | 16.67 ms |
| 02 — grass off | 60.00 | 2.20 | 1.61 | 3.81 | 457 | 54.7% | 5.38 | 16.67 ms |
| 03 — grass off | 60.00 | 0.92 | 1.69 | 2.60 | 532 | 49.4% | 4.94 | 16.67 ms |
| 04 — grass on | 60.00 | 3.18 | 1.73 | 4.91 | 714 | 73.2% | 9.74 | 16.67 ms |

The mean of the two visit means is **4.79 W on / 3.21 W off** for combined CPU+GPU,
and **3.08 W on / 1.56 W off** for GPU alone. These are descriptive averages, not
a precise attribution of 1.58 W to grass. Adjacent on/off comparisons give combined
power differences of approximately **0.86 W** and **2.30 W**. The uncertainty is
material to any claim about grass's percentage of the frame or energy budget.

Grass-on is comparatively consistent: combined power differs by 0.23 W across
visits and GPU frequency is approximately 714 MHz in both. Grass-off combined power
differs by 1.21 W, driven by GPU power. CPU power is approximately 1.6–1.7 W in all
visits. The first off visit requests the lowest listed GPU state for about 96% of
the measured time, versus about 71% in the second, yet reports higher power. Active
frequency alone does not explain the difference.

Thirty-second GPU-power averages show this is not one anomalous sample:

| Visit | 0–30 s | 30–60 s | 60–90 s | 90–120 s |
| --- | ---: | ---: | ---: | ---: |
| 01 — on | 3.11 W | 2.94 W | 2.95 W | 2.88 W |
| 02 — off | 2.22 W | 2.25 W | 2.17 W | 2.19 W |
| 03 — off | 0.84 W | 0.85 W | 0.96 W | 1.03 W |
| 04 — on | 3.33 W | 3.28 W | 3.07 W | 3.03 W |

Using the second GPU-power section of the raw powermetrics log produces essentially
the same off averages, about 2.199 and 0.916 W. The spread is not caused by adding
the duplicate GPU power field or choosing the wrong occurrence. GPU power in the
app-closed gaps returns to approximately 0.03 W. This does not identify the source
of the visit-to-visit difference; platform state, other activity or the estimator
remain possible explanations, not established causes.

## What the toggle measures

Code inspection and run audits agree: the disabled profile skips grass generation
and grass draw submission; generation/preparation dispatch counts stay zero,
blade preparation is disabled and candidate caching is inactive. Grass-on visits
execute generation/preparation as the camera moves.

The scene, resident source data, object rendering, prepared terrain and ground
canopy treatment remain present. GPU buffers remain allocated. Removing visible
blades exposes more terrain, and GPU performance states change. Thus this is the
whole-scene effect of disabling the procedural grass pipeline, not an isolated
grass pass cost or the complete CPU cost of all vegetation-related systems.

Do not subtract the 9.74 ms grass-on HUD duration from the off duration to declare
that grass costs about 4–5 ms: the achieved clocks and rendered visibility differ.
Likewise, do not infer a maximum blade count or a PC budget from these watt values.

## Pacing follow-up: GP-012 remains open

Approximately 99.6%, 95.0%, 98.4% and 98.9% of the respective visits' correlated HUD
samples round to 16.7 ms. Occasional 8.33/25 ms pairs remain, with a few 33.33 ms
samples in the first visit. The earlier near-even split between 8.33 and 25 ms is
absent. HUD samples overlap, so these are sample proportions, not independent
frame/drop counts.

The app logs 1, 0, 2 and 2 intervals above its 25 ms late-update threshold across
the four visits; the largest is about 29.9 ms. Passing the current average/p95
screen does not mean every frame is perfect or that the earlier issue is fixed.
All four `OK` results are valid under the current checks; that status does not
assess repeatability of power between separate visits.

## Decision and next work

Retain the current renderer as the baseline. These two grass-on measured windows
show a much lighter and better-paced 1440p60 workload than the earlier repeated
120 fps case, with no elevated pressure in this session. They do not replace a
long grass-on-only sustained test: the intervening off visits reduce load.

The next useful experiment is **GP-013: matched GPU pass/work attribution** for the
unchanged baseline, separating scheduling/generation, blade preparation and drawing/
fragment work. Use explicit 1440p controls and separate captures/counters from power
runs. An old default frame capture at 75% internal scale is not this workload.
Choose one optimization only after identifying the expensive work and preserving
the visual reference. GP-012 remains an independent pacing diagnosis; reproduce
its symptom before changing the limiter or renderer.

If a candidate's claimed gain is small, the grass-off spread makes another
controlled power comparison necessary. The present suite establishes the direction
of the disabling effect, not a precise saving or a reason to lower density now.
No renderer, shader, density setting or profiler behavior changed during this analysis.
