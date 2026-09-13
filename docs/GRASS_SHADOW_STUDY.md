# Grass shadow fixture — 2026-09-13

This experiment establishes the blade-shadow signal before choosing a production representation.
It does not implement a terrain impostor or add screen-space shadows to gameplay. Everything native
lives under `#[cfg(test)]` in `vegetation_render::renderer::shadow_study`; the normal renderer only
gains that test-module declaration. No production shader, density, topology budget, render target,
wind setting, catalog database or scene is changed. The fixture is checkpointed for further review.

## Reproduce and inspect

Use a Python environment with NumPy and Pillow, plus a native Metal/Vulkan/DX12 GPU:

```sh
python3 tools/grass_shadow_study.py \
  --load content/vegetation/distance-01.ron \
  --method pixel --steps 16 --strength 0.45 \
  --output .editor/vegetation/experiments/shadow-study-new
```

`--quick` captures one patch view/light/pose for shader checks. Full capture produces seven camera
views, two sun directions and four frozen wind phases (0, 1/60, 0.5, 1 seconds). Capture requires a
new output directory. `--report-only --output <existing directory>` regenerates and validates the
report without GPU access. Open `comparison.html` in the output folder. Both panes have mode
selectors, synchronized zoom/pan, camera/sun/pose controls and discrete captured-pose playback.

The first run is `.editor/vegetation/experiments/shadow-study-01/physical/comparison.html`.
The current combined comparison is
`.editor/vegetation/experiments/shadow-study-02/pixel-16/comparison.html`.
Generated captures remain local and ignored by Git. A copied source study, hashes, parameters,
lossless compressed readbacks, PNGs and per-frame measurements accompany the report.

## Controlled fixture and rendering

- 83 visible blades / 584 triangles: an isolated main blade, two crossing main blades and 40 pairs.
  The patch uses the current three short-grass species in a 65/25/10 distribution. Root coordinates
  are explicit diagnostic placements, not a test of production Voronoi/clump placement.
- Species come from the saved study and use the renderer's actual species packing. The native
  harness assembles the current `vegetation_blade.wgsl` and `vegetation_debug_draw.wgsl` functions.
  It calls `geometry_vertex` for both triangle export and rasterization, using the real index
  template. There is no independently implemented curve, wind or blade mesh.
- Full shape, current physical widths and wind. View opening is disabled through the existing
  full-shape inspection controls. Opening would otherwise make the caster change with the camera.
  All views are inside the wind's distance-independent near range.
- A neutral ground plane reveals coverage gaps. Single-sample 1280 × 720 rasterization. A fixed
  100,000-lux white sun, the source exposure and blade material functions are used. Output is linear
  shading encoded to sRGB without the game tonemapper; these are diagnostic images, not gameplay
  screenshots. No world objects/CSM or terrain texture are present. Standard isotropic GGX helpers
  mirror Bevy 0.19; the fixture replaces only its view/shadow bindings.
- Baseline, exact reference and approximation share one raster pass with multiple color outputs.
  Their coverage and interpolated material data therefore match. Grass-shadow visibility changes
  direct lighting only; it does not multiply the existing root AO or ambient/body contribution.

The exact reference intersects a ray toward the sun against the same wind-deformed, two-sided
triangles used for drawing. It records visibility over both 2 metres and 35 cm. Rays start 1.5 mm
along the light direction to avoid numerical self-intersection. This is a hard-shadow reference
within its stated range, with no penumbra, filtering or temporal accumulation. It is intentionally
an offline tiny-fixture reference, not a proposal to trace every production blade.

The first approximation marched 16 midpoint samples along 35 cm toward the sun and compared their view
depths with the captured nearest surface. The initial assumed thickness is 18 mm. The capture stores
world positions for convenient depth reconstruction; the approximation has no access to hidden
triangles, receiver identities or reference visibility. A runtime implementation would need actual
grass-inclusive depth and world-position reconstruction. It is not Bevy's contact-shadow algorithm.

## Validation

`cargo test --offline -p yarra-vegetation-render` passes 21 tests; eight native tests are ignored by
default. The new native capture test passed for all 56 cases. Report validation checks:

- Finite readbacks, binary reference visibility, valid approximation confidence and matching
  captured/final receiver identities.
- Exported geometry remains identical across cameras and sun directions for each wind phase.
- Wind actually changes geometry across phases.
- Independent CPU rays agree with sampled GPU reference pixels (up to one boundary pixel per 256).
- The crossing fixture's lower blade receives a shadow specifically from the other blade's eight
  triangles, rather than merely counting self-shadowing as success.

## First findings

The triangle reference produces partial blade bands and spatially corresponding ground shadows.
The isolated blade remains exposed in the inspected high-sun view. A simple single-depth-layer
approximation recovers some bands but breaks ground shadows into dashes and loses substantial
inter-blade detail. At low sun it also falsely shadows exposed surfaces.

For the initial frozen patch view:

| Sun | Missed / actually shadowed grass pixels, equal 35 cm range | False shadows / all visible grass pixels |
| --- | ---: | ---: |
| High | 64.96% | 1.50% |
| Low | 52.16% | 8.06% |

The vertical top view at low sun falsely darkens 16.21% of visible grass pixels. These denominators
are deliberately different: missed shadows are relative to the reference shadow pixels; false
shadows are relative to all grass pixels. The report's inline percentages use all visible pixels
for both, while raw counts in `comparison.json` allow either calculation. The 2 m reference is
separate, so a short ray is not blamed for shadows beyond its supported range.

**Do not promote this first approximation to gameplay.** This result rejects these simple sampling
and thickness settings as a finished effect; it does not establish that every screen-space approach
will fail. Before integration, distinguish undersampling, false thickness hits, hidden blockers and
out-of-view blockers using these same saved surfaces. A useful next comparison is traversal in
screen pixels and better intersection rejection under a declared sample budget. Avoid concealing
missing information with generic animated noise or increasing the geometry budget.

No runtime timing conclusion follows from these offline captures. Any candidate that survives
visual review still needs a complete added-cost measurement: grass depth participation, ray pass,
read/write bandwidth, compositing, resolution and motion stability. Broad ground occlusion remains
a separate unsolved requirement.

## Second experiment: contiguous pixels under a fixed read budget

Three matched runs compare regular world-space samples (16 total depth reads) with perspective-correct
screen-pixel traversal at 8 and 16 total reads. All three now use the same maximum **45% reduction of
direct lighting**, including the exact reference shown in color. Reference visibility masks remain
binary. The new point-sampled control therefore differs from the first run: 15 ray samples plus the
receiver read, and restrained shadow strength rather than complete direct-light removal.

The pixel traversal follows the approach described by McGuire and Mara in
[Efficient GPU Screen-Space Ray Tracing](https://jcgt.org/published/0003/04/04/), independently implemented
in `shadow_study/pixel_trace.wgsl`. It:

- Advances one pixel along the projected ray's major axis, interpolating reciprocal clip W and
  ray distance/W for perspective-correct depth intervals.
- Includes the initial receiver-depth read in the budget: **7 or 15 subsequent ray reads**. When
  these cannot cover 35 cm, the supported world-space reach is shortened instead of skipping pixels.
- Intersects the complete depth interval with a 4 mm slab behind the recorded surface; fades
  confidence with penetration, over the last 40% of the supported reach, and at the image boundary.
- Estimates the local receiver plane from derivatives of captured depth-reconstructed positions.
  It rejects near-coplanar samples within 1.5 mm when that estimate is reliable. At depth
  discontinuities the plane rejection is disabled. No true mesh normal, receiver ID, triangle list
  or reference visibility is available to the approximation.

The read cap counts shader texture accesses per receiving pixel; it is not a measurement of memory
transactions, cache efficiency or execution time. Quad derivatives do not require extra explicit
texture reads, but thin-edge estimates retain the limitations a screen pass would face.

The report records actual reach, total reads, the reference within that reach, and a separate exact
35 cm reference. This prevents a shorter ray from appearing successful merely because its comparison
reference also lost most shadows. False-shadow pixels exceed 5% raw occlusion confidence. Recovered
35 cm shadow is the sum of raw confidence on genuinely shadowed pixels divided by the number of those
reference pixels; it is a weighted fraction, not binary recall, and is measured before the shared
45% strength adjustment.

Initial frozen pose, visible grass pixels:

| View / sun | Method | False shadows / all grass | Recovered 35 cm shadow | Mean supported reach |
| --- | --- | ---: | ---: | ---: |
| Patch / high | Point 16 | 1.29% | 29.13% | 35.00 cm |
| Patch / high | Pixel 8 | 0.29% | 5.71% | 2.45 cm |
| Patch / high | Pixel 16 | 0.55% | 24.10% | 4.96 cm |
| Patch / low | Point 16 | 7.82% | 46.22% | 35.00 cm |
| Patch / low | Pixel 8 | 2.48% | 4.10% | 1.87 cm |
| Patch / low | Pixel 16 | 2.65% | 8.62% | 3.62 cm |
| Top / low | Point 16 | 15.59% | 58.30% | 35.00 cm |
| Top / low | Pixel 8 | 5.02% | 14.18% | 4.61 cm |
| Top / low | Pixel 16 | 5.90% | 25.87% | 9.48 cm |

**Decision:** Pixel 16 is a candidate for subtle, local blade-contact shading, not a replacement for
ground shadows. Pixel 8 retains too little detail in these views. Cleaner ground in Pixel 16 largely
means long ground shadows disappeared; it must not be reported as fixing their continuity. Isolated
blade edges still falsely darken: the isolated low-sun camera has 0.86% false grass pixels for Pixel
16 versus 0.14% for Point 16. Neither candidate is ready for gameplay.

All **168 native captures** (56 per method) pass reference checks, geometry invariance, matching
baseline/reference across methods, and the recorded read limits. Pixel 16's false-shadow fraction
across the four captured wind poses is 0.50–0.58% for patch/high, 2.45–2.72% for patch/low, and
5.10–5.90% for top/low. These sparse frozen poses do not establish temporal stability during play.
The crate suite passes 21 tests with eight native tests ignored by default. Report JavaScript parses
and all generated image paths resolve; automated browser interaction was not validated.

To reproduce the combined comparison (each capture output must be new):

```sh
python3 tools/grass_shadow_study.py --method point --steps 16 --strength 0.45 \
  --output .editor/vegetation/experiments/shadow-study-next/point-16
python3 tools/grass_shadow_study.py --method pixel --steps 8 --strength 0.45 \
  --output .editor/vegetation/experiments/shadow-study-next/pixel-8
python3 tools/grass_shadow_study.py --method pixel --steps 16 --strength 0.45 \
  --output .editor/vegetation/experiments/shadow-study-next/pixel-16 \
  --compare .editor/vegetation/experiments/shadow-study-next/point-16 \
  --compare .editor/vegetation/experiments/shadow-study-next/pixel-8
```

`--thickness` overrides the assumed slab thickness in metres. `--report-only` can also accept
`--compare` to rebuild a combined viewer from saved captures. The default sides compare Point 16
against Pixel 16; mode selectors include Pixel 8, exact/reference masks, actual reach, and unchanged
root AO. Geometry, lighting and strength must match between compared runs.

No production vertex budget or draw has changed, and no runtime performance claim follows from this
test. Integrating a contact pass would first require grass-inclusive depth without another grass
geometry draw, appropriate direct-light compositing, and measured GPU cost at the game's resolution.
The separate broad ground-occlusion experiment should derive coverage from the actual root/clump and
height data, with visible gaps retained, before combining it with any fine contact layer.
