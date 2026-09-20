# In-game Performance panel

The game now starts with a small **Performance | F1** button instead of the legacy
text overlays. Click it or press F1 to open/close the panel. Escape closes it or
cancels an active capture. `--performance-open` opens it at launch; `--render-audit`
also opens the new panel and retains audit logging. The editor is unchanged.

All controls are temporary and share the existing audit settings backend.
**Reset launch settings** restores the initial configuration, including applicable
command-line overrides. Nothing writes to authored or published world data.

## Pages

- **Overview:** recent frame intervals, GPU render median/p95, CPU system work,
  surface acquisition/submission/presentation-related durations, a frame spike graph,
  main-app elapsed time, thermal/low-power state, resolution, pipeline compilation, page
  loading, terrain triangles/patches, and available object LOD counts.
- **Features:** clouds, grass, sky/haze, bloom, terrain/object draws, shadow filtering,
  and grass wind. Click controls to toggle or cycle their labelled modes.
- **Quality:** 3D resolution (100/75/50%), MSAA, grass density, terrain error
  (1/2/4/8 pixels), object LOD size multiplier (0.5/1/2), and near terrain detail.
  Larger terrain error permits coarser geometry. A smaller object size multiplier
  selects coarser *available* visual assets. Neither changes collision detail.
- **Compare:** capture the current settings into A/B, restore either settings
  configuration, and export a report plus per-frame CSV files.
- **Advanced:** grass workload isolation, scene isolation, shading/material paths,
  prepass, expensive GPU counters, render path, input lock and legacy overlays.
  The **GPU pass timings** control enables a sampled command-group breakdown.
  It is off by default because the extra markers perturb execution. CPU system
  timings remain available. Missing or invalid GPU samples are not treated as zero.

## What a switch actually disables

- Grass off skips game-side terrain conformance and grass render preparation,
  generation and draws. Published vegetation/source streaming and terrain contact
  requirements remain. Ground canopy colouring is a separate terrain effect.
- Terrain/object draw switches hide mesh draws, including newly streamed meshes.
  Streaming, collision and animation continue. Their previous visibility is restored.
- Sky/haze and bloom remove their camera pass components. Authored illumination stays.
- Cloud Off skips cloud rendering/shadow passes. Shared material conversion,
  bookkeeping and resident caches remain; this is not a complete atmosphere teardown.
- Grass Compute only retains generation; Schedule only retains scheduling;
  Draw frozen retains generated geometry and locks view controls. Return to Full
  for normal gameplay, or use Reset launch settings.
- Terrain contact requirements can limit how much a coarser error setting saves.
  Clouds and scene animation continue during comparisons, including when the grass
  wind control is off.

The legacy vegetation debug keyboard cycles are disabled in the game so they do
not silently override panel settings. Editor/study shortcuts are unchanged.
Existing field/catalog and sun-motion shortcuts remain; avoid them during captures.
The operating system's Metal HUD is separate from the game's legacy overlays.

## A short comparison

1. Stay at one viewpoint and fixed time of day. Capture A.
2. The panel closes and view controls lock. Capture waits at least 2 seconds for
   loading/compilation to settle, then samples 10 seconds. It cancels if settling
   takes more than 8 seconds or the window loses focus. F1/Escape cancels manually.
3. Change one setting and capture B. The old slot survives a cancelled capture.
4. Compare **GPU render milliseconds**, CPU work and conditions, even if FPS stays
   capped. Leave GPU pass timings in the same mode for A and B. The report lists changed settings
   and flags viewpoint/time/thermal differences, camera movement or loading during
   capture. Restore A/B changes settings only, **not** camera or weather state.
5. Export writes `tmp/performance/comparison-<timestamp>/report.txt`, frame interval
   CSVs (`A.csv` / `B.csv`), GPU samples (`A-gpu.csv` / `B-gpu.csv`), and CPU categories
   (`A-cpu.csv` / `B-cpu.csv`). Override the output root with `YARRA_PERFORMANCE_DIR`. A/B slots live
   only for this session; exports are reports, not importable presets.

## What the timings measure

GPU render time uses hardware timestamps surrounding the queued render commands,
once every six source frames. It excludes presentation, implicit uploads before the
render graph, and the instrumentation readback. Readback uses a fixed four-slot ring
and asynchronous completion callbacks; it never waits for the GPU. Unsupported
devices show unavailable. On Metal, stage-boundary compute markers avoid Bevy's
disabled encoder-timestamp path; query resolution waits for submission completion
asynchronously to avoid stale counter data.

Detailed command-group spans surround each rendering system's queued buffers.
They include marker/scheduling overhead, can overlap on the GPU, and are a guide
for choosing A/B toggles, not isolated per-feature costs. Grass draws share the
opaque pass with terrain and objects. Invalid detailed scopes are omitted without
discarding a valid total. Compare total GPU time with a feature toggled to establish
its effect. Use `--gpu-timing-detail` to enable details at launch,
`--gpu-timing-off` to disable GPU probes entirely, and `--timing-log` for sampled logs.

CPU work sums measured system durations in the main and render schedules. Parallel
jobs may overlap; durations include waits inside those systems and exclude detached
background work. They are **not CPU utilization**. Surface acquisition and queue
submission have separate measurements because they can block. Present+readback is
the render call outside the render graph, not an exact VSync-only measurement.
Main-app First-to-Last time also includes waits and excludes render-thread work.
Never add CPU work, GPU time and waits into a frame total.

GPU/CPU samples retain their source frame and settings revision. A/B reports select
completed samples from the recorded frame range and revision, excluding delayed
results from earlier settings. A few trailing GPU samples may still be in flight
when a capture closes; the actual sample count is shown. Frame intervals still
include VSync/presentation waits. Equal capped FPS does not imply equal GPU load or
power consumption; ten-second captures do not establish sustained thermal performance.

The panel does not run an automatic long benchmark. UI text/graphs refresh at 4 Hz;
closed panels do not rebuild their detailed labels, and old overlays stay hidden
unless explicitly requested.
