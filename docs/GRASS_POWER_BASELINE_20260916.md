# First powered grass comparison — September 16, 2026

Tracked as **GP-009** (powered baseline) and **GP-010** (pacing-report correction)
in the [optimization log](GRASS_OPTIMIZATION_LOG.md). The
[preserved evidence bundle](performance/20260916-190422/README.md) includes raw logs,
manifests and machine-readable results outside the ignored temporary directory.

Follow-up: the [grass on/off suite](GRASS_ON_OFF_20260916.md) completed with matching
inputs, nominal pressure and 16.67 ms HUD interval p95 in all visits. The earlier
pacing pattern did not reproduce despite no intentional setup changes reported
by the user. Its cause remains unresolved; the historical evidence below is unchanged.

Later fullscreen follow-up: [GP-015](GRASS_FULLSCREEN_120_20260916.md) sustained
near-120 for 15 minutes, with temporary pressure and recovery while still at 120.
The earlier failure remains valid for this windowed session, but is not the Mac's
absolute ceiling. The conditions differ; no grass optimization was applied.

The current low-camera scene is substantially cheaper to run at 1440p60 than at
1440p120 on this M2 Max. The second 120 fps repetition develops thermal pressure,
lower GPU clocks and missed cadence. Returning to 60 restores the requested
average update rate and the recorded thermal state recovers. This supports the
user's concern that short GPU durations alone are a poor efficiency comparison.

There is a separate presentation-pacing concern at 60 fps: the app updates evenly,
but HUD presentation intervals cluster around 8.33 and 25 ms, averaging 16.67 ms.
The initial report's `OK` for the first run checked average cadence and missed
this pattern. The analyzer now adds a pacing warning and displays interval p95.
This does not invalidate the measured power or average rate, but prevents a claim
that these results establish evenly presented 60 fps.

## Evidence and controls

Fullscreen clarification, September 16: these runs used a **1280×720 logical
window**, with **2560×1440 physical presentation and world rendering**. They were
not fullscreen tests. The user wants fullscreen play; that baseline is tracked
separately as GP-015/GP-016. These historical results are unchanged.

Session: [20260916-190422](../tmp/grass-profiles/20260916-190422/).
The session completed with all four games exiting normally, an empty collector
stderr file, and 98.4–99.4% measured-window power coverage. Each run uses 30 seconds
of warmup and 120 seconds of measurement, separated by 15 seconds with the game
closed. Those gaps do not reset thermal conditions.

All four runs have identical binary, database, canopy and shader hashes. Focus,
2560×1440 internal pixels, 4× MSAA, Balanced density, wind, lighting and render
settings pass their checks. Source revision stays 51, with 49 source pages and
49 prepared terrain pages active. Counters are disabled, so no new exact visible
grass count is available from this session. The repro disables the depth prepass,
as before; this is not the full normal gameplay configuration.

Host: Mac14,6 / Apple M2 Max / 32 GiB. AC power and charging were recorded at session
start; Low Power Mode stayed off in the app audits. Charger state was not sampled
continuously, and fans, temperatures, ambient conditions and other processes were
not separately recorded. Power below is an estimated CPU/GPU subsystem quantity,
not wall-plug, battery or isolated grass power.

## Measured averages

| Run | App fps | GPU W | CPU W | CPU + GPU W | GPU active MHz | GPU active residency | HUD GPU ms | HUD interval p95 ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| First 60 | 60.00 | 3.65 | 1.52 | 5.18 | 702 | 70.4% | 9.96 | 25.00 |
| First 120 | 119.73 | 16.05 | 4.54 | 20.59 | 1,361 | 86.2% | 5.16 | 8.34 |
| Second 120 | 103.35 | 9.33 | 2.90 | 12.23 | 813 | 95.4% | 9.29 | 25.00 |
| Final 60 | 60.00 | 4.15 | 1.60 | 5.74 | 712 | 69.9% | 9.84 | 25.00 |

The first 120 fps measurement uses approximately 4.0 times the CPU+GPU power and
4.4 times the GPU power of the first 60 fps measurement. It uses 3.6 times the
CPU+GPU power of the final 60 fps measurement. These are observed ratios within
this session, not universal frame-rate scaling rules.

The first 60 run reports almost twice the GPU duration of the first 120 run,
despite much lower power. At 60 the GPU operates around 700 MHz; the first 120 run
operates around 1,360 MHz. Thus treating 5.16 ms as intrinsically cheaper than
9.96 ms would reverse the efficiency interpretation here. Neither duration gives
a fixed-clock grass budget, and the lower power of the second 120 run comes with
a missed performance target.

## Thermal and clock chronology

Times below are local UTC+03; power text timestamps have approximately one-second
precision. App thermal labels and powermetrics labels are separate scales.

- **19:05:56–19:07:56:** first 60 measurement. Thermal state stays nominal.
- **19:08:42–19:10:42:** first 120 measurement. Thermal state stays nominal,
  although nominal does not mean temperature is constant or no heat is accumulating.
- **19:11:25:** powermetrics enters moderate pressure during the second 120 warmup.
- **19:11:27:** pressure becomes heavy, just before its measurement starts at
  19:11:27.554. The app reports fair pressure throughout that measurement.
- **19:11:27–19:13:27:** second 120 measurement. Its four successive 30-second
  blocks average approximately **111.4, 101.0, 97.2 and 104.2 fps**. GPU frequency
  averages approximately **853, 802, 775 and 824 MHz**, with about 95–97% active
  residency. The raw GPU software-requested state stays at P8, its highest listed
  state, while achieved active frequency remains much lower than the previous run.
  Together these observations strongly support thermal/performance limiting.
- **19:13:56:** pressure falls to moderate during the final 60 warmup.
- **19:14:12–19:16:12:** final 60 measurement maintains 60.00 app fps.
  Powermetrics returns to nominal at **19:14:32**, approximately 19 seconds into
  measurement (49 seconds after this game's startup). The app independently logs
  nominal at about the same time. The original `CHECK` includes this recovery period;
  it does not mean the final 60 run missed its average target.

We have not measured temperatures or established the cooling policy's detailed
cause. The data supports thermal limiting much more strongly than a content or
configuration regression: inputs and settings match, pressure rises, achieved
clocks fall while requested state remains high, and cadence deteriorates.

## Presentation pacing and report correction

Both 60 runs have no app-update intervals above the tool's 25 ms late-update
threshold, and their one-second update windows remain close to 60. The HUD data,
however, is dominated by approximately 8.33 and 25 ms presentation intervals.
Interval p95 is 25 ms in both cases. The first 120 run has p95 8.34 ms.

[Apple documents the HUD log format](https://developer.apple.com/documentation/xcode/monitoring-your-metal-apps-graphics-performance/)
as presentation interval / GPU time pairs. This is a presentation-pacing signal
that the average-FPS check missed. HUD samples overlap and repeat, so their count
is not an independent presented-frame count. Do not turn their tail fraction into
an exact dropped-frame percentage or a 1% low FPS metric.

The cause is unresolved. The profiler's event-loop limiter, render/presentation
queue, display refresh behavior and workload completion timing need a dedicated
presentation trace or controlled pacing test. This session alone cannot assign
it to grass, the limiter or the compositor.

The report generator now warns when interval p95 exceeds 1.25 times the requested
frame interval. This is a screening heuristic, not a universal deadline rule.
The regression test uses alternating 8.33/25 ms samples that pass the average-60
test and must still produce a pacing warning. All 15 Python tests pass.

The original generated report is preserved as `report-original.html`,
`report-original.md` and `report-original.json`; raw game and power logs were not
modified. The regenerated report now labels the first 60 run `CHECK`, retains
`OK` for the first 120, `MISSED TARGET` for the second 120, and `CHECK` for the final
60. No game or renderer behavior changed during this analysis.

## Next measurement

After the Mac settles, run the existing grass on/off suite at 1440p60 using the
same binary and settings. The current release binary and canopy file were checked
against this session's hashes and match, so rebuilding is unnecessary:

```sh
python3 tools/grass_profile.py suite tools/profiles/grass-on-off.json --no-build
```

Keep the charger, display, power mode and background activity consistent. This
test measures how disabling grass changes whole-scene power and cadence, including
changed ground visibility and clock behavior. Inspect pacing in both variants.
Then choose a grass-specific bottleneck experiment and investigate presentation
pacing separately. Do not infer a maximum grass density, remaining headroom for
a populated world, or mainstream-PC equivalence from this Mac session.
