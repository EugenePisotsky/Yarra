# Trackpad event delivery audit

The user still reports uneven trackpad rotation at 60 FPS. No camera behavior was
changed in this audit. Synthetic tests cannot establish real trackpad smoothness.

## Source inspection

- winit 0.30.13 `macos/view.rs::scroll_wheel` forwards each AppKit scroll event,
  including momentum, converting precise logical deltas to physical pixels.
- Bevy 0.19.1 `bevy_winit/state.rs` buffers window events even when
  `react_to_window_events` is false. That option controls waking the app; the buffer
  is drained immediately before `app.update()`. MouseWheel messages are not cleared
  on intermediate display waits. winit defers reentrant events instead of dropping them.
- The camera reads the entire MouseWheel batch, including start/end phase events.
  The HUD routes input before `GameInputSystems`. Camera controls and the camera
  transform update are chained in the same Update schedule.
- Intentional rejection: gameplay locked, pointer over/captured by the HUD, or
  the first 0.5 seconds after startup. Input is consumed during rejection so it
  cannot replay later. Each precise event is still clamped to ±80 physical pixels.
- Orbit smoothing has a 1/20 s = 50 ms time constant. It spreads each frame's
  displacement across that frame, without individual native timestamps. Equal
  event totals do not guarantee equal perceived motion for uneven packet arrival.
- Bevy's automatic Time comes from a render-world channel (`bevy_time::time_system`,
  `bevy_render::send_time`), while native input is delivered on the main thread.
  Those intervals may differ. This is a candidate to measure, not a confirmed cause.

## One focused capture

```sh
cargo run --release -p yarra-app-game -- --fps 60 --upscaler metalfx-temporal --trace-camera-input
```

Rotate with the trackpad as usual, keeping the pointer outside the Performance
panel. The first nonzero scroll starts a 20-second capture. It saves automatically
and prints its directory, `tmp/camera-input-<unix-seconds>-<pid>/`. Closing the game
normally also saves a shorter capture. Forced termination cannot save it.

The AppKit monitor observes only scroll events addressed to this game's primary
window. It returns the original event unchanged. Nothing is injected. Buffers are
bounded; all file writes happen after collection. The native monitor is removed
after capture. Without the flag, the monitor and diagnostics resource are absent.
This uses Apple's [local event monitor](https://developer.apple.com/documentation/appkit/nsevent/addlocalmonitorforevents%28matching%3Ahandler%3A%29),
which observes events before normal dispatch; it does not expose raw hardware
samples or events consumed by nested AppKit tracking loops.

- `native.csv`: source timestamps, dispatch times, Retina-adjusted deltas, gesture
  and momentum phases. `dispatch_delay_ms` compares AppKit event time with system uptime.
- `frames.csv`: native batch boundary, actual events read by the camera, raw deltas,
  clamp count, rejection flags, requested/applied orbit, target and smoothed yaw.
  It also records actual input-read intervals beside Bevy's smoothing interval.
- `oldest/newest_dispatch_to_read_ms` measure main-thread delivery to camera input
  consumption using the same monotonic clock. Negative values flag an event that
  arrived after that camera read; compare the next batch before calling it lost.
- `oldest/newest_event_age_ms` are measured at the PostUpdate trace sample, not
  presentation. No field measures end-to-end display latency.

Compare cumulative event counts and displacement (pixel and line separately)
before declaring a loss; a one-frame shift can be batching rather than loss.
`native_end` is an exclusive CSV event index. Ignore events beyond the final frame
boundary and duplicate camera sequence IDs if the camera system did not run.
`clipped` identifies intentional magnitude reduction; `requested_orbit` is already
clamped and `applied_orbit` is zero when routing rejects scroll input. Momentum
and phase-only zero events count as events, even when they do not move the camera.

## Verification

Engine tests cover identical 120 Hz input batched at 60/120 FPS, uneven batches
with idle frames and start/end phases, per-event clipping, and consumption during
UI capture or gameplay lock. They verify input accounting, not macOS delivery.
Those tests alone cannot verify actual trackpad delivery or justify a smoothing change.

Validation on September 21: `cargo check -p yarra-app-game --offline`, the release
game build, and all four engine `input_tests` passed. A three-second native smoke
run at a small surface installed the AppKit monitor, exited normally, removed it,
and saved both CSV headers without errors. It contained no gestures, so it verifies
monitor lifecycle and file output only, not event delivery or subjective smoothness.

## First real trackpad capture

Capture: `tmp/camera-input-1789991480-33519/` (`native.csv`, `frames.csv`,
and computed `analysis.json`), September 21. No camera behavior was changed after
reading this capture.

- 1,202 consecutive camera updates spanning 20.016 seconds. All 1,315 AppKit
  events were consumed. **Every individual frame's count and X/Y displacement
  matched its native batch**, not just the final totals. No duplicate camera
  sequence IDs, UI blocking, disabled gameplay, or startup rejection occurred.
- Native event timestamp → camera read: median 6.52 ms, p95 15.85 ms, maximum
  19.64 ms. These exclude smoothing and presentation. No large input-queue stall
  appears in the captured interval.
- Five momentum events exceeded the ±80-pixel clamp. The clamp removed 34 of
  13,518 absolute horizontal pixels (0.25%). This is real magnitude reduction,
  but too localized to explain persistent unevenness by itself.
- Camera-read intervals averaged 16.666 ms, ranging from 13.734 to 19.518 ms.
  Bevy's smoothing intervals ranged from 14.253 to 18.401 ms. Their absolute
  difference was median 0.375 ms, p95 2.629 ms, maximum 3.836 ms.

One illustrative momentum segment contains alternating empty and double batches:

| Camera update | Interval between camera reads | Smoothing interval | Scroll events | Horizontal pixels |
| --- | ---: | ---: | ---: | ---: |
| 1101 | 13.73 ms | 17.28 ms | 0 | 0 |
| 1102 | 19.38 ms | 16.21 ms | 2 | -4 |
| 1103 | 14.02 ms | 16.42 ms | 0 | 0 |
| 1104 | 19.51 ms | 16.91 ms | 2 | -4 |
| 1105 | 13.86 ms | 17.26 ms | 0 | 0 |
| 1106 | 19.51 ms | 16.30 ms | 2 | -4 |

Native timestamps and callback times also show uneven delivery: event 671 was
dispatched 13.72 ms after its source timestamp, followed by event 672 at 2.43 ms.
Both were consumed in update 1102. Empty batches here are not lost events.

macOS momentum supplied 9,820 of 13,518 absolute horizontal pixels (72.64%).
Fourteen momentum episodes lasted 0.506–0.961 seconds (median 0.841 seconds),
including two vertical-only episodes. The camera applies its existing 50 ms
exponential smoothing time constant on top of those native momentum deltas.
During the fastest flick its yaw trailed the target by up to 39 degrees. This
is a consequence of the configured smoothing at high rotation speed, not evidence
of a delayed or overwritten transform. Recorded yaw matches the implemented
smoother, with no discontinuity or competing camera update detected.

Interpretation: game-side event loss is not supported by this run. Timestamp-aware
camera sampling and smoothing are the next focused area to investigate. The trace
does **not** establish which timing policy would feel better, nor that the OS
momentum is unwanted. In particular, irregular CPU update intervals do not prove
irregular displayed frame intervals; no presentation timestamps were captured.
