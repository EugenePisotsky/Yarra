# Near pairs and a cheaper far canopy — 2026-09-14

**Rejected experiment.** Game review found the single far ribbon too sparse and its curvature
unacceptable. The implementation below is archived in `after/`; it is no longer the current
renderer. See [the far-LOD restoration](GRASS_FAR_LOD_RESTORE.md) for the current shape and counts.
The timing results below apply only to these rejected variants.

The shared-base experiment reduced low-pair retention from 0.65 to 0.39 to fit five low triangles
into the former three-triangle budget. Game review exposed unacceptable loss of distant coverage.
The user accepted a simpler distant representation while retaining paired blades nearby.

## Rejected representation

- Near: the same 15-input/13-triangle shared-base pair, authored curves, clumps and wind.
- Far: one wide-root bent ribbon, five inputs and three triangles. It retains a shoulder instead
  of replacing the entire arch with one straight triangle.
- The companion collapses onto the common base edge through the existing morph. The main's
  width increases smoothly to 1.5 times its existing coverage width. This factor is an experimental
  coverage approximation, not an area-conservation guarantee.
- The high-detail radius and the Balanced projected-spacing curve are unchanged. Low retention
  returns to 0.65, giving 65%, 35.75% and 19.5% targets at the full/middle/far density anchors.
  Authored source density remains 44 roots/m² before field and clump filtering.
- Production low prepares one blade instead of two. Shape-inspection modes still prepare both,
  since their diagnostic draw uses the high template. The fallback shader uses the same shape.
- No new texture reads, draw bins or larger buffers. The provisional broad-leaf fixture shares
  these bins and also draws only its main leaf at low detail; dedicated broad-leaf LOD is separate work.

## Workload comparison

Same saved 64 m native-editor field, 1280×720, MSAA off, wind disabled, Medium blade marks:

| Counter | Rejected low pair | One far ribbon |
| --- | ---: | ---: |
| High roots | 5,305 | 5,305 |
| Low roots | 12,396 | 20,680 |
| Prepared blades | 35,402 | 31,290 |
| Vertex inputs | 166,347 | 182,975 |
| Submitted indices | 392,835 | 393,015 |
| Capacity drops | 0 | 0 |

The density restoration adds 66.8% more distant roots. Compared with the rejected version,
vertex inputs increase 10%, preparation records decrease 11.6%, and submitted triangles increase
0.05%. Compared with the earlier 18-high/7-low layout at the same retained density, vertex inputs
fall from 240,250 to 182,975 and indices from 408,930 to 393,015. Fragment cost still needs actual
measurement because the surviving far ribbon widens.

## Whole-game GPU timing

Four sequential native Metal HUD runs used archived binaries/shaders, the same runtime database,
`grass-close` camera, deterministic wind, 2,400 frames, 1920×1080 3D rendering and 4× MSAA.
The first five HUD packets were discarded. Order was old/new/new/old:

| Run | Mean GPU ms | Median GPU ms | P95 GPU ms |
| --- | ---: | ---: | ---: |
| Rejected pair 1 | 3.414 | 2.88 | 6.28 |
| Far ribbon 1 | 3.360 | 2.93 | 5.89 |
| Far ribbon 2 | 3.054 | 2.82 | 5.39 |
| Rejected pair 2 | 2.921 | 2.68 | 4.74 |

Mean-of-run-means is 3.168 ms before and 3.207 ms after, about 1.2% higher, with larger drift
between repeated runs. This does **not** establish a GPU speedup or a significant slowdown.
It supports roughly comparable whole-game cost with the previous distant population restored.
These are neither isolated vegetation timings nor results for the user's higher-resolution view.
The benchmark driver found no matching live Yarra review processes to suspend; metadata, raw HUD
packets, snapshots and per-run summaries remain alongside the study.

## Validation

The native prepared/fallback comparison passes with wind, MSAA and forced arena overflow.
The 99-case anchor test passes. Actual high/low raster comparison passes at low and overhead
cameras, with two density targets and blade marks Off/Medium; worst mean byte error is about
0.0024, with 13 channels differing by more than two. Existing tolerances were not relaxed.
The debug editor and release game were built. Changes are retained in the grass checkpoint.

Artifacts are under `.editor/vegetation/experiments/far-canopy-01/`. `before/` archives the rejected
version's executable and shaders; `after/` archives this version. The native field capture is
`field-64/viewport.png` with replay and diagnostic counters alongside it. The far shape still has
a different appearance from the near pair; passing the boundary test rules out a bin-switch
discontinuity, not a visible gradual change of representation.
