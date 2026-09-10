# Vegetation study workspace

Status: comparison foundation implemented, 2026-09-10. The editor now has a real Vegetation
workspace; the earlier discussion mockup remains separate from its rendered output.

## Using the implemented workspace

Choose **Vegetation** in the editor workspace menu. It previews the selected population from the
same validated catalog draft used by World. The inspector retains the existing Save/Publish
flow. Switching workspaces preserves the draft, camera, pending saves, and prior World wind,
lighting and rendering settings.

The stage offers flat **4, 16, 64, and 128 m square fields**, with the production grass shaders,
automatic geometry LOD, and a fixed **1280 × 720 / MSAA off** render target. Window/panel resizing
does not change its projection or grass LOD. The 4 m patch retains its 65,536 candidate limit;
larger fields allow 4,194,304 source candidates, divided into stable 16 m pages. Exceeding the limit suspends the
preview with an error rather than silently reducing density. GPU counts are sampled asynchronously;
capacity drops and high/low topology counts are shown separately. Larger fields keep density unchanged
and extend farther along -Z for LOD inspection. Editor FPS is not a grass GPU timing.

- The reference and live output occupy **equal-sized canvases**. New studies use 16:9 to match the
  supplied references. Older 1280 × 960 studies replay at their original size; references with a
  different aspect are center-cropped to the canvas without stretching.
- **References…** opens a floating picker. Selecting an image restores its approximate camera,
  character placement, field size and ground (or a saved override), shows the character, resets
  linked zoom, and closes the picker. All six supplied references have individual setups. Low views
  use the legs and feet to match scale; the LOD view uses the full body. Clothing, the hat and camera
  intrinsics differ, so these remain manual approximations rather than exact recovered calibration.
  **Save setup for this reference** stores refinements in local `reference-setups.ron`.
  Older camera-only `reference-cameras.ron` overrides are still read.
- **Inspector…** opens the existing catalog controls in a movable, resizable floating window.
  Both windows start closed; their close buttons or **Hide windows** expose the whole comparison.
- **Grass edge** clears the foreground using the production root-coverage field. It starts enabled
  for `edge_1` and `edge_2`, with a diagonal, slightly irregular boundary beside the character.
  Complete blades can extend over the boundary, so their silhouettes stay intact. Interior density,
  root identities and catalog settings stay fixed. **Inspector → Grass boundary** adjusts its
  position, angle and transition width. Save setup/study retains these values. Existing captures
  and saved setups without an edge retain the full field.
- Scroll either canvas or use **Linked zoom** to magnify both around the same normalized point.
  Drag the reference or Shift-drag either canvas to pan both. **Fit both** restores full framing.
  This is image inspection zoom: the underlying camera, render resolution and LOD remain stable.
- Drag Yarra to orbit; right-drag to move its camera target. Distance/FOV controls and Low,
  Overhead, Top and Scale presets adjust the actual camera. Scale frames the whole character.
  These inspection presets leave the selected reference fixed; **Match reference setup** restores
  its setup. Camera eye height is displayed. **Inspector → Camera and scale character** exposes
  character feet X/Z, facing, camera pitch and target. Camera, character placement, field size,
  ground mode, seed and linked zoom/pan are included in study metadata.
- Wind starts paused at an exact phase. Enable/disable, play/pause, reset, step 1/60 s, scrub the
  seconds field, or change playback speed. The study owns the wind clock and suppresses the
  renderer's single-letter diagnostic shortcuts while active.
- **Character** defaults on for new studies and reference selections. It is the existing game model
  at its imported scale, paused in idle. **2 m ruler** is a separate optional toggle, marked every
  25 cm, beside the character. This does not claim the character itself is 2 m tall. Character meshes
  are isolated from the World and Animation cameras. Old studies retain their saved visibility.
- **Uncut grass / Dried grass / Neutral** switches the ground. Textured modes use the published
  default world's cell-0 terrain surfaces and actual terrain material, texture arrays, mip chains,
  normals and authored metre-scale tiling. They extend across a 512 m ground plane beneath the
  bounded vegetation field. Missing terrain resources are reported; textures must load before
  capture. Replay requires the same published terrain catalog and assets. Neutral remains useful
  for seeing gaps without a grass texture underneath.
- The six named images in `content/references` are imported automatically. Additional PNG/JPEG
  images can be dropped into the editor or imported by local path. Exact source files are copied
  under `.editor/vegetation/references/originals`, so temporary screenshot paths may disappear
  safely after import. The selected reference displays at full source resolution (up to the GPU's
  texture-size limit), including during linked zoom; only one full-resolution reference texture
  is kept on the GPU. Picker thumbnails
  are at most 1600 px per side and live under `.editor/vegetation/references`.
  Existing thumbnail-only imports still work; reimporting their source restores full detail without
  changing the reference ID or saved setups. The six supplied images upgrade automatically.
  Imports reject files above 32 MiB and dimensions above 8192 px, with a decoder allocation limit.
- **Save study** writes `.editor/vegetation/study.ron`; this is local reproduction data, not a
  catalog save or publication. **Capture** writes `viewport.png`, `editor.png`, `study.ron`, and
  `diagnostics.txt` to a new local capture directory, with wind paused for the capture.

For quick agent or developer inspection, from the repository root:

```sh
python3 tools/vegetation_study.py open
python3 tools/vegetation_study.py capture --camera overhead --time 0
python3 tools/vegetation_study.py capture --camera low --character --time 2.5
python3 tools/vegetation_study.py open --camera scale
python3 tools/vegetation_study.py open --select-reference edge_2
python3 tools/vegetation_study.py capture --select-reference top_down_close --zoom 2
python3 tools/vegetation_study.py open --inspector --picker
python3 tools/vegetation_study.py open --select-reference bottom_straight
python3 tools/vegetation_study.py capture --select-reference lod --field 128 --ground meadow
python3 tools/vegetation_study.py capture --field 4 --ground neutral --no-character
python3 tools/vegetation_study.py open --ruler
python3 tools/vegetation_study.py open --select-reference edge_1
python3 tools/vegetation_study.py capture --select-reference edge_1 --no-edge
python3 tools/vegetation_study.py capture --select-reference edge_2 --edge
python3 tools/vegetation_study.py inspect
python3 tools/vegetation_study.py capture --load .editor/vegetation/study.ron
```

The helper builds the debug editor by default; add `--no-build` for repeated runs. `--reference PATH`
can be repeated; its last import replaces the selected reference even with `--load`, preserving the
loaded camera and stage unless another option overrides them. `--select-reference` takes precedence
over imported selection and restores that reference's setup. `--output DIR` selects a fresh capture
directory. Native GPU/window access is
required on macOS. A capture waits for warm-up and matching GPU telemetry, waits for both actual
screenshot callbacks, rejects error/panic messages in the native log, reports failures, and exits. It has an internal 60 s deadline and an external
90 s helper timeout. The helper terminates only the process it started.

Replay imports the captured catalog **as an unsaved draft**, preserving the project's existing save
baseline. The stage seed override stays separate from the authored population seed. Metadata stores
the camera, wind field/time, lighting, renderer controls, full catalog, reference identifier, scale
figure visibility, shared image zoom/pan, and build description. `viewport.png` is the complete clean
render; `editor.png` shows the displayed comparison crop. Exact pixels require the same renderer/assets/device.
New captures use study version 2. Version 1 remains readable with its original 4 m stage and coupled
character/ruler visibility. Old studies without comparison framing load at 1× centered; absent stage fields restore the original
neutral ground and fixed character placement. Replays require the Full rendering workload. GPU timings
remain unavailable, and tiny-patch captures do not establish full-world performance.

Still planned: Pair/Clump stages, baseline pinning, independent reference crop authoring, forced geometry
LOD controls, and the actual shape/wind experiments. The rest of this
document describes that broader direction. The foundation does not alter production blade shape,
wind deformation, or default density.

Validation for the foundation: 54 editor tests and 19 renderer tests passed (four unrelated optional
GPU tests remain ignored). Native Metal captures checked overhead, low and full-character views.
The low-view capture and its RON replay had identical metadata and differed at 57 of 1,228,800
pixels, with a maximum channel difference of 6/255. This supports reproducible visual comparison,
not bit-exact GPU output. Repeated workspace ownership/restoration is covered by an automated test.

Comparison-layout revision: 59 editor tests pass, including shared zoom/pan bounds, equal canvas
sizes, distinct reference cameras, saved-camera precedence, selection resetting the view without
changing wind, and old-study framing compatibility. Native captures checked the equal comparison,
floating controls at 2× zoom, replayed zoom, and preservation of the old 1280 × 960 render size.

Scale/field revision: 61 editor tests pass, including projected body landmarks, stable common pages
between the 64/128 m fields, unchanged authored density, bounded field work, complete reference setup
precedence, and legacy stage deserialization. Native Metal captures checked all six reference setups
with the character and textured ground. The 128 m LOD capture emitted 8,407 high and 142,594 low units
with zero capacity drops; this is a workload sample, not a GPU timing. Its version-2 replay retained
identical study metadata. The old 1280 × 960 scale study also replayed successfully.

Grass-edge revision: 62 editor tests pass. Coverage tests verify clear ground at the character and
foreground, full coverage inside the field, a partial-coverage transition, unchanged catalog and
candidate domains, and rejection of invalid boundary values. Boundary edits invalidate the scene
signature, and reproduction metadata round-trips them. Native captures checked both edge references
and replayed the first with identical study metadata and no render errors.

Full-resolution import revision: all 62 editor and 19 renderer tests pass (four optional native GPU
tests remain ignored). Native Metal captures checked a 3840 × 2160 JPEG at 3× linked zoom and a
temporary 2880 × 1800 PNG import. The PNG study replayed with identical metadata after deleting its
temporary source. All six supplied JPEG originals were verified byte-for-byte. Egui receives the
actual GPU texture-size limit before its pass, replacing its conservative 2048 px default.

## Purpose

Make grass quality experiments reproducible inside Yarra Editor. Compare user-supplied reference
images with a bounded patch rendered by the actual game vegetation pipeline. Establish convincing
paired silhouettes and constrained wind before changing field density or production LOD.

The user wants the reference's recognizable blade curvature and facing to survive wind: blades
may sway vertically and sideways, but their change in shape and direction must remain limited.

## Workflow boundary

Add `Vegetation` alongside `World` and `Animation`. World continues to own spatial field placement.
Vegetation owns species/population studies, its camera, reference board, and playback controls.
Both use the same catalog draft and existing save/conflict/publication boundary. A World population
can be opened in the study workspace without losing its unsaved edits or the World camera state.

A study contains editor-only context, not a new runtime species format:

- local reference images, selected crops, and an associated camera bookmark per reference;
- selected species/population, test-stage settings, stable seed, and catalog snapshot identifiers;
- camera target, orientation, distance, vertical FOV, and viewport aspect/render dimensions;
- light direction/color, ambient settings, exposure, ground material, and AA settings;
- wind enabled/paused state, exact time, playback speed, and selected motion components;
- independent geometry LOD and population-density policies;
- a captured baseline with its reproduction metadata and the current experimental draft.

Reference camera and lighting are approximate matches. Screenshots alone do not recover exact
camera intrinsics, world scale, exposure, or the reference renderer's implementation. Store manual
matches as bookmarks; never present an approximate match as calibrated evidence.

Keep reference assets and study metadata local to editor studies and outside runtime publication.
An explicit local import should preserve originals, create bounded display thumbnails, and avoid
depending on temporary screenshot paths. Asset paths, ownership, and size limits must be specified
before implementing persistence. Do not embed user reference media into cooked game assets.

## Main view

Use equal reference/live canvases with optional floating controls.

- Left: the active reference image.
- Right: an equally sized live Yarra viewport using the production shaders and material path.
- Floating windows: reference picker and existing population/species controls, hidden by default.
- Above the live viewport: stage and camera bookmark selection; a visible indication when the
  camera has departed from a bookmark.
- Below: wind transport and baseline/capture controls. Diagnostics are expandable.

Use two initial reference views: close paired blades and overhead canopy. Add low, overhead, and
top-down inspection bookmarks. The reference image stays fixed during free orbit; returning to its
bookmark restores the approximate matched view. Image pan/zoom never silently changes the 3D camera.

The user can collapse the reference or inspector for a larger live view. Captures use an explicit,
stable render size and aspect so UI layout changes cannot silently change projection, LOD, or cost.

## Three bounded stages

1. Pair: isolate a deterministic real render unit. Inspect its actual silhouette, rest curve,
   topology, width distribution, and wind. Use the same shader path, not an independently drawn
   editor approximation. An overlay may show the authored curve separately.
2. Clump: inspect correlated pairs and the effect of root attraction, relative facing, and motion.
3. Patch: a small fixed-area field (start with a 4 m square) with a sparse edge and visible ground.
   Include production ground-material mode and a neutral inspection mode, clearly distinguished.

Bound actual candidate and emitted work independently of the authoring density slider. Display
requested and emitted populations and any capacity drops; a clipped preview is not accepted as the
requested field. Start with one population and permit a catalog assemblage once the basic study
works. A Pair view must not rely on a seed search that accidentally emits unrelated roots.

## Experiment controls

The study selects an experiment and exposes the relevant controls. Preserve existing authoring
fields; do not duplicate a second independently saved species editor.

### Shape

- Freeze wind, seed, camera, lighting, and density.
- Compare the existing pair construction with shared-curve, laterally separated blades.
- Compare width profiles at approximately equal leaf area before increasing overall width.
- Inspect Pair and Patch at high geometry with explicit population thinning disabled.

### Wind

- Off, pause/resume, exact time scrub/step, and repeat the same interval.
- Independently isolate coherent motion, bend change, bob, and flutter.
- Compare existing additive deformation with bounded root-pivot motion; introduce small upper
  bend changes only after whole-curve motion has been judged.
- Keep paired blades related and connect species response limits to actual deformation.
- The study transport owns wind time while active; wall-clock wind advancement cannot overwrite it.
- A time scrub evaluates motion deterministically and leaves placement identities unchanged.
- Still references establish shapes only. A user-supplied video can later support reference-motion
  comparison; do not infer exact motion from the two static images.

### LOD

- Separate high/low/automatic geometry from full/authored/production population thinning.
- Keep stable roots and compare representation changes before changing population.
- A forced high-geometry diagnostic remains subject to the bounded stage budget and is labeled.
- Never silently reduce the high-detail radius when an experiment claims it is locked.

### Material and ground

- Switch between neutral silhouette/coverage inspection and the production material path.
- Keep sunlight, exposure, ground treatment, and camera stable within a comparison.
- Root darkening, ordinary object shadow reception, and grass casting are distinct observations.
  This workspace does not revive either rejected grass-shadow implementation.

## Baseline and evidence

Pin a baseline image plus its full study/catalog settings. Compare that image beside the current
live view or switch the single live viewport between A and B with the same camera, seed, and wind
time. Do not render two simultaneous live grass scenes for the initial comparison mode.

Camera, lighting, wind time, or projection changes make an old capture visibly unmatched. Re-capture
the baseline or replay both states under the new conditions. Do not allow a stale baseline to imply
an equivalent comparison. For wind, replay the same interval for each state; video-pair export is
later work, not needed for the first screenshot comparison.

Capture should export the clean Yarra viewport, active reference/crop identifier, and machine-readable
reproduction metadata. Include renderer/build identity when available. Later scripted runs should
consume the same study format and render the same scene, rather than duplicate its construction.

Expose only useful diagnostics: actual render dimensions/AA, emitted roots/blades, topology counts,
capacity drops, and measured GPU spans when available. Unavailable timings are unavailable, not zero.
Editor FPS and tiny-patch timings do not establish full-world performance. Capture and profiling
modes must distinguish screenshot/reference-panel overhead from the grass work being measured.

## Existing integration points and hazards

- `workspaces/mod.rs`: typed workspace states, persistent workspace state, frame-pacing ownership.
- `shell.rs`: explicit camera activation for all workspaces. Its current Animation activation is
  `!world_active`; adding a third workspace without changing this would activate the wrong camera.
- `vegetation_authoring.rs`: validated shared catalog draft, existing controls, source save and
  publication. Its World live-preview system must not overwrite an active study scene.
- `VegetationDebugScene`: can supply bounded in-memory page fields to the existing renderer.
- `VegetationDebugView`: current automatic attachment covers every 3D camera; constrain ownership
  so exactly the intended active preview camera contributes vegetation work.
- `vegetation_render::renderer`: one global scene/buffer set and view selection through
  `views.iter().next()` require deliberate active-view ownership. Initial single-live-view design
  avoids assuming that independent simultaneous study scenes already work.
- `VegetationWind`: study playback must have explicit ownership and restore prior World state.
- World streaming, input, lights, render layers, source demand and camera state must be isolated
  while the study is active, then restored without losing dirty edits or pending save completion.
- Existing game render-audit capture code provides a pattern for clean capture and fixed poses.

## Implementation slices

1. Comparison foundation: Vegetation workspace, shared draft, bounded Patch, one production viewport,
   two imported references, camera bookmarks, wind freeze, and save/restore workspace lifecycle.
   Accept by visual inspection of references and actual grass, including repeated workspace switches.
2. Reproducible studies: local persistence, precise wind transport, baseline/capture metadata, explicit
   geometry/population controls, Pair/Clump stages, and compact actual-work diagnostics.
3. First experiments: pair construction and width-profile variants, then constrained wind variants.
   Compare at matched settings before deciding which changes should become authoring defaults.

Visual validation must check the actual editor on Mac. Focused tests should cover ownership,
deterministic replay, save/draft preservation, and resource/work bounds. Existing shader-string
checks alone cannot establish rendering quality. Do not change production grass behavior as an
unlabeled side effect of creating the workspace.
