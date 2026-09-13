# Folded grass pair experiment — 2026-09-13

**Historical experiment.** Its far density reduction was rejected during game review.
The shared near strip remains, but the low representation is superseded by
[the restored bent main blade and companion triangle](GRASS_FAR_LOD_RESTORE.md), with the previous
density restored. The intervening single far ribbon also failed visual review.

The preceding pair spent vertices on two independently oriented ribbons with pointed roots.
This experiment reconstructs the shared-base short-grass diagram on slide 27 of
`content/gdc_2021_procedural_grass_in_got-2.pdf`. It is an interpretation of that diagram, not
access to Ghost of Tsushima's implementation or confirmation of Ghost of Yotei's topology.

## Geometry and curvature

Both blades reference the same two indexed base vertices. They share facing, base width and
curve variation; their upper control points spread apart. The base remains planted through wind
and view opening. The companion retains its shorter, gentler curve. Ribbon width follows
`1 - t²`, giving a wide root and tapered tip.

| Pair | Before | Experiment |
| --- | --- | --- |
| High | 18 inputs, 14 submitted triangles | 15 inputs, 13 submitted triangles |
| Low | 7 inputs, 3 submitted triangles | 7 inputs, 5 submitted triangles |
| Main sampling | Five high sections | Four high sections; two low sections |
| Companion sampling | Four high sections | Three high sections; blunt low quad |

High main rows sample `t = [0, .215, .5, .70, 1]`; companion rows use
`[0, .25, .61, 1]`, then apply the existing authored longitudinal exponent. These are our
sample locations, not values from the talk. The low main retains a shoulder; the companion
contracts its pointed tail into a blunt edge. High geometry morphs onto that exact low shape.

Reducing the number of sections does not intrinsically improve curve approximation. The purpose
is to recover the source's connected shape and sensible low representation, then tune curve
distribution within that topology. Strong curves and distant highlights still expose faceting.

Clumps, candidate seeds, authored population density, near-detail radii, the existing bounded
wind motion and blade-shadow marks remain connected to the production renderer. There is no new
texture read, geometry arena or enlarged prepared-blade record.

## Cost and tradeoff

The production low-pair retention multiplier changes from 0.65 to 0.39:
`3 × .65 = 5 × .39`. Existing stable-rank fading and width compensation use the same multiplier.
Consequently fewer distant pairs survive; their coverage compensation is stronger. This is a
visible tradeoff, not free extra curvature. Full-density diagnostic mode bypasses the multiplier.

For the saved 64 m field, 1280×720, MSAA off, frozen wind, Medium blade marks:

| Counter | Earlier 18/7 prototype | Shared-base 15/7 |
| --- | ---: | ---: |
| High pairs | 5,305 | 5,305 |
| Low pairs | 20,680 | 12,396 |
| Topology vertex inputs | 240,250 | 166,347 |
| Submitted indices | 408,930 | 392,835 |
| Capacity drops | 0 | 0 |

Vertex inputs fall 30.8% and submitted triangles fall 3.9% in this fixture. The earlier prototype
used the preceding topology budgets and placement, but its image was already a base experiment;
it must not be presented as the original pointed-root visual baseline. These are workload counts,
not measured GPU-time savings. Wider roots and coverage compensation can change fragment cost.

The initial broad-leaf fixture shares the split bins. It inherits their shared index layout,
reduced sample count and lower distant retention, while retaining its own leaf taper and motion.
It has no ribbon coverage compensation, so its distant coverage can decrease. A dedicated
broad-leaf topology remains outside this grass experiment.

## Validation and review

- Renderer suite: 23 passed, eight native tests ignored in the default run.
- Editor vegetation suite: 23 passed.
- Native prepared/fallback equivalence: passed with wind, MSAA and arena overflow.
- Native LOD anchors: 99 cases passed, including different wind phases and common-base checks.
- Native raster boundary comparison: passed for low and overhead cameras, two density targets,
  and blade marks Off/Medium. Worst mean byte error was approximately .0131; existing tolerances
  were not relaxed.
- Debug editor and release game built successfully.

Replays and full-resolution captures are under
`.editor/vegetation/experiments/folded-pair-01/`:

- `shared-sparse`: wide joined bases, quarter density, wind off.
- `shared-low-rest`: dense low angle, wind off.
- `shared-low-wind`: same camera, wind on at time 1.5.
- `shared-field-64`: game-like camera and scale character, frozen wind off.

The original visual baselines are `../joined-base-01/before-sparse` and `before-low`.
Open a moving study with:

```sh
python3 tools/vegetation_study.py open --no-build \
  --load .editor/vegetation/experiments/folded-pair-01/shared-low-wind/study.ron \
  --play --inspector
```

The experiment is checkpointed. Next evaluation should judge base shape, curve faceting and
distant coverage visually before profiling a promising result or changing density again.
