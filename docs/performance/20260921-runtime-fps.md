# Runtime FPS selection — 2026-09-21

The Performance panel now changes the frame cap without restarting the game.
The common button cycles Follow display / 30 / 60 / 120, with `--fps` retained as
the initial value. Reset and A/B restore include the cap. Choices are session-only.

## Transition policy

`FramePacing` holds the requested cap; a main-thread controller applies it after
UI input in PostUpdate. The render thread receives the applied policy so the
event loop and Metal presentation use the same rate. An FPS-only change does not
change audit scene settings or invalidate Temporal history.

On macOS 14+, the controller creates one window display link lazily. Rate changes
update that link's preferred range. Follow display pauses it, gates its watchdog,
restores the original focused/unfocused event-loop settings, and clears Metal's
minimum presentation duration. Returning to a cap reuses the link. Ticks still
coalesce rather than accumulating catch-up work.

Timer fallback and diagnostic timer mode leave the Metal limiter disabled; the
native display mode retains the presentation policy validated in the
[60 FPS investigation](20260921-frame-pacing/README.md).

Recent FPS averages clear after a cap change. GPU/CPU samples get a new settings
revision. A/B captures save and restore the cap, and reject an external rate change
during recording. Scripted profiling locks the panel's FPS control.

## Verification

All 40 release game unit tests pass. Relevant coverage includes repeated runtime
changes, restoration of the original event loop, native/timer policy selection,
paused watchdog behavior, custom launch values, FPS-only scene isolation, Reset,
and A/B capture/restore.

The bounded native probe uses the production pacing module in a minimal VSync
window on the M2 Max's 120 Hz ProMotion display:

```sh
cargo build --release -p yarra-app-game --bin yarra-app-game --example frame_pacing_switch --offline
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 RUST_LOG=warn \
  target/release/examples/frame_pacing_switch
```

It switches Follow display → 60 → 30 → 120 → Follow display → 60 in one process,
allowing two seconds to settle and sampling three seconds per phase. Assertions
check continued updates and capped rates; Metal HUD logs verify presentation
independently of application update timing. This is a transition test, not a
rendering-cost or thermal benchmark.

The probe completed all six phases and exited successfully. Presentation samples
from the HUD were:

| Selected cap | Median interval | p95 interval | Mean presentation FPS |
| --- | ---: | ---: | ---: |
| Follow display, initial | 8.33 ms | 8.34 ms | 118.02 |
| 60 | 16.67 ms | 16.67 ms | 59.80 |
| 30 | 33.33 ms | 33.34 ms | 29.80 |
| 120 | 8.33 ms | 8.34 ms | 115.68 |
| Follow display, resumed | 8.33 ms | 8.34 ms | 115.30 |
| 60, resumed | 16.67 ms | 25.00 ms | 58.16 |

The expected presentation cadence changes with each cap. There are occasional
missed frames, especially in the last phase; this short check does not establish
a perfect frame lock. The maximum observed application interval across the
post-startup phases was 69.30 ms, with no transition hangs. All phases stayed
focused. Consecutive identical HUD packets and the first packet in each measured
window were excluded; packets overlap, so samples are correlated.

The actual release game was also checked interactively with `--fps 60
--performance-open --upscaler metalfx-temporal`. The button changed 60 → 120 →
Follow display → 30, stayed visible on the Quality tab, and Reset restored 60.
The displayed backend remained MetalFX Temporal. The game log confirmed each
requested cap and corresponding Metal interval, with a clean exit and no errors.
Local raw evidence is in `tmp/runtime-fps-20260921/` (`transitions.log`,
`transition-summary.json`, and `game-ui.log`).
