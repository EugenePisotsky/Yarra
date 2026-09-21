# MSAA cost in the grass scene, 2026-09-21

The useful comparison for Temporal includes the MSAA cost it replaces. A matched
native-resolution pair isolates that cost before comparing the complete Temporal
frame. MSAA-off is only a diagnostic baseline, not a proposed gameplay setting.

M2 Max, release build, 3456×1942 output, the same static world and close-view camera,
full grass, bloom on, 120-FPS profile pacing, 15-second warmup and 10-second
measurement. Normal GPU probes are disabled; timings are Metal HUD medians.

| Complete rendering path | Input | GPU median | Mean GPU clock |
| --- | --- | ---: | ---: |
| Native, MSAA off | 3456×1942 | 4.92 ms | 1377 MHz |
| Native, 4× MSAA | 3456×1942 | 6.47 ms | 1390 MHz |
| Temporal, MSAA automatically off | 1728×971 | 7.64 ms | 1397 MHz |

Enabling 4× MSAA increased the native frame by approximately **1.55 ms (31.5%)**.
Temporal at 50% remained approximately **1.17 ms (18.1%) slower** than native with
4× MSAA. The Temporal native command-buffer span was 3.09 ms across ten samples,
all with reset=false. That span is not an additive pass cost.

All three runs were focused, completed without reported errors, and used the same
release executable. GPU clocks were close but not locked. Frame pacing averaged
approximately 120 updates/s in the native cases and 119 in Temporal. HUD packets
overlap; the figures are descriptive short-run results, not independent-frame
confidence intervals or sustained thermal/quality acceptance. They do not measure
iPhone performance. The profile configuration logs the selected MSAA preference
as four for Temporal; `apply_render_path` replaces it with `Msaa::Off` while Temporal
is active, as verified by the game tests and the earlier empty-scene diagnostic.

The grass pipeline sets the multisample count but does not request sample-frequency
fragment shading. Four samples do not mean four complete executions of grass
generation/deformation. Coverage, sample storage, depth processing and resolve
still cost work, especially with thin geometry. Apple's
[MSAA and tile-memory guidance](https://developer.apple.com/videos/play/wwdc2020/10632/)
explains why the render-pass structure also matters.

Source inspection found a concrete difference to investigate in the Temporal path:
ordinary grass joins the opaque phase, while Temporal removes it from that phase
and draws color, depth and motion in a separate pass after opaque terrain/objects.
This can force extra attachment traffic and shade terrain subsequently covered by
grass. The potential saving has not been measured, and it cannot explain the
reconstruction cost observed without grass. No ordering or quality change was
made during this comparison.

[Commands, input/executable hashes and results](msaa-grass/results.json), raw logs,
telemetry and the bounded runner are retained alongside this report. No production
code, launch defaults or saved scene settings were changed.
