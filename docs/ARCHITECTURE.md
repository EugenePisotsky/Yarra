# Architecture

Current implementation, consolidated September 21, 2026. Use [workflows](WORKFLOWS.md) to operate the tools, [performance](PERFORMANCE.md) to measure them, [experiments](EXPERIMENTS.md) for decisions/results, and [refactoring](REFACTORING.md) for remaining structural work.

## Ownership

| Crate | Owns |
| --- | --- |
| `app_game` | Executable/platform composition, game presentation, frame pacing, F1 and profiling/capture |
| `app_editor` | Workspace shell, authoring commands/history, draft previews, save/recovery and publication coordination |
| `engine` | Actor/camera/input/animation integration; world streaming, origin, residency, terrain contact and vegetation integration |
| `atmosphere` | Bevy sky, sun/moon, ambient illumination, haze, clouds and shared cloud transmission |
| `terrain_render` | Terrain materials, geometry LOD algorithms, prepared/stochastic caches, composite atlas and near detail |
| `vegetation_render` | GPU placement/scheduling, caches, procedural drawing, wind/lighting, canopy and diagnostics |
| `upscaling` | Backend selection/capabilities, Linear/MetalFX, temporal contracts and output composition |
| `world` | Bevy-free coordinates, IDs, payload ABI, terrain/atmosphere/cloud contracts and view bookmarks |
| `environment` | Bevy-free environment definitions, presets, brushes, roads and scatter contracts |
| `environment_compile` | Pure bounded derivation of ground, vegetation, road relief and scattered objects |
| `vegetation` | Renderer-independent species/population/field contracts and deterministic sampling |
| `vegetation_compile` | CPU mask compiler and reference placement used as a test oracle |
| `world_db` | Revisioned authoring and immutable runtime SQLite readers/writers, schemas and bounded queries |
| `world_cook` | Staged cooking/publication, material baking, preview products and explicit fixture generators |

Game launch options are parsed once in `app_game/launch.rs` and injected as resources/settings. Shared runtime/rendering libraries do not inspect process arguments or bind experimental keys. [RuntimeSettingsPlugin](../crates/app_game/src/runtime_settings.rs) owns the shared render/feature snapshot and applies it to cameras, materials, atmosphere, vegetation and input. It initializes before output setup and before F1 captures its reset baseline. [PerformancePanelPlugin](../crates/app_game/src/render_audit.rs) composes the optional controls, UI and capture session; game startup and [reproduction routes](../crates/app_game/src/repro.rs) work without it. Diagnostic overrides remain fields of the shared snapshot so CLI/F1/reset/A/B use the same state.

Within [performance diagnostics](../crates/app_game/src/render_audit/performance.rs), `panel` owns layout/input/visibility, `capture` owns launch/reset snapshots and bounded A/B recordings, `telemetry` owns frame/app history and pipeline readiness, and `report` owns statistics, comparison text and CSV export. Capture completion returns an outcome to presentation; the recording session has no UI entities or panel visibility state.

`--diagnostics off|panel|full` controls composition. Full preserves the previous default. Panel installs F1 and frame/app cadence collection without CPU system wrappers or GPU probes; Off omits both panel and timing plugins. The vegetation renderer no longer installs Bevy's diagnostics recorder itself; game/editor composition chooses it explicitly. Renderer workload bookkeeping remains part of rendering, and explicit repro/profile automation still works in Off mode. F1 captures canopy values, terrain macro variation and page gizmos alongside other settings.

Keep data/compiler crates independent of Bevy. Renderers do not own SQLite or editor workflows. Currently `engine` depends on both renderers, atmosphere and `world_db`; `world_db` includes authoring/compiler dependencies; vegetation rendering depends on upscaling for temporal integration. These are real dependencies, not a fully separated gameplay/runtime architecture yet.

```text
content/world.project.sqlite
  → bounded source snapshot + environment compilation + world cooking
  → staged, validated runtime publication
  → assets/generated/world.runtime.sqlite
  → read-only database worker → asynchronous decode
  → bounded attachment/removal → renderers and gameplay consumers
```

The editor overlays unsaved source edits on the published runtime. Save changes source; publication creates a new immutable generation. Failed publication leaves the previous generation usable. Runtime pages are derived products, not authoring state.

## World, source and persistence contracts

[world_db](../crates/world_db/src/lib.rs) keeps its public API at the crate root. `records` defines data contracts; `project` separates bounded editor reads/writes from whole-document import/export; `runtime` separates immutable reading from whole-build/incremental output writes. `catalog`, `page`, `storage`, `limits` and `error` own shared encoding, validation and budgets. `cook_store` owns only the bounded source snapshot. Existing environment, road, atmosphere and terrain stores retain their domain transactions and queries. These are internal modules; runtime-only dependency packaging remains future work.

- Stable IDs identify worlds, definitions, assets, placements, presets, layers and roads. Visual entities are disposable representations of those identities.
- Logical positions use world/cell coordinates; render transforms are relative to a floating origin. Rebasing must preserve world-aligned texture, wind and canopy phase.
- The default authored world uses 8 m cells. `DEFAULT_CELL_SIZE = 32` also serves older fixtures; it is not proof that every world uses that grid.
- Current versions: source **24**, runtime **20**, page payload **8**, recovery journal **14** (reader accepts journal 12–14). Source/runtime constants live in [world](../crates/world/src/lib.rs); journal versions in [journal](../crates/app_editor/src/journal.rs). Incompatible source formats are rejected, not silently reset.
- Editor reads/writes are bounded and revision-checked. Undo/redo uses stable-ID commands. Save commits coherent revisions; stale writes report conflicts. Recovery is separate from ordinary save.
- Production cooking uses one consistent source snapshot, bounded cell/halo batches and staged output. Whole-document helpers remain fixture/parity references and share compilation code with production.
- Unloaded source is unknown, not empty. Road/coverage queries must certify sufficient bounds/halos before compilation.
- Schema or bincode enum changes require explicit version/recook decisions. The old `TerrainRender` payload is still readable, while current cooking emits `TerrainHeightfield`; deleting its enum variant casually would shift serialized discriminants.

## Environment authoring

Reusable typed presets and compositions feed ordered spatial layers with coverage masks. One compiler derives ground palettes/weights, vegetation fields and generated objects. Empty coverage uses the world's explicit base material. Terrain surfaces have semantic IDs; texture-set array layers are resolved during cooking.

Curved roads have stable source geometry and shared styles controlling wheel tracks, center/shoulder wear, vegetation retention and relief. Relief is derived from original source heights, never repeatedly added to previous output. Explicit junctions connect 2–4 compatible arms. Spatial certification, intersections, samples and output work remain budgeted; querying one region must not recursively load the entire route network.

Asset collections deterministically scatter registered visuals with weights, scale/spacing, slope and clearance controls. Generated placements are render-only and are not individually editable source objects. Manual object placements remain separate and preserve stable identity.

Asset management currently consists of local source/runtime files, tracked pack manifests, SQLite asset/LOD records, a searchable editor placement palette and collection selection. `world-cook import-assets` validates a bounded RON catalog and local file availability, then registers stable key-derived IDs transactionally through `ProjectWriter`. Reimports refresh supplied variants and preserve unrelated records, placements and definition aliases. Asset conversion remains an offline tool, without an editor import UI. The Forest Tree Starter Kit summer catalog includes 11 trees and 8 shrubs with four mesh LODs each; licensed binaries remain ignored local files.

## Terrain and streaming

The default terrain hierarchy supplies distant coverage, bounded material streaming, smooth geometry transitions and nearby detailed shading. `--terrain-legacy` selects the older nearby world renderer for diagnostics; it does not change publication. Normal cooking includes distant material products and requires prepared CPU inputs. `--geometry-only` is an explicit fixture option.

[Database transport](../crates/engine/src/world_streaming/database.rs) owns worker startup, bounded channels and shutdown. Its protocol and immutable-reader queries depend only on `world`/`world_db` data, not terrain-rendering or residency modules. Source and terrain/material consumers submit requests without blocking; the streaming coordinator routes replies and owns world/index state and transitions. [Source residency](../crates/engine/src/world_streaming/residency.rs) separately owns page requests, asynchronous decoding, admission, cooling, statistics and stale-result rejection. A staged generation can answer terrain queries, but source reads remain on the active generation until the exact commit is acknowledged.

Dropping the worker disconnects a separate cancellation channel before joining. Both request and reply waits can then exit even when queues are full; an already-running synchronous SQLite read still finishes first. Runtime worker omission/restart is not a supported streaming mode.

[Attachment](../crates/engine/src/world_streaming/residency/attachment.rs) converts prepared pages into CPU sources, objects or legacy terrain entities, tracking page-owned meshes, materials and images for removal. Whole-page object validation and fallible terrain preparation finish before spawning entities or storing owned assets. Cooling retains ownership for two seconds and can revive the same entities. World/generation changes clear pages and cancel decode jobs without reusing pending request identities; same-world hierarchy rebases preserve canonical sources. These internal systems remain ordered inside `WorldStreamingSystems`; residency omission or runtime suspension is not a supported mode.

Important invariants:

- Authoritative leaf heights retain f32 precision. CPU grounding/raycasting and grass placement interpolate the same piecewise-linear triangles as terrain geometry, not a different bilinear surface.
- Parent coverage must not fill holes. Retain a complete active cover while replacements load; parent/child transitions, stitched edges and morphs must remain crack-free.
- Geometry/material visibility and detailed source demand are separate. Elevated views must not demand finest leaf geometry everywhere below the camera.
- Actor contact outranks vegetation/contact and visual refinement. Grass may be retired atomically under pressure; actor grounding must not disappear. Readiness checks use the drawn surface, including transitions.
- Coarse material fallbacks remain available while fine products stream. Near detail uses existing hierarchy geometry, not another coincident ground mesh. `TerrainMaterial` remains shared by near-detail preparation and editor previews even if legacy world rendering is later removed.
- Rebase, source edits, publication and shader changes invalidate the appropriate caches. Obsolete asynchronous results cannot replace a newer revision.

Current source-stream limits include 16 pages loading/decoding/waiting for attachment, two actual page attachment attempts/frame, 64 MiB decoded source residency and 256 MiB estimated source GPU residency. Transport separately holds at most 16 queued requests and 32 replies; draining those queues does not bypass source-page admission. Hierarchy geometry/material caches have separate budgets. These counters are not total process/GPU memory. Admission skips blocked pages when a later page fits; priority eviction and aggregate pre-decode byte reservation remain open.

[ObjectLodPlugin](../crates/engine/src/object_lod.rs) owns visual-object scene selection after transform propagation. Streaming includes it for game/editor; collection previews can use it without database streaming. Streamed objects and editor previews share initial LOD construction, with the existing 12% hysteresis, 0.25–4 scale clamp and 32 switches/frame. Residency reads LOD telemetry through accessors. Omitting it in standalone composition leaves the initially attached scene; it does not unload visuals.

In the normal hierarchy path, static-object residency uses a separate 192 m camera-to-cell-bounds radius in all directions. Camera frustum visibility only prioritizes loading; it must not retire off-screen trees that can cast shadows onto visible ground. Bevy performs camera and shadow-cascade mesh culling independently. The descriptor window covers the larger of grass contact and object visibility ranges, within the existing 4096-cell query budget. Grass contact and gameplay demand keep their own ranges. This replaces the three-cell object limit that reduced visibility to about 24 m in 8 m worlds. The final mesh LOD remains selected below its threshold; billboard/fade transitions are not implemented. The legacy diagnostic path retains its three-cell window. Source memory and attachment budgets still apply, so 192 m is a demand limit, not a guarantee under memory pressure.

The local material supports one/two active surfaces despite the data contract allowing eight slots. Albedo is sRGB; normal/AO/roughness and macro maps are linear. Prepared albedo and stochastic-buffer references remain available. Nearby detail blends to baked ground; material preparation must preserve shared borders and fallback behavior.

## Vegetation

The runtime joins compact vegetation fields with terrain surfaces, generates candidates on the GPU, and emits bounded indirect draw bins for single/split topology and high/low geometry. It does not store an entity or database row per blade. Stable ownership, seeds and thinning preserve placement across views; telemetry exposes capacity drops.

The [renderer entry point](../crates/vegetation_render/src/renderer.rs) owns render-world composition and resource initialization. Internal modules separate CPU/WGSL layouts (`gpu_types`), CPU scene/LOD packing (`packing`), fixed index templates (`topology`), allocation/bind groups (`buffers`), pipeline specialization (`pipelines`), uploads/view inputs (`prepare`), cached placement dispatch (`generation`), opaque queue/draw (`draw`), and optional counter readback (`telemetry`). Candidate caching, blade preparation, temporal rendering and asynchronous canopy work retain their existing owners. Scheduling remains explicit at composition, including temporal dependencies and the iOS readback exclusion; these modules are not independently removable plugins.

Placement, source acceptance and per-frame blade deformation use separate caches. Wind phase must not invalidate source acceptance/placement unnecessarily. Prepared-blade overflow evaluates the original vertex path rather than dropping grass. Tests compare accepted instance multisets and rendered output, not append order alone.

[WorldVegetationPlugin](../crates/engine/src/world_vegetation.rs) composes the renderer, published field/terrain joining, logical LOD focus and ground-canopy integration. The game installs it explicitly after world streaming; F1 applies settings before its update set. `VegetationSceneState` and `VegetationView` name the production render snapshot and camera opt-in. The serialized `VegetationDebugSettings` type remains unchanged for saved-study compatibility; its production and diagnostic fields still share one resource.

Game and editor share terrain-to-vegetation conversion and [GroundCanopyPlugin](../crates/engine/src/world_vegetation/canopy.rs). The editor retains authority over accepted draft catalogs/coverage and workspace selection. Terrain and vegetation keys match by space/cell/LOD and extent, ignoring their different page domains. Source joining continues while grass rendering is disabled, and clears stale pages when the world/catalog disappears. Joined in-place edits invalidate the snapshot; unrelated terrain pages do not.

The canopy cache keeps one immutable source snapshot plus per-tile indices. It compares actual nearby sources, including relief, before accepting bake results. World/origin/catalog changes, material replacement and unload release affected masks and cancel jobs. Disabling canopy detaches coverage and cancels jobs, retaining valid resident masks for reuse; source edits are checked before reattachment. Scheduling remains capped at two jobs after 0.35 seconds of source settling. Canopy updates run in `Update`, before terrain material preparation in `PostUpdate`. This replaces the two separate game/editor caches; no performance improvement has been measured.

[TreeWindPlugin](../crates/engine/src/tree_wind.rs) is installed explicitly by game/editor composition. It reads the existing `VegetationWind` direction, strength, gust field and phase; the vegetation renderer remains the clock owner, including external editor/replay transport. `TreeWindResponse` supplies the separate canopy/leaf response in metres. glTF materials opt in with `extras.yarra_wind = "foliage_uv1_v1"`; UV1 stores `(flutter, branch)` weights. Only tagged static, alpha-masked meshes are converted, with no path-name/prefab heuristics or database schema change. The pack exporter writes the tag for every foliage LOD and leaves bark rigid.

Tree materials extend the existing cloud-shaded PBR material, preserving textures, alpha clipping, normal maps and cloud transmission. Tagged foliage explicitly opts into that pipeline in isolated collection studies, where the study atmosphere disables cloud transmission; unrelated study materials retain their ordinary path. Conversion follows cloud material conversion and retries until scene dependencies and bounds exist. Instances share cached materials, source edits propagate, and unloading scenes releases the cache's references. Mesh bounds include the maximum allowed deformation. One shared 96-byte GPU buffer carries current/previous wind poses, so time updates do not rewrite per-instance transforms or dirty every material. Independently reduced f64 phase offsets preserve the field across floating-origin rebases.

Colour, depth and shadow passes evaluate identical tree geometry. The motion-vector prepass evaluates the previous model matrix with the last GPU-submitted wind pose, including its settings and origin; it does not reconstruct history from a time delta. This supports MetalFX Temporal and Bevy temporal consumers, paused/scrubbed transport and wind toggles. PostUpdate replay producers run before `TreeWindSystems`. New LOD topology still uses the renderer's ordinary history rejection; this does not implement LOD crossfading or guarantee shimmer-free foliage. A native GPU regression checks animation, pause, zero weights, disable and rebase cases, and the game path has been exercised with MetalFX Temporal.

Canopy shading is a shared artistic treatment of ground and lower blades, not physical grass self-shadowing. Source-boundary/coverage bakes run asynchronously with revision checks and bounded scheduling. Shading controls must not trigger unrelated placement/pose rebuilds. Grass casting onto ground remains unresolved; previous proxy casters were removed after visual rejection.

## Characters and editor lifecycle

[MinimalGamePlugin](../crates/engine/src/gameplay.rs) is a convenience composition of environment, terrain, streaming and `GameplayPlugins`. The gameplay group has four owners:

| Plugin | Responsibility |
| --- | --- |
| `GameplayPlugin` | Player root, character presentation, motors, grounding, shared control resources and schedule |
| `GameInputPlugin` | Pointer/touch destinations, keyboard/gamepad intent and orbit/zoom controls |
| `GameCameraPlugin` | Camera creation, launch bookmark and actor follow |
| `MovementTargetPlugin` | Destination ring entity, assets and terrain-aligned placement |

Custom composition can install `GameplayPlugins.build().disable::<GameInputPlugin>().disable::<MovementTargetPlugin>()` after the rendering/streaming prerequisites. Keep the core `GameplayPlugin`; camera omission is also supported, with camera-dependent input idle. The editor continues composing its own previews and camera/input stack. These are startup choices, not hot plugin removal or new CLI flags.

`GameplaySystems` orders camera input → pointer destination → movement intent → motor → grounding → marker → camera follow, after streaming transitions/rebasing. UI routing and settings run before `CameraInput`; repro camera overrides run after `CameraFollow`. Character presentation resolves motor settings before movement. `GameInputEnabled(false)` freezes controls/motors while grounding/follow continue and native gesture readers drain events. Pointer capture alone preserves keyboard/gamepad input.

Actor roots own movement and logical location. Imported presentation scenes are children; meshes/animation never own streaming or camera focus. Player control, camera target and stream focus are independent roles. Stable presentation profiles resolve compatible models, skeletons, animation banks and semantic clips. The current motor drives Idle/Walk/Jog with acceleration, turning, arrival and playback-rate output; terrain grounding is implemented. Party AI, navigation/collision, crouch, equipment, combat, IK and root motion are not implemented foundations to assume.

World, Animation, Vegetation and Presets workspaces have separate cameras, input and preview lifecycle. Switching cancels transient interactions and restores workspace-owned settings while retaining drafts/history and world navigation. The shell camera remains active. Window visibility is presentation only; tool activation controls source demand. Bounded source working sets and stable-ID selection support promoted editor proxies; matching cooked visuals are hidden while proxies are ready.

The [World workspace](../crates/app_editor/src/workspaces/world.rs) composes camera navigation, shortcuts/picking, source-object proxies, grouped gizmo transactions and overlays. Its UI module composes the toolbar and separate hierarchy, inspector, assets, navigator and diagnostics views. [Vegetation authoring](../crates/app_editor/src/vegetation_authoring.rs) separates the validated catalog draft/save lifecycle, accepted live preview, inspector, population/species controls and ribbon-curve canvas. The [Vegetation study](../crates/app_editor/src/workspaces/vegetation.rs) separately owns session state, viewport entities/scene updates, workspace enter/leave restoration and bounded export/capture. Existing plugins still own system ordering and save completion; internal modules are not independent activation switches.

## Atmosphere and presentation

One authored atmosphere/weather profile belongs to each world space. Pure evaluation lives in `world`; the Bevy adapter owns sky, directional/ambient illumination and haze. The game advances session time; editor studies/preview transport can temporarily own it. Restoring a workspace must restore the correct owner and camera environment.

Balanced clouds use a 2048×256 azimuth × elevation sky panorama (square-root elevation warp, rows along the horizon) refreshed a few rows every frame, four complete refreshes per second at any frame rate. The composite cross-fades the two latest complete refreshes while a third image is refreshed, so clouds change continuously rather than in 4 Hz steps behind a moving refresh seam; High retains per-frame half-resolution rendering. Cloud altitude, field size, seed or enablement changes, teleports and resume require a full refresh; coverage, density, erosion and thickness (weather) and normal evolution update progressively. Cloud Off skips drawing/shadow passes, including weather fog, but preserves weather's ambient response and some shared bookkeeping.

Game weather is [pure data in `world`](../crates/world/src/weather.rs): Clear/Scattered/Overcast/Rain/Storm presets, a weighted random sequence with hold/transition durations, continuous blending from the current values and a wetness integrator. The defaults are hard-coded until the editor authors them. The [engine adapter](../crates/engine/src/weather.rs) advances it in the game only (`AtmosphereOwner::Game`) and overlays it on the authored profile: cloud shape, daytime exposure and skylight colour through `AtmosphereState::effective_profile`, the grass ambient bound through `VegetationAmbientGain` (exposure adaptation, since grass lighting is bounded in exposed units), and the shared wind through `VegetationWind` strength, gusts and `rate`. Grass, trees and cloud drift share the authored cloud wind direction while weather is active. Weather changes the wind clock rate rather than `speed`, because phase is `time × speed`. Reduced visibility is not written into Bevy's scattering medium: its half-float transmittance tables divide values that underflow below a few kilometres and produced NaN streaks. During a weather change each region of the cloud field blends from the previous coverage, extinction and erosion at its own time (a noise-driven arrival over ~40% of the change) instead of one global threshold; cloud shadows share the same density function. Weather fog instead adds the missing extinction, lit by desaturated ambient light, in the existing cloud composite pass. That pass works on the resolved colour, so edge pixels touching the sky take the sky's fog transmittance; exact edges require owning the sky composite. `--weather authored`, the default for profiles, repros, captures and smoke runs, presents the profile unchanged. F1 A/B captures pause the weather clock and record the weather.

[Precipitation](../crates/atmosphere/src/precipitation.rs) draws rain as instanced camera-facing streaks in three camera-anchored boxes (near, mid, far) that wrap a world-fixed drop lattice. Positions come from hashes plus a CPU-accumulated fall offset, so there is no per-drop state and wind changes never make drops jump. Streaks keep at least one pixel of width with proportionally reduced alpha, fade with weather fog and use a soft test against scene depth; nothing shelters them under trees yet. The pass runs between `EarlyPostProcess` and `PostProcess`: after Temporal reconstruction (no ghosting, output resolution) or on the resolved MSAA image, and before bloom and tone mapping. Wetness from `AtmosphereState` reaches every surface through `CloudParams::weather`: the shared PBR adapter darkens albedo and lowers roughness on sky-facing surfaces, and grass darkens its body colour.

Game defaults are 50% of physical width/height, 4× MSAA, native-resolution UI, Auto upscaling, Balanced grass/clouds and optional counters off. Auto uses MetalFX Spatial when supported, otherwise Linear. Explicit Linear samples the scene directly during UI composition. MetalFX Temporal is an integrated, selectable game backend: it replaces MSAA, requires mesh depth/motion, writes grass color/motion/final depth, reconstructs HDR before full-size postprocessing, and resets history on discontinuities. Motion excludes jitter; exposed HDR uses a unit exposure hint. Recorded performance and moving-grass quality issues remain unresolved; these are follow-up issues in an implemented feature, not missing temporal integration. See the ledger.

`MsaaColorStorePolicy::Automatic` discards multisample color only when later consumers permit it; depth is retained. New passes loading that color must request Preserve or extend the consumer check. This adapts Bevy's opaque schedule and must be rechecked on upgrades. The [wgpu-hal patch](../third_party/wgpu-hal/YARRA_PATCH.md) supplies the Metal minimum presentation interval for frame pacing; it is not obsolete vendored experimentation.
