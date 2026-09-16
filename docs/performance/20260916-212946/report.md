# Grass profiling report

Session: complete.

Power and clock context take priority over small HUD-duration differences. Invalid runs remain visible.

| Run | Status | Window | World pixels | Surface pixels | App fps | GPU W | CPU W | GPU MHz | Active % | mJ/app frame | HUD GPU ms | HUD interval p95 ms |
| --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 01-density96-prepared120 | CHECK | fullscreen | 2592x1626 | 3456x2168 | 119.60 | 20.52 | 4.61 | 1303.97 | 88.48 | 210.12 | 5.06 | 8.33 |

App-closed idle estimate: GPU 0.06 W, CPU 0.73 W. Not automatically subtracted.

mJ/app frame uses system CPU+GPU estimates divided by application updates, not independently counted presentations. HUD samples overlap. Power timestamps have approximately one-second alignment precision. The table does not declare an optimization winner or establish cross-device equivalence.

## 01-density96-prepared120

Settings: `{"binary": "tmp/grass-draw-density-20260916/density96-prepared/inputs/game", "canopy": "tmp/grass-draw-density-20260916/density96-prepared/inputs/canopy.ron", "counters": false, "density": "balanced", "fps": 120, "grass": "full", "msaa": 4, "prepared_blades": 524288, "seconds": 900, "shaders": "tmp/grass-draw-density-20260916/density96-prepared/inputs/assets/shaders", "size": "game", "view": "low-walk", "warmup": 60, "window": "fullscreen", "world_db": "tmp/grass-draw-density-20260916/density96-prepared/inputs/runtime.sqlite"}`

Observed display: `{"monitor_hz": "120.000", "monitor_px": "3456x2234", "render_px": "2592x1626", "scale_factor": "2", "surface_px": "3456x2168", "window_logical": "1728x1084", "window_mode": "fullscreen"}`

Application thermal states: fair, nominal. Power sampler: heavy, moderate, nominal.
- Elevated thermal pressure: inspect timeline and repeated-run order
