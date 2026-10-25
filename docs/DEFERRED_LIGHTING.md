# Deferred lighting: design and plan

The plan for lighting each final pixel once instead of inside every material's shader. It is
deleted when the work is done: what was decided moves to [Decisions](DECISIONS.md), how it works to
[Architecture](ARCHITECTURE.md).

## Why

Measured on October 25 (`docs/PERFORMANCE_REVIEW.md`, direction 9): with lighting stubbed to
surface colour, grass and terrain together freed 2.5 ms in the meadow (11.91 → 9.45) and 2.1 ms at
the start view (13.44 → 11.35). GPU counters show the grass pass ALU-limited (78%) in fragment
shading. The lighting runs inside large material shaders that keep few pixels in flight, on 2×2
pixel quads that thin blades and distant small triangles mostly waste, and on terrain that the
grass, drawn later in its own pass, then covers. Resident Evil Village's frame has the shape this
plan builds: 40–53% of its GPU time is compute overlapping the drawing.

The goal is the most efficient frame, not the quickest change. Each stage is measured with
`tools/bench.py` against `tools/frame_budget.toml` and judged on look captures.

## What exists

From three read-only surveys of the code (October 25):

- **One lighting core with material branches.** `assets/shaders/lighting/pbr_lighting.wesl`
  (`apply_pbr_lighting`, our fork of Bevy's) lights terrain (`TERRAIN_SURFACE`), every
  `EnvironmentMaterial` (rocks, props, buildings, characters, ground details, plants), tree bark
  (`TREE_BARK_SKY`), crowns (`TREE_CROWN_SHADING`, `TREE_CROWN_SKY`) and impostors
  (`IMPOSTOR_SELF_SHADOW`). Grass has its own model (`vegetation/draw/lighting.wesl`, `shade()`).
  Fog and aerial perspective are already outside materials (the sky composite). No point or spot
  lights exist; Bevy's SSAO, SSR, IBL, clearcoat and transmission are unused.
- **Renderers.** Terrain is a Bevy `Material` with no prepass (camera motion comes from
  `CompleteTemporalMotion`). Trees are custom phase items in Bevy's opaque, alpha-mask, prepass and
  shadow phases, with pipelines from `TreeWindMaterial`; they write motion in the prepass and
  shade and alpha-test again in the main pass. Impostors light a point behind their quad but write
  the quad's depth. Grass is a non-mesh item in `Opaque3d`, or under Temporal its own pass after
  the main pass, writing colour, depth and its own motion. Characters, details and placed objects
  are `EnvironmentMaterial` (an `ExtendedMaterial` on `StandardMaterial`).
- **Bevy 0.20's deferred path.** Its prepass plumbing works: `DeferredPrepass` allocates an
  `Rgba32Uint` G-buffer (16 B), an `R8Uint` lighting-pass id, motion and depth, and runs the
  `Opaque3dDeferred`/`AlphaMask3dDeferred` phases in one render pass in the `Prepass` set;
  materials opt in with `opaque_render_method()` and a deferred fragment shader. Its lighting pass
  is not usable here: it runs Bevy's `apply_pbr_lighting`, its shader cannot be replaced, it
  rebuilds positions from the full viewport (wrong at 50% scale, `view_transformations.wesl:222`),
  and its packing has no room for our inputs. It forces MSAA off on deferred cameras. The view
  depth is `Depth32Float` without stencil.

## Decisions this needs

1. **MSAA retires for world cameras.** *Decided* (user, October 25). Deferred cameras cannot
   multisample. The game's default was 50% + MetalFX Spatial + 4× MSAA
   (`crates/app_game/src/game_render.rs:58-65`); MetalFX Temporal becomes the default (it is the
   look target), Spatial and native take FXAA, and so does the editor. With MSAA go
   `MsaaColorStorePlugin`, the crossfade sample masks (dither everywhere), and the sky composite's
   per-sample depth loops.
2. **Grass keeps its own lighting model.** Moving it onto the shared core would add terrain
   horizon, ground bounce and terrain sky light and change its look; it gets its own kernel.

## Design

### Frame order (Temporal)

1. Shadow cascades (unchanged).
2. **G-buffer stage**, nearest coverage first so the GPU skips hidden surfaces (early depth test
   on PS5; hidden-surface removal on the Mac works within a pass only): grass in a render pass of
   its own before Bevy's prepasses (Bevy's `early_deferred_prepass` is private, so nothing can be
   ordered between it and the trees' depth prepass), then Bevy's deferred prepass with trees,
   impostors, objects and terrain. Every writer writes depth, motion and its G-buffer words;
   alpha tests happen once, here. The trees' separate depth prepass goes.
3. **Lighting kernels** in `DeferredLighting`, one per model, each with only its own registers and
   owned by the model's renderer (`atmosphere` cannot depend on `vegetation_render`): standard
   (terrain, objects, bark, crowns, impostors by flags) and grass. Each is a full-screen draw at
   the far plane, so depth skips the sky, that discards other models' pixels, reads the G-buffer
   and depth, and writes the lit colour into the HDR target's region.
4. **Not built yet, each waiting for the trace to show what it would save.** Tile classification
   (compute, 8×8 tiles over `main_pass_viewport`, indirect tile draws per kernel) would replace
   the kernels' discards. A sun visibility pass (cascade filter, cloud beam and scattered share,
   forest march and terrain horizon past the cascades, `ground_in_light`; direction 5's mask)
   would compute what every kernel computes now, once, and its slow terms at half resolution.
5. Forward passes as today: anything not yet converted, the cloud volume, light shafts, sun glare,
   the sky composite (reads depth, blends over the lit image), particles, MetalFX, post.

`CompleteTemporalMotion` goes once every writer writes its motion (terrain and impostors write
camera motion themselves).

### G-buffer

Bevy's attachments, our packing. Per pixel: the `Rgba32Uint` surface word (128 bits), motion
(`Rg16Float`) and depth; models that need more add their own target in their own pass (grass an
`Rgba8Unorm`), or the `Rgb10a2` normal-prepass slot in Bevy's pass. About 45 MB written and read a
frame at 1728×1084. The `R8Uint` id is not needed (the model is in the surface word); its copy
pass (`copy_deferred_lighting_id`) is a no-op in the schedule.

| Model | Surface word (128 bits) | Extra target |
| --- | --- | --- |
| Standard (terrain, objects, bark) | albedo gamma 2.2 24, roughness 8; shading normal oct 2×12, model 8; diffuse occlusion 8, specular occlusion 8, reflectance 8, metallic 8; geometric normal oct 2×12 (shadow bias), spare 8 | unused |
| Crown (v1, v2, impostor) | as standard; crown sky share and self-shadow terms in the spare words; impostor depth through `frag_depth` | crown normal oct 2×10 (sun facing, bias) |
| Grass (`vegetation/draw/gbuffer.wesl`) | albedo 24, occlusion 8; flat normal oct 2×12, model 8; rounded normal oct 2×12, √ sheen alpha (its `fwidth` variance folded in) 8; clump normal oct 2×8, √ blade alpha 8, sheen strength 8 | own `Rgba8Unorm`: transmission, canopy depth, canopy height, spare |

Precision is checked on captures (crown occlusion may need 10 bits). Grass keeps both sheen
normals, so its near and far lobes blend as in the forward shader. Derivative-based work
(texture filtering, grass sheen variance, prints, snow) stays in the material, which writes its
result.

### Bevy integration

- Cameras: `DeferredPrepass`, `DepthPrepass`, `MotionVectorPrepass` (Temporal), the normal-prepass
  slot enabled as an `Rgb10a2` target when a model in Bevy's pass needs it. Bevy's deferred
  lighting plugin stays for its pipeline layout: every deferred view gets `SkipDeferredLighting`,
  and the standard kernel is specialised from `DeferredLightingLayout` with our shader and the
  environment group at group 2 (`crates/atmosphere/src/environment/deferred.rs`).
- Terrain: `opaque_render_method() -> Deferred` and a deferred fragment shader (material
  evaluation, then pack). `EnvironmentMaterial` and `TreeWindMaterial` likewise through their
  extensions (`material.wesl` already has a dormant `deferred_output` branch).
- Trees and impostors: queue into `Opaque3dDeferred`/`AlphaMask3dDeferred` instead of the main
  and prepass phases. Bevy draws the deferred opaque phase only when its mesh bins are non-empty;
  non-mesh items alone are skipped, so check the trees' bins.
- Grass: its own render system before the `Prepass` set (`vegetation_render`
  `renderer/deferred.rs`), the first to take the G-buffer, motion and depth attachments each
  frame, so it clears them. It binds an empty group in the view's slot: the view's bind group
  holds the G-buffer for reading, which a pass writing it may not bind.
- Our passes reconstruct position with `view.main_pass_viewport`, never `view.viewport`.
- A deferred material draws with `deferred_vertex_shader`, which defaults to Bevy's prepass
  vertex shader, not to the material's `prepass_vertex_shader`; `enable_prepass()` must be true or
  Bevy never queues it. The prepass view group holds Bevy's globals at binding 1, not 11
  (`shaders/frame.wesl`).
- On Metal, WGSL `pack4x8unorm` stored zero for a whole word; the G-buffer packs bytes by hand.
- The lighting core moves into functions the kernels call; the forward path keeps calling the
  same functions until every material has moved, then it is deleted.

## Stages

Each stage ends with the benchmark (all views, history row) and look captures of the views it
touches against the stage before.

1. **Groundwork and terrain.** *Done* (`f49c88d`, `f9854ff`). Cameras, the AA change, G-buffer
   packing (`shaders/lighting/gbuffer.wesl`), terrain's deferred output with camera motion, and
   a full-screen fragment pass that runs the shared lighting per G-buffer pixel
   (`shaders/lighting/deferred.wesl`). With terrain the only deferred model, classification and
   per-model kernels had nothing to split and the depth test already skips the sky, so they move
   to stage 2. Against `0a5ac41`, before the AA change: GPU busy fell 0.4–0.75 ms in five views
   (meadow 11.90 → 11.18, forest-pine 12.25 → 11.82, storm-road 11.44 → 10.69, mountain-hamlet
   11.25 → 10.57, brook-mouth 10.73 → 10.01) and rose 0.22 at the start view (13.34 → 13.56),
   where grass covers terrain that is now shaded in the G-buffer pass and lit. The lighting pass
   takes 0.46–0.68 ms.
2. **Grass.** *Done*. The blade shader's `surface` goes into the G-buffer from a pass before the
   terrain's, and the grass kernel runs `light` on its pixels; the forward draw (the editor)
   calls the same two. Each model's renderer owns its kernel, and the standard one
   discards grass pixels; classification and sun visibility wait (see Frame order). Until trees
   join the G-buffer, grass under a tree's prepass depth is still lit and then drawn over.
   Captures against `1da048f` match within run-to-run noise (mean absolute difference 1.3–1.4
   under Temporal). Against stage 1: GPU busy fell in the grass views (meadow 11.18 → 10.27,
   start 13.56 → 12.64, forest-pine 11.82 → 11.29, brook-mouth 10.01 → 9.85) and held in the
   others (storm-road 10.69 → 10.73, mountain-hamlet 10.57 → 10.67). In the meadow the grass
   line fell 2.14 → 1.14 and opaque 2.03 → 1.62 (no terrain shaded under blades), and lighting
   rose 0.66 → 0.86 with the grass kernel. Kernels must be ordered: unordered systems record on
   other threads, so the target's clearing pass could be submitted after another kernel and
   blank its pixels on some frames; the standard kernel always opens the first pass.
3. **Objects.** `EnvironmentMaterial`: rocks, props, buildings, characters, details, plants
   (crown v1).
4. **Trees and impostors.** Crown v2 and bark-sky flags, the crown normal, impostor depth, motion
   from the deferred pass; the trees' depth prepass goes.
5. **Cleanup.** Forward lighting code paths, MSAA paths and `CompleteTemporalMotion` deleted;
   lower-rate visibility terms if the visibility pass shows up in the trace.

## Risks

- The editor's Presets and Animation views render under an isolated atmosphere to an `Rgba8`
  image, and some meshes there stay on Bevy's stock forward shader; they need either a deferred
  path of their own or to stay forward with the shared lighting functions.
- Bandwidth: the G-buffer round trip (~90 MB a frame) is new cost on every platform (wgpu has no
  tile memory). It is far below the lighting it replaces, but the benchmark decides.
- Bevy's deferred pass copies the full window's depth after each deferred prepass, as the
  forward prepass does today.
