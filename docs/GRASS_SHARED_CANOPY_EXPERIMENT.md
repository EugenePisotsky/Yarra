# Shared ground and lower-blade shading experiment

This tests the observation that lower blades merge into dark pockets while some ground remains
visible and brighter. It is an artistic approximation, not a reconstruction of Yōtei's shader
or a measurement of vegetation occlusion.

## Try it

```sh
target/release/yarra-app-editor --study-load content/vegetation/studies/canopy-gradient.ron --study-canopy
```

The current gradient setup is a 64 × 64 metre field, copied from the latest saved user study
(including its palette), with full reference density and geometry LOD enabled. The older
`shared-canopy.ron` file is retained as a historical preset and has an older palette.
Open **Canopy…** in the Vegetation workspace. **Enabled** gives an immediate on/off comparison;
**Combined**, **Ground only**, and **Blades only** isolate the contributions. **Colors…** is the
existing catalog palette editor. Wind can be played or paused through the study transport.

Controls:

- Maximum darkness: the attenuation ceiling in sheltered pockets at the far distance.
- Ground / blade amount: independent weights; zero disables that surface's contribution.
- Shade height: metres above each blade's local root terrain plane, rather than normalized blade
  length. Bent portions low in the canopy darken; upper portions keep their existing lighting.
- Height softness: how gradually shade disappears before the top of the envelope.
- Patch size, patch variation, openings: shape a stable world-space pattern with lighter pockets.
- Nearby shade fraction: the fraction of maximum shade before the fade starts; zero leaves
  nearby ground and blades unaffected by this effect.
- Fade starts / Maximum at: actual camera-to-surface distances, initially 3 m and 20 m.
  This is evaluated across the view, not once from the camera's zoom setting. In Study, use
  the actual camera distance; Linked zoom only magnifies the rendered image.
- Pocket expansion: distant shade grows outward from stationary pocket cores while retaining
  lighter openings. Zero keeps the pocket shape fixed and changes only its darkness.
- Grass edge fade: metres inward from the authored grass boundary, initially 1.25 m. The ground
  and lower blades share this fade, including boundaries between grassy and empty streamed pages.
- Keep all roots / Balanced: a separate density selector. Keep all roots retains geometry LOD
  and isolates canopy from density thinning. Balanced remains the normal game startup mode;
  use `--grass-density full` or O for the full-density comparison.

In the editor's **World** workspace, activate **Vegetation** with **Live preview** and open
**Tools → Canopy** (or **Canopy…** in the Vegetation window). The world uses the same canopy
controls as the study on both grass and terrain. The current canopy settings carry in both
directions when switching workspaces; camera, sunlight and wind remain workspace-specific.
The editor loads `content/vegetation/canopy-look.ron` at startup. Ground coverage comes from the
live vegetation draft and streamed neighboring fields, so empty terrain stays untreated. Coverage
is cached per tile and rebuilt asynchronously when source vegetation changes, not when sliders,
wind or camera distance change.

**Save study** retains the controls with the catalog, lighting, wind and camera. **Save canopy
look** writes `content/vegetation/canopy-look.ron`; the game loads it at startup, and **H** reloads
it without a restart. **G** still compares against the old catalog with this experiment disabled.

## Implementation and limits

`grass_canopy.wgsl` evaluates the same smooth two-scale world-space pattern for both materials.
The ground samples the existing source-root coverage bake so it only receives the effect under
vegetation, independent of rendered density/LOD. Blades sample the pattern at their shaded
position and fade the effect with physical height above the root plane. World-origin offsets
keep the pattern anchored when the game rebases. Wind does not animate the pattern, although
moving blades still cross its envelope and their normal lighting still changes naturally.

The effect attenuates shaded radiance on both surfaces, before tone mapping where the pipeline
provides it. It does not add shadow maps, another grass population, geometry, or more roots.
Material controls are excluded from placement and blade-preparation cache keys, and editor
slider changes do not rebake coverage.

The pattern is not derived from actual occluders or Voronoi clump membership. This deliberately
isolates whether a shared soft underlayer and lighter openings improve integration. It cannot
produce accurate directional cast shadows, and it will not repair a density or geometry problem.
A static distance-to-grass-boundary field is baked alongside source-root coverage. Terrain stores
coverage and boundary depth in RG8; grass reads the boundary field from an independent GPU buffer, baked by one background
job after streaming changes settle. It is no longer computed inside render-thread scene packing. The normal resident area uses a 25 cm grid. Slider changes and
camera motion do not rebuild it. This field follows source vegetation coverage, not the instantaneous
wind silhouette or individual gaps. Terrain still receives filtered source-root coverage in addition
to the shared boundary fade, so sparse individual roots remain an approximation.

The effect also fades out between 80 and 96 horizontal metres from the camera to avoid shading
bare terrain beyond the current procedural grass range. This is a representation boundary, separate
from the artistic near-to-far gradient. It will need to follow any future change to that render range. Large settings can visibly paint patches or swallow whole
short blades. The controls are meant to expose that tradeoff rather than hide it in constants.

Studies with no canopy block still deserialize with the effect disabled. Older canopy blocks load
with defaults for the new controls; their obsolete `distance_boost` field is ignored. The current
preset uses 0.85 maximum darkness, a 0.24 m height envelope, 1.85 m patches, no nearby shade,
0.65 pocket expansion, and a 1.25 m boundary fade. Previous look and study files are backed up in
`.editor/vegetation/experiments/canopy-gradient-01/previous-*.ron`.

## Validation

Release game and editor builds passed. Eight focused shader, placement/preparation cache, coverage,
and study-persistence checks passed (the existing native candidate-cache GPU test remains ignored).
The native game rendered the new material without shader errors. Editor on/off captures used the
same camera, catalog, 4× MSAA and fixed wind; both emitted 20,550 units / 410,250 indices with zero
capacity drops. This comparison checks identical geometry, not performance.

Captures and exact replay documents are in `.editor/vegetation/experiments/shared-canopy-01/`.
The on/off views show the shading-only difference. No timing benchmark was run. The Canopy panel
was inspected in the native editor, and the shared-look study was tested through RON save/load.

Also fixed the study capture path to create its output folder before writing ground diagnostics,
and corrected the editor's hard-coded MSAA label to show the actual sample count.

World integration: release editor/game builds passed, along with the workspace-switch regression
and three shared coverage checks (empty ground, translated pages, matching tile borders). Native
World on/off and World → Study → World checks passed with the saved canopy values. No performance
benchmark was run.


## Distance-gradient validation (2026-09-15)

Release game/editor builds passed. The shared WGSL parser validates all blade-band variants.
Coverage tests check clear ground, translated tiles, matching RG tile borders, and matching scene/
tile boundary depths without a seam at an internal page border. Study serialization checks preserve
the new controls and full-density selection; old studies remain readable. Scene packing and blade
preparation cache checks also pass.

Native full-field captures at 4.7, 10 and 24 metres, plus a clear-grass-edge view and a canopy-off
comparison, are in `.editor/vegetation/experiments/canopy-gradient-01/`. The far on/off views use
identical geometry, palette and fixed wind. These are visual checks; no timing benchmark was run.

The native game rendered the new material without shader errors. World and Study both expose the
distance and density controls; World → Study retained the canopy settings. The editor is left in
the full-field study with Keep all roots selected. Visually, maximum-strength pockets are still
pronounced in a distant World view; their intensity and growth remain art-direction choices.


## Streaming stall correction (2026-09-15)

The first gradient implementation rebuilt the whole resident boundary raster in `pack_scene`,
synchronously on the render thread and regardless of canopy enablement. Lowering O only changed
emitted grass density; G disabled appearance but did not skip this raster. A representative 49-page
224 × 224 m source produced 876,096 boundary cells and 3,504,416 bytes. One release diagnostic on
this machine measured the old bake at 39.40 ms; source packing without it took 0.38 ms (the fixture
has one populated field per page). These are isolated CPU operations, not before/after frame times.

The boundary now has a separate buffer and one asynchronous job, with 150 ms coalescing of source
changes. Completed results from an obsolete revision are discarded; an origin change clears the
old field. Same-origin streaming retains the previous field until the replacement is ready. A
job already executing may finish after disabling, but its result is discarded and no new job starts.
Canopy buffer updates do not invalidate placement or candidate caches. G off / canopy disabled also
stops game ground-mask scheduling and uploads. Scene signatures now include only terrain joined
to vegetation, plus the coordinate frame; unrelated terrain LOD changes cannot invalidate grass.

Release game/editor builds and focused regression checks passed. Native movement checks used
`grass-stream` with authored density, canopy enabled and G baseline disabled. CPU samples show
boundary and ground-mask bakes under Async Compute Task Pool when enabled and no canopy bake
calls in the disabled capture. These checks verify the repaired blocking path, not the absence of
all possible streaming hitches. Logs and samples are `/tmp/yarra-canopy-stall-fixed-{on,off}.*`.
The saved palette and canopy look were not changed by this correction. Visual quality is unresolved;
background scheduling does not establish this shading model as the right artistic solution.
