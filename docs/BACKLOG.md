# Backlog

Open work only, by area. What was decided and why is in [Decisions](DECISIONS.md); how the code
is laid out is in [Architecture](ARCHITECTURE.md). Remove an item when it is done; record a
decision when one is made.

## Bugs

- **Distant terrain shadows usually do not show.** The GPU horizon search runs once per relief and
  light direction and completes, but the map it wrote is then overwritten in place (most likely
  the image's CPU data, all "open", uploaded again into the same texture), and since the texture
  keeps its identity the search counts as done and never runs again. Runs where the overwrite
  lands before the search ends (seen under heavy CPU load) do show the ridges' shadows; keeping
  the search running shows them every time (`central-far` at phase 0.34). Present before the
  October 20 cleanup. Fix: keep the map's results out of the CPU image (e.g. a GPU-only texture
  the search owns, bound in its place), or rerun the search when the image is uploaded again.

## Look and content

- **Trees.** Pines in the vegetation kit, in small steps from the longleaf forms towards Scots pine
  (shorter upturned tufts, brighter yellow-green, more clumps), each judged in the game against
  photos. Bare birch loses wood at LOD2 (coverage 0.044 → 0.025; cards cannot fix it). The fill
  cost of the grown LOD1/LOD2 cards is unmeasured. `--tree-shadow-lod 1` saves a third of the
  cascades but lightens forests 5–7%: the default waits for a look verdict.
- **Grass to the distance.** A fade band instead of the line at 96 m; ground beyond shaded with the
  grass field's measured appearance and patch pattern; a cheaper far tier, its range chosen by
  measurement. Grass shadows. The per-page grass cover map (`VegetationFieldPageData::cover_map`)
  is not drawn by terrain v2.
- **Roads.** The wear brush and per-junction toggles in the editor; weeds (waiting for a plantain or
  clover rosette in the kit); objects on junction islands; a performance pass once the look holds.
- **Forest planting.** `forest_plan.py` v2: edges, young trees and understory.
- **Terrain.** Dual-scale tiling and slope snow, if a view shows the need; grazing-angle normal
  aliasing.
- **Snow.** Deep snow: finer CDLOD levels inside the print windows so legs sink into displaced
  ground, with slower walking; prints filling in while it snows; snowfall above a snow line, its
  fog and blizzards; snow cover building up. The ground light under every material assumes grass
  albedo, so characters and trees on snowfields lack the snow's bounce. Sand and mud could take
  prints with a small `soft_depth`.

## Performance

Measure the play setup (native fullscreen, 50% + MetalFX Temporal, day clock on) with work counts
and Metal System Trace, never the HUD's GPU time ([Workflows](WORKFLOWS.md#captures-and-measurement)).

- **Sky composite.** One large pass for sky, sea, fog, aerial perspective, shafts and glare (about
  5 ms at native in the forest). Split it into smaller passes, or run the air at half resolution
  per pixel block's nearest and farthest depth.
- **MetalFX Temporal** costs about 4 ms, most of it MetalFX's own; nothing on our side found.
  Dynamic resolution (render scale held to the frame budget) would hold 60 fps as the machine
  heats.
- **Shader pre-warm.** WESL pipelines compile synchronously on first use (startup about 310 ms; a
  variant first needed mid-game stalls its frame). Compile the known variants while the world
  loads. Startup logs transient "module not found" errors while imports register; harmless, noisy.
- **Tree cascades** are mostly vertex work: compressed vertex attributes are a candidate after a
  trace shows the vertex-fetch limiter. Whether deep-forest trees may stop casting into the far
  cascade (leaving it to the forest shadow map) is to be judged on the image.
- **Terrain shading at native** (2.8–4.3 ms, mostly lighting).

## Platforms

- **iOS.** Terrain v2 sizes (512² clipmap levels are set, not measured on a device); packaging of
  the world as downloadable content; BC7/ASTC textures instead of transcoding UASTC at load.

## Code

- **Tree renderer crate.** Trees are a full renderer inside `engine` (`engine::trees`, about
  4,500 lines). A `tree_render` crate like `terrain_render` and `vegetation_render` needs
  `LodProjection` and the far-object batches to move with it; do it when the next tree work
  touches those boundaries.
- **Shared GPU helpers.** Growable storage buffers and readbacks are written twice (the GPU
  trees' `Growing`, the grass renderer's buffers); the particle renderers (rain, ambient
  particles) are near-twins. Extract when a third user appears.
- **Shaders.** Random and noise helpers differ per shader (`vegetation/random.wesl`,
  `terrain/stochastic.wesl`, `terrain/wear.wesl`, `weather/*`, `sky/celestial.wesl`); merging them
  changes patterns, so do it with a look pass. The grass blade wind (`vegetation/blade.wesl`) is not
  tested against the CPU wind as the tree wind is. Long functions worth splitting when next edited:
  `sky/composite.wesl` (fragment, ~190 lines), `tree/lod.wesl`, `weather/ambient.wesl`,
  `terrain/clipmap.wesl`, `water/sea.wesl`.
- **World data.** Heights are stored twice (cell page and level-0 node); the engine reads page
  heights for grass and contact, so dropping them needs engine work. The project and runtime
  schemas repeat the world-space and terrain-catalog tables; the stores' `invalid()` helpers map to
  unrelated error variants; `import_roads` and the road `apply_transaction` are long; the 32/128 MiB
  root budgets are unnamed; two generation-id formulas exist. After a change to compiler code
  (roads, ruts, scatter), cook with `--full`: incremental cooks follow source revisions.
- **Gameplay.** Triggers subscribe to signals with concrete actor ids, so "any party member
  enters" needs one subscription per actor. Each Luau call reloads its module and registers its
  API; measure µs per condition before conditions multiply. `InventoryError` overlaps `Rejection`.
  `Step` (scenarios) mirrors `Command`. Shared test fixtures for the gameplay crates. The story
  adapter (`app_game::story`) becomes app-side plugins (actor index, commands in, events out, HUD)
  before companions or combat reach the engine.
- **Editor.** Atmospheres, areas and roads keep working sets of their own shapes; share more only
  if a fourth domain repeats one. The live terrain preview lives inside environment paint.
