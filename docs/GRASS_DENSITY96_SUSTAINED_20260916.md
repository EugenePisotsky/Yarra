# Denser fullscreen grass at 120 fps — September 16, 2026

GP-019 is the sustained follow-up to the GP-018 preparation-buffer experiment.
Session: `20260916-212946`. **96 roots/m² with the enlarged buffer ran mostly near
120 fps for 15 measured minutes, but did not deliver consistently smooth 120.**
An initial thermal event recovered; three brief, deeper stalls occurred later
while the sampled thermal labels were nominal. Keep the candidate experimental.

**Follow-up completed:** [GP-020 matched default-buffer control](GRASS_PREPARATION_POWER_COMPARISON_20260916.md)
also stalls and uses less GPU power in this pair despite longer GPU duration.
The next-control instructions below are retained as the historical protocol;
that run is complete. Default allocation remains unchanged.

## Configuration and provenance

M2 Max / Mac14,6, 38 GPU cores, 32 GiB, macOS 26.4.1. AC, battery 100% charged
at startup; Low Power Mode off. Fullscreen surface **3456×2168**, internal world
**2592×1626**, 120 Hz display, 4× MSAA, Balanced thinning, low-walk route, wind on,
repro prepass off, counters off. Thirty seconds app-closed idle, 60 seconds warmup,
900 seconds measurement. All measured samples retained focus. Source revision 51,
49 source/prepared terrain pages and presentation settings stayed constant.

Actual preparation allocation was 70,516,748 bytes, matching 524,288 prepared
blades. Binary, database, canopy and shader hashes all match the frozen GP-018
candidate. The original 66-root GP-015 baseline has different binary/database
hashes but matching canopy and shaders. See [evidence](performance/20260916-212946/README.md).

The authored density is 45.5% higher than 66. This uses the existing adaptive LOD:
nominal paired high-detail radius is about 4.79 m rather than 5.78 m. It is not a
comparison at a fixed near-detail distance. This powered run has no GPU counter
readback; zero fallback/drop acceptance comes only from the separate GP-018
diagnostics, not from the zero placeholders in this run's audit.

## Results and timeline

All times below start at **measurement**, one minute after the game starts.
Power samples have approximately one-second alignment precision. Five-minute
groups include only whole app/power sample intervals inside each group.

| Measured period | Average app fps | GPU W | CPU+GPU W | Active GPU MHz | Late updates |
| --- | ---: | ---: | ---: | ---: | ---: |
| Whole 15 minutes | 119.600 | 20.52 | 25.13 | 1,304 | 366 |
| First five minutes | 119.614 | 20.57 | 25.09 | 1,285 | 117 |
| Middle five minutes | 119.917 | 20.00 | 24.94 | 1,289 | 26 |
| Last five minutes | 119.266 | 21.01 | 25.38 | 1,340 | 222 |

One late update falls outside the five-minute groups' whole-sample boundaries.
The full run has 107,640 application updates, 366 late intervals (0.340%), and a
23.06 ms maximum update interval. The late-interval threshold is 1.5 times the
target interval. These are application counters, not independently counted
displayed frames. Whole-run HUD presentation interval p95 is 8.33 ms, which hides
the short stalls; HUD samples overlap and do not provide an independent 1% low.

- **0:00–3:59:** thermal pressure nominal; 30-second average GPU clocks around
  1,345–1,355 MHz. GPU power rises to approximately 22.6 W before the thermal event.
- **3:59:** moderate pressure; **4:01–6:06:** heavy pressure. GPU clocks fall to a
  minimum sampled 744 MHz. The initial noticeable cadence dip is around
  **4:24–4:30**, with a worst one-second app window of **100.99 fps**. The
  4:00–4:30 average is 117.83 fps. GPU power falls alongside clocks, so lower watts
  during this period are not an efficiency improvement.
- **6:06–7:05:** moderate pressure, recovering clocks. At **7:05** pressure returns
  to nominal and remains nominal through the end. The application's own thermal
  label independently goes fair → nominal at approximately the same boundaries.
- **11:48–11:49, 12:52–12:53, 13:56–13:58:** brief late stalls. These deserve
  separate investigation even though the longer thermal event has recovered.

| Late event, measurement time | Lowest approximately one-second app rate | Nearby minimum sampled GPU MHz | Thermal label |
| --- | ---: | ---: | --- |
| 11:48–11:49 | 84.38 fps | 1,045 | Nominal |
| 12:52–12:53 | 94.06 fps | 964 | Nominal |
| 13:56–13:58 | 77.31 fps | 942 | Nominal |

Nearby Metal HUD packets contain bursts of 16.67 ms presentation intervals at
all three late events. They are not just an application FPS display artifact.
GPU and CPU power also drop briefly. Their approximate 63–64 second spacing is
an observation from three events, not an established periodic mechanism. These
logs cannot determine whether clock reduction causes the stalls or follows reduced
work submission/presentation. Nominal pressure is not proof that every thermal or
power controller is inactive. There is no evidence here assigning them to another
application; the user's focused/no-heavy-workload account stands.

User observation, preserved verbatim:

> my impressions: first when fps dropped (you probably will see) the mac was hot then coolers started working louder then slowed down and it stabilized

That sequence is consistent with the initial pressure/clock event and recovery.
Fan RPM and case temperatures were not recorded, and the observation has no exact
timestamp. Therefore the fan response is useful context, not a measured causal
explanation or proof of thermal equilibrium. The final five-minute GPU power
is roughly steady around 21 W, but the late stalls prevent a clean cadence pass.

## What this establishes

The M2 Max can run this denser candidate near 120 for most of this route. This
does not meet a strict “smooth 120 without a hot/fan-ramp transition” criterion.
The original report's CHECK is appropriate; its near-target mean alone is too
permissive for acceptance. There is no sustained collapse, but there is also no
basis to mark the whole run stable or advance directly to a 128-root acceptance test.

For context, the original 66-root run's last five minutes were 119.897 fps,
19.41 W GPU and 23.54 W CPU+GPU. This candidate's corresponding values are
119.266 fps, 21.01 W and 25.38 W: approximately 8.2% more GPU power and 7.8% more
combined subsystem power in these particular runs. Density, preparation capacity
and binary changed together. These numbers **do not isolate the optimization's
energy saving** or imply linear scaling with density. App-closed idle estimates
also differed (this run 0.80 W combined, prior run 0.33 W); they are context, not
automatically subtracted application power. Power covers CPU/GPU subsystems,
not total laptop consumption or isolated grass watts.

The GP-018 matched replay still supports reduced repeated vertex work. Sustained
power benefit at identical density remains unmeasured. Native 1440p60 on named
mainstream PC hardware, populated scenes, other views and visual acceptance
remain necessary to set a shipping budget.

## Reporting change and next controlled test

The analyzer now reports the worst full app sampling window and warns when a
window falls below 95% of the cap, even if whole-run FPS/p95 pass. Short final
partial windows below 0.9 seconds are excluded from this check. This run has
11 such windows covering 11.06 seconds of sampled time; that is not an exact
duration of lost presentation or a 1% low. The HTML timeline now includes app
cadence and sampled thermal shading. Original reports remain untouched;
[reanalysis](performance/20260916-212946/reanalysis/report.html) contains the new warning.

Next, run the **same frozen 96-root workload with the original 131,072-blade
preparation capacity** after the machine has settled:

```sh
python3 tools/grass_profile.py suite tools/profiles/grass-fullscreen-96-default-120.json
```

Same binary/database/shaders/canopy, resolution, route, 120 cap, 60-second warmup
and 15-minute measurement. Only preparation capacity changes. This is about
16½ minutes including app-closed idle; the existing local sudo step is unchanged.
It provides the missing same-density control. Compare pressure recovery, final
five-minute watts, and brief stalls; a small gain or conflicting outcomes will
still require repeats in reversed order. The original 66-root run cannot supply
that control. If late stalls reproduce, the next diagnostic is a short native
CPU/GPU/presentation trace around an affected interval, separately from power
acceptance, before guessing at a renderer fix.

No production density or preparation default was changed. Verification: 23 Python
runner/report tests, exact reproduction of the original report from raw logs,
follow-up analysis round-trip, and evidence checksums.
