# Follow-up: can Temporal actually beat native?

**The short benchmark improved at 33%, but subsequent gameplay testing rejected
that setting as unusable.** The user launched with
`cargo run --release -p yarra-app-game` and reported rapid heating/fan ramp,
severe performance degradation and poor image quality even at 33%. Linear at
roughly 70–75% was substantially better in that test. This is a user observation,
not an additional instrumented measurement. The figures below remain evidence
about the bounded benchmark only; they do not establish a usable Temporal preset
or explain the sustained gameplay regression. No experimental renderer changes
were retained.

All game timing comparisons used the release build, the same world/camera as the
initial report, 3456×1942 physical output, grass off, bloom on, 120-FPS pacing,
15-second warmup and 10-second measurement. This time the independent GPU markers
were disabled with `--gpu-timing-off`. Metal HUD supplied the GPU duration samples;
macmon supplied system-wide power/clock estimates.

| Configuration | Input pixels | HUD GPU median | Mean estimated GPU power |
| --- | --- | ---: | ---: |
| Native + MSAA 4× | 3456×1942 | 6.30 ms | 24.14 W |
| Temporal, existing 50% path | 1728×971 | 7.15 ms | 22.29 W |
| Temporal, shared-motion experiment | 1728×971 | 7.26 ms | 22.62 W |
| Temporal, approximately 33% input, before native | 1152×648 | 5.53 ms | 11.56 W |
| Temporal, approximately 33% input, after native | 1152×648 | 5.49 ms | 12.72 W |

The 33% runs reduced GPU duration by about **12–13% versus native**, and by about
23% versus 50% Temporal. Estimated GPU power was roughly half native's in these
short measurements. All accepted runs delivered approximately 120 updates/s and
8.34–8.35 ms mean presentation intervals. The native and both 33% runs reported
thermal state `nominal`, with similar mean GPU clocks around 1377–1388 MHz. The
earlier 50% runs reported `fair`, at about 1395 MHz. Power estimates have nine
measurement-window samples per run and are system-wide, not process-attributed.
HUD samples overlap; these are not independent-frame statistical confidence
intervals or sustained thermal measurements.

The quoted watts are GPU-only estimates. Mean CPU power was another 9.3–10.4 W
in the 33% runs, versus 9.5 W for native; "half native GPU power" does not mean
half the computer's power or heat. GPU activity remained 92.2–96.0% at 33%.
Normal gameplay uses its standard VSync/event-loop behavior and enables the
panel's sampled GPU probes; these runs explicitly capped updates at 120 FPS
and disabled those probes. The native baseline also retained the new compositor
and Linear copy at 100%; it was not the renderer before the integration.
These differences must be controlled in any follow-up comparison.

**The user rejected image quality at 33%.** The benchmark itself contains no grass
and does not establish foliage, camera-motion or disocclusion quality. This resolution
renders only about one ninth of the output pixel count. These runs used the
existing diagnostic `--profile-size 1152x648` override. Following this experiment,
33% was added to the normal **F1 → Quality → Resolution** control for gameplay
testing. The startup default remains 50%.

## Actual-frame reconstruction replay

The flat-color benchmark was insufficient to settle the input-content question.
A temporary diagnostic captured the game's actual pre-reconstruction color
(RGBA16Float), final depth (Depth32Float), and motion (RG16Float) at Temporal history
index 1200. Capture used a debug build strictly for extracting data; no debug-build
timings enter the comparison above. The corrected capture completed without
rendering validation errors. An initial partial depth-copy attempt failed WebGPU
validation and its zero-filled outputs were replaced and excluded.

[replay.swift](replay.swift) uploads that captured input to native Metal textures
and runs MetalFX without Bevy, wgpu, game rendering, UI or bloom. As in the initial
probe, each case uses 240 frames and discards the first 60. Input is held fixed,
so this is a reconstruction-cost experiment, not an animation/quality test.

| Actual captured input configuration | Native GPU median |
| --- | ---: |
| Full-size input allocations, color writable like Bevy, A / B | 2.311 / 2.309 ms |
| Full-size input allocations, color read-only, A / B | 2.312 / 2.314 ms |
| Compact input allocations | 2.282 ms |

This confirms that approximately 2.3 ms of reconstruction cost exists with real
game input on this M2 Max at the tested 2× scale. It is not explained by flat test
data, input texture write usage, or output-sized input allocations. It also does
not prove that every difference between standalone and in-game measurements is
avoidable: in-game native command-buffer spans include dependencies and varied
substantially between samples. A runtime Metal System Trace was attempted but
timed out while saving; no dependency attribution is claimed from it.

## The motion-pass experiment

The [experimental patch](shared-motion-experiment.patch) shared Bevy's existing
motion attachment and skipped the additional allocation/copy/background pass.
The original and experimental paths were selected in the same release executable.
It removes about 25.6 MiB of texture storage at this surface, but it did **not**
demonstrate a frame-time or power improvement in the grass-off comparison above.
This experiment was not validated for grass rendering or its resource bindings.
It was reverted instead of retaining additional complexity as an alleged fix.

Disabling the normal GPU markers likewise did not remove the 50% regression.
The misleading timing panel is a real observability defect, but it is not the
whole explanation for the poor performance.

The current 50% setting has too little saving to pay for its reconstruction cost
in this scene. A significant win at that same setting remains unproven. More
aggressive scaling improved the short benchmark but failed the user's subsequent
gameplay/quality check, so it is not an accepted performance preset. The public
MetalFX API does not provide a free quality-preserving switch that these tests
have shown to remove the 2.3 ms cost.

## Evidence and cleanup

[followup-results.json](followup-results.json) records accepted measurements and
input/build hashes. [replay-results.txt](replay-results.txt) contains native probe
output. Raw logs, power samples, captured textures, the analysis helper, and
temporary capture source are in `tmp/metalfx-followup-20260920/` locally. The
incomplete Instruments trace was discarded.

To repeat the native probe after retaining that local input directory:

```sh
xcrun swiftc -O -module-cache-path /tmp/yarra-metalfx-probe-cache \
  docs/performance/20260920-metalfx-investigation/replay.swift \
  -o /tmp/yarra-metalfx-replay
/tmp/yarra-metalfx-replay
```

The follow-up used the initial report's game command with `--gpu-timing-off`
instead of `--timing-log`; Native uses `--upscaler linear --profile-size 3456x1942`,
50% Temporal uses `--upscaler metalfx-temporal --profile-size game`, and 33% uses
`--upscaler metalfx-temporal --profile-size 1152x648`. The experimental comparison
additionally used `--temporal-shared-motion` in its temporary build.

Both experimental rendering/capture changes were removed. Temporal source and
the original release executable were restored; the debug executable was rebuilt
from the restored source. The user's original uncommitted integration remains.
