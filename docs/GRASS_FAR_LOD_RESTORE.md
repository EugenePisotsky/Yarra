# Restore distant grass coverage and curvature — 2026-09-14

Subsequent tuning: [smaller LOD width compensation](GRASS_LOD_WIDTH_TUNING.md).
The topology and population described here remain; the later study adjusts their shading and width.

Game review rejected both the reduced-density five-triangle low pair and its replacement single
far ribbon. Matching a triangle budget or passing a morph boundary test did not make either
representation visually acceptable.

The renderer now retains the shared-wide-base near pair and restores the earlier low topology:

| Detail | Vertex inputs per root | Submitted triangles per root |
| --- | ---: | ---: |
| Near shared-base pair | 15 | 13 |
| Far bent main + companion triangle | 7 | 3 |

The far main uses its cubic midpoint as a shoulder, a width vector aligned to the curve there,
and its actual drooping tip. The previous experiment instead raised that tip and used the root's
width direction at the shoulder, changing the silhouette. The companion again contributes a
full triangle. Near roots stay wide; the geometry morph collapses them into the old far topology.
Preparation evaluates both blades in both split bins. No buffer size, texture read or draw-bin
increase is introduced.

Low retention is restored from the rejected 0.39 to 0.65. Authored source density remains
44 roots/m² before field and clump filtering, and the Balanced density curve and near-detail
radii are unchanged. The common pair facing, curve seed and upper-point spread from the near
experiment remain, so this is not a bitwise restoration of every earlier blade shape.

## Fixed-field workload

The saved 64 m editor study uses 1280×720, MSAA off, wind disabled and Medium blade marks.

| Counter | Earlier 18/7 topology at 0.65 | Restored far shape with 15/7 near/far |
| --- | ---: | ---: |
| High roots | 5,305 | 5,305 |
| Low roots | 20,680 | 20,680 |
| Vertex inputs | 240,250 | 224,335 |
| Submitted indices | 408,930 | 393,015 |
| Capacity drops | 0 | 0 |

This is 6.6% fewer topology inputs and 3.9% fewer indices than the earlier 18/7 layout, at the
same retained root counts. These are workload counts, not measured GPU-time savings. The
rejected single-ribbon benchmarks in `GRASS_FAR_CANOPY_STUDY.md` do not measure this restoration.

## Matched game timing

Four subsequent sequential runs compared the rejected single far ribbon with this restoration
in single/restored/restored/single order. Each ran 2,400 frames with the same runtime database,
`grass-close` camera, deterministic wind, 1920×1080 3D rendering and 4× MSAA. The first five Metal
HUD packets were discarded. The interactive game previously launched by this task was already
closed; no process needed pausing. Binaries, shaders and database hashes accompany the results.

| Run | Mean whole-game GPU ms | Median ms | P95 ms |
| --- | ---: | ---: | ---: |
| Single 1 | 4.201 | 4.21 | 5.39 |
| Restored 1 | 4.222 | 4.18 | 5.40 |
| Restored 2 | 4.257 | 4.22 | 5.46 |
| Single 2 | 4.291 | 4.26 | 5.47 |

The mean of the two run means is 4.246 ms for the rejected single and 4.239 ms for the restored
version: -0.007 ms (-0.15%), smaller than repeat-run variation. No meaningful whole-game GPU
increase or speedup was measured in this scene. This does not isolate the grass pass, compare
against the older dense implementation's GPU time, or verify native-resolution/iPhone cost.

Relative to the rejected single's fixed-field counters, restoration adds 22.6% topology inputs
and 66.1% prepared blade records while keeping submitted triangles identical. Relative to the
earlier dense 18/7 topology, the counts remain lower as shown above. Geometry counts alone do
not predict whole-game timing. Artifacts: `restoration-timing/` within the experiment folder.

## Verification

- Renderer unit suite: 23 passed; eight native tests ignored in the default run.
- Native cached/fallback equivalence passes with wind, MSAA and forced arena overflow.
- Native root/endpoint checks pass all 99 cases.
- Native high-to-low raster boundary comparison passes at low and overhead cameras, two density
  targets and blade marks Off/Medium. Worst mean byte error is about 0.000954; at most ten channels
  differ by more than two. Existing tolerances were not relaxed.
- Debug editor and release game built successfully.
- Native `grass-close` game capture visually checked against the rejected version: distant
  coverage is fuller and the bent silhouette is restored. The 1,200-frame smoke run completed
  without rendering errors. Its whole-game HUD mean was 4.07 ms at 1920×1080, 4× MSAA; one run
  is insufficient to establish the performance delta.

Artifacts are under `.editor/vegetation/experiments/far-canopy-01/`. The current editor capture
and replay are `restored-field-64/viewport.png` and `restored-field-64/study.ron`; the native game
capture is `restored-game/game.png`. Other captures in this folder include rejected experiments
and must not be presented as the current result. Changes are retained in the grass checkpoint.
