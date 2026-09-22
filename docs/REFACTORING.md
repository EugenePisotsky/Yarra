# Refactoring backlog

Source audit at `2afce93`, September 21, 2026; first cleanup applied below. [Architecture](ARCHITECTURE.md) maps all 14 crates. [Experiments](EXPERIMENTS.md) records retained/rejected approaches and performance limits.

**Keep existing crate boundaries initially. Fix ownership and plugin composition first.** Data/compiler separation is useful; production behavior inside experiments, competing settings writers and subsystem lifecycle are the larger problems. The remaining proposed plugins are listed below.

## Completed: confirmed leftovers and documentation

- Removed the unreachable 160-second baseline runner, its state, scheduling, button guard and log field. Its request was only set in tests; current F1 A/B capture is a separate implementation and remains.
- Removed the previous-catalog comparison, standalone banding controls, sun stress key, duplicate overlays and implicit renderer/debug hotkeys. Canopy reload, macro variation and page gizmos now belong to F1; Tab requires `--debug-world-switch`.
- Consolidated 87 Markdown documents into five guides; corrected current schemas/defaults and separated operating instructions from historical observations. Raw non-Markdown evidence remains intact. Historical prose is recoverable at `2afce93`.

## Completed: launch configuration and runtime settings ownership

[LaunchOptions](../crates/app_game/src/launch.rs) validates the game interface once before startup, including unknown/retired flags, values, duplicates and conflicting control/exit modes. Normal, repro and profile precedence is explicit; useful automation interfaces remain. Shared libraries and default constructors no longer read process arguments. Standalone examples/editor supply their own configuration.

F1 now owns canopy reload, terrain macro variation and page gizmos. Reset/A/B snapshots store actual canopy values, so restoring does not reread a changed file. The old independent B/G/H/U/V writers are gone. The previous catalog asset and intermediate display-only pacing experiment were deleted. Useful overlay counters moved to F1 Overview.

[RuntimeSettingsPlugin](../crates/app_game/src/runtime_settings.rs) now owns shared settings and application systems, independently of F1. [PerformancePanelPlugin](../crates/app_game/src/render_audit.rs) owns controls, panel sampling and capture state. Runtime initialization precedes output setup and the panel's launch baseline. Repro/profile setup is independent of panel installation; input locking still applies without UI. Diagnostic overrides remain in the shared snapshot so existing Reset/A/B behavior stays complete.

`--diagnostics off|panel|full` now chooses startup composition. Full preserves existing defaults. Panel omits CPU system wrappers, GPU timestamps and the Bevy recorder while retaining F1/frame/app timings; Off also omits F1 and its sampling. The vegetation renderer no longer installs instrumentation implicitly. The editor opts into its existing recorder explicitly. Timing infrastructure no longer imports game settings, and the empty-frame example supplies its own frame stamp without a fake audit resource.

Validation through this batch: **478 Rust tests passed / 29 ignored**; game-specific startup, Reset, capture and composition checks pass with and without the panel/probes. Workspace/all-targets check and clippy completed (existing style/complexity warnings remain), formatting/diff checks pass, and iOS compile checking passes. Three bounded native Metal runs (`off`, `panel`, `full`) loaded the world, saved inspected screenshots and exited successfully at frame 900; these are functional checks, not performance comparisons or device acceptance. All 154 raw evidence files remain byte-identical, and the five guides have no broken local links. The previous controls batch also passed all 29 Python profiler tests and the no-MetalFX build; neither path changed here.

Remaining settings work: separate diagnostic override data further if a second settings consumer needs that API; keep catalog/file overrides explicit with identity/revision and capture invalidation. The known Temporal GPU-marker undercount remains unresolved; sampled spans must not be presented as total cost.

## Completed: production vegetation integration

[WorldVegetationPlugin](../crates/engine/src/world_vegetation.rs) now owns game scene assembly, source joining, LOD focus and canopy composition. `main` installs it explicitly. The old `grass_field` module is gone; canopy file loading stays with application settings. `VegetationSceneState` and `VegetationView` replace the misleading render scene/view names without changing serialized settings or payload formats.

Game/editor share terrain conversion and one [GroundCanopyPlugin](../crates/engine/src/world_vegetation/canopy.rs); the editor still owns draft selection and publication. The extraction also fixes stale-source cases: disabled grass across world/rebase transitions, in-place joined relief/coverage changes, pending bake results after source changes, and material replacement/unload. Editor fallback now matches terrain and vegetation domains by their shared cell/LOD/extent. Canopy shading changes do not rebake unchanged sources; unrelated pages do not invalidate existing masks.

Grass Disabled remains render isolation, with source joining now kept current for canopy/contact consumers. This changes CPU activity during disabled-grass streaming, so old grass-off measurements are not a matched control for this batch. Source comparisons and the shared cache also need a controlled comparison before claiming a performance gain. Editor-specific accepted-source/revision handling remains outside the game plugin.

Validation: **485 Rust tests passed / 29 ignored**, including eight vegetation lifecycle tests covering source edits, world/origin/catalog changes, disable/resume, material replacement, stale jobs and bounded baking. Workspace clippy and iOS compile checking pass with existing warnings; formatting/diff checks pass. Native Metal game runs with hierarchy and legacy terrain each captured a screenshot and exited at frame 900. The editor study captured its viewport, UI, settings and diagnostics using a temporary project database. All three captures were inspected; this does not cover manual editor authoring or establish a performance comparison.

Further vegetation work: separate the serialized settings' production and diagnostic fields only with a saved-study compatibility plan. Full subsystem suspension/teardown is a separate contract from hiding vegetation draws.

## Completed: gameplay, input and camera ownership

[GameplayPlugins](../crates/engine/src/gameplay.rs) composes core actor/movement/grounding behavior with optional native input, camera and destination-marker plugins. `MinimalGamePlugin` now only composes existing subsystems; the engine root is an export facade. Camera/control algorithms and launch defaults are unchanged. Omitting native input permits scripted movement/follow without Bevy input resources; omitting the marker avoids its entity/assets and update system.

Named `GameplaySystems` stages replace the broad `GameInputSystems` chain. F1/settings run before input, streaming finishes before gameplay consumers, presentation resolves motor configuration before motion, and repro overrides run after camera follow. Streaming no longer names a game-input set. The editor retains its own input/camera composition. See [Architecture](ARCHITECTURE.md#characters-and-editor-lifecycle) for ownership and composition contracts.

Validation: **489 Rust tests passed / 29 ignored**. New tests exercise all eight optional-plugin combinations, same-frame intent/movement/grounding/follow, late camera overrides, freeze/resume and keyboard input during pointer capture. Existing gesture batching/draining and bookmark tests still pass. Workspace clippy, formatting/diff and iOS compile checks pass with existing warnings. A native Metal zoom route ran to frame 900 with full diagnostics/F1, with two inspected captures and changing camera poses in the log; the editor study also captured/exited against a temporary database. Physical gamepad/touch-device acceptance and performance comparisons were not run.

## Completed: database worker and request protocol

[Database transport](../crates/engine/src/world_streaming/database.rs) now owns its startup plugin, thread and channels. Plain request/reply types and all immutable SQLite queries moved out of residency and terrain/material modules. The streaming coordinator routes replies and owns world/generation transitions; source decoding/admission moved into residency in the following batch. Request identity, two-phase generation adoption, queue capacities and page budgets are unchanged; the source-page limit now has a name that covers loading, decoding and waiting for attachment.

Teardown previously sent `Shutdown` through the bounded work queue and joined a worker that could already be blocked delivering a reply. A separate cancellation channel now interrupts both channel waits before joining; synchronous SQLite work still completes before exit. Tests force full request/reply queues, an idle worker, a startup reply wait and open failure. No performance gain or runtime worker-disable mode is claimed.

Validation: **492 Rust tests passed / 29 ignored** in the normal workspace run. Workspace clippy, formatting/diff, game/editor builds and iOS compile checking pass with existing warnings. The ignored native Metal terrain lifecycle test also passed separately against its temporary 2 km world: geometry/material IO and uploads, rendering, live authoring, rebasing, world transitions, contact coverage, and publication rejection/retry. This is functional acceptance, not a performance comparison. The five guides remain the only documentation guides; all 154 raw evidence files remain unchanged.

## Continue modularizing streaming and experiments

Runtime settings, F1 and timing probes now have independent owners. Their composition contract is documented in [Performance](PERFORMANCE.md). CPU wrapping remains a startup choice, and instrumentation overhead still needs a controlled comparison if it becomes a performance question.

Further composition within existing crates:

| Proposed module/plugin | Responsibility |
| --- | --- |
| Internals of existing `WorldStreamingPlugin` | Further isolate world/generation transition coordination if needed; residency, attachment and object LOD are already separated |
| `ExperimentsPlugin` | Explicitly enabled alternative implementations/catalogs |

`GameRenderPlugin` already owns presentation/output; platform pacing, profile routes and native captures remain explicit application setup.

Keep further splits within existing crates until stable resource contracts justify a crate boundary. Legacy overlays are removed and page gizmos have an explicit F1 setting. Keep detailed CPU wrapping a startup choice unless safe dynamic removal is implemented.

Define three different “off” contracts: hide output for measurement; suspend runtime activity with resume/cleanup; omit a capability at composition/build time. Existing grass/terrain/object/cloud toggles are partial isolation, not complete teardown. `run_if` does not dispose of async jobs, entities or GPU resources. Preserve contact data while actors need it; establish resource contracts before allowing plugin omission.

Acceptance for further splits: ordinary game/editor behavior survives omission where supported, and resume/cleanup contracts are covered. Instrumented/uninstrumented cost must be measured independently when needed.

## Completed: source residency and attachment

[SourceResidency](../crates/engine/src/world_streaming/residency.rs) now owns page lifetime separately from world/index state. Its systems handle bounded requests, decode completion, admission, cooling, statistics and cleanup. [Attachment](../crates/engine/src/world_streaming/residency/attachment.rs) owns payload-to-entity conversion, including CPU height sources previously mixed into demand planning. The coordinator shrank from 2,543 to 1,610 lines. Public APIs, system order, budgets, cooling duration and serialized payloads remain unchanged; no new crate or runtime toggle was added.

Fixed failed attachment leaving earlier objects unowned when a later object had invalid dependencies/definitions. Entire object pages are validated before spawning; fallible terrain preparation also precedes owned asset creation. Existing same-world rebase preservation and generation/world cleanup contracts remain covered. No performance improvement is claimed.

Validation: **498 Rust tests passed / 29 ignored**, with six new regression tests for failed attachment, stale replies/decode jobs, cooling revival/expiry, owned asset removal and shared texture accounting. Workspace clippy, formatting/diff, iOS compile and game/editor builds pass with existing warnings. The native Metal terrain test passed separately, covering rendering, rebasing, live edits, world transitions and rejected/retried publication against a temporary world. A legacy-terrain game run also reached frame 900 and saved an inspected screenshot. These are functional checks, not performance comparisons or physical iOS acceptance.

## Completed: object LOD and opt-in streaming smoke

[ObjectLodPlugin](../crates/engine/src/object_lod.rs) owns shared runtime/editor visual LOD selection and initialization. Thresholds, hysteresis, switch limits and post-transform ordering are unchanged. The coordinator now contains 1,254 lines (down from 1,610 before this batch).

[StreamingSmokePlugin](../crates/engine/src/world_streaming/smoke.rs) is installed by the game only for `--streaming-smoke`. Removed its always-registered system/runtime boolean from normal world-streaming composition; the scenario now runs explicitly after streaming and gameplay. Its old render-space/fixed-radius traversal assertion was incompatible with floating origins and camera-driven source demand. It now verifies the canonical destination and rejects any surviving page outside current demand. The demo-world activation and exit checks remain; this is not a general validator for arbitrary authored worlds.

Validation: **504 Rust tests passed / 29 ignored**. Six new tests cover LOD scheduling/budgets/scale limits, plugin omission, canonical source ownership and one-time diagnostic exit. Workspace clippy, formatting/diff, iOS compilation and game/editor builds pass with existing warnings. Native smoke passed all three checkpoints in both terrain modes using a newly cooked temporary demo. An initial run against the current 8 m world failed the fixed three-second LOD-readiness assertion; its normal hierarchy run subsequently reached frame 900 with an inspected screenshot. The fixture requirement and runnable commands are recorded in [Workflows](WORKFLOWS.md#validation-and-platforms). These are functional checks; no performance improvement is claimed.

## Completed: F1 capture and presentation separation

[Performance diagnostics](../crates/app_game/src/render_audit/performance.rs) now composes four internal modules: panel UI, capture lifecycle, rolling telemetry, and reports/statistics/export. `PanelState` only holds visibility/tab selection; `CaptureSession` owns the launch baseline, A/B slots and active recording. Settings controls no longer depend on UI state, and recording tests run without the panel. Capture completion is passed back to presentation. The former 1,454-line file is a 19-line composition entry point; the largest new implementation module is the 451-line panel.

Reset/restore values, two-second settling, eight-second readiness deadline, ten-second sampling, cancellation, probe filtering, CSV formats and diagnostic modes remain unchanged. Removed an unused thermal cache; no CLI switches, crates or documents were added. This is an ownership refactor, with no performance improvement claimed.

Validation: **510 Rust tests passed / 29 ignored**. Six new tests cover UI cancellation/result preservation, completion/abort ordering, overlapping recordings, CPU sample provenance, rolling history limits and empty export. Existing Reset, canopy, A/B restore, GPU filtering and CSV escaping checks pass. Workspace clippy, formatting/diff, iOS compile and game build pass with existing warnings. Native Full and Panel runs each reached frame 900 and saved inspected F1 screenshots; Full showed probe data and Panel correctly reported probes disabled. These are functional checks, not performance comparisons or physical-device acceptance. The five-guide structure and all 154 raw evidence files remain intact.

## Completed: world database ownership

[world_db](../crates/world_db/src/lib.rs) now exposes the same public API through a 34-line facade, down from a 2,684-line root. Internal modules separate record/error contracts, bounded editor reads and revisioned transactions, whole-project import/export, immutable runtime reads, runtime output writing, and shared catalogs/codecs/budgets. The incremental cook writer now lives beside the whole-build writer and shares its SQL; the source cook snapshot has no output-writing responsibility. Domain stores import their internal dependencies explicitly.

This is a structural move: all 205 SQL literals, schemas, binary encodings, limits and transaction boundaries are unchanged. No migration, recook, new crate or dependency change is required, and no performance/build-size improvement is claimed. Existing database tests were retained; the source/runtime round-trip check now exercises the public crate API as an integration test.

Validation: **510 Rust tests passed / 29 ignored**, including existing bounded-read, conflict/rollback, snapshot isolation and staged/reference cook parity coverage. Workspace clippy, API documentation generation, formatting/diff, iOS compilation and game/editor builds pass with existing warnings. The native Metal terrain lifecycle test passed separately against temporary databases, covering uploads/rendering, live authoring, world/rebase transitions and rejected/retried publication. All 213 public declarations and existing function bodies/data declarations were retained; the five guides and 154 raw evidence files remain intact. No physical-device or performance acceptance is claimed.

## Completed: vegetation renderer ownership

The [renderer](../crates/vegetation_render/src/renderer.rs) is now a 73-line composition entry point, down from 3,034 lines. Internal modules own GPU layouts, CPU packing, index templates, buffers/bind groups, pipelines, uploads, placement dispatch, drawing and telemetry. Existing cache, blade-preparation, temporal and canopy modules import those dependencies explicitly. Packing, topology, allocation and cache tests live beside their implementations; native fixtures and shared shader checks remain available.

All 199 existing function bodies/data declarations and all constants are preserved apart from internal paths and visibility. Shader files, binding layouts, budgets, cache invalidation, resource initialization and render ordering are unchanged, including temporal dependencies and the iOS readback exclusion. Public APIs, serialized settings, CLI controls and dependencies are unchanged. This establishes internal ownership; it adds no independently removable plugin or performance claim.

Validation: **510 workspace tests passed / 29 ignored**. Seven selected native Metal regressions passed separately: scheduler bounds/counters, placement rejection equivalence, prepared/reference wind/MSAA/overflow, candidate-cache/source lifetime, rebasing, terrain gating/frozen draws, and temporal motion/history/fallback. Workspace clippy, formatting/diff, iOS compile checking and game/editor builds pass with existing warnings. These are functional checks, not performance comparisons or physical iOS acceptance. The five guides and all 154 raw evidence files remain intact.

## Completed: editor world and vegetation ownership

[World workspace composition](../crates/app_editor/src/workspaces/world.rs) now names dedicated camera, input/picking, object-proxy, gizmo and overlay modules. Removed the generic 1,616-line `world_impl.rs`; the former 1,477-line `world_ui.rs` is split into toolbar/window composition and individual views. [Vegetation authoring](../crates/app_editor/src/vegetation_authoring.rs) shrank from 1,869 to 64 lines, with draft/save handling, live preview and inspector controls in separate modules. [Vegetation study composition](../crates/app_editor/src/workspaces/vegetation.rs) shrank from 1,170 to 80 lines, separating session state, viewport, enter/leave restoration and capture/export. Existing tests moved beside their owners.

This is a structural move. Camera/picking behavior, grouped undo commands, save conflicts, source acceptance, workspace restoration, study formats, capture readiness and UI labels/layout remain unchanged. System registration/order, run conditions and external editor entry points are preserved; no new crate, plugin toggle or performance improvement is claimed.

Validation: **510 workspace tests passed / 29 ignored**, including 130 editor tests. Workspace clippy, formatting/diff and the native editor build pass with existing warnings. Native checks against a copied project database covered World windows, selection/inspector routing, World → Vegetation → World restoration, and vegetation inspector/capture/export; both completed without runtime errors. These are functional checks, not performance measurements. The five guides and all 154 raw evidence files remain intact.

## Further structural work

Preserve shader layouts, ordering, bounds, stale-result rejection and transaction semantics. Share low-level joining/cache mechanisms without merging editor drafts into gameplay. Use meaningful `SystemParam`/query groups, not opaque wrappers solely to silence argument-count warnings. Review large queue/state enum payloads for boxing only where storage/allocation tradeoffs justify it; no performance gain is assumed.

Possible later crates: shared devtools once a second consumer exists; `world_runtime` after viewpoint/contact-demand contracts separate it from actors; runtime DB reader if excluding authoring/compiler dependencies measurably helps builds/packaging. Do not create a crate for every system or experiment.

## Completed: standalone experiment retirement

Removed 13 files / 2,888 lines of standalone studies and historical recipes: the test-only shadow/tuft fixture bundle and report tools; `grass_field_study.py/.html`; `grass_study_benchmark.py`; `grass_density_experiment.py`; and `grass_curve_sampling.py`. Removed their module registration and unused preprocessing wrapper. The field runner generated a duplicate shader output location; the benchmark overwrote live shaders. Neither is needed by the current editor or controlled profiler.

Production shaders, Temporal integration, fallback/reference paths and current placement/shading/overflow tests are unchanged. Current study capture, density profiling, shape comparison and the isolated game benchmark remain. Results and limitations stay in [the experiment ledger](EXPERIMENTS.md); removed implementations are recoverable from Git at `efc6566`.

Validation: **508 workspace tests passed / 27 ignored**, plus all 29 Python profiler tests. The retired fixtures account for two passing tests and two ignored captures removed from the previous total. Workspace clippy passes with existing warnings; formatting, diff, surviving Python imports and documentation links pass. All 154 raw evidence files remain byte-identical. No native render rerun was needed for this test/tool-only deletion.

## Retire old implementations with explicit gates

Remaining recommendations from the September 22 caller/dependency audit:

| Candidate | Recommendation / dependency |
| --- | --- |
| Blade-band experiment | **Retire next:** shader/pipeline branches, editor controls, CLI and `grass_blade_band_study.py/.html`. Defaults and tracked studies use Off; it was never accepted as physical shadows. Handle saved-study fields explicitly; the script imports `sparse_source` from the ground-study script. |
| `VegetationLightingMode::Legacy` | **Retire next:** previous empirical lighting remains selectable in the editor and covered in shader comparisons. Preserve current lighting, unlit/vertex diagnostics, numeric IDs and deliberate old-study handling. |
| Old ground-treatment modes | **Trim separately:** Original/Darkened/Understory trials and their runner. Keep canopy integration, its shared bake/shader code and useful coverage diagnostics; two tracked studies use `CanopyGroundStudy`. Do not delete `ground_treatment` wholesale. |
| `PagePayload::TerrainRender` | **Retire in a format batch:** current cooker emits heightfields; remaining constructors are tests. Readers still accept it. Reserve its bincode encoding or bump payload/runtime versions and recook; never shift discriminants silently. |
| `--terrain-legacy` / iOS flattening | **Keep pending acceptance:** hierarchy sustained cost remains unresolved. Legacy iOS attachment also substitutes flat geometry/contact data. Retire after a matched soak and physical-device relief/contact check. Shared `TerrainMaterial` remains in current near detail/editor previews. |
| Grass and terrain reference paths | **Keep:** unprepared blades and uncached candidate acceptance handle normal misses/overflow; early-rejection reference is a correctness oracle. Terrain procedural/uncached controls and portable textures handle loading, budgets and device support. Removing diagnostic switches does not remove these responsibilities. |
| Temporal, Linear and cloud alternatives | **Keep as supported rendering paths:** Temporal is integrated and selectable, with recorded cost/blur issues requiring follow-up; it is not a legacy-removal candidate. Linear is the portability fallback. Balanced cached clouds and High direct clouds are current quality modes. Standard tone-map output remains a fallback for unsupported direct-output conditions. |

Suggested order: blade bands/legacy lighting with saved-study handling and native image checks, then ground-mode simplification. Payload and terrain-renderer retirement are separate batches.

Preserve CPU vegetation oracles, explicit cooker fixtures/parity helpers, Linear upscaling, timer pacing fallbacks and the documented wgpu patch. The staged cooker calls shared compilation code also used by the whole-document reference; deleting it as legacy would break production.

Each batch should remain independently reviewable. Keep payload changes, algorithm changes and structural moves separate. CPU tests/fmt/clippy are the routine gate; run targeted native/image/lifecycle checks when touching renderer behavior. Existing ignored hardware tests are not covered by ordinary `cargo test`. No tracked GitHub Actions workflow was found in the audit; a CPU-only validation job is a later improvement if no external CI covers it.
