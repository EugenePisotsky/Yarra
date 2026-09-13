# Static ground material experiment — 2026-09-13

Status: checkpointed editor-only experiment. No gameplay renderer, blade shader, wind,
vegetation catalog, density budget or world database changes in this experiment.

The user supplied IMG_1266 and IMG_1269–1273, and observed that the broad dark ground
patches stay fixed while grass moves. This supports trying a static under-grass
material; it does not establish how Yōtei implements it. Moving dark bands on
blades remain a separate problem. The earlier screen-space shadow studies are
not enabled here.

## Comparison

Actual editor captures: `.editor/vegetation/experiments/ground-study-01/final/comparison.html`.
28 captures: original / darken-only / detailed understory / coverage debug, across
seven views: edge without wind, edge at wind 0 and 1 seconds, dense top, dense
close, dense low, and quarter-density top. The close view is intended for ground
texture inspection, not a calibrated reconstruction of the reference camera.

All variants use the same published meadow terrain material and lighting. The
editor wraps `TerrainMaterial` in an `ExtendedMaterial`; its shader is assembled
from the production terrain WGSL with one base-color hook. Terrain preparation
events are mirrored into all four variants, so their prepared textures and
stochastic cache bindings remain identical. Only the selected four existing-size
ground planes draw. Other variants are hidden. Base terrain source is unmodified.

- **Original:** production ground with a unit multiplier.
- **Darken only:** coverage-dependent constant tint and mean scalar multiplier.
- **Understory:** same coverage and tint, with fine recess detail from the existing
  meadow normal/material source's AO channel. Scalar `0.06 + 0.62 * AO^8` is baked
  into an R8 texture before generating its mip chain. The 2 m tile shares the
  production normal/AO UVs. This retains average darkness at distance and avoids
  a nonlinear operation on already averaged AO.
- **Coverage:** unlit grayscale on the ground; white means full treatment, black
  means untouched. Grass stays visible over it.

The tint is `(0.82, 0.90, 0.72)`. The darken-only scalar is the detail texture's
mean, `0.397953`. It is an approximately matched control, not equal perceived
brightness: detailed darkening correlates with the source material's AO.

## Source coverage and stability

The CPU bake samples the existing reference candidate placement and retention
functions, including world ownership, surface validity, clumps, population
retention, field coverage and the stable occupancy hash. It bins **source retained
roots**, not emitted render units. The grass-study scene has one selected population;
multi-population pages are rejected rather than approximating competition.

Counts are smoothed over a nominal 0.22 m radius, converted to roots/m², then
smoothstepped to full treatment at 44 roots/m², matching the accepted specimen's
authored density. Roots beyond an exposed field boundary contribute zero. The
shader explicitly returns zero outside the field bounds, preventing clamp smears.
The mask is rebuilt only when the source scene revision changes. Camera, wind,
render LOD, ground choice and image zoom do not change it.

The 16 m bake uses a 256² R8 mask plus mips. Resolution targets 16 texels/m and is
capped at 1024². Larger fields therefore trade mask detail for a bounded allocation.
The CPU/GPU reference placement implementations can differ at floating-point
boundaries; this mask is a density proxy, not exact blade shadow geometry.

## Verified results

- Editor build and 17 vegetation-workspace tests pass; no shader errors in the
  28 native captures. Report JavaScript syntax and referenced files checked.
- All four material variants have identical emitted grass counts in every view.
  The full 16 m source contains 10,559 retained roots; edge source contains 5,033.
- Full-field coverage hash is identical at top, low and close cameras. Edge hash
  is identical with wind disabled and at wind phases 0 and 1 seconds. Each capture
  reports one coverage rebuild.
- In the edge wind pair, 605,890 pixels classified as ground visible in both
  captures stayed byte-identical. Ground classification uses the grayscale
  coverage render; it excludes green blade pixels. Uncovered ground pixels also
  stay identical between original and understory captures.
- Dense top-view ground's mean RGB code value changes from 67.05 to 42.39;
  darken-only gives 42.83. Sparse top changes from 67.04 to 64.21. These are image
  observations, not linear-light measurements or quality scores.

Initial visual assessment: removing bright ground helps more than the fine-detail
addition. The detailed version has visible small recesses up close, but this
existing mossy/litter texture does not reproduce the reference's broad dark soil
pockets. It does not solve unshadowed blade crossings. Review the darken-only
control before spending more on detail or adding another shadow technique.

## Cost and limits

- Zero additional grass vertices or visible ground draws; no shadow pass.
- Darken-only adds one filtered coverage sample to each shaded ground fragment.
  Understory adds two: coverage and detail. Terrain can shade pixels later covered
  by grass, so this is not limited to ground visible in the final image.
- Extra texture payload for the 16 m test: **1,485,482 bytes (1.42 MiB)** including
  mips. This excludes driver allocation alignment and the retained CPU copies.
- Source-mask bake observed at approximately **1.0–3.4 ms** in the optimized debug
  editor, only on source changes. This excludes loading/baking the initial fine
  detail image and GPU uploads; large-field CPU bake time has not been measured.
- **GPU milliseconds are not measured.** Sample count is an implementation cost,
  not a demonstrated frame-time improvement over previous shadow experiments.
- This changes base color under all lighting, including ambient light. It is an
  artistic ground material, not directional occlusion; broad patches intentionally
  do not follow sun angle or individual moving blades. Day/night and large-field
  mip/LOD behavior need evaluation before any gameplay integration.

## Reproduce and inspect

```sh
python3 tools/grass_ground_study.py --output .editor/vegetation/experiments/ground-study-next
```

The tool uses the accepted `content/vegetation/distance-01.ron` checkpoint, enables
wind in local copies, and quarters only the selected population for the sparse
case. It writes captures, replay RON, coverage images/hashes and a local HTML
comparison. It refuses to overwrite an existing experiment. `--no-build` reuses
the current editor binary.

For interactive review of this experiment:

```sh
python3 tools/vegetation_study.py open --no-build \
  --load .editor/vegetation/experiments/ground-study-01/final/dense-close-understory/study.ron \
  --ground understory --no-character --play
```

The ground menu switches between all four test modes. CLI equivalents are
`--ground original`, `darkened`, `understory`, and `coverage`. Captured study files
retain the selected mode. All generated captures and derived textures remain local.
