# Grass profiling report

Session: complete.

Power and clock context take priority over small HUD-duration differences. Invalid runs remain visible.

| Run | Status | Window | World pixels | Surface pixels | App fps | Worst app window fps | GPU W | CPU W | GPU MHz | Active % | mJ/app frame | HUD GPU ms | HUD interval p95 ms |
| --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 01-density96-default120 | CHECK | fullscreen | 2592x1626 | 3456x2168 | 119.56 | 73.00 | 19.68 | 4.38 | 1346.32 | 96.77 | 201.28 | 6.64 | 8.33 |

App-closed idle estimate: GPU 0.02 W, CPU 0.04 W. Not automatically subtracted.

mJ/app frame uses system CPU+GPU estimates divided by application updates, not independently counted presentations. Worst app window uses sampling windows of at least 0.9 seconds (normally about one second), not a 1% low or presentation counter. HUD samples overlap. Power timestamps have approximately one-second alignment precision. The table does not declare an optimization winner or establish cross-device equivalence.

## 01-density96-default120

Settings: `{"binary": "tmp/grass-draw-density-20260916/density96-prepared/inputs/game", "canopy": "tmp/grass-draw-density-20260916/density96-prepared/inputs/canopy.ron", "counters": false, "density": "balanced", "fps": 120, "grass": "full", "msaa": 4, "prepared_blades": 131072, "seconds": 900, "shaders": "tmp/grass-draw-density-20260916/density96-prepared/inputs/assets/shaders", "size": "game", "view": "low-walk", "warmup": 60, "window": "fullscreen", "world_db": "tmp/grass-draw-density-20260916/density96-prepared/inputs/runtime.sqlite"}`

Observed display: `{"monitor_hz": "120.000", "monitor_px": "3456x2234", "render_px": "2592x1626", "scale_factor": "2", "surface_px": "3456x2168", "window_logical": "1728x1084", "window_mode": "fullscreen"}`

Application thermal states: fair, nominal. Power sampler: heavy, moderate, nominal.

Worst app window: 73.00 fps over 1.000 s, ending 831.80 s into measurement. Windows below 95% of target: 14 (14.07 s of sampled time). Late updates: 399 / 107602.
- Brief/periodic cadence loss: 14 full app sampling window(s) below 95% of target; inspect the timeline even if average FPS passes
- Elevated thermal pressure: inspect timeline and repeated-run order
