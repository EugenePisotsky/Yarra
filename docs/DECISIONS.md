# Decisions

Approaches kept and approaches rejected, one entry each, so nobody re-tries a rejected idea or
undoes a kept one without knowing why. Current behaviour is in [Architecture](ARCHITECTURE.md); the
step-by-step ledgers this replaces (`EXPERIMENTS.md`, `REFACTORING.md`) are in Git history.

State on 2026-10-20: terrain v2 only (GPU clipmaps, per-pixel materials); Reteya is the one world;
trees come only from the YarraVegetation kit and are drawn on the GPU in game and editor.

## Rendering & presentation

- **Look target: 50% render scale + MetalFX Temporal at 60 fps.** *Kept* (user, October 9 and 17).
  Native is for faster machines. The 60 fps cap is the default (`cb74dda`), and so is Temporal
  (October 25).
- **A frame budget decides, with quality tiers.** *Kept* (user, October 25). The budget may
  override a look verdict: work that does not fit its system's line is optimised first, and what
  still does not fit ships as a quality tier. Lower tiers may swap techniques and content, such as
  card grass on slower devices (as Where Winds Meet does) or lighter tree variants.
- **Dynamic resolution as an option.** *Not built* (user, October 25). The render scale held to the
  frame budget under Temporal, so a heavy moment shows a slightly softer image instead of a drop
  from 60 to 40 fps. It helps pixel work, not vertex work. Village has none on the Mac; on PS5 it
  renders fixed 4K checkerboard.
- **No MSAA; Temporal by default, FXAA otherwise.** *Kept* (user, October 25). Deferred shading
  cannot multisample, and FXAA + TAA, as Village uses, looks good. MetalFX Temporal is the
  default; Spatial, Linear and the editor take FXAA. The MSAA colour-store pass, the crossfades'
  sample masks and the sky composite's per-sample classification went with it.
- **Pacing by display link plus Metal's minimum presentation interval.** *Kept.* 99.7% of intervals
  on time against 81% with a timer; the Metal interval alone and higher thread QoS were rejected.
  Keep the wgpu-hal patch (`third_party/wgpu-hal/YARRA_PATCH.md`) until upstream has it.
- **Deferred lighting for every opaque surface.** *Kept* (user, October 25: "the most efficient
  ways, not the fastest"; [Architecture](ARCHITECTURE.md#deferred-lighting)). Lighting inside
  grass and terrain materials cost 2.1–2.5 ms on 2×2 quads and on terrain the grass then covered.
  Terrain (`f9854ff`), grass (`e3f6fc1`), objects and tree meshes (`05d5956`) and impostors
  (`3eae681`) moved in turn, each matching the forward look in captures; the forward lighting of
  terrain, grass and impostors and the static-motion completion pass went after. GPU busy at
  Maximum from `0a5ac41`, before the AA change: meadow 11.90 → 10.06 ms, start 13.34 → 11.08,
  forest-pine 12.25 → 10.80, storm-road 11.44 → 10.57, mountain-hamlet 11.25 → 10.51,
  brook-mouth 10.73 → 9.70. Our own lighting pass replaces Bevy's:
  its lighting is not ours, and it places pixels by the whole view, wrong under Temporal's region.
  *Rejected:* a kernel per model (a full-screen pass each, about 0.1 ms where its model is
  absent) and tile classification (+0.04–0.26 ms: classifying cost 0.08–0.11 ms, and small tile
  quads shade their diagonal twice). Branching per pixel costs one-model views a little (the
  meadow's terrain-only kernel 0.50 → 0.59 ms). *Open:* a sun visibility pass computing shadows,
  cloud, forest and horizon once per pixel, slow terms at half resolution. This supersedes the
  depth prepass only for Temporal (`3bc9be4`); a foliage or global depth prepass against overdraw
  was rejected twice (+0.3 ms GPU, +0.7 ms render prep).
- **Bloom and eye adaptation of our own, before the upscaler.** *Kept* (October 26). Bevy's ran
  on the 7.5 Mpx output after MetalFX (bloom 0.84 ms, exposure 0.20) and cannot read Temporal's
  rendered region. Ours keep Bevy's filters, its natural preset's weights and its metering
  (`atmosphere::bloom`, `adaptation`), run as compute on the rendered image and blend back before
  reconstruction: 0.25 ms together. GPU busy at Maximum from `f5ac211`: meadow 10.06 → 9.48 ms,
  start 11.08 → 10.43, forest-pine 10.80 → 10.14, storm-road 10.57 → 9.93, mountain-hamlet
  10.51 → 9.96, brook-mouth 9.70 → 8.99. Captures show the same glow (correlation 0.97 in the
  start view) and an image 2–8% brighter: Bevy's exposure curve is an 8-bit table, truncated and
  read by floor, that darkened by up to an eighth of a stop, and its bloom levels (11/10-bit
  floats) lost energy. Rain and ambient particles, drawn after reconstruction, no longer bloom.
- **The UI draws in the tone map's pass, on the window.** *Kept* (October 26). Under Temporal a
  UI camera cleared a 7.5 Mpx texture of its own (0.14 ms), drew the world image full-screen under
  the UI (0.19) and was copied to the window (0.11). The world camera targets the window instead,
  and one pass tone-maps onto it and draws the UI (`upscaling::DirectTonemapOutput`), whose
  pipelines take the window's format; the UI adds about 0.03 ms to the tone map. GPU busy at
  Maximum from `f447d04`: meadow 9.48 → 9.30 ms, start 10.43 → 10.12, forest-pine 10.14 → 9.86,
  storm-road 9.93 → 9.72, mountain-hamlet 9.96 → 9.67, brook-mouth 8.99 → 8.83. The LOD lab's
  panel and `--temporal-standard-output` keep the UI camera: egui draws only on a camera's own
  texture.
- **MetalFX Temporal's scaler takes full-size textures.** *Kept.* A region-sized scaler (`b2c8cdf`)
  saved 0.8 ms that MetalFX's internal wait absorbed, and its history went blocky in motion;
  reverted (`c70158a`). Temporal's ~4 ms is MetalFX's own.
- **Upscaling trials.** *Rejected:* Temporal at 33%, packed RG11B10F HDR, a shared motion
  attachment, the reactive mask, compact input packing. *Kept:* direct tone-map output and Linear
  composition, with fallbacks.
- **Khronos PBR Neutral with a hue-preserving toe.** *Kept* (`ed74252`). The user chose Neutral over
  Tony McMapface and AgX; its toe stripped blue from dark colours (meadow blue/green 0.21 → 0.05),
  so `atmosphere::tonemapping` patches Bevy's shader (re-apply on upgrades); `DAYLIGHT_BALANCE` is
  ~5500 K.
- **Eye adaptation on.** *Kept.* No fixed EV suits both beach and spruce stand (L* 46 against 19);
  it follows half a metered change within +1.5/−1 stop, and `NIGHT_BRIGHTEN` eases off at night.
- **One sky composite instead of Bevy's sky pass.** *Kept.* Sky, air, clouds, weather fog, sea and
  glare in one pass that integrates extinction itself: Bevy's half-float LUT and aerosol medium gave
  NaN streaks.
- **The composite draws the sea, the air, then the fog.** *Kept* (October 23). As one shader it held
  the GPU's most registers (12.9% occupancy) and spilled 400 B a pixel, the water and blowing snow
  taxing pixels that showed neither. Three draws in the same pass, each in front of the last,
  need only their own: 1.91 to 0.99 ms at the meadow, 2.15 to 1.29 ms at brook-mouth, the images
  within noise. At silhouettes each draw multiplies the mean transmittance, as the composite always
  did for the scene behind it.
- **Composite shortcuts.** *Kept* (`f83773f`, `189488a`). Air at a silhouette pixel's nearest and
  farthest samples, interpolated, or by side where the sea's surface or the cloud base lies
  between; fog light once per pixel; dark lights skipped (~1 ms). A froxel
  mist grid was reverted (mist 0.4 ms, the pass up to 0.45 ms).
- **Light from below takes the ground's colour.** *Kept* (October 21). Every lit material's lower
  half took light reflected by grass (albedo 0.09, 0.12, 0.05), so a figure on a snowfield stayed
  dark on its shaded side where Red Dead Redemption 2 lights it from the snow. A compute pass fills
  a top-down map around the camera each frame (128² at 2 m, and at 32 m out to 2 km): each surface's
  mean texture colour under its grass field's, blended by the control shares, water below the sea,
  and the cosine at which the sun meets the ground past the terrain's horizon. The bounce takes the
  sun only where the ground lies in it, and the sky as far as the terrain leaves it: otherwise a
  figure on a shaded snow slope shone brighter than the snow. The shadow maps tell it a metre
  towards the sun from a figure, a trunk or a shaded tilted point of the terrain (past its own body
  or hollow), with four points around where that one is shaded, so a figure's narrow shadow leaves
  a trench in it lit by the snow around while a house's or a ridge's does not; crowns and level
  ground keep their own shadow. The print walls' own bounce, which ignored shadows, went; the
  pixel's own shadow alone (a first version) darkened trenches inside a figure's shadow. Snow
  walking ~0.14 ms slower (0.6 of 65 fps), the village route and a forest view within noise.
- **Bevy left alone.** Rewriting its atmosphere LUT pass: not pursued (static tables 0.08 ms). Mesh
  shaders for grass: not now (no hardware mesh shading on M2). Real leaves: no (30–300× triangles).
  Lights are never hidden: hiding one loses shadow casters for good.

## Terrain

- **Terrain v2: per-pixel materials at every distance on GPU clipmaps, CDLOD geometry.** *Kept; the
  only path* (`be073ac`, `fb582d2`, `124d85f`). Baked composites (2 m texels, 2–7× too coarse at
  50%) blurred 300 m–3 km and seamed where near detail ended; Witcher 3-style control tiles fixed
  both, faster (journey 104 against 78 fps).
- **CPU-built terrain removed.** *Superseded by v2* (`d63908b`, `e86431b`, `392d819`). Planner,
  morph meshes, composite bake, near-detail atlas, page material: ~17,000 lines. Lessons kept:
  detail must not depend on view direction, and planning stays off the main thread.
- **Virtual texturing.** *Deferred.* wgpu has no sparse textures, and it needs readback and runtime
  ASTC on iOS; only if per-pixel shading proves too costly.
- **Rejected before v2.** 1 m composites (~10% more contrast for +360 MB, a 7-minute cook), sharper
  snow masks (the bake's averaging was the blur), a smaller triangle budget (1.15 ms for 9 → 24 px
  error), skipping zero-weight layers (slower).
- **Control decode with fixed candidates.** *Kept* (`13ee912`). A per-pixel surface array cost
  ~3.5 ms of a 10.4 ms frame. Ground grass covers ≥0.7 takes one plain sample under the field
  colour. Candidates are every sample's two heaviest surfaces and the nearest's third (October
  21): the nearest sample's second alone stepped where it changed, and roads refined from the
  wear field turned the step into half-metre blocks of snow in packed roads.
- **Top-three height blend; biplanar on steep ground.** *Kept* (`2827271`, `124d85f`). Blend height
  rides in base-colour alpha; planar projection smeared cliffs.
- **Roads as a wear field per page.** *Kept* (`c1d20d3`, `47fc8de`). 256² (12.5 cm) wear, rut depth
  and direction; each surface names what it tramples and wears to. It replaced per-pixel centre-line
  search; v2 draws ruts from a wear atlas, for horses and carts, never cars. Pages were sampled
  clamped at their own edges, so every road stepped at every cell border; within a texel of an edge
  the material filters across into the next cells' pages (October 21). The refinement from the
  field fades in as both the worn and the unworn ground gain room to trade.
- **Neighbouring pages agree at edges.** *Kept* (`602ac07`). Up to 12 surfaces per cell, chosen with
  edge weight ×16 and road ground first; independent choices of eight drew 328 seams.
- **Ground prints as caps on the snow, from a press log.** *Kept* (October 21). Feet stamp a log;
  two toroidal windows around the player (2 cm over 41 m, 8 cm over 164 m) are drawn from it, so
  trails survive walking away and back. As in Rise of the Tomb Raider, a texel holds not relief
  but how high over the firm ground the snow may stand, and the lowest sole that reached it; the
  vertices lower the soft layer to that, so a planted foot pushes 40 cm of snow to the ground and
  one swinging low ploughs a trench, while 6 cm of packed snow takes prints. Rims rise beside a
  trench where a sole went below the snow's top, by the distance past the sole recovered from the
  cap. A moving foot logs its sweep from its last heel (a row of soles scalloped the walls).
  Parallax prints over the surface went with real geometry (characters on the surface, 7 cm,
  only within 1.5–3.5 cm of the ground). Walls take the snow's light from below like any surface
  (the ground albedo map). Walking through deep snow measured within noise of the build before
  snow.
- **Prints drawn from a resolved, filtered relief.** *Kept* (October 21). Vertices read the caps
  directly: 12.5 cm to 1 m apart over 2 and 8 cm texels, morphing with the camera's distance,
  they caught different samples as the player walked, so trails far behind crawled. A pass now
  resolves each touched tile (with the soft layer's depth) into the lowering and its slope, and
  fills six mip levels; a vertex takes the level of its spacing, widening as it morphs, so a
  fully morphed vertex takes what the coarser grid's takes, and pixels filter the slope like any
  texture (one sample a window, not eight loads four times). Steep walls get streaks finer than
  the relief, faded by the pixel's footprint. A texel's slope reads its neighbours, so the tiles
  beside every listed one are resolved again: a tile entering at a window's edge otherwise kept
  the slope it took beside ground from across the toroidal window, and trails 41 or 164 m away
  came back as ladders of lines a tile apart (the user's "weird patterns").
- **Each leg ploughs a channel through deep snow.** *Kept* (October 21). Two feet sweeps alone
  left two narrow lanes with scalloped walls; Red Dead Redemption 2 and The Last of Us Part II
  show one channel with the feet lanes in its floor. A circle at the character's root (October 21
  morning) parted the snow ahead of and around the legs whatever they did (the user: "it
  disappears around me and not affected by the character's feet"); each leg now presses a circle
  at its ankle (`wade_radius_m` 0.14, floor `wade_floor_m` 0.12 over the firm ground, no lower
  than its heel), and the two legs' sweeps join into one channel between them. 6 cm of packed
  road takes none.
- **Trenches grow with the feet, broken unevenly.** *Kept* (October 21). Presses are logged once
  they move 10 cm, and were drawn only then: at a jog the channel's front jumped 15 cm every third
  frame. Each frame now also draws every moving foot's sweep since its last logged press, and a
  foot that stops logs its sweep however short. A sweep follows the ball's way at the sole's
  lowest floor with no waist: from the last heel to the ball, consecutive sweeps overlapped with
  different floors and left a crescent each. Outlines waver with world-space noise (up to 2.5 cm,
  over 16 and 4 cm), rims lump, crumbs lie thrown beside trenches and floors are trodden rough,
  all in the passes that press and resolve the touched tiles, so drawing costs nothing more and a
  trail a window takes again comes back the same.
- **Snow glints.** *Rejected* (user, October 21: "rather distractive and don't look very
  natural", little value). Crystal facets in a share of the cells of a world grid a few pixels
  across, tilted at random up to 63° with roughness 0.15, caught the sun and the moon as a
  specular lobe of their own; they scattered as single bright dots over moonlit snow. Tilting the
  shading normal itself had checkered the snow light and dark.
- **Detail levels below the leaves for deep snow, not tessellation.** *Kept* (October 20). wgpu has
  no tessellation; shipped deep snow (Rise of the Tomb Raider, God of War Ragnarök, Ghost of Yōtei)
  displaces real geometry near the camera and keeps parallax for far or fallback snow (Batman:
  Arkham Origins: parallax "will never be as thick"). CDLOD splits soft leaves further near the
  camera, on the leaves' triangles: +260k triangles on the pass basin with no measurable cost, and
  4M triangles in the main pass measured within noise, so geometry is not the limit.
- **Roads pack snow across their whole width.** *Kept* (October 21). Snow wears and tramples to
  packed snow (`soft_depth` 0.06), and `whole_road` packs it under a road's whole corridor, not
  only its wheel tracks (a cart track's centre strip kept 40 cm of snow between two trenches), in
  the page's own weights: the wear field wears only where traffic does. Roads through snow run as
  packed tracks below the banks, taking shallow prints.
- **Snow cooked into the leaves as a layer over the firm ground.** *Kept* (October 21). Actors
  stand on the firm ground and wade; feet press the layer down to it. Drawn at run time from the
  half-metre control samples (October 20: raised by each surface's `soft_depth` out to 300 m),
  banks stair-stepped along the samples, their triangles and shadows showed past the detail
  levels and shading lost the banks past level 0, for 0.3–0.6 ms on a screen of snow. The cook
  now smooths the depth over 0.75 m (reading neighbouring cells, every sum in one order so
  shared borders agree exactly) and raises each leaf to the layer's top, its normals tilted with
  it; parents decimate it like any relief, so snow stands at every distance. Leaves keep the
  depth in height steps, so the firm ground comes back exactly. The run-time lift and slope are
  gone; a screen of snow measured within noise of the run-time layer. Smoothing the depth itself
  spread it into grass among the snow, which then took trenches (the user's report): the layer
  now ends where half the ground is soft once soft ground is drawn back 1.5 m from its edge, and
  thickens over about 3 m into the snow, so patches of firm ground hold none. The same smoothing
  filled a 2 m path of packed snow to 15–20 cm, so it took trenches like open snow; deep soft
  ground is drawn back 1 m from shallower before its depth is smoothed, so a path keeps its 6 cm
  and the banks rise from its sides. Grass whose control samples mix in snow still holds some of
  it: prints tilt only the soft surfaces the pixel shows.
- **Clean snow: one surface's own whiteness, shaded by its shape.** *Kept* (October 21). The
  carved windswept scan's dark crevices, tiled every 2 m, and the profile's macro albedo
  variation (×0.76–1.31) read as dirt on every snowfield; prints were tinted blue and darkened
  with depth. Snow takes a gently wind-shaped scan and a fifth of the macro variation
  (`TerrainSurface::macro_strength`), roads a trampled-snow scan, and prints only their slope,
  as in Red Dead Redemption 2 and The Last of Us Part II.
- **Terrain shadows and sky occlusion from a GPU horizon map.** *Kept* (`3a151a2`). The CPU search
  on every core each 0.5° of sun dropped a frame every ~2 s, only with the day clock on (214
  drops → 8).
- **Heights on a world-wide 1/1024 m grid.** *Kept.* Millimetre road relief at any altitude and
  identical shared edges.

## Vegetation: grass

- **GPU grass pipeline kept; the ground decides what grows.** *Kept* (`2e054fb`).
  `assets/packs/grass/baltic.ron` names types per surface; coverage follows surface weights. Beaches
  are bare.
- **Ghost of Tsushima/Yōtei look.** *Kept* (`73efae0`, `c4813e5`). Lying blades, twin blades in a V
  from one strip (+1.4 ms, accepted), tussock patches, canopy darkness by depth, noise shadow lines.
- **Blades bent once per frame, before drawing.** *Kept.* A compute pass (`pose.wesl`) bends each
  drawn blade by trampling and the wind, this frame and last, into 64-byte records: the curve in
  halves and last frame's as its change, so motion vectors keep their precision, then what else
  its vertices need of it (`DrawnBlade`). The vertex shader only evaluates the curves. Every vertex used to bend its blade twice: at the start view
  grass took 4.5 of 17.1 ms GPU busy (12.7 without grass; 15.3 bending once, 15.7 without wind).
  Now start 17.1 → 14.5 ms (47 → 52 fps uncapped) and the journey route 16.5 → 13.6 ms (48 → 53
  fps); the pass costs 0.14 ms. Overcast captures at fixed exposure match the old draw to
  0.4/255. The records take 112 MiB at the arenas' full capacity.
- **A grass vertex carries little.** *Kept* (October 26). On Apple's GPUs each vertex's outputs
  are written out for the tiler, and that, not the vertex math, was most of the blades' vertex
  stage: the same math with one output took the meadow's 1.09M vertices 0.18 ms, against 0.58
  with 35 floats. They output 23: what is the same along a blade travels as one flat word, and
  the fragment takes its type's material, its occlusion from the canopy depth and this frame's
  screen position for motion from its own. Vertex 0.58 → 0.32 ms, fragments 0.64 → 0.56 (less to
  interpolate). Blades are drawn 32 to an instance (one to an instance, the same vertices took
  0.73 against 0.58 ms) and take their per-blade values from the pose record (0.80 → 0.73).
  Triangle strips took 2.28 ms.
- **Critically damped trampling.** *Kept.* Instant presses (no animation) and underdamped springs
  (read as shaking) were rejected.
- **Gusts change amplitude only; weather scales the phase clock.** *Kept.* A gusting rate times the
  absolute clock made blades vibrate more as play went on.
- **Grass history survives streaming.** *Kept.* Residency changes no longer reset MetalFX history
  (24 resets → 0); pages upload by content key (7.8 MB → ~0.2 MB, `d8d7ac6`).
- **Grass takes sky light as PBR ground does.** *Kept.* Full-albedo sky light made night meadows as
  bright as bare road.
- **Rejected grass trials.** Shadow sheets and clump proxies, blade bands, compact scheduling
  (0.06 ms), clump light per vertex, a +48 MiB arena, blue against saturation (teal dusk). Open:
  grass shadows, a tier past 96 m.
- **Ground details as CPU-placed plain meshes.** *Kept.* Low-tier scans (36–370 triangles) instanced
  by Bevy are cheap; spawn per page and type.

## Trees

- **One vegetation pipeline: the YarraVegetation kit.** *Kept.* No bought assets (a SpeedTree pine
  was refused), no parallel pipelines; the Houdini tree pipeline was removed (`e7cfee6`). Improve
  species in small steps, judged in game against photos.
- **A separate Scots pine path.** *Rejected* (October 17, tag
  `archive/trees-v2-scots-pine-2026-10-17`). Generator, needle geometry and Karma-baked cards read
  as "a shapeless mess" beside longleaf stands, though its reference renders looked right.
- **Cut-out overdraw, not triangles, is the foliage cost.** *Lesson.* Cut-out leaves were ~2.3 of
  4.1 ms; LODs that cut triangles but keep card area save nothing, and thinned crowns read as thin.
- **Kit meshes are welded at import.** *Kept* (October 25). The kit writes a tangent per face, so
  no corner was shared and every pass shaded three vertices a triangle. `import_vegetation_bundle.py`
  merges vertices that agree in all else and averages their tangents: near LODs 3–3.5× fewer
  vertices, far ones 2–2.5×, buffers as much smaller; snowy spruces on the storm road 12.13 →
  11.41 ms GPU, the image unchanged. Moving the fix into the kit's exporter would serve its other
  uses too.
- **GPU-driven trees in game and editor.** *Kept* (`2f52071`, `9cf8c56`). One pool from far-object
  blocks; compute picks LODs, fades and culls into indirect draws. Main thread −1.5–2 ms in forests
  at GPU parity. Culling bounds must match per primitive: a looser sphere cost up to 1 ms.
- **Culling bounds grow with this frame's wind.** *Kept* (October 23). Bounds padded for the
  shader's hardest lean (0.8 rad) on every axis culled a 19 m pine as a box up to 90 m wide, so
  trees well off screen drew in the prepass and main pass: looking down in a dense forest, 922
  primitives with nothing on screen. Each primitive keeps its sweep (anchor height, offset, reach)
  and the frame's wind bounds the lean and turns: 164 primitives, GPU busy 13.8 → 12.1 ms; facing
  the trees 15.8 → 15.3 ms. Entities still grow by the strongest wind.
- **The wind's per-tree work runs once per tree.** *Kept* (October 24). On the snowy spruce road
  in a storm (~1,000 primitives, 1.3 M triangles in view) the prepass was vertex-bound, 5.75 ms
  of vertex work against 0.5 of fragment. Every vertex sampled the gust noise at its tree's root,
  twice per pose, and turned each point and direction by three rotations in turn. The LOD pass
  now writes each tree's wind for both poses into its frame: the gust strength and the sines and
  cosines of the sway's four bands. A vertex composes its turns into one matrix, expands the
  sway's lag without `atan2`, and turns its cards by sine and cosine, not by angle. The main pass
  poses a vertex once for its position, normal and tangent. GPU busy 23.0 → 20.2 ms; with no wind
  at all it is 18.4.
- **A tree's limbs posed once a frame.** *Rejected* (October 26). In the pine forest the wind is
  about 0.4 of the G-buffer pass's 0.82 ms of vertex time (0.42 with every tree at rest) and
  about 0.28 ms more in the shadow cascades; the storm road and the hamlet lose about 0.3 ms in
  all. A compute pass posing each mesh-drawn tree's limbs (6 to 85 per form) once a frame, which
  their vertices read by a limb number added to the meshes, cut vertex time by 0.07–0.09 ms in
  those views and itself cost 0.04 ms: GPU busy did not move. A limb's frame is a fifth of the
  wind's cost and the cards' turns and flutter another 0.05 ms; last frame's pose, for motion
  vectors, is the largest part (one pose: 0.82 → 0.59 ms).
- **A snow chunk moves with its vertex's card, not by a pose of its own.** *Kept* (October 24).
  The snowy spruces' prepass posed each vertex and its chunk's middle, now and a frame earlier:
  four inlined poses, a 720-byte register spill, and 6% vertex occupancy for every vertex,
  shedding or not. The wind and a card's own turns move a card rigidly, so the pose carries how
  the card's points turned. The chunk stands at the vertex plus that turn of their offset. Prepass
  4.5 → 2.6 ms, GPU busy 20.2 → 18.4 ms on the storm road.
- **Timed dissolves, not distance crossfades.** *Kept.* Switch 4% past the band, dissolve over
  0.6 s, dithered (`crossfade.wesl`), shadows doubled mid-fade. Distance crossfades drew
  two LODs across a band and popped shadows.
- **Impostors from a texel per output pixel.** *Kept* (`d76b928`). Spruce 92 → 172 m, LOD2 drawn
  from 240 px; the fixed 200 px hand-off magnified impostors ~2.6×.
- **Impostor quality.** *Kept* (`4586161`, `d74df76`, `d04c6fa`). Ray-plane view reads, the view's
  mip bias, per-view mips with a gutter, baked from LOD0 (an LOD2 bake thinned crowns). Load normal
  atlases as linear: Bevy transcodes UASTC to sRGB.
- **Lower LODs keep LOD0's coverage.** *Kept* (`7f845a2`). The kit grows surviving cards back toward
  their branch, fitted within 0.5%.
- **Far cards between meshes and impostors.** *Rejected* (October 6). Crosses showed on spruces,
  impostors stayed blurry, 145 → 121 fps.
- **Impostors cast no shadows; a forest shadow map works past the cascades.** *Kept.* Sun-facing
  quads shadowed their own crowns black; past 80 m everything marches a 1024² crown map
  (`atmosphere::forest_shadow`).
- **Casters culled by where their shadow falls.** *Kept.* Frustum casters drew up to 2.3× the trees
  (+8% GPU). One LOD coarser (`--tree-shadow-lod 1`) saves a third of cascade time but lightens
  forests 5–7%; default 0 (check: awaiting a look verdict).
- **Baked crown and bark sky visibility; swaying impostors.** *Kept* (`71a07de`, `07cda97`).
  Impostors bend with their trunk, still by 2.5× the hand-off.
- **LOD from side-on height.** *Kept.* The projected root-to-top segment switched LODs as the camera
  pitched.
- **Branches part where characters push through, from a field of springs.** *Kept* (October 21).
  Like grass's trample map but in 3D: 64×32×64 voxels at 25 cm around the player, each a spring
  holding how far the foliage resting there is pushed. Bodies are upright capsules (0.3 m and a
  0.12 m margin, 1.8 m), springs follow the way out in a few hundredths of a second and swing back
  underdamped over about two. Every vertex of a limb turns about the limb's root (`_WIND_PIVOT`)
  by the largest turn that points between the root and it need, so a limb pushed anywhere swings
  whole; reading the field at the vertex alone moved only the leaves inside the body. Trees beyond
  a limb's length of the moving voxels skip it, which took the cost out of forests nobody walks
  through (it was ~1 ms when every tree near the player sampled it). Walking through dense shrubs
  ~0.35 ms, a forest view and snow walk within noise. Limbs, not twigs: the kit's hierarchy is
  trunk, primary limb, cards; the trunk stands.
- **Snow on spruces from the kit, where the twigs are known.** *Kept* (October 22). Whitening cards
  in the shader alone painted whole cards: a card is a picture of a spray, its normals face the
  camera or the crown, and its sky visibility is one value. The kit bakes how deep snow lies on
  each atlas texel (ridges along the photographed shoots, deeper where they crowd, ranked so a
  threshold covers that share of needles) into the foliage ORM's unused blue; a card covers the
  texels past a threshold set by its sky visibility and tilt, bridging the gaps between needles,
  and from below shows snow only in them. Snow loads are VDB unions of particles over each bough
  card's white texels, open below and moving with their card; bark takes snow on its upper sides.
  `spruce_snow` is the forest spruce with all three and its own impostor; the planner grows it
  alone on snow ground, which the rest of the forest keeps open. Heightfield loads over each card
  read as paper sheets and are gone.
- **Far snowy foliage reads a filtered cover, not a filtered depth.** *Kept* (October 22). Spruces
  were snowy as impostors, green from about 75 m to 170 m, and snowy again near: thresholding a
  minified depth averages it under every threshold (the bake samples full resolution, so the
  impostor kept its snow). The ORM's alpha holds the share snow covers at a whole amount, box
  filtered; where the texture is minified the shader reads it, scaled to the card's amount, and
  up close the depth's crisp threshold. Depth mips keeping each tile's distribution did not help
  alone (filtering between texels still averages), and the impostor's views from 8° up were not
  the cause. Past 12–35 m a card shows all its snow from either side, as the impostor, seen from
  further, bakes it, and the far LOD carries coarse loads.
- **Snow falls when its bough moves, limb by limb.** *Kept* (October 22). Each snowy tree in the
  GPU trees' pool holds a state per primary limb (when it shed, how fast its bough moved), for
  as long as its batch stays in the pool. A compute pass reads the touch field over each nearby
  limb's bough, across the width its foliage spreads: pushed past 6 cm, the limb sheds. Its
  needle and bark snow slides off over a third of a second. Its loads, broken by the kit into
  chunks of about 15 cm (Voronoi cells of the surface, each vertex carrying its chunk's middle
  and a seed), slide off with the bough's motion, drop against the air's drag and crumble within
  about half a second into the clumps and powder the ambient particles carry on, born along the
  chunks' way. The bough, unloaded, springs up to its pose 7° higher for a whole load, past an
  underdamped wobble, through the same limb turn the touch field drives. Vertices find their limb
  by the number the kit writes in their second UV's y. Chunks are their load's open top surface:
  tumbling showed them as paper shards, so they turn at most 2 rad/s and break up early. *Rejected first:* the CPU guessing
  contact from a per-form outline of the snow against a widened body, logged into a 64 m map:
  snow fell before anything was touched, from the body's whole height, and the bursts started
  round the body rather than on the boughs. Reading the field only along each limb's axis
  missed most pushes: a body parts a bough's sprays, rarely its middle. Powder as large puffs
  read as smoke under the crowns; it is fine flakes lit with the snow ground's bounce.

## Atmosphere & weather

- **Snow is where the weather falls, not a weather of its own.** *Kept* (October 22). The island
  keeps one sequence of clouds, wind and storminess; the mountains have their own, snowing about
  four fifths of the time from flurries to a blizzard with clear spells, blended in by the share
  of snow ground within three cells of the camera. Once that share passes 0.6 its precipitation
  falls as snow (back to rain below 0.4, changing over 4 s: rain and snow falling together read
  as a fault, user October 22): flakes
  as two boxes of ambient particles (lit by the sun's shadow maps and the sky, with the snow
  ground's light thrown back up), no rain, splashes or wet ground, and hardly any lightning. The
  weather fog over snow takes the ground's light too, so in a blizzard ground and sky meet
  without a horizon. A sunny valley can lie under a snowing mountain. Held weather
  (`--weather`, F1) holds in both. A blizzard is about 120 m of visibility, the nearest trees
  faint shapes (user: 480 m did not feel like one, 50 m hid every tree). No Houdini: falling snow
  is endless and wind-driven, which particles and fog do at runtime; baked effects wait for
  one-off set pieces.

- **Bevy's static atmosphere tables are built once.** *Kept* (October 26). Bevy rebuilds its four
  tables every frame; the transmittance and multiple-scattering ones depend on the atmosphere and
  its medium alone. `atmosphere::tables` zeroes their dispatch sizes in the render world's
  settings on frames where neither those nor their textures changed (the shaders keep the
  uniform's), an adapter on Bevy 0.20's pass like the sky pass swap. The sky-view and
  aerial-view tables follow the view every frame; the aerial one, 1,024 threads each walking 64
  samples, keeps the pass's span. GPU busy at Maximum: meadow 9.16 → 9.07 ms, start 10.12 →
  10.03, forest-pine 9.86 → 9.82, storm-road 9.41 → 9.25, mountain-hamlet 9.22 → 9.12,
  brook-mouth 8.83 → 8.63. Captures unchanged.
- **Blowing snow's ground fields are worked out once a frame.** *Kept* (October 26). Measured with
  `--profile-drift off`, blowing snow cost 0.60 ms of the storm road's frame and 0.69 of the
  mountain hamlet's: every pixel's seven stretches each sampled the ground map three times and
  the sheets' noise once, and hashed two octaves of gusts. All of that depends on the ground
  alone, so a pass fills it into a field over the ground map's texels (0.015 ms), and each
  stretch reads the field and the ground's height; the height profile stays exact per ray. GPU
  busy at Maximum from `40ab1c1`: storm-road 9.72 → 9.41 ms, mountain-hamlet 9.67 → 9.22, meadow
  9.30 → 9.16 (no snow; noise). Captures match the baseline (mean difference 1.2/255 on the storm
  road, all of it falling flakes and the figure's pose).
- **Blowing snow is a medium of the fog, driven by one pattern of gusts.** *Kept* (October 22,
  after the user's photo of a ground blizzard). Snow lifts off snow ground where the air near it
  runs past 3 m/s and blows in full from 6.5: in a blizzard (wind 2.3, about 6 m/s) it always
  blows and the gusts carried downwind thicken it in passing sheets; in steady snow it lifts only
  in the strongest gusts. It is integrated in the sky composite with the haze, mist and weather
  fog: a layer 0.4 m deep under a thin veil, over the snow's top as the ground albedo map holds it
  (the mist map's 16 m ground floated it at chest height), in broad smooth sheets carried with
  the wind, cut at the same distances on every ray. It is the snow lifted and lit as the snow
  ground is (the materials' sky light, the sun past the terrain), so it matches the snow and
  shows by hiding what lies in it. *Rejected*: fine streaks ("feels more like a noise", dark
  stripes) and light taken from the weather fog, which greyed the snow. The gusts are one frozen
  pattern evaluated alike on the CPU and GPU, so a sheet that passes the camera also hurries its
  snowflakes, and swells the wind of the grass and trees under it: each keeps its own motion
  (the grass's rolling patches, the trees' sway), scaled by the gust where it stands.

- **Snowfall fills prints and lays snow back on shaken boughs.** *Kept* (October 22). One total
  of snow fallen on the mountains (`GameWeather::snowfall`, 1.2 mm/s at their heaviest snowfall,
  in proportion at lighter; a third of that filled too slowly, user) drives both: a print is
  drawn as much shallower as has fallen since it was made (a footprint fills in about a minute
  of a blizzard, a trench in five), and a shaken limb's needles whiten, its loads build up where
  they lay and its bough sinks again until 0.3 m has fallen; it sheds again from 0.3 of that. The press log stamps each
  press with the total, so the window refilling ground it moves onto draws prints as filled as
  they are by now, without any per-texel time; tiles holding prints are brought up to date a
  budget at a time while snow falls.

- **Blizzards frost snow country's bare ground, near the camera only.** *Kept* (October 22,
  user: bare patches "during the strong storm" looked unrealistic). In the mountains' blizzards
  (snowfall past 0.85 of theirs) fresh snow settles over about 90 s wherever the camera is and
  blows off over about 4 minutes after, one weather value the shaders read. It frosts the bare
  ground of snow country, where the ground map's 32 m layer is mostly snow: a whitening of 45 to
  85% over metres that the ground's texture shows through, grass at its tips, fading out from 60
  to 140 m so the land seen from afar keeps its look. The drift blows over those patches too.
  *Rejected* (`c9c1a61`): fresh snow in any snowfall, thawing over ten minutes, covering the
  ground from its hollows up with the highest stones bare: the mountains, snowing most of the
  time, lost their texture from afar, and the stones at patch edges drew dark outlines.

- **A moving moon, not a fixed giant planet.** *Planet rejected* (October 11, `feat/planet`,
  `5a35e8f`): "a moving moon is better". Fix flat dusks inside the moon model; measure key/fill
  (liked 0.5–10, flat 0–0.1).
- **One sun above the atmosphere; exposure from light on open ground.** *Kept* (`14c0f0d`). Authored
  per-phase suns reddened twice (purple dusk); exposure by sun elevation left dusk darker than
  night.
- **Haze 30 km thinning every 250 m, 60 km visibility, valley mist, no sea mist.** *Kept.* Thicker
  haze greyed the sea, 20 km left sunsets grey, and sea mist drew bright bands along shores.
- **Clouds from a 3072×768 horizon panorama, refreshed once a second.** *Kept.* A 512²
  stereographic cache made a diagonal sawtooth, and 256 rows blurred billows into blobs.
  Refreshes cross-fade, and the composite carries each image with the wind by the clouds' stored
  distance, so a slow refresh still moves smoothly; shapes change by region; High quality stops
  at 80 steps.
- **Cloud shape and light, after Nubis.** *Kept* (`b1bc9cf`). GPU noise volumes cut by a
  cumulus or deck height profile, the detail eroding only the outline (billows above, wisps
  below). Multiple scattering carries most of the light (white sides, grey bases). Texels keep
  light per unit of sun and moon, which the composite colours, so dusk tints clouds as fast as
  the sky. Cloud-passed light is diffuse and shadowless; the beam fades with (1 − overcast)².
  *Rejected:* skyboxes and cards (no moon, lightning, regional weather or matching shadows); the
  slab clouds (flat grey, hard-halved at sunset); distance LODs of light steps and detail (rings
  in the overcast deck, no measurable saving).
- **Shafts from air under crowns; screen-space sun rays.** *Kept.* Haze alone was too faint; rays
  add ≤0.3 ms.
- **Ambient particles keep to shade.** *Kept.* Real-size motes were invisible; in open sky they read
  as stars.
- **A transparent sea without a mesh.** *Kept.* Terrain continues as a seabed; foam follows ground
  slope (by depth it covered tens of metres); waves arrive from deep water.
- **Rain shelter from a top-down map of placed objects.** *Kept.* Rain, splashes and puddles stop
  under objects; splashes are flat ripples, not upright crowns.

## World data & cook

- **No migrations: worlds, bundles and saves are rebuilt.** *Kept.* Formats change with a version
  bump; `world-cook migrate` and old readers are gone, and the schema version covers every blob
  (`a200449`). Only Reteya v13's inputs and route must survive.
- **Reteya is the one world.** *Kept* (`d6e8c8f`). Inputs `content/reteya` and
  `assets/local/worlds/reteya-v13`, Houdini source `reteya_detail_v13.hiplc`; `build_world.sh`, then
  `install_world.sh`.
- **Cook cells directly; outputs decide a new generation.** *Kept* (`9069bc0`, `d184e72`). Content
  hashes cover outputs, not source revisions; imports take `--no-cook`, so a build cooks once.
- **Incremental cooks equal `--full`.** *Kept.* Only changed fingerprints recompile. Road and rut
  code changes need `cook --full` (check: incremental cooks kept old wear fields on October 20).
- **One codec; no per-LOD fields.** *Kept* (`a200449`, `ffb49d3`). One bincode configuration with a
  trailing-byte check; LOD keys and geometric errors went with the old renderer.
- **Control tiles replace composites.** *Kept.* Parents filter shares, never downsampled indices;
  Reteya's tiles cook in 5.7 s.
- **Far-object blocks hold every tree with an impostor.** *Kept.* 512 m blocks to 2,048 m carry mesh
  LODs, so trees ignore the 192 m object residency.
- **Gameplay areas publish outside the content hash.** *Kept.* Repainting an area never recooks
  terrain.

## Editor

- **The editor draws what the game draws.** *Kept* (`6e5ab71`, `392d819`, `9cf8c56`). v2 terrain
  with live overlays (no preview bake), a one-node clipmap for presets, GPU trees; hiding a cell
  entity hides its instance.
- **Publish re-reads in place.** *Kept.* v2 keeps what it draws until nodes are read again; only a
  new extent or textures reload.
- **Retired tools stay retired.** *Kept* (October 10). Grass study workspace, legacy terrain
  preview, placeholder previews, old journal schemas; recover from `7208dd6` only with a purpose.
- **One owner per piece of editor state.** *Kept* (`b0e140b`, `6d27856`, `aabb56c`). One
  `SourceActivity`, one `SaveChannel` per domain, the road tool's mode as one enum.

## Gameplay & content

- **The playthrough lives in memory.** *Kept.* A SQLite working store with budgets and leases was
  rejected as over-engineered: state is under 2 MB, three item commands against 20,000 actors
  ~0.1 ms. Journaled commands, snapshot saves, graphs on demand.
- **Content names are identities.** *Kept.* A name hashes to the runtime's 16 bytes (`X::named`);
  renaming changes identity.
- **Luau through the built-in actions.** *Kept.* Typed and sandboxed; effects run as `Action`s in
  the journal. Script only what built-in rules express badly.
- **Flat dialogue graphs; event-driven triggers.** *Kept.* BG3/Obsidian-style nodes; triggers fire
  on journal signals, nothing polls, a command finishes what it caused (256-step guard).
- **Rules as data, formulas in Luau, actions in time.** *Kept.* Derived stats stored and read by
  lookup; durations and cooldowns, no turns, so real-time or paused-queue combat stays open.
- **Limits only against real failure.** *Kept.* Content count caps went after one could lock the
  game; runaway-work and memory guards stay.
- **Saves survive content edits.** *Kept* (`6e76e57`). Loads recompute stats and rebuild indexes,
  refusing only what no longer fits.
- **Authored content apart from runtime content.** *Kept* (`0c547f6`). Validate at publication and
  at bundle open; one bundle row per graph or text contract.
- **Cloth (branch `cloth-lab`, paused).** *Rejected as fixes:* a narrower cape (±110° → ±95°), which
  limits garment design (fix the mechanism), and stepping distant cloth every other frame (39–45 mm
  shake). Resume from compromises.

## Performance & measurement method

- **A frame budget of 10.5 ms GPU at the play setup, measured at Maximum.** *Kept* (October 25).
  Its lines, one per group of passes, are in `tools/frame_budget.toml`; `tools/bench.py` measures
  six views against them and keeps the history ([Workflows](WORKFLOWS.md#benchmark-and-frame-budget)).
  Runs repeat only at the GPU's Maximum state, but minutes of play hold it at Medium (1.2–1.4× as
  long), so the budget is what holds 60 fps there, about 14 ms (user: Village, run ten minutes,
  was at Medium too and must be the target in either case). The play setup's 1728×1084 is about
  what a PS5 renders at 50% of 4K.
- **The target is PS5-class hardware at 60 fps; the MacBook is where the work happens.** *Kept*
  (user, October 23). Low settings for phones and handhelds (iPhone, Switch 2, Steam Deck) may
  follow. Optimise real work wherever measuring finds it; a drop that is only the Mac's GPU clock
  is not a goal in itself.
- **A Mac lowers its GPU's performance state after minutes of near-full load.** *Kept* (October
  23). Jogging from the start into the meadow before the gorge, every pass took twice as long,
  fixed-size ones too, with the GPU in its Medium state the whole time. Standing there from a
  fresh start it stayed at Maximum, 12.5 ms a frame. Streaming CPU (1.6 against 1.4 cores),
  memory and the frame's work were unchanged. Read the performance state before chasing a drop;
  headroom below the frame budget is the lever.
- **The Metal HUD's GPU time is not a cost.** *Kept.* macOS clocks the GPU to the load (light scenes
  ran passes 2× slower). Budget in clock-free counts; time passes in Metal System Trace against a
  reference pass.
- **Alternate A/B runs and mind the heat.** *Kept.* Runs sag 5–30% as the machine heats; a baseline
  binary needs its own assets root and database; no traces while Houdini runs.
- **Measure the play setup.** *Kept.* Day clock and weather on, fullscreen 50% + Temporal
  (1728×1084): frozen-clock routes missed the terrain-shadow drops.
- **A big shader's registers decide its speed.** *Kept* (October 23). Each call site of a function
  is inlined, so a heavy one called from five places costs five copies' registers. Look for spills
  first (`--spills` in `tools/summarize_gpu_intervals.py`), then give heavy work one call site in a
  loop, or a draw or pass of its own.
- **Markers and stubs mislead.** *Caveat.* GPU markers undercount Temporal; stubbing part of a big
  shader shrinks it, so stubs overstate (the shaft readback rewrite won 3.5%).
- **Name slow frames first.** *Kept.* `YARRA_SPIKE_LOG` names systems and pipelines; never run
  all-core work inside a running game.
- **Shader pre-warm.** *Not built.* Pipelines compile synchronously but only while loading; WESL
  raised startup to 310 ms (check: GPU trees, the stated trigger, have arrived).
- **Rewrite weak foundations, don't patch.** *Kept.* Study shipped engines, rewrite in gated steps
  (as terrain v2 did); early 120 fps micro-optimisations are history.
- **Equivalence gates for structural changes.** *Kept.* Shader checker, look-capture views within
  noise, identical instance counts. Say what a long run answers before starting it.
