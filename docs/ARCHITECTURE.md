# Architecture

Which crate owns what, how data moves, and the rules that keep it that way. History lives in
git, decisions in [decisions](DECISIONS.md), open work in [backlog](BACKLOG.md), how to run
things in [workflows](WORKFLOWS.md).

## Overview

Yarra is a third-person game (`yarra-app-game`) and a world editor (`yarra-app-editor`), both
Rust on Bevy 0.20, running on macOS and iOS through Metal. There is one world, Reteya, an
island, rebuilt from its inputs (a Houdini heightfield export, roads, packs); see
[its README](../content/reteya/README.md).

**World data.** The editor and importers write a mutable, revisioned SQLite *project* database;
`world_cook` compiles it into an immutable SQLite *runtime* database, one generation at a time;
the engine streams the runtime around the camera into the renderers, actors and tools.

```text
heightfield, roads, packs --(yarra-world-cook import-*)--+
editor --(Save)------------------------------------------+
                                                         v
                      content/world.project.sqlite         source, revisioned
                                 | world_cook::cook_project (Publish, yarra-world-cook cook)
                                 v
                      assets/generated/world.runtime.sqlite   immutable generation
                                 | engine world streaming (read-only worker)
                                 v
         source pages, clipmap nodes, far-object blocks --> renderers, actors, editor tools
```

**Gameplay content.** Rules, items, characters, quests and dialogue are authored as RON, Fluent
and Luau under `content/gameplay/`. `game_content` publishes a project as an immutable SQLite
bundle plus a language pack per locale; a `gameplay::GameSession` runs against it in memory, and
`save` writes slot files. No Bevy: the game uses this only in the opt-in `--story DIR` slice,
the editor not at all.

```text
content/gameplay/<project>/ --LoadedProject::load_directory--> LoadedProject::build,
                                                               build_language_pack
bundle + language packs --ContentRepository::open--> GameSession / Driver --> save slots
```

### Crates and their workspace dependencies

Packages are named `yarra-<crate>` (`cargo run -p yarra-world-cook`); code uses the short
names. In dependency order:

```text
vegetation           -
world                vegetation
environment          world
environment_compile  environment, vegetation, world
world_db             environment, vegetation, world
world_cook           environment, environment_compile, vegetation, world, world_db
game_types           -
gameplay             game_types
localization         game_types
save                 game_types, gameplay
scripting            game_types, gameplay
game_content         game_types, gameplay, localization, save, scripting
upscaling            -
atmosphere           world
terrain_render       atmosphere, world
vegetation_render    atmosphere, upscaling, vegetation
engine               atmosphere, terrain_render, vegetation, vegetation_render, world, world_db
app_game             engine, game_content, game_types, gameplay, localization, save,
                     terrain_render, upscaling (metalfx), vegetation, vegetation_render, world
app_editor           atmosphere, engine, environment, environment_compile, terrain_render,
                     vegetation, vegetation_render, world, world_cook, world_db
shader_check         -   dev-dependency of atmosphere, engine, terrain_render, vegetation_render
rust_tidy            -   tools/rust_tidy, package yarra-tidy
```

The world-data and gameplay crates have no Bevy; of them only `world_db` and `game_content` use
SQLite. The renderers (Bevy) have no SQLite: `engine` is the only library joining the two.

## World data crates

### vegetation

- Owns what grass and ground details are: `GrassType`, `DetailType`, `VegetationCatalog`, built
  from a `GrassPack` (`assets/packs/grass/baltic.ron`) keyed by terrain surface; the per-cell
  coverage the cook writes (`VegetationFieldPageData`, `VegetationFieldPage`, `GrassField`).
- Owns deterministic blade placement (`Lattice`, `sample_candidate`, `sample_clump`,
  `accepted`); `reference` is the CPU placement tests hold the GPU to.
- Invariants: no Bevy, SQLite or GPU. `shaders/vegetation/placement/sampling.wesl` mirrors
  `placement.rs`; change both together. Depends on no workspace crate.

### world

- Coordinates and identities: `WorldSpaceId`, `CellCoord`, `WorldPosition` (space, cell and an
  offset within it), `StableObjectId`, `ObjectDefinitionId`, `AssetId`.
- Cooked page contracts: `PageKey`, `PageDomain` (Terrain, StaticObjects, GameplayObjects,
  Vegetation), `PagePayload`, `encode_page_payload`; far-object blocks (`FarObjectsPage`,
  `FarObjectForm`, `FAR_OBJECT_BLOCK_CELLS` = 16).
- Terrain contracts: `TerrainHeightfieldPage` (heights, surfaces, `TerrainWear`), the hierarchy
  (`TerrainNodeKey`, `TerrainNode`), control tiles (`TerrainControlTile`, `CONTROL_SAMPLES` = 65,
  `CONTROL_TOP_LEVEL` = 5), the soft layer (`SoftGround`, `SoftLayer`: snow's depth over the
  firm ground from the control tiles, smoothed across cells and ending where the ground's
  surfaces show its edge), editor previews
  (`TerrainPreviewProducts`).
- Renderer-free atmosphere and weather (`AtmosphereProfile`, `evaluate_at`,
  `weather::WeatherRuntime`), `GameplayArea`, `WorldViewBookmark`, `road_noise`.
- Invariants:
  - One binary codec (`codec`). Blobs carry no version: `PROJECT_SCHEMA_VERSION` and
    `RUNTIME_SCHEMA_VERSION` cover everything stored, so changing a stored type (or reordering
    a bincode enum such as `PagePayload`) bumps one.
  - Heights are quantised to `TERRAIN_HEIGHT_STEP` (1/1024 m), normals to `TERRAIN_NORMAL_BITS`
    (12): previews, cooked and decoded data are identical and shared cell edges agree exactly.
  - At most `MAX_TERRAIN_SURFACES_PER_CELL` (12) surfaces a cell. Decoding is bounded
    (`MAX_DECODED_PAGE_BYTES`, 64 MiB) and refuses trailing bytes.
- Must not depend on Bevy, SQLite or `environment`.

### environment

- Owns ground authoring source: `EnvironmentDefinition` (layers, `WornGround`,
  `GroundTreatment`), coverage (`CoverageSnapshot`, `CoverageIndex`, `halo`), `brush`,
  `PresetLibrary`, render-only scatter (`AssetCollection`).
- Owns road source: `Road`, `RoadKnot`, `RoadSpan`, `RoadJunction`, `RoadWear`, `RoadSnapshot`,
  and the `validate` checks `world_db` applies before it writes source.
- Invariants: no editor, database or renderer dependency. Neighbouring cells share edge
  samples, so a cell compiles only with its whole halo loaded; a cell that is not loaded is an
  error, and truncated road reads are never taken as empty.
- Must not depend on Bevy, SQLite, `world_db` or the compiler.

### environment_compile

- Owns deterministic cell compilation: `CompilePlan::new`, then `compile_cells` gives
  `CompiledCell` (ground palette and weight pages, vegetation fields, generated objects, roads,
  wear field, input fingerprint). Budgets are explicit in `CompileProfile`.
- Owns how roads wear and grade the ground (`TerrainRoads`, `RoadShape`, `junction_blend`),
  ground they wear across their whole width (`WornGround::whole_road`, snow packed under a road's
  corridor in the page's own weights) and scatter thinning.
- Invariants: no Bevy, SQLite, filesystem or GPU work. A batch is all or error and returns cells
  in canonical order. Road relief grades the original source heights, never earlier output.
  Scatter roots do not move when paint or density change.
- Must not depend on `world_db` or Bevy.

### world_db

- Project database: bounded editor access (`ProjectReader`, `ProjectWriter`), whole-document
  import/export (`ProjectDocument`), domain stores (environment, roads, atmosphere, areas,
  `TerrainImportWriter`, asset import) and the cook's snapshot (`ProjectCookSnapshot`).
- Runtime database: `RuntimeReader::open_immutable`, `RuntimeCookWriter`, `RuntimeManifest`,
  terrain nodes and control tiles, far-object pages (`rebuild_far_objects`).
- Invariants:
  - Opening an existing database checks `PRAGMA user_version` against this build's constant
    (`WorldDbError::SchemaVersion` otherwise). No migrations: worlds are disposable and rebuilt.
  - Source writes run in IMMEDIATE transactions, compare revisions (a stale write is a conflict)
    and commit a whole gesture or nothing; their sizes are bounded (`limits`).
  - Runtime reads never touch authoring tables; render weights and vegetation fields are never
    authoring records. Start view and gameplay areas sit outside the runtime's content hash.
- Must not depend on Bevy or `environment_compile` (a dev-dependency for tests).

### world_cook

- Owns `yarra-world-cook`: `cook [--full]`, `import-heightfield|roads|grass|assets` (each cooks
  afterwards unless `--no-cook`), and the test fixtures `create-demo`, `create-road-demo`.
- Owns the production cook (`cook_project`, `cook_project_fresh`): one source snapshot, cells
  compiled in batches on every core (`parallel`), terrain hierarchy (`terrain_cook`), control
  tiles (`control_cook`), far-object blocks; and the editor's previews (`preview_terrain`).
- Invariants:
  - Reads and writes happen in key order on the cook thread, so output matches a sequential
    cook. An incremental cook (same global inputs and terrain cells) recompiles only cells whose
    input fingerprint changed, and their ancestors; tests hold it to a full cook's content hash.
    A leaf's soft layer reads its neighbours' control tiles, so a changed tile rebuilds the
    leaves around it, in the cook and in previews.
  - Output is staged, validated, then renamed over the runtime: a failed cook leaves the
    previous generation. Previews never modify either database.
- Must not depend on Bevy or the engine.

## Gameplay crates

### game_types

- Owns durable 16-byte identities (`ActorId`, `DialogueId`, `PlaythroughId` and the rest of
  `ids!`; `named(name)` hashes an authored name), `Key`, `TextKey`, `TextRef`, `GameTime`,
  `TextContract`, `BoundText`, and `Invalid`/`require`.
- Invariant: contracts only, no runtime services; depends on no workspace crate.

### gameplay

- Owns the rules and domains (`rules`, `actors`, `inventory`, `quests`, `dialogue`, world objects,
  areas and triggers) and the playthrough: `SessionState`, `GameSession`, `Command`, and `Driver`
  (fixed time steps, in the game and headless alike).
- Owns two ports: `ContentSource` (`identity`, `core`, `dialogue`), implemented by
  `game_content::ContentRepository` and by `ToolContent`; and `ScriptEngine`.
- Invariants:
  - No Bevy, SQLite, filesystem or Fluent; rendered text never enters state.
  - The whole playthrough is in memory. Records never changed are absent and read as defaults,
    so state and saves hold only what differs from the authored world. Derived indexes such as
    carried inventories are rebuilt on load, not saved.
  - `GameSession::apply` accepts a command whole or leaves state, clock and random streams
    untouched: every write goes through a `Tx` that remembers what it replaced. Read models never
    consume randomness. Dialogue graphs load on demand, at most `MAX_LOADED_DIALOGUES` (32).
- Must not depend on the other gameplay crates; the `fixtures` feature is for tests only.

### localization

- Owns Fluent presentation: `Localization`, `LanguageResource`, `contract`, `contract_hash`. A
  package's source-language file defines its messages and their arguments; translations are
  checked against it. Must not depend on `gameplay`.

### save

- Owns slot files: `SaveDirectory`, `SaveSlot` (`Manual`, `Quick`, `Auto`), `SaveInfo`.
- Invariants: one file a slot, a JSON header line then the state as JSON, written beside the slot,
  synced and renamed over it. Loading brings state in line with the content at hand and refuses,
  with the reason, what no longer fits. Must not depend on SQLite or `game_content`.

### scripting

- Owns the Luau engine `LuauScripts` (a `gameplay::ScriptEngine`) and its API definitions
  (`API_DEFINITIONS`); content names a function as `module.function`.
- Invariants: each call loads its module afresh into its own sandbox, with a memory cap and a
  step budget, and can do only what gameplay's script scopes allow.

### game_content

- Owns the authored project (`LoadedProject`, `ProjectFile`), publication
  (`LoadedProject::build`, `build_language_pack`), reading (`ContentRepository`,
  `LanguageRepository`, `LanguageSource`) and the `yarra-game-content` CLI (validate, build,
  inspect, scenarios, demo).
- Invariants: a bundle is a manifest, one core row a session reads up front, and a row per
  dialogue graph and per text contract, read when needed. Language packs are separate and never
  part of a save's content identity, so wording changes leave mechanics alone. Existing bundle
  files are never overwritten. Domain crates never depend on this crate.

## Rendering crates

### upscaling

- Owns `UpscaleMethod` (Auto, Linear, MetalFX Spatial, MetalFX Temporal), `UpscaleView`,
  `UpscaleStatus`, `UpscalingPlugin`, temporal contracts (`temporal::TemporalView`, `jitter`)
  and `DirectTonemapOutput`.
- Invariants: Auto picks MetalFX Spatial where supported, otherwise Linear. MetalFX sits behind
  the `metalfx` feature, which only `app_game` enables. Depends on no workspace crate.

### atmosphere

- Owns the sky and the air: `WorldEnvironmentPlugin` (`game()`/`editor()`), `AtmosphereState`,
  `AtmosphereOwner` (Game, Editor, Isolated), the sky composite (`sky`), clouds, light shafts,
  sun glare, rain, particles, lightning, eye adaptation, the tone map (`WORLD_TONEMAPPING`).
- Owns what all lit materials share (`environment::EnvironmentParams` and its maps), the
  terrain-derived maps (`valley_mist`, `shore`, `terrain_shadow`) and the place of the ground
  albedo map around the camera (`ground_albedo`), which a terrain renderer fills.
- Invariants: profile evaluation is pure and lives in `world`; this crate is its Bevy adapter.
  Bevy's sky pass is replaced (`BevySkyPassReplaced`); recheck on Bevy upgrades. Isolated views
  get no clouds, weather or particles.
- Must not depend on SQLite, `world_db` or `engine`.

### terrain_render

- Owns terrain v2's drawing: `TerrainRenderPlugin`, `clipmap::ClipmapTerrainMaterial`,
  `ClipmapTextures`, `ClipmapUploads`, CDLOD selection (`ClipmapLayout`, `LodRanges`, `select`,
  `MORPH_SHARE` = 0.3), ground surfaces (`TerrainSurfaceLayer`), and `ClipmapPatch` (one cell
  outside streaming, for the editor's preset preview).
- Owns ground prints (`prints`): the application lists feet within the snow's reach each frame
  (`GroundPresses`, in `GroundPressSystems`, heights of their soles over the firm ground);
  presses go into a log of 65,536 and a compute pass draws them into two toroidal windows centred
  on the player (2048² at 2 cm and at 8 cm, `terrain/print_map.wesl`): how high the soft layer
  may stand and the lowest sole near. A second pass resolves the same tiles and those beside
  them (a slope reads its neighbours), with the leaves' layer depths (`PrintGround`), into the
  relief: how far feet lowered the ground, rims beside trenches and its slope, with six mip
  levels. Vertices take the coarse window's relief filtered to their spacing; the material tilts
  and streaks the soft surfaces it shows by it at the pixel's footprint.
- Fills the environment's ground albedo map each frame (`ground_albedo`, from the streamed world's
  clipmap, `GroundAlbedoSource`): the surfaces' colour and how squarely the sun meets the ground,
  for the light every material takes from below (`lighting/ground.wesl`).
- Invariants: selection uses distance to a node's box, so turning the camera changes only
  culling. A node splits only when all four children are resident and vertices morph to the
  coarser grid, so levels meet without cracks. Soft leaves near the camera split into detail
  levels (`DETAIL_LEVELS` = 3: 0.5, 0.25 and 0.125 m within 48, 24 and 12 m) whose vertices lie
  on the leaves' triangles. Heights are the drawn ground, snow's top where it lies (the cook
  raises leaves by their soft layer); the leaves' layer depths, uploaded to a level-0 texture,
  are all the prints need to lower it, and tiles under a leaf whose depths arrive are resolved
  again. The print windows hold nothing the log does not: ground a window moves onto starts
  undisturbed and takes every logged press reaching it.
- Must not depend on `world_db` or `engine`.

### vegetation_render

- Owns procedural grass after Ghost of Tsushima: `VegetationRenderPlugin`,
  `VegetationSceneState`, `VegetationView`, `VegetationWind` (which trees also read), trampling
  (`VegetationBenders`, the characters among the plants, which trees also read),
  `VegetationDiagnostics`.
- Invariants: dormant until a `VegetationSceneState` exists and a camera has `VegetationView`;
  installs no GPU timing probes; no entity or row per blade. The candidate cache holds only
  stable acceptance, never view-dependent visibility or LOD.
- Must not depend on `world`, `world_db` or `engine`: it knows grass, not the world.

## engine

- World at runtime: `WorldStreamingPlugin` (`game()`/`editor()`), `WorldOrigin`,
  `WorldRenderRoot`, `ClipmapTerrain`, `WorldGenerationReload`, `LiveTerrainPreview`.
- Objects and vegetation: `ObjectLodPlugin`, GPU trees and wind (`TreeWindPlugin`,
  `TreeInstancing`), `WorldVegetationPlugin`, ground details, `OceanPlugin`. Characters push
  branches aside through a 3D field of springs around the player (`trees::touch`), which every
  tree material reads in every pass and turns each limb about its root by.
- Actors: `GameplayPlugins` (player, camera, input, destination marker), `GameplaySystems`,
  `MoveIntent`, and `MinimalGamePlugin` (environment, terrain, streaming and actors). Characters'
  feet (`feet`, joints and soles from the character catalog) list their presses on the ground,
  and each leg the channel it wades through deep snow around its ankle.
- Time and weather: `GameDayClock`, `GameWeather`, `GameLightning`.
- Invariants: actor roots own movement; presentation scenes are children and never move them.
  Libraries do not read process arguments; the apps inject settings. The `gameplay` module here
  is actor control, not the `gameplay` crate.
- Must not depend on the gameplay crates, `world_cook` (tests only), `bevy_egui` or the apps.

## Applications

### app_game

- Composition (`main.rs`, `launch`): parses launch options once (one flag table,
  `launch/flags.rs`, which also generates `--help` and the table in
  [workflows](WORKFLOWS.md#launch-options)) and injects them as resources; installs
  `MinimalGamePlugin`, vegetation, trees, ocean, weather and the day clock.
- Presentation (`GameRenderPlugin`, see [the frame](#rendering-frame)), `runtime_settings` (the
  one writer of the settings F1 changes) and frame pacing: the event loop, `CADisplayLink` on
  macOS and the patched wgpu-hal minimum presentation interval
  (`third_party/wgpu-hal/YARRA_PATCH.md`).
- The `--story` slice (`story`), which publishes the authored project, sends input and engine
  observations as commands and shows read models.
- Development tooling under `dev/`, installed by `dev::install`: the F1 panel and its timing
  layer (`render_audit`), repro routes, timed profiles, look captures, the LOD lab, Metal
  captures and live grass-pack reload. The game modules do not depend on it.

### app_editor

Owns the shell, workspaces, tools, working sets, saving, publication and the recovery journal
(see [the editor](#the-editor)). Must not depend on the gameplay crates.

## Tools

- **shader_check** composes WESL as Bevy's shader cache does (same `wesl` compiler and options,
  Bevy's modules from its crate sources) and validates it with naga; see [shaders](#shaders).
  Render crates use its `Shaders` API in tests.
- **rust_tidy** (`cargo run -p yarra-tidy [-- --write]`) only inserts the blank lines rustfmt
  leaves out: between items and around a function's multi-line steps.

## Rendering frame

The game camera (`WorldViewCamera`) renders in HDR and tone-maps into an offscreen image at a
share of the window's physical size (`resolution_scale`, 0.5 by default) with 4× MSAA; with
MetalFX Temporal, MSAA is off and the backend reconstructs at output size. In order:

1. Compute before the camera: grass trampling, candidate cache and placement; ground prints
   (`shaders/terrain/press.wesl`, `print_relief.wesl`, `print_mips.wesl`); GPU tree LOD and
   culling (`shaders/tree/lod.wesl`); the terrain-horizon search; cloud shadows.
2. Main passes: terrain v2 patches (one shared grid mesh and `ClipmapTerrainMaterial`; heights
   come from the clipmap), grass as indirect opaque draws (no shadows), trees and impostors as
   indirect draws per form and representation in the main, prepass and shadow phases, meshes.
3. After the main pass: the sky composite, one full-screen pass over the resolved image (sky, sun
   and moon, aerial perspective, clouds, haze, mist, fog, the sea), fed by quarter-resolution
   light shafts, sun occlusion and sun rays.
4. Temporal resolve (MetalFX Temporal only); rain and ambient particles between
   `EarlyPostProcess` and `PostProcess`; bloom, eye adaptation and tone mapping.
5. Upscaling to the window (MetalFX Spatial, or Linear during UI composition) under a UI camera
   at native resolution.

Every lit material binds the shared environment at slots 120–129 (`EnvironmentParams`, cloud
shadow, rain shelter, forest and terrain shadow, ground albedo). `MsaaColorStorePlugin` discards multisample
colour after the resolve only while `BevySkyPassReplaced` holds. The editor uses the same
renderers.

## Streaming and residency

All runtime IO goes through one read-only worker holding a `RuntimeReader`, behind bounded
channels (16 requests, 32 replies); each request is numbered and its reply routed to the
consumer that sent it. Four kinds of data stream independently:

- **Source pages** by cell around the view: terrain relief and wear within 240 m (80 m on
  iOS), objects within 192 m (`OBJECT_RESIDENCY_METRES`), grass fields, gameplay objects in the
  game only. Admission is bounded (64 MiB decoded, 256 MiB estimated GPU, two attachments a
  frame); departing pages cool for two seconds.
- **Terrain clipmap nodes** (`ClipmapTerrain`): a 31×31-node window per level, six levels from
  32 m leaves to 1 km nodes, toroidal 1024² layers (512² on iOS), parents first. Shaders read
  the first resident level at or above the one wanted, so a loading node shows its parent,
  never a hole; actors stand on resident leaf triangles less their soft layer
  (`ClipmapTerrain::ground_height`), wading through snow.
- **Far-object blocks**: 16×16-cell blocks of impostor-ending objects within 2 km, carrying
  whole LOD chains into the GPU trees' instance pool and the forest shadow map.
- **Coarse maps**: every level-3 node, read once per generation for the valley mist, shore and
  distant terrain-shadow maps.

A new generation is adopted in two phases (`WorldGenerationReload`): nothing read for it
reaches the live generation before the commit. Render space has a floating origin: `WorldOrigin`
moves to the focus's cell once the focus is over `FLOATING_ORIGIN_THRESHOLD_CELLS` (8) cells
away. Only `WorldRenderRoot` entities move; far-object batches sit relative to their own
origins, and grass, wind and details are anchored to canonical coordinates
(`VegetationRenderOrigin`), so a rebase changes nothing on screen.

## Shaders

All shaders are WESL in `assets/shaders/`, a folder per subsystem:

- `terrain/`: clipmap material and vertex shader, height blend, road wear, anti-tiling, ground
  prints (`print_map` their texels, `press` pressing them, `print_relief` and `print_mips` the
  relief, `prints` read by the terrain).
- `vegetation/`: grass `schedule`, `placement/`, `candidate_cache`, `blade`, `draw/`,
  `trample`, sharing `types`, `random` and `lod`.
- `tree/`: tree material, bark, wind and its prepass, impostors, the LOD compute pass.
- `sky/`: the composite and its parts (air, fog, celestial, sun, cloud layer), light shafts,
  sun occlusion and sun rays. `clouds/`: density, panorama, tracing, shadows.
- `lighting/`: shared lit-material code: `material` (adapted from Bevy's PBR, see
  `THIRD_PARTY.md`), `pbr_lighting`, `surface`, forest and terrain shadows, terrain horizon, the
  ground below (`ground`).
- `environment/`: the `EnvironmentParams` layout and map lookups. `weather/`: rain, particles,
  ambient particles, ground wetness. `water/`: the sea and its waves.
- Root: `crossfade` (LOD crossfades as MSAA sample masks), `environment_coverage` (editor).

Modules import each other as `package::shaders::<folder>::<file>` and Bevy's as
`bevy_pbr::...`; integer limits arrive from Rust as `constants::NAME` definitions. Bevy composes
a variant only when a pipeline first needs it, so run `cargo run -p yarra-shader-check` after
shader changes: it checks every variant in `crates/shader_check/entries.ron` (definition sets
from the game's pipeline log, refreshed when pipelines change); `--each-flag`, `--parse-all`.

## The editor

- **Workspaces** (`EditorWorkspace`): World; Animation (character previews); Presets
  (environment presets, collections, road styles), the last two under an isolated atmosphere.
  Each owns its cameras, UI and systems; project workers and save completion stay in the shell.
- **Tools** register an `EditorToolDescriptor` naming their workspace and the source domains
  (`EditorSourceDomain`) the bounded query window loads while active. The World workspace places
  objects and authors areas, the atmosphere, roads and environment paint
  (`workspaces/world/tools`).
- **Working sets** hold edits by stable ID: `EditorObjectWorkingSet` for placed objects,
  `SourceWorkingSets` for atmospheres, areas, roads, definitions, presets and coverage cells;
  `EditorHistory` gives undo and redo. `SourceActivity` decides in one place whether a save,
  publication, gesture, conflict or pending draft blocks a change.
- **Save** (`EditorSaveCoordinator`) drains every domain in dependency-safe order through the
  project thread (`ProjectEditorStore`), which owns the one connection and sends one save at a
  time through a `SaveChannel` per domain; stale revisions come back as conflicts.
- **Publish** (`RuntimePublicationPlugin`) runs `world_cook::cook_project` on a worker; the
  engine adopts the result, or keeps the previous generation on failure. Save & Publish saves
  first. Unsaved terrain edits are drawn from `world_cook::preview_terrain` (`LiveTerrainPreview`).
- **Journal** (`EditorJournalPlugin`): dirty objects, definitions, presets, coverage, roads,
  areas and atmospheres are written atomically to `<project>.editor-journal.ron` and restored
  into the working sets on the next start; a journal for another project or schema is refused.

## Formats and versions

Every reader accepts only its current version: no migrations, fallback readers or silent
resets. Change a format, bump its constant and rebuild (Reteya from its inputs).

| Format | Constant, file | Value |
| --- | --- | --- |
| Project database, blobs included | `PROJECT_SCHEMA_VERSION`, `world/src/lib.rs` | 35 |
| Runtime database, blobs included | `RUNTIME_SCHEMA_VERSION`, `world/src/lib.rs` | 35 |
| Gameplay project (`project.ron`) | `SOURCE_FORMAT_VERSION`, `game_content/src/project.rs` | 11 |
| Content bundle | `BUNDLE_SCHEMA_VERSION`, `game_content/src/bundle.rs` | 14 |
| Language pack | `VERSION`, `game_content/src/languages.rs` | 3 |
| Save slot | `SAVE_FORMAT`, `save/src/slots.rs` | 17 |
| Editor journal | `JOURNAL_SCHEMA_VERSION`, `app_editor/src/journal.rs` | 15 |
| Heightfield manifest | `VERSION`, `world_cook/src/heightfield_import.rs` | 2 |
| Asset import catalog | literal in `AssetImportCatalog::validate` | 1 |
| Character catalog | literal in `engine/src/character_catalog.rs` | 2 |
| Impostor descriptor | literal in `engine/src/trees/impostor.rs` | 3 |

The SQLite formats keep their version in `PRAGMA user_version`; bundles and language packs also
set an `application_id`, so the wrong kind of file is refused.
