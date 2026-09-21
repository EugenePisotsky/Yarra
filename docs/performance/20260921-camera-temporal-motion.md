# Trackpad rotation and Temporal motion checks

Tested on M2 Max / macOS 26.4.1, September 21, 2026.

## Trackpad correction

Camera orbit previously clamped the accumulated scroll displacement once per rendered
frame. At 60 Hz, an update can contain twice as many native trackpad events as at 120 Hz,
so a sufficiently fast identical gesture lost more distance at 60 Hz. Clamp and convert
each event before accumulating it instead. Captured/disabled input still consumes events.

The existing exponential orbit smoother remains at rate 20. It now integrates gesture
movement across the elapsed interval, instead of treating each frame's whole event batch
as an instantaneous target change at the start of that interval. A regression test feeds
the same 120 Hz event stream into 60 Hz and 120 Hz updates, at ordinary and clipping-level
speeds, and checks both target orbit angle and smoothed camera angle. Input capture/unlock
coverage also passes. Subjective trackpad smoothness still needs an in-game check.

## Temporal sampling

`temporal_motion_sampling_probe` renders a known analytic grayscale pattern at 128×128,
inside 256×256 allocations, and reconstructs it to 256×256 through the production MetalFX
backend for 32 frames. It supplies exact unjittered motion and Bevy's sampling convention.
The last output is compared with the analytic full-resolution image, excluding borders.

| Pattern movement | Production jitter, RMSE | Reversed jitter, RMSE |
| --- | ---: | ---: |
| Static | 0.006701 | 0.038347 |
| Horizontal, 1.25 input pixels/frame | 0.010194 | 0.039701 |
| Vertical, 1.25 input pixels/frame | 0.009264 | 0.037130 |

The existing negative native jitter offsets are correct for this pipeline. No backend
sign, motion scale, sharpening or history settings were changed. The test now asserts
low reconstruction error and that production's convention beats the reversed sign.

## Actual scene motion

`temporal_strafe_motion_matches_reprojection` uses the real Bevy mesh prepass and custom
grass renderer. Wind is disabled to isolate camera movement. It reads final depth and
RG16Float motion, reprojects each visible sample into the previous camera, and compares
that displacement with the recorded motion in input pixels. Temporal history must remain
valid during these small translations. This test uses the motion debug mode; it checks
the inputs, not the reconstructed grass image.

| Geometry / camera translation | Mean motion (px) | 95th-percentile motion error (px) |
| --- | ---: | ---: |
| Mesh / X +0.1 m | 1.747751 | 0.000908 |
| Mesh / Z +0.1 m | 0.695527 | 0.000436 |
| Grass / X +0.1 m | 1.300308 | 0.001107 |
| Grass / Z +0.1 m | 0.867189 | 0.000925 |

These checks rule out a basic missing-motion, direction, scale or jitter-sign error in
the exercised paths. They do not establish the cause of the user's in-game strafing blur
or prove that fine foliage retains its detail during reconstruction. That visual issue
remains unresolved; do not present these results as a blur fix or a performance gain.

Both native tests take seconds and render small offscreen targets. They are image/data
correctness checks, not FPS or power measurements.

```sh
cargo test --release -p yarra-engine input_tests
cargo test --release -p yarra-upscaling --features metalfx temporal_motion_sampling_probe -- --ignored --nocapture
cargo test --release -p yarra-vegetation-render --features upscaling/metalfx temporal_strafe_motion_matches_reprojection -- --ignored --nocapture
```
