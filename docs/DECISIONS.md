# Decisions

Approaches kept and approaches rejected, one entry each, so nobody re-tries a rejected idea or
undoes a kept one without knowing why. Current behaviour is in [Architecture](ARCHITECTURE.md); the
step-by-step ledgers this replaces (`EXPERIMENTS.md`, `REFACTORING.md`) are in Git history.

State on 2026-10-20: terrain v2 only (GPU clipmaps, per-pixel materials); Reteya is the one world;
trees come only from the YarraVegetation kit and are drawn on the GPU in game and editor.

## Rendering & presentation

- **Look target: 50% render scale + MetalFX Temporal at 60 fps.** *Kept* (user, October 9 and 17).
  Native is for faster machines; MSAA is off under Temporal, so MSAA-based ideas do not help. The
  60 fps cap is the default (`cb74dda`); the default upscaler stays `Auto` (MetalFX Spatial):
  Temporal is chosen in F1 or with `--upscaler metalfx-temporal`.
- **No feature cuts as optimisations.** *Kept.* MSAA, the AA modes and shadows are supported
  options; turning them off (MSAA ~2 ms, shadows ~2.2 ms at native) is a visual choice, not a fix.
  Optimise real work or accept it.
- **Pacing by display link plus Metal's minimum presentation interval.** *Kept.* 99.7% of intervals
  on time against 81% with a timer; the Metal interval alone and higher thread QoS were rejected.
  Keep the wgpu-hal patch (`third_party/wgpu-hal/YARRA_PATCH.md`) until upstream has it.
- **A light depth prepass, only for Temporal.** *Kept* (`3bc9be4`). Nothing else reads it (~1 ms
  facing a forest); terrain and impostors leave it and `CompleteTemporalMotion` writes camera motion
  from depth. A foliage or global prepass against overdraw was rejected twice (+0.3 ms GPU, +0.7 ms
  render prep).
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
  NaN streaks. Splitting it into smaller passes is open.
- **Composite shortcuts.** *Kept* (`f83773f`, `189488a`). Air at a silhouette pixel's nearest and
  farthest samples, interpolated; fog light once per pixel; dark lights skipped (~1 ms). A froxel
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
  grass shadows, a tier past 96 m, per-instance blade preparation.
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
- **GPU-driven trees in game and editor.** *Kept* (`2f52071`, `9cf8c56`). One pool from far-object
  blocks; compute picks LODs, fades and culls into indirect draws. Main thread −1.5–2 ms in forests
  at GPU parity. Culling bounds must match per primitive: a looser sphere cost up to 1 ms.
- **Timed dissolves, not distance crossfades.** *Kept.* Switch 4% past the band, dissolve over
  0.6 s, MSAA samples split (`crossfade.wesl`), shadows doubled mid-fade. Distance crossfades drew
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

- **A moving moon, not a fixed giant planet.** *Planet rejected* (October 11, `feat/planet`,
  `5a35e8f`): "a moving moon is better". Fix flat dusks inside the moon model; measure key/fill
  (liked 0.5–10, flat 0–0.1).
- **One sun above the atmosphere; exposure from light on open ground.** *Kept* (`14c0f0d`). Authored
  per-phase suns reddened twice (purple dusk); exposure by sun elevation left dusk darker than
  night.
- **Haze 30 km thinning every 250 m, 60 km visibility, valley mist, no sea mist.** *Kept.* Thicker
  haze greyed the sea, 20 km left sunsets grey, and sea mist drew bright bands along shores.
- **Clouds from a 2048×256 horizon panorama.** *Kept.* A 512² stereographic cache made a diagonal
  sawtooth; refreshes cross-fade, shapes change by region, High quality stops at 80 steps.
- **Cloud shape and light.** *Kept.* Half-rate vertical noise (no lenses), two-stream diffusion
  (grey, not black, bases). Cloud-passed light is diffuse and shadowless; the beam fades with
  (1 − overcast)².
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

- **The Metal HUD's GPU time is not a cost.** *Kept.* macOS clocks the GPU to the load (light scenes
  ran passes 2× slower). Budget in clock-free counts; time passes in Metal System Trace against a
  reference pass.
- **Alternate A/B runs and mind the heat.** *Kept.* Runs sag 5–30% as the machine heats; a baseline
  binary needs its own assets root and database; no traces while Houdini runs.
- **Measure the play setup.** *Kept.* Day clock and weather on, fullscreen 50% + Temporal
  (1728×1084): frozen-clock routes missed the terrain-shadow drops.
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
