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

1. **MSAA retires for world cameras.** Deferred cameras cannot multisample (user, October 25:
   deferred may give up MSAA; FXAA + TAA as Village uses look good). The game's default today is
   50% + MetalFX Spatial + 4× MSAA (`crates/app_game/src/game_render.rs:58-65`). Proposal: MetalFX
   Temporal becomes the default (it is the look target); Spatial and native take FXAA; the editor
   takes FXAA. With MSAA go `MsaaColorStorePlugin`, the crossfade sample masks (dither everywhere),
   and the sky composite's per-sample depth loops.
2. **Grass keeps its own lighting model.** Moving it onto the shared core would add terrain
   horizon, ground bounce and terrain sky light and change its look; it gets its own kernel.

## Design

### Frame order (Temporal)

1. Shadow cascades (unchanged).
2. **G-buffer stage** in the `Prepass` set, nearest coverage first so the GPU skips hidden
   surfaces (early depth test on PS5; hidden-surface removal on the Mac works within a pass only):
   grass (own render pass, before Bevy's deferred pass), then Bevy's `early_deferred_prepass` with
   trees, impostors, objects and terrain. Every writer writes depth, motion and its G-buffer words;
   alpha tests happen once, here. The trees' separate depth prepass goes.
3. **Classification** (compute, 8×8 tiles over `main_pass_viewport`): which models each tile
   holds; tile lists and indirect arguments per kernel; sky-only tiles skip lighting.
4. **Sun visibility** (compute, non-sky tiles): cascade filter with overlap blend, cloud beam and
   scattered share, forest march and terrain horizon past the cascades, `ground_in_light` where a
   model asks for it; written to an `RG8` texture. Direction 5's mask. Slow terms (cloud, forest,
   horizon) can later run at half resolution.
5. **Lighting kernels** (compute, indirect, one per model, each with only its own registers):
   standard (terrain, objects, bark, crowns, impostors by flags) and grass. They read the
   G-buffer, depth and visibility, add sky light, ground bounce and wetness, and write the lit
   colour into the HDR target's region.
6. Forward passes as today: anything not yet converted, the cloud volume, light shafts, sun glare,
   the sky composite (reads depth, blends over the lit image), particles, MetalFX, post.

`CompleteTemporalMotion` goes once every writer writes its motion (terrain and impostors write
camera motion themselves).

### G-buffer

Bevy's attachments, our packing. Per pixel: the `Rgba32Uint` surface word (128 bits), an `Rgb10a2`
target in Bevy's normal-prepass slot (32 bits), motion (`Rg16Float`) and depth: 24 B and depth,
about 45 MB written and read a frame at 1728×1084. The `R8Uint` id is not needed (the model is in
the surface word); its copy pass (`copy_deferred_lighting_id`) is removed from the schedule.

| Model | Surface word (128 bits) | Rgb10a2 |
| --- | --- | --- |
| Standard (terrain, objects, bark) | albedo sRGB 24, roughness 8; shading normal oct 2×12, model 3 + flags 5; occlusion 8, reflectance 8, metallic 8, spare 8; spare 32 | geometric normal oct 2×10 (shadow bias), spare 10 |
| Crown (v1, v2, impostor) | as standard; crown sky share and self-shadow terms in the spare words; impostor depth through `frag_depth` | crown normal oct 2×10 (sun facing, bias) |
| Grass | albedo 24, occlusion 8; diffuse normal oct 2×12, model + flags 8; flat normal oct 2×8, sheen normal oct 2×8; sheen alpha (its `fwidth` variance folded in) 8, sheen strength 8, transmission 8, canopy depth 8 | canopy height 10, sky facing 10, spare 10 |

Precision is checked on captures (crown occlusion may need 10 bits; a second sheen normal for the
16–64 m band, if the blended lobe shows, takes the grass spare bits). Derivative-based work
(texture filtering, grass sheen variance, prints, snow) stays in the material, which writes its
result.

### Bevy integration

- Cameras: `DeferredPrepass`, `DepthPrepass`, `MotionVectorPrepass` (Temporal), the normal-prepass
  slot enabled as our `Rgb10a2` target. `PbrPlugin { add_default_deferred_lighting_plugin: false }`.
- Terrain: `opaque_render_method() -> Deferred` and a deferred fragment shader (material
  evaluation, then pack). `EnvironmentMaterial` and `TreeWindMaterial` likewise through their
  extensions (`material.wesl` already has a dormant `deferred_output` branch).
- Trees and impostors: queue into `Opaque3dDeferred`/`AlphaMask3dDeferred` instead of the main
  and prepass phases. Bevy draws the deferred opaque phase only when its mesh bins are non-empty;
  non-mesh items alone are skipped, so check the trees' bins.
- Grass: its own render system in `Prepass`, after `early_prepass` and before
  `early_deferred_prepass`, loading the G-buffer, motion and depth attachments.
- Our passes reconstruct position with `view.main_pass_viewport`, never `view.viewport`.
- The lighting core moves into functions the kernels call; the forward path keeps calling the
  same functions until every material has moved, then it is deleted.

## Stages

Each stage ends with the benchmark (all views, history row) and look captures of the views it
touches against the stage before.

1. **Groundwork and terrain.** Cameras, G-buffer packing (`assets/shaders/deferred/`), terrain's
   deferred output with camera motion, classification, sun visibility, the standard kernel with
   the terrain flags. Other materials stay forward and draw after the lighting. Also the AA
   change (decision 1), since a deferred camera cannot multisample. Expect most of the terrain's
   lighting share (up to 1.7 ms in the meadow) less the new passes.
2. **Grass.** G-buffer output from the blade shader, drawn before the terrain; the grass kernel.
   Removes grass lighting from 2×2 quads and the terrain's shading under blades.
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
