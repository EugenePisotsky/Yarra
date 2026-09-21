# 60 FPS presentation on ProMotion — 2026-09-21

The old cap paced application updates but let Metal present each completed frame
as soon as possible. On this ProMotion display, steady average 60 FPS could still
alternate between 8.33 ms and 25 ms presentations. Display-synchronized updates
alone did not eliminate that pattern. Adding Metal's minimum presentation
duration did.

Run normal gameplay with:

```sh
cargo run --release -p yarra-app-game -- --fps 60
```

The macOS display can remain on ProMotion. No system display settings were changed.
The default launch without `--fps` retains its existing VSync behavior.

## Matched scheduler comparison

M2 Max, macOS 26.4.1, release build, windowed 3456×1942 physical output, 50% scene
resolution, Linear, 4× MSAA, full grass and bloom. Each visit has 15 seconds of
warmup and 10 seconds of measurement. The final two visits use the same executable,
database and camera. All completed successfully and remained focused.

| Scheduler / presentation | HUD samples near 16.67 ms | Interval p95 | Mean presentation rate |
| --- | ---: | ---: | ---: |
| Original timer baseline | 81.42% | 25.00 ms | 60.10 FPS |
| Display callback, ordinary presentation | 84.30% | 25.00 ms | 59.99 FPS |
| Display callback + Metal minimum duration | **99.65%** | **16.67 ms** | **59.89 FPS** |
| Timer control in the same patched build, minimum duration disabled | 41.69% | 25.00 ms | 60.05 FPS |

The timer's variable results are part of the problem: its phase relative to the
display can differ between launches. An average FPS counter hides the uneven
delivery. The native presentation interval, rather than a change to scene
complexity or rendering quality, accounts for the improvement in this comparison.

## Normal launch path

A further **60-second measured run**, after 15 seconds of warmup, used `--fps 60`
and `--profile-native-pacing` so profiling did not replace the normal scheduler.
It requested the default **Auto upscaler**, 50% scene resolution,
4× MSAA, full grass, bloom, and the same physical output. This is an additional
validation, not a rendering-cost comparison with the Linear visits.

- 3,600 application updates in 60.013 seconds; focused throughout; clean exit.
- **99.86%** of HUD interval samples near 16.67 ms; **16.67 ms p95**.
- 59.97 FPS from the mean HUD presentation interval.
- 7,130 samples near 16.67 ms, 8 longer samples, and 2 shorter samples.

"Near" means within 1 ms of 1000/60. HUD packets overlap; these are correlated
samples, not independent frame counts. Consecutive identical packets and the
first packet crossing the measurement boundary are discarded. This verifies
the reproduced pacing problem under these conditions; it does not establish
long-session thermal behavior, iPhone performance, or deadlines for heavier scenes.
The reused runner also collected macmon telemetry, preserved here, but power and
GPU-duration comparisons are not used to evaluate this fix.

## Implementation

- `frame_pacing.rs` uses an NSView display link on macOS 14+, asks for the chosen
  frame rate, and wakes Bevy through its event-loop proxy. Input events are
  processed on the next update instead of bypassing the cap.
- Pending ticks coalesce rather than accumulating catch-up frames. A 250 ms
  watchdog keeps lifecycle handling alive when a hidden view stops callbacks;
  it does not request frames while display callbacks continue. The link is
  invalidated and the watchdog stopped when the app exits.
- A small vendored **wgpu-hal 29.0.4** patch exposes a per-queue minimum presentation
  duration. The game sets it to 1/FPS and retains VSync. Metal schedules presentation
  relative to the previous frame's actual presentation time.
- `--frame-pacing-timer` retains the original limiter for comparison.
  `--frame-pacing-display-only` disables only the Metal interval. `--fps 0` disables
  this cap. A nonzero `--profile-fps` shares the new path unless native pacing was
  requested. Other platforms retain the timer fallback.

See the [small upstream patch](../../../third_party/wgpu-hal/yarra-metal-presentation.patch)
and [maintenance notes](../../../third_party/wgpu-hal/YARRA_PATCH.md). The queue's
default duration is zero, preserving upstream behavior unless explicitly enabled.

Apple references: [view display callbacks](https://developer.apple.com/documentation/appkit/nsview/displaylink(target:selector:)),
[minimum presentation duration](https://developer.apple.com/documentation/metal/mtlcommandbuffer/present(_:afterminimumduration:)),
and [variable-refresh presentation guidance](https://developer.apple.com/videos/play/wwdc2021/10147/).

Raw `.log`, `.json` and `.power.jsonl` files accompany this report. Exact commands
are in each case JSON; `manifest.json` records input and measured executable hashes.
`pacing-summary.json` contains the interval statistics.

Validation: `cargo test --release -p yarra-app-game --bin yarra-app-game --offline`
passed all **34 tests**, including the new concurrent tick-coalescing regression
test. The release game also completed every runtime visit above without errors.
