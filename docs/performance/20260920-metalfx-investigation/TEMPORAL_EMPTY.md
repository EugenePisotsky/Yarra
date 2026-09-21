# What Temporal does with an empty scene

Temporal reconstruction remains expensive when the scene contains no objects.
A direct native MetalFX run reproduces a substantial cost without Bevy, wgpu,
scene rendering, tone mapping, UI or presentation. This narrows the problem to
reconstruction and its configuration, while leaving possible integration overhead
unquantified. It does not establish that the current configuration is optimal.

## One controlled question: reconstruction versus its surrounding path

Release empty-frame example, M2 Max, macOS 26.4.1, 1728×971 input, 3456×1942
output, normal VSync at approximately 120 FPS. Each case warms up for 8 seconds
and measures for 8 seconds. No meshes, world, grass, sky, clouds, lights, shadows,
bloom or audit timing probes are installed. The example imports the game's actual
presentation code. Both engine cases log actual MSAA = 1 sample.

| Case | Measured GPU duration | Mean GPU clock |
| --- | ---: | ---: |
| Temporal path, reconstruction replaced by HDR linear enlargement | HUD median 2.30 ms | 444 MHz |
| Temporal path, normal MetalFX reconstruction | HUD median 4.72 ms | 1100 MHz |
| Native MetalFX alone, matching empty inputs/settings | Command-buffer median 3.96 ms | 532 MHz |

The normal engine case also sampled the MetalFX command buffer itself: median
3.61 ms across eight samples, all with history reset false. Status confirms actual
MetalFX Temporal, with no fallback. Both windowed cases stayed focused, completed
at approximately 120 FPS and reported no errors. The native-only case measured
961 reconstruction calls at approximately 120 calls/s; p95 was 5.22 ms.

**These rows are not an additive pass breakdown.** GPU clocks differ, whole-frame
HUD and native command-buffer scopes differ, and native command-buffer spans can
include dependencies. Do not subtract the rows or linearly normalize their clocks
to estimate integration overhead. HUD packets overlap. These short Mac runs do not
establish sustained thermal behavior or iPhone performance.

## Work that remains without meshes

The current engine path still:

1. Maintains jitter and allocates/clears color, depth and motion attachments.
2. Runs the depth/motion prepass and the complete-scene motion initialization pass.
   With no geometry, the latter still reprojects background pixels.
3. Calls MetalFX once per frame with HDR color, reversed depth, motion and jitter.
   MetalFX maintains previous-frame history; automatic exposure is enabled.
4. Produces the full 3456×1942 HDR result, then tone-maps to the native-size LDR
   scene image and composites it into the window.

The low-resolution main pass uses a subrectangle of native-size attachments.
The shader work for motion initialization uses the input-sized viewport. The
spatial upscale path excludes Temporal views, and MSAA is disabled. The scaler
is retained between frames; synchronous initialization is requested, and the
sampled steady frames do not reset history.

Temporal uses current color/depth/motion and previous-frame data to reconstruct
the output. Its benefit depends on saved scene-rendering work exceeding the added
reconstruction work. Empty geometry removes nearly all expensive shading that
could pay for reconstruction, while the 6,711,552-pixel output remains. This
explains why an empty scene can be slower with Temporal; it does not excuse an
avoidable integration cost or prove a production scene will benefit.
See [Apple's explanation and integration guidance](https://developer.apple.com/videos/play/wwdc2022/10103/).

## Native-only contract and reproduction

The [native probe](temporal-empty/native-empty.swift) uses the same color/depth/motion
formats, full-size allocations with input content 1728×971, 2× scale range, automatic
exposure, reversed depth, pre-exposure and Halton jitter sequence as our backend.
Its constant color matches the example's clear color converted to linear HDR;
depth and motion are zero. Inputs are initialized once. Only reconstruction runs
inside the measured command buffer, paced to 120 calls/s. CPU completion waits
are deliberate for isolation; production does not use these waits. This is a
synthetic reconstruction test, not a captured frame or a visual-quality test.

```sh
cargo build --release -p yarra-app-game --example empty_frame --offline
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 target/release/examples/empty_frame --scale 0.5 --upscaler metalfx-temporal --temporal-bypass --metalfx-timing-log
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 target/release/examples/empty_frame --scale 0.5 --upscaler metalfx-temporal --metalfx-timing-log
```

Accepted runs use the Retina-enabled wrapper recorded in
[results and hashes](temporal-empty/results.json); verify focused=true and the
physical output dimensions. Raw logs, telemetry and bounded runners are alongside
that file. Only the optional diagnostic example gained a bypass flag and status
logging. Production Temporal behavior and startup settings are unchanged.
