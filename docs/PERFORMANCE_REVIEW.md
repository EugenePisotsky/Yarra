# Performance review: what is structurally wrong

A review made on October 24, after the game was compared with Resident Evil Village on the same
Mac. It records why weeks of optimisation have not changed the frame rate the player feels, how
shipped games solve the same problems, and which structural changes would. It holds findings and
proposals, not decisions: when one is decided it moves into [Decisions](DECISIONS.md) or
[Backlog](BACKLOG.md), and this file shrinks until it can be deleted.

Five research passes produced it (terrain, vegetation, sky and lighting, frame structure, how the
game is developed), each reading the code, the history and published talks. Claims about our code
were checked against the files; claims about other games cite a source, and estimates say so.

## The comparison

| | Frame rate | What is on screen |
| --- | ---: | --- |
| Resident Evil Village, night snow forest | 123 fps | About a dozen bare trees, fog and darkness closing the view at 20–30 m |
| Resident Evil Village, house interior | 211 fps | An enclosed hallway |
| Yarra, meadow, no vegetation, no objects | ~100 fps | Terrain and sky only |

All at 1920×1080 on the M2 Max; Village's upscaling and anti-aliasing settings are not yet known.
On PS5, Village runs at 60 fps at 4K checkerboard, about 4.1 Mpx shaded a frame (Digital Foundry).
Its forest is not a fair comparison for ours, since its level design limits what is drawn. But
an empty terrain frame costing as much as Village's whole forest frame is a fair indictment.

### Where an empty frame goes

Meadow, terrain and sky only, 100% 1920×1080, MSAA 4×, no frame cap; Metal System Trace, GPU
busy time per frame at full clock. The GPU was busy 95% of the time, so the frame is GPU-bound;
CPU main ~2.5 ms, render prep ~1.8 ms.

| Pass | ms |
| --- | ---: |
| Main opaque pass, all fragment work: the terrain shader | 4.67 |
| Sky composite | 1.35 |
| Atmosphere LUTs | 0.49 |
| Light shafts, particles, bloom, sun occlusion, clouds, shadows, UI, rest | ~1.7 |
| **GPU busy** | **8.2** |

1920×1080 is about the pixel count of the play setup (50% of 3456×2168 = 1728×1084), so this
cost is in every view before a tree, blade or rock is drawn.

### The views that matter

| View (play setup unless noted) | GPU busy |
| --- | ---: |
| Empty meadow (100% 1080p) | 8.2 ms |
| Meadow with grass | 11.7 ms |
| Pine forest | 15.3 ms |
| Storm spruce road (23.0 before this week's wind work) | 18.4 ms |

60 fps on the Mac needs about 14 ms busy (about 2.6 ms of every frame is idle, 1.4 of it waiting on
MetalFX), and views above ~12.5 ms push the Mac into its Medium GPU clock. The 60 fps cap shows
only 60, 40 or 30 fps, so a 1–2 ms win in an 18 ms view changes nothing the player sees.

## The short answer

1. **There is no frame budget, so nothing is ever over budget.** Every gate is relative ("within
   noise", "faster than the old path"), features land in shared hot shaders unpriced, and
   performance work starts when the frame rate is felt to drop.
2. **Nothing is cached or spread over frames.** The terrain re-derives its material for every
   pixel every frame; each tree vertex is posed four to six times a frame; every shadow cascade is
   redrawn every frame; fog, mist and aerial perspective are integrated per pixel at full
   resolution. Shipped open worlds are built around caches and low-resolution volumes refreshed a
   little at a time.
3. **Every feature and all the lighting live inside one forward shader per material.** These
   uber-shaders are limited by registers and memory latency, not arithmetic, and they run again for
   the quad lanes and edges of triangles smaller than a pixel.
4. **The content is over its own budgets and badly packed.** Tree meshes reach the GPU as triangle
   soup (three vertices per triangle), with 104–164-byte vertices, at three to five times the
   vegetation kit's triangle ranges; terrain keeps 1 m triangles to 192 m.
5. **Nothing limits what is drawn or how long a frame takes.** Culling is frustum and distance only,
   the render scale is fixed, and there are no quality tiers.
6. **We measure on a machine unlike the target, with noise larger than a feature's cost.** Apple's
   tile GPU hides overdraw and alpha-test costs a PS5 would pay; heat and clock states move results
   5–30%, more than most features cost; no history is kept.

Bevy and wgpu overhead are not the cause: Bevy's own passes are about 1 ms of the empty frame, and
the CPU is not the limit. The techniques are.

## Findings by area

### Terrain

- **What runs per pixel** (`assets/shaders/terrain/clipmap.wesl:362-543`): residency lookups through
  the state texture (four loops of loads, `clipmap_common.wesl:72-105`); four control loads and a
  ranking of nine candidate surfaces; up to three surfaces sampled one after another (the second and
  third wait for the first's blend height), each with three stochastic samples, a side sample on
  steep ground and a normal map near the camera; three macro-variation samples; road wear and rut
  parallax within 56 m; prints within 100 m; frost noise everywhere; in rain, 52 height loads for
  hollows. Then lighting: Bevy's clustered loops, a 9-tap sun shadow (and a second cascade in the
  overlaps), `ground_in_light`, cloud shadow, the forest shadow march and terrain horizon beyond the
  cascades, transmittance, and ambient terms that fetch sky visibility, bounce and forest sky again.
  About 35–50 fetches a pixel along 7–9 dependent memory round trips, half of them in lighting.
- **The meadow's surfaces already take the cheap path** (one colour sample under the grass field's
  tint), so its 4.67 ms is mostly lighting, decode, memory latency and small-triangle shading, not
  texture splatting. A material cache saves less in the meadow than on roads, rock and forest floor.
- **Shading is tied to the triangles.** The 1 m level is drawn to 192 m (`LEAF_RANGE`,
  `crates/engine/src/world_streaming/terrain_clipmap.rs:58`); past ~50 m at eye height its quads are
  under a pixel tall, so several triangles' 2×2 quads shade each pixel, under MSAA up to four.
- **Nothing is reused across frames**, although the ranking, blend, anti-tiling, macro variation and
  static wear do not depend on the view. `ground_albedo.wesl` already fills a camera-centred map
  every frame from the same decode: a precedent for caching in texture space.
- **The stated budget was never checked.** The terrain v2 plan said "about 2 ms GPU at the look
  target, to be measured" (`git show 1a0f0d7:docs/REFACTORING.md`); its gate was passed by
  comparing with the old path (105 against 91 fps), and the figure was lost when the ledgers were
  retired.
- **How shipped games do it.** Battlefield 3 caches terrain splatting in a procedural virtual
  texture (Widmark, GDC 2012). Far Cry 4's adaptive virtual textures hold 10 texels/cm near the
  camera over a 10 km world, with decals baked in and page rendering spread over frames (Ka Chen,
  GDC 2015). Unreal's Runtime Virtual Texturing exists to cache landscape shading. Lighting runs
  once per pixel in deferred engines (Battlefield 3 tiled deferred; Horizon Forbidden West
  deferred texturing). Our prints follow shipped deformable-snow designs and are not the problem.

### Vegetation

- **Tree meshes are triangle soup.** The kit exporter computes one tangent per face and copies it to
  the face's three corners (`~/Dev/Assets/vegetation/scripts/vegetation_export.py`, `tangents`); the
  weld that follows matches whole vertex records, so the per-face tangent stops nearly every merge.
  Checked in the game's files: `spruce_snow.glb` has 42,694 vertices for 14,247 triangles,
  `spruce_forest.glb` 13,509 for 4,516; nothing in the cook or loader welds them again. An offline
  vertex-cache simulation without the per-face tangent needs 2.0–3.0× fewer vertex-shader
  invocations (snowy LOD0 42,694 → 18,142). That multiplies every pass's vertex work, and no shader
  tuning can recover it.
- **The far mesh band holds most triangles, over the kit's own ranges.** Kit ranges for a forest
  tree: 2,000–4,000 / 600–1,200 / 150–400 (`GOALS.md`, "provisional", written for PS4 and iPhone).
  Plain spruce: 4,516 / 2,248 / 1,326; snowy spruce 14,247 / 4,433 / 1,865, of which snow loads are
  9,731 triangles at LOD0. With switches at 34/69/154 m, trees per band run about 1 : 3 : 16, so
  LOD2 carries about half the triangles (estimate), at about 1.5 covered pixels per triangle at the
  impostor hand-off.
- **Vertices are fat**: all f32, 104 B (bark), 148 B (foliage), 164 B (snow), with per-card and
  per-limb data copied into each vertex. Horizon Forbidden West uses half-precision positions,
  16:16 UVs and 10:10:10:2 normals.
- **The wind runs from scratch per vertex per pass**: twice in the prepass (this frame and the last,
  for motion), once in the main pass, once per cascade the vertex lands in, each the whole trunk →
  limb → card → flutter chain (`assets/shaders/tree/material.wesl:418-474`, `wind.wesl`).
  Horizon Zero Dawn cached object-space positions to avoid repeated transforms and to get last
  frame's positions for motion vectors; UE5's Witcher 4 demo animates foliage by compute rigid
  skinning; SpeedTree lowers wind quality per LOD. Our own grass proved the pattern: posing blades
  once a frame in compute took grass from 4.5 to 1.9 ms (`fd37757`).
- **Every pass draws all it might see.** No occlusion culling; three sun cascades redrawn every frame
  with full wind; impostors (11,200 on the storm road) each blend four views with eight fetches and
  full PBR.
- **Alpha.** The main pass discards foliage again after the depth prepass. Horizon Zero Dawn ran its
  geometry pass with depth-equal and no alpha test; we tried that on October 24 and it cost 0.5 ms
  more on the Mac, because Apple's hidden-surface removal already handles it. On a PS5 (no HSR) the
  result may reverse.
- **Grass** already follows Ghost of Tsushima (compute pose once a frame, opaque blades).

### Sky, atmosphere, lighting, shadows and post

- **The air is integrated per pixel, every frame, at full internal resolution.** The sky composite is
  three full-screen draws (sea, air, fog; `crates/atmosphere/src/sky.rs`), each reloading the depth
  samples and evaluating air and fog at up to three distances, with valley mist a four-step march
  and blowing snow on top. Light shafts march 32 steps through the shadow map at quarter
  resolution; sun rays and sun occlusion add more. The cost scales with pixels: 1.35 ms at 1080p,
  about 5 ms at native in the forest (Backlog).
- **Bevy's atmosphere LUTs are rebuilt every frame at its defaults** (`AtmosphereSettings::default()`,
  `crates/atmosphere/src/lib.rs:600`): a 400×200 sky-view table where Unreal's reference uses
  200×100. Hillaire's paper (EGSR 2020) puts all four tables at 0.17 ms and the on-screen sky and
  aerial-perspective apply at 0.14 ms on a GTX 1080; ours are 0.49 ms of tables alone.
- **Sun visibility is recomputed inside every material.** The 9-tap cascade filter (twice in cascade
  overlaps), `ground_in_light` (1–5 more taps), the 8-step forest march, terrain horizon, cloud beam
  and transmittance are inlined into the terrain, tree and impostor shaders through
  `apply_pbr_lighting` (`assets/shaders/lighting/pbr_lighting.wesl`), so they are paid per fragment,
  overdraw and quad lanes included, and they add to each material's registers.
- **Shadows have no caching or update rates.** Three cascades at 20/40/80 m, 2048², all redrawn every
  frame with full wind, for the sun and, near dusk, the moon too; the sun moves continuously, so
  nothing could be cached as things stand.
- **The output end is the largest fixed cost.** MetalFX Temporal ~2 ms plus a ~1.4 ms wait (it is
  submitted as two extra command buffers), then bloom, auto exposure (its histogram over the full
  viewport) and tone mapping at the 7.5 Mpx output size. Clouds are already amortised (a 3072×768
  panorama swept once a second) and cost ~0.1 ms.
- **How shipped games do it.** Ghost of Tsushima: haze in a 128×64×64 froxel volume from 10 cm to
  100 km on async compute, about 0.5 ms on a base PS4; sky tables only resampled for the current sun
  and moon; reflection probes relit one per frame; clouds timesliced over 60 frames (Patry,
  SIGGRAPH 2021 Advances). Assassin's Creed IV froxel fog about 1.1 ms on PS4 including the apply
  (Wronski, 2014). Unreal's forward renderer packs stationary lights' shadows into a screen-space
  shadow mask and evaluates height fog analytically in the base pass. Cyberpunk 2077 redraws up to
  four cascades every frame and four distant levels over several frames, culling casters with the
  main view's occlusion (SIGGRAPH 2021). CryEngine renders far cascades once and recentres them.
  Unreal's virtual shadow maps cache pages, but wind (world-position offset) invalidates them, so
  Epic advises turning foliage wind off with distance. AMD runs bloom and tone mapping after FSR2,
  which costs about 1.7 ms at 4K output on an RX 6700 XT, roughly PS5-class.
- **Before any geometry**, a shipped 60 fps frame spends perhaps 2.5–3.5 ms on atmosphere, air,
  clouds, upscaling and post (estimate from the sources above); ours spends about 5–5.5 ms.

### Frame structure and engine

- **The frame** (every pass every frame unless noted): our compute (grass trample, placement and
  pose; prints; tree LOD and culling; touch and shed; ground albedo; cloud shadows; horizon search
  when the light changes) → Bevy mesh preprocessing, atmosphere LUTs, three sun cascades (three
  moon cascades at night) → depth and motion prepass (forced by Temporal) → main opaque pass →
  `CompleteTemporalMotion` full-screen → grass → transparent, clouds, quarter-resolution shafts →
  sky composite (three full-screen draws) → MetalFX Temporal on its own command buffers → particles,
  bloom, exposure, tone map, UI.
- **Post runs at output size under Temporal.** The world target is window-sized and only the main
  pass is limited to the 50% region (`crates/app_game/src/game_render.rs:232-256`); bloom, exposure
  and tone mapping run after the upscaler on 3456×2168, about 7.5 Mpx. Cost not yet measured.
- **GPU-driven, partly.** Trees, impostors and grass are GPU-culled indirect draws, but wgpu's Metal
  backend turns multi-draw-indirect into a CPU loop of single draws, and there is one queue (no
  async compute).
- **Engine limits.** Bevy's meshlets need 64-bit texture atomics that wgpu enables only on M3 and
  later; wgpu has no mesh shaders on Metal and no sparse textures. **wgpu has no PS5 backend:**
  shipping on the stated target means writing a console backend under NDA or changing engine. That
  is a platform question, not today's performance limit.
- **How shipped engines are shaped.** RE Engine before Dragon's Dogma 2 already used
  multi-draw-indirect with GPU occlusion culling per instance and per 256-triangle section, plus
  artist-placed occluders; Dragon's Dogma 2 and Monster Hunter Wilds moved to a visibility buffer,
  meshlets, two-phase Hi-Z culling and bindless materials (Capcom, REAC 2025). Horizon Forbidden West
  draws foliage into a visibility buffer and shades it in compute on async pipes (McLaren, GDC 2022).
  Cyberpunk 2077 renders near cascades every frame and distant shadow levels over several frames,
  culling casters with the main view's occlusion (SIGGRAPH 2021). Consoles hold 60 fps with
  dynamic resolution and checkerboarding.

### How the game is developed

- **No time budgets exist anywhere.** "budget" appears 159 times in docs, crates, tools and shaders,
  always as a memory, IO or compiler limit. The checks in AGENTS.md cover style and correctness;
  rendering is judged on look. There is no CI.
- **Budgeting was deferred and then lost.** The performance ledger of September 23 said to build
  day and night, weather, impostors and a large area first, "then choose density, visual distance,
  LOD and shadow budgets". That environment existed by about October 9; the budgets were never
  chosen and the ledger was retired on October 20.
- **Features first, recovery later.** Since October 1, 89 feature, fix and tune commits touched
  `assets/shaders`; 25 (28%) quote any cost and none against a budget. Shader source grew from 2,377
  lines on September 1 to 11,247 on October 24. Performance bursts (September 26–29, October 9,
  October 23–24) each followed a report of a felt drop. One example: this week's fix of a 720 B
  register spill (`53c3df2`) undid the snow-chunk pose added two days earlier (`5bc688c`) with no
  cost quoted. The sky composite grew from 0.52 ms (September 29) to 6.3–7.1 ms in a native forest
  view (October 9) over ten days of atmosphere features, weather fog logged as "cost unmeasured".
- **Measurement cannot see a feature.** Heat and clock drift (5–30%) exceed a typical feature
  (0.1–0.4 ms), runs are manual and long, evidence was deleted with the ledgers, and the regression
  scripts live in session scratch. "Within noise" lets everything through.
- **Scope and look were set without cost, and the usual levers are ruled out.** A 10 km world, a
  2 km forest view, weather, snow and a day clock everywhere; reference games that mostly shipped at
  30 fps on base consoles; "no feature cuts" and "no design limits" as standing rules; no quality
  tiers; performance defaults (`--tree-shadow-lod 1`, about 1 ms) waiting weeks for a look verdict.
- **The target moved three times in three weeks** (PS4 and iPhone, then the Mac at 50% + Temporal,
  then PS5) without a budget derived from any of them; the vegetation kit still targets PS4/iPhone.
- **How studios do it.** A target platform and frame budget from pre-production, content budgets per
  area ("Art Direct for 100 MPH", GDC 2020), automated performance captures on every build
  (Activision, "Automated Testing and Profiling for Call of Duty", GDC 2018; Riot's build
  verification), systems reported as fixed small costs (Horizon Zero Dawn's GPU placement: ~250 µs
  for over 100,000 objects, GDC 2017), dynamic resolution as the safety net.

## Directions

Ranked by value for effort. Each starts with a cheap check that confirms or kills it before
anything is built; gains are estimates until that check runs.

1. **A frame budget and an unattended benchmark** (process, days). A budget table for the play setup
   at the Mac's full clock that maps to PS5 60 fps, for example: upscaler, post and UI 2.5 ms; sky,
   air and clouds 1.0; terrain 2.0; trees 2.5; grass 1.5; all shadows 1.5; characters, water and
   particles 0.5; reserve 0.5; 12 ms in all. A `--bench` mode steps the frame clock through a fixed
   set of views (meadow, pine forest, storm road, mountain hamlet, journey, brook mouth), records
   ms per pass normalised to a fixed-size pass plus clock-free counts, appends to a tracked CSV, and
   flags any system over its line. AGENTS.md gains a rule: a change that adds per-pixel,
   per-vertex or per-instance work states its counts and benchmark change against its system's
   line, and a change over the line finds savings in the same system or ships as a quality tier.
2. **Rebuild the tree geometry** (content, days). Weld with per-vertex tangents (cards are planar,
   so the look does not change), order for the vertex cache (meshoptimizer), quantise the vertex to
   ~32–48 B with per-card and per-limb data in a per-form buffer, bring LOD1 and LOD2 to budget
   (LOD2 ≤ ~500 triangles; snow loads at LOD0, coarse at LOD1), and add kit validators for vertices
   per triangle, bytes per vertex and triangles per pixel at the switch distance. Estimate: storm road
   −3.5 to −5 ms, pine forest −2 to −3. *Check first:* weld `spruce_snow` alone and A/B the storm road.
3. **Pose each tree vertex once a frame** (medium). A compute pass writes posed positions (this frame
   and the last) for the instances any view draws, or a limb palette the vertex shader reads; every
   pass fetches instead of recomputing, and the previous pose is read, not recomputed. Estimate
   −1.5 to −3 ms in forests, overlapping direction 2. *Check first:* the rest-pose wind stub on the
   storm road again, now that this week's fixes are in; under 1 ms means skip it.
4. **Shade the terrain once per pixel, then cache its material** (weeks). First a deferred terrain
   pass: the terrain draws depth (and a tag) only, then one full-screen or compute pass reconstructs
   the ground point and runs material and lighting once per pixel with an analytic footprint, split
   into a material kernel and a lighting kernel for occupancy. Then a camera-centred material
   clipmap (a virtual texture without feedback, so turning costs nothing) caching ranking, blend,
   anti-tiling, macro variation, normals and static wear; rut parallax, prints, rain, frost, cliff
   sides and lighting stay per pixel. Estimate: terrain from 4–4.7 to about 1–2 ms. *Check first:*
   Metal GPU counters on the empty meadow's main pass (occupancy and limiters); `LEAF_RANGE` 96 m
   (overshading); MSAA off; stubs that split decode from splat.
5. **Sun visibility once per pixel, and shadows at their own rates** (weeks). A compute pass after
   the depth prepass resolves the cascade filter, `ground_in_light`, forest march, terrain horizon
   and cloud beam into a screen-space mask that materials read with one fetch. Cascades get update
   rates: the near one every frame, the two far ones on alternate frames, the far one with frozen
   wind and lower caster LOD, the sun direction quantised (it moves 0.3°/s) and cascade fits
   texel-snapped. Estimate: mask −0.5 to −1 ms; cadence a third of the cascades' vertex work, about
   −2 ms on the storm road. Cadence means owning the directional shadow path, which Bevy's cascades
   cannot schedule. *Check first:* the per-cascade triangle split in `TREE_GPU_COUNTS` on the storm
   road (under 40% in the far two means skip cadence); the terrain fragment's register count.
6. **Draw less: occlusion** (medium). Hi-Z occlusion in the tree LOD compute from the previous
   frame's depth (terrain included) for meshes and impostors, later for shadow casters. Estimate
   0 to −3 ms by view. *Check first:* a debug count of tree parts whose screen bounds lie behind the
   previous depth on three views; under 15% means skip it.
7. **The air in one froxel volume** (weeks). About 160×90×64 cells, exponential depth to ~2 km,
   reprojected over time, holding aerial perspective, haze, mist, weather fog and the shadowed sun's
   in-scattering; the composite applies it with one fetch. It must replace the shaft march and the
   per-pixel mist and fog, not add to them: the mist-only grid tried in October added a grid and
   removed nothing. Sea and the blowing-snow layer stay analytic. Estimate 1.3–1.6 → 0.5–0.7 ms in
   the play setup, ~5 → under 1.5 ms at native. Atmosphere tables at Unreal's sizes and updated when
   the sun moves enough are a smaller step beside it. *Check first:* the composite at 50% and 100%
   scale; near-linear scaling confirms the slope a volume removes.
8. **The output end and the frame time** (small to medium). Exposure histogram at internal
   resolution or on bloom's first mip; find whether MetalFX's 1.4 ms wait is GPU idle or its own
   work; dynamic resolution (the scaler takes a min/max range and its history survives size
   changes); quality presets (object detail, shadow LOD, grass range) chosen by the budget. Dynamic
   resolution helps fragment-bound views and clock drops, not vertex-bound ones. *Check first:* the
   wait and `auto_exposure` in an existing trace; the critical views at scale 0.42.
9. **Later: a visibility buffer or deferred lighting for every opaque material**, as Horizon
   Forbidden West and RE Engine do. Large; it removes lighting from every material shader and lets
   lighting terms run at lower rates; it gives up the MSAA option (already off under Temporal).
   Only after 2–6, measured; direction 5's mask is its first step.
10. **Platform.** Measure the meadow and forest views on a PC with an RDNA2 GPU (wgpu over Vulkan or
    DX12) to learn which Mac costs transfer, and decide the console path (a backend or another
    engine) before content scale grows further.

## Questions for the user

- May a frame budget override look verdicts, with quality tiers taking the place of the "no feature
  cuts" rule?
- Is dynamic resolution acceptable?
- If deferred or visibility-buffer shading comes, is losing MSAA acceptable (it is off under
  Temporal already)?
- Resident Evil Village's settings (MetalFX mode, resolution, anti-aliasing, "Interlaced") and its
  frame rate in open, daylit areas; a foliage-heavy open-world game on the same Mac (Assassin's
  Creed Shadows, Death Stranding, Cyberpunk 2077) would be a fairer yardstick.

## Sources

- McLaren, "Adventures with Deferred Texturing in Horizon Forbidden West", GDC 2022,
  guerrilla-games.com/read/adventures-with-deferred-texturing-in-horizon-forbidden-west
- Capcom, "RE ENGINE Meshlet Rendering Pipeline", REAC 2025, enginearchitecture.org/downloads/REAC_2025_Capcom.pdf
- Widmark, "Terrain in Battlefield 3", GDC 2012, media.contentapi.ea.com/content/dam/eacom/frostbite/files/gdc12-terrain-in-battlefield3.pdf
- Ka Chen, "Adaptive Virtual Texture Rendering in Far Cry 4", GDC 2015, gdcvault.com/play/1021760
- Epic, Runtime Virtual Texturing, dev.epicgames.com/documentation/en-us/unreal-engine/runtime-virtual-texturing-in-unreal-engine
- van Waveren, "Software Virtual Textures", 2012
- Andersson, "DirectX 11 Rendering in Battlefield 3", GDC 2011
- Fatahalian et al., "Reducing Shading on GPUs using Quad-Fragment Merging", SIGGRAPH 2010
- Karis, "Nanite: A Deep Dive", SIGGRAPH 2021 Advances
- Haar and Aaltonen, "GPU-Driven Rendering Pipelines", SIGGRAPH 2015 Advances
- Dybisz and Witanowski, shadows in Cyberpunk 2077, SIGGRAPH 2021 Talks
- "Large Scale Animated Foliage in The Witcher 4 UE5 Tech Demo", Unreal Fest Stockholm 2025 (secondary summary)
- Wohllaib, "Procedural Grass in Ghost of Tsushima", GDC 2021, gdcvault.com/play/1027214
- van Muijden, "GPU-Based Procedural Placement in Horizon Zero Dawn", GDC 2017
- van Valburg, "Automated Testing and Profiling for Call of Duty", GDC 2018, gdcvault.com/play/1025064
- "Art Direct for 100 MPH", GDC 2020, gdcvault.com/play/1026692 (abstract only)
- Hillaire, "A Scalable and Production Ready Sky and Atmosphere Rendering Technique", EGSR 2020,
  sebh.github.io/publications/egsr2020.pdf
- Patry, "Real-Time Samurai Cinema" (Ghost of Tsushima), SIGGRAPH 2021 Advances,
  advances.realtimerendering.com/s2021/jpatry_advances2021/index.html
- Wronski, "Volumetric Fog" (Assassin's Creed IV), SIGGRAPH/GDC 2014
- Schneider, "The Real-time Volumetric Cloudscapes of Horizon Zero Dawn", SIGGRAPH 2015
- Epic, Forward Shading Renderer and Virtual Shadow Maps documentation, dev.epicgames.com
- CryEngine 3 documentation, cascaded shadow maps (cached far cascades)
- AMD FidelityFX FSR2 documentation (cost tables, post-processing placement),
  github.com/GPUOpen-Effects/FidelityFX-FSR2
- Binks, "Dynamic Resolution Rendering", Intel
- Apple, WWDC20 sessions 10602, 10603, 10631 (Apple GPU architecture, hidden-surface removal, counters)
- Digital Foundry on Resident Evil Village (PS5 4K checkerboard 60 fps) and Horizon Forbidden West
  (1800p checkerboard 60 fps), via secondary reports
