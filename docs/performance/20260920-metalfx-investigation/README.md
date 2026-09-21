# MetalFX performance investigation — 2026-09-20

The later [empty-frame isolation](EMPTY_FRAME.md) investigates the user's roughly
4 ms screenshots with scene draws disabled. A stock empty Bevy camera measured
0.79 ms; the game's empty 33% presentation measured 1.25 ms with Linear and 2.74 ms
with Spatial. The remaining full-game overhead is not completely attributed.

The Python helpers preserve the original experiment setup and depend on its local
app bundles, fixtures and macmon binary. They refuse to run while a release game
is already open; they do not pause or resume other game processes.

The [follow-up investigation](FOLLOWUP.md) tests actual-frame native replay,
removing duplicated motion work, and a 33% input size. The short benchmark
improved at 33%, but the user's subsequent release gameplay test still found
severe heating/performance degradation and rejected its image quality. Temporal
has no accepted performance preset from this investigation. The 50% setting
below remains slower than the native-sized composite baseline.

The Temporal regression is reproducible without grass. In this release-build
comparison, Temporal costs about 11% more GPU elapsed time than native 4× MSAA;
Spatial costs about 38% less. This does not reproduce a several-fold slowdown
relative to native at the same physical resolution. It does reproduce near-total
GPU activity with Temporal. Native also runs close to full GPU activity here.

The largest demonstrated cost is temporal reconstruction, including its runtime
dependencies. There is no evidence from this investigation of per-frame scaler
creation, CPU pixel copies, a second application queue, repeated history resets,
or an accidentally full-resolution main raster pass. There are integration
inefficiencies and a confirmed measurement problem, described below. No renderer
code was changed and no performance fix is claimed.

The tested checkout is commit `8b668796946ffc256b18b6e5399043ff3b90011d` **plus the
existing uncommitted integration**. The entire `crates/upscaling` directory is
untracked; its game, terrain, cloud and vegetation integration includes modified
tracked files. Commit HEAD alone does not reproduce this experiment.

## Matched release results

Apple M2 Max, macOS 26.4.1, physical output **3456×1942**, fixed
`tmp/cloud-performance/static.sqlite` world and `close-view.ron` camera, grass
disabled, bloom enabled except where specified, native UI, 120-FPS pacing.
Native uses 3456×1942 input and 4× MSAA. Spatial and Temporal use 1728×971 input;
Temporal replaces MSAA with its own antialiasing. The native case uses the
composite presentation path at 100% with Linear copying, so presentation stays
comparable; it is not the separate Direct path.

Each accepted run used 15 seconds of warmup and 10 seconds of measurement.
`cargo build --release -p yarra-app-game --offline` succeeded before measurement.
No detailed pass probes, screenshots, GPU captures or compilation ran alongside
these comparisons. Normal sampled timestamps and Metal HUD logging were enabled
in every case. The power runs also used the existing macmon v0.8.2 binary.

| Rendering path | Metal HUD GPU median | GPU active, mean | GPU clock, mean | Estimated GPU W, mean |
| --- | ---: | ---: | ---: | ---: |
| Native, 4× MSAA | 6.36 ms | 97.5% | 1387 MHz | 24.2 W |
| MetalFX Spatial, 50%, 4× MSAA | 3.97 ms | 85.7% | 1165 MHz | 10.7 W |
| MetalFX Temporal, 50% | 7.07 ms | 99.4% | 1395 MHz | 21.9 W |
| Temporal prerequisites + linear reconstruction bypass | 4.01 ms | — | — | — |
| Temporal, bloom disabled | 6.85 ms | 99.0% | 1393 MHz | 19.8 W |

Accepted runs delivered approximately 120 application updates/s and 8.33–8.35 ms
mean presentation intervals. A preceding warmed native run also had a 6.36 ms
HUD median. The first two runs with only five seconds of warmup included loading
and pipeline stalls and were excluded.

HUD packets overlap: these medians describe the sampled values, not a population
of independent frames. Power is a system-wide estimate, not process attribution;
there were nine complete measurement-window power samples per power run. Clocks
were not locked. Most runs reported thermal state `fair`, the earlier warmed
native run `nominal`. These short runs do not establish sustained thermal
behavior. In particular, **higher Temporal power than native was not reproduced**:
native consumed more estimated GPU power despite its shorter GPU duration.

The bypass comparison retains Temporal's jitter, prepasses, motion preparation,
native-sized HDR targets and native-sized postprocessing. The difference from
Temporal is therefore a useful reconstruction-on/off experiment, though it
includes changed dependencies, clocks and the replacement linear pass. It is
not an exact isolated MetalFX kernel duration or an equivalent-quality AA mode.

## What the native probe rules out

[native.swift](native.swift) ([raw results](native-results.txt)) invokes MetalFX directly, with no Bevy, wgpu, scene
geometry, grass, bloom or presentation. It initializes synthetic flat HDR color,
depth and motion, and measures completed native command buffers. Each case runs
240 frames, discarding the first 60; CPU waits exist only in this isolated probe.
This is a synthetic baseline, not a prediction of real-scene timing or image quality.

| Input → output | Configuration | Median reconstruction |
| --- | --- | ---: |
| 1728×971 → 3456×1942 | Output-sized allocations, small content rectangle, auto exposure | 2.377 / 2.369 ms |
| 1728×971 → 3456×1942 | Compact allocations, auto exposure | 2.335 / 2.346 ms |
| 1728×971 → 3456×1942 | Compact allocations, auto exposure disabled | 2.355 ms |
| 1280×720 → 2560×1440 | Output-sized allocations / compact allocations | 1.439 / 1.435 ms |

The full-sized descriptor input dimensions are unusual but match the actual
textures; `inputContentWidth/Height` specify the smaller rendered rectangle.
Changing to compact allocations saves only about 0.03 ms at the tested physical
surface. It cannot explain a multi-millisecond regression. Auto exposure is not
the major cost either. Adding a packing pass purely to change this descriptor
is not supported by these measurements.

In the actual game, native MetalFX command-buffer elapsed medians were 3.33 ms
with bloom and 3.42 ms without it. These include dependency stalls and cannot be
added to HUD or pass timings. The gap from the synthetic 2.35 ms baseline remains
unattributed: it needs a runtime dependency trace and representative input
comparison before calling it avoidable integration overhead.

The scene must save more than the reconstruction and prerequisite costs to win.
At this surface, lowering scene resolution while retaining the Temporal path
saves roughly 2.35 ms versus native in the HUD comparison; enabling reconstruction
then adds roughly 3.06 ms. That explains the measured loss in this scene without
requiring grass. MSAA's four samples also do not mean four complete executions
of ordinary pixel shading. Apple's tile-based resolve and the project's existing
resolve-only color-store optimization make native MSAA a stronger baseline than
that assumption suggests. See Apple's [MSAA sample](https://developer.apple.com/documentation/metal/improving-edge-rendering-quality-with-multisample-antialiasing-msaa).

## Confirmed measurement defect

`crates/app_game/src/render_audit/timing/gpu.rs` surrounds the frame with separate
one-invocation compute-marker command buffers. Those markers do not consume the
frame's textures. Their timestamps are not a reliable completion envelope for
all graphics, MetalFX and postprocessing work. They also perturb scheduling.

| Temporal configuration | In-game marker median | Metal HUD median |
| --- | ---: | ---: |
| Bloom on | 6.63 ms | 7.07 ms |
| Bloom off | 3.93 ms | 6.85 ms |

The marker suggests a 2.70 ms saving; HUD shows only 0.22 ms and GPU active time
stays near 99%. This confirms the concern previously recorded in `UPSCALING.md`.
The panel's normal GPU figure and A/B report use these markers even when detailed
probes are off. They must not be treated as authoritative whole-frame costs or
used to attribute the apparent difference to bloom. Repairing this requires
timing actual workloads/completion, not merely sampling the empty markers more
often. A deliberately serialized diagnostic must also be identified as such.

The earlier temporary profiling app
`tmp/performance-ui/YarraPerformance.app/Contents/MacOS/YarraPerformance` points
to **target/debug/yarra-app-game**. Its earlier results are useful diagnostics but
are not release measurements. This investigation used the separate release
bundle and verified `debug_assertions=false` in the audit logs. Debug versus
release does not explain away the Temporal regression.

## Integration cleanup candidates

The following costs are present in source; their individual performance impact
was not measured in this investigation:

* **Duplicated motion work.** `upscaling/src/temporal.rs::make_history` allocates
  another full-output-sized RG16Float texture. `initialize_motion` copies mesh
  motion into it and recomputes sky motion. Bevy 0.19.1 already writes mesh and
  background motion in its prepass. Investigate using that attachment for the
  custom grass motion writes, with camera/reset/jitter correctness tests. This
  could remove a separate pass and a 25.6 MiB allocation at this surface. It is
  not a demonstrated multi-millisecond fix.
* **Full-sized prerequisite attachments.** `MainPassResolutionOverride` reduces
  the raster viewport, not allocation dimensions. Depth, motion and HDR surfaces
  stay native-sized; Bevy's prepass also copies its full-sized depth attachment.
  Compact scene attachments could reduce memory/clear/copy costs, but the native
  probe shows that simply packing inputs does not make reconstruction much faster.
* **Unused spatial presentation allocation in Temporal.** The game keeps its
  separate full-sized `assets.upscaled` image even while displaying `assets.target`
  for Temporal. It remains allocated, but the spatial system correctly skips the
  Temporal view; this is memory waste, not a second upscale every frame.
* **Native-size postprocessing.** Temporal reconstructs before bloom and tone
  mapping. That follows [Apple's integration guidance](https://developer.apple.com/videos/play/wwdc2022/10103/).
  Moving those effects ahead of reconstruction changes the input/quality contract.
  Bloom-off measurements do not support blaming it for the full regression.

Scalers are cached by view and dimensions, the native backend uses the existing
wgpu Metal device/queue, steady-state samples have `reset=false`, and Temporal
sets MSAA off. Synchronous initialization is requested when creating the scaler;
it is not a per-frame wait. Apple explicitly supports that setting to avoid the
slower interim upscaler during background compilation ([initialization docs](https://developer.apple.com/documentation/metalfx/mtlfxtemporalscalerdescriptor/requiressynchronousinitialization)).

The next implementation work should first make the performance collector honest,
then benchmark removal of duplicate motion work, then trace the remaining native
command-buffer dependencies. A broad attachment rewrite should require a measured
win. Auto currently selects Spatial, which delivered the intended optimization
in this test; Temporal remains an explicit quality experiment with a substantial
output-resolution-dependent cost.

## Reproduction and artifacts

[summary.json](summary.json) records exact accepted results, build/input hashes,
methodology and limitations. [commands.json](commands.json) contains each command
and environment. Raw logs, power samples and the analysis helper are retained
locally in `tmp/metalfx-investigation-20260920/`. The helper selects the actual
`measure_start`→`complete` interval and discards the first overlapping HUD packet;
the earlier ad-hoc run script's warmup-index summaries are not used in this report.

Run the standalone probe from the repository root:

```sh
xcrun swiftc -O -module-cache-path /tmp/yarra-metalfx-probe-cache \
  docs/performance/20260920-metalfx-investigation/native.swift \
  -o /tmp/yarra-metalfx-probe
/tmp/yarra-metalfx-probe
```

The game comparisons all exited successfully. Existing rendering code and the
user's uncommitted implementation were preserved; this investigation adds only
the report and reproduction artifacts.
