# Workflows

Commands run from the repository root. [Architecture](ARCHITECTURE.md) describes ownership; [performance](PERFORMANCE.md) describes controlled measurement.

## Prepare, edit and publish

Restore local assets according to [pack contracts](../assets/README.md). Licensed sources and generated databases stay ignored. After restoring terrain inputs:

```sh
python3 tools/compile_terrain_textures.py
python3 tools/prepare_terrain_bake.py
cargo run -p yarra-world-cook -- init
cargo run -p yarra-app-editor
cargo run --release -p yarra-app-game
```

`init` creates source only if absent, then cooks it: the procedural Phase 0 island (8 km of sea and island, a few minutes to cook), with start views in `content/world.project.views/` (`spawn`, `beach`, `summit`, `hills`), e.g. `cargo run --release -p yarra-app-game -- --start-view content/world.project.views/beach.ron`. The local default world is currently imported from Houdini instead (see [Terrain from Houdini](#terrain-from-houdini)). To start over, delete the project and runtime databases and run `init` again. `cook` continues the existing runtime: it recompiles only cells whose sources changed and updates the terrain hierarchy and ground composites above them. Ground-composite cores are kept in `content/world.project.cook-cache.sqlite` for this (about 570 MB for the island). Deleting it is safe; the next cook re-evaluates what it needs. `cook --full` rebuilds the runtime from scratch. `cook` requires existing source and never creates a demo. The editor defaults to `content/world.project.sqlite`; the game reads `assets/generated/world.runtime.sqlite`. A fresh public clone needs the local pack inputs before ordinary material cooking succeeds.

**Save** writes source edits. **Save & Publish** also cooks and atomically replaces the runtime. To publish from a terminal:

```sh
cargo run -p yarra-world-cook -- cook
```

Explicit paths: `init PROJECT_DB RUNTIME_DB`, `cook PROJECT_DB RUNTIME_DB`, editor `--project-db PROJECT_DB --world-db RUNTIME_DB`, game `--world-db RUNTIME_DB`. The editor validates the source/runtime pair before opening its window. Recook outdated runtime data; source incompatibility requires an explicit replacement decision, not an automatic reset.

### Terrain from Houdini

A heightfield from a terrain tool defines the world: its footprint sets the cells, and its heights replace the default world's terrain. Houdini's `hython` (Apprentice or Indie) writes the neutral format, float32 heights plus a JSON manifest:

```sh
/Applications/Houdini/Current/Frameworks/Houdini.framework/Versions/Current/Resources/bin/hython \
  tools/houdini_export_heightfield.py ~/Dev/world_next.hipnc /tmp/island --start 2500 4330
cargo run --release -p yarra-world-cook -- import-heightfield /tmp/island/heightfield.json
cargo run --release -p yarra-app-game
```

The script exports the display node's `height` volume (or `--node SOP`) with Houdini's axes unchanged: in the top view +X is right and +Z is down. `import-heightfield MANIFEST [PROJECT_DB] [RUNTIME_DB]` creates the project if it is missing, then cooks. It samples the heightfield every metre with smooth (Catmull-Rom) interpolation and paints ground from height and slope. The world is widened to whole 1 km blocks of flat sea so the terrain hierarchy closes. The `--start` point (by default the shore nearest the centre) becomes the world's start: the game and editor begin there unless `--start-view` overrides it. `start` and `summit` views are also written beside the project. Moving the start recooks nothing. Re-importing after a change in Houdini rewrites only cells whose heights or paint changed, so the cook that follows is incremental. Objects and roads in the project are kept; a cell the new footprint no longer covers is removed and fails if it still holds them. Sculpt in Houdini, not in the editor: a re-import replaces heights.

To stress streaming on foot, `--render-repro actor-walk --start-view VIEW` walks the player along the view's `route` at 20 m/s. The camera follows, holds its heading for 60 s, then looks back and forth every 20 s. Add `--fps 60` to match a 60 Hz display. `ACTOR_WALK` lines log progress. Whenever an actor waits more than 5 s for ground, the game logs `TERRAIN_STALL` with the loader's state; a loader that stops for good logs `TERRAIN_LOD_FAILED`. To send a log of a normal session: `cargo run --release -p yarra-app-game 2>&1 | tee tmp/walk.log`.

## Game and F1

- Click/tap ground for a destination; WASD/left gamepad stick gives camera-relative movement.
- Right-drag, horizontal trackpad/two-finger gestures or right stick orbit. Wheel/pinch/vertical gestures zoom.
- Tab cycles demo world spaces only with `--debug-world-switch`; ordinary launches have no world-switch shortcut.
- F1 or **Performance** opens the panel; Escape closes it or cancels capture. `--performance-open` starts open. `--diagnostics panel` keeps F1 with frame/app timing only; `--diagnostics off` omits F1 and timing instrumentation. Full diagnostics remain the default.
- The persistent **FPS limit** button cycles Follow display → 30 → 60 → 120. `--fps N` accepts 0 or 15–240; Reset restores the launch cap, including custom values. VSync remains enabled.
- Quality controls resolution (100/75/50/33%), upscaler, MSAA, density and terrain/object detail. Auto/Spatial/Linear are normal choices; Temporal remains an explicit prototype.
- Weather follows a random sequence by default. `--weather clear|scattered|overcast|rain|storm` starts in a held preset; `--weather authored` shows the published profile unchanged. **F1 → Weather** forces presets (60 s, 10 s or instant blends), toggles the automatic sequence, starts the next change and accelerates the weather clock (1×/10×/60×/stopped). Rain and Storm draw rain streaks, splashes and, as wetness builds up (about 1.5 minutes to soak, 4 minutes to dry), wet surfaces and puddles; ground under trees and other objects stays drier. **Rain rendering** hides streaks and splashes for comparisons.

On macOS 14+, capped modes coordinate display callbacks and the Metal minimum presentation interval. Other platforms/older macOS use the timer fallback. This changes app pacing, not the display's system setting. The normal launch follows the display and uses 50% resolution with 4× MSAA.

F1 Advanced owns terrain macro variation, terrain page gizmos and canopy reload. Reset and A/B restore include those settings and the actual canopy values. B/G/H/U/V and the old X/P/O/L/K/I renderer handlers were removed. Normal grass always uses the published catalog; separate legacy overlays and banding controls are gone.

## World authoring

World and Inspector are the default windows. Tools opens optional Assets, Navigator and Diagnostics. Switching workspaces retains source drafts/history; opening a window does not automatically activate its authoring tool.

Navigation: right/middle drag orbits, Shift+right drag pans, wheel/pinch zooms, and right mouse + WASD/QE flies. Navigator bookmarks can be passed to either app as `--start-view FILE`; the file stores a logical view rather than a screenshot.

**Objects:** select visible objects or stable IDs in the bounded asset tree. Shift/Cmd-click builds a multi-selection. 1/2/3 selects move/yaw/uniform-scale gizmos. Delete/Backspace is undoable. Cmd+Z / Cmd+Shift+Z undo/redo; Cmd+S saves. The inspector affects the active object; gizmo operations can apply to selected companions. Source changes are represented by temporary proxies until publication.

**Environment:** choose World → Environment, select a layer, drag to paint, Shift-drag to erase. One stroke is one undo step. Inspector supports Nearby/All, layer creation/order, coverage and per-use settings. **Edit shared preset…** opens Presets with an isolated ground/grass/object preview. **Apply preset changes** and **Apply settings** are distinct undo steps.

**Collections:** Presets → Environment → New → Asset collection. Select registered tree/bush/rock assets, weights, scales, spacing, slope and road clearance. Paint a collection directly or include it in a composition. Layer density/seed control deterministic generated placements. Generated objects are not individually editable; use manual placement when independent identity/editing is needed.

**Forest assets:** Follow [asset setup](../assets/README.md) to export the Forest Tree Starter Kit, then run `cargo run -p yarra-world-cook -- import-assets assets/packs/forest_tree_starter_kit/summer.catalog.ron [PROJECT_DB]` (omit the optional project argument for the current world). This registers 11 summer trees and 8 shrubs, each with four authored mesh LODs. Restart the editor to refresh the palette; search “Forest” for manual placement or select the assets in a collection. Registration preserves existing world placements and does not publish automatically. Save & Publish after authoring. The normal renderer retains object pages within 192 m in all directions, subject to residency budgets, and uses their mesh LODs. Off-screen trees remain available to cast shadows after camera turns; Bevy still culls individual draws. Visible pages load before off-screen ones; there are no usable billboards yet. Legacy terrain diagnostics retain their old local object window.

Imported forest foliage now animates in both game and editor. It follows the shared wind enable/direction/strength and preview transport, with rigid bark and authored leaf weights. The existing game Wind control affects trees and grass together. `--upscaler metalfx-temporal` uses deformation-aware tree motion vectors automatically. To check the wind implementation, run `cargo test -p yarra-engine tree_wind`; with native GPU access, also run `cargo test -p yarra-engine tree_wind::gpu_tests -- --ignored`. The current local exports already include wind metadata; no world recook is needed for the shader/material update.

For tree LOD dropout regression, run `cargo test -p yarra-engine imported_tree_lod_materials_remain_drawable -- --ignored --nocapture` with native GPU access and the local forest pack. It switches an imported tree through all four LODs without Temporal AA and checks that bark and foliage both have prepared GPU materials and compiled color/depth/motion pipelines on every transition frame. Material conversion must finish before Bevy's asset events; wind bounds expand separately before culling. `--metalfx-timing-log` also logs every camera/history/target reset as `TEMPORAL_HISTORY_RESET`, so remaining full-view flicker can be distinguished from local LOD draw gaps.

For grass streaming/history regression, run `cargo test -p yarra-vegetation-render --features upscaling/metalfx temporal_offscreen_grass_streaming_preserves_history -- --ignored --nocapture` with native MetalFX access. It repacks offscreen pages while stationary, strafing and animating wind, exercises empty/resident and disabled/enabled transitions, and checks history plus depth-derived motion in prepared/procedural paths. Page residency must not reset whole-view history; catalog edits and camera cuts still do. `--metalfx-timing-log` includes `GRASS_TEMPORAL_HISTORY` events with source/catalog/visibility/discontinuity reasons alongside native reset/timing logs.

**Roads:** activate Roads, choose a style, then New cart road and place two points. Edit control points, tangents and widths; extend/split with the tool controls. Road styles define wheel/center/shoulder wear, retained grass, ground mixtures and rut/relief variation. Explicit junctions connect compatible 2–4-arm endpoints. Save checkpoints roads and painter changes together; publication derives ground, grass and relief from the same source.

**Atmosphere:** open World → Atmosphere for the active space's profile and preview time/weather. **Weather** edits each preset (clouds, visibility, fog and skylight grey, exposure, wind, rain) and the random sequence (change and hold durations, next-state weights); the preview row shows any preset with a chosen wetness, without saving. Apply authored profile edits through normal undo/save/publication. Preview transport and temporary quality controls do not rewrite startup time/weather merely by being adjusted. Clouds Off removes rendering/shadows, not the weather's ambient response.

Save conflicts indicate newer source revisions: resolve/reload the draft rather than forcing a stale overwrite. Recovery data under `.editor` is separate from saved source. Generated preview failures must remain visible as stale/error state; they are not publication success.

## Vegetation and animation studies

The Vegetation workspace renders isolated specimens/fields with camera, wind/time, catalog, palette, ground, density/LOD and reference-image controls. Shape and Colors edit the draft; **Save study** stores local reproduction settings, while **Save & Publish** applies authored catalog changes to the world. Linked image zoom is not camera movement. Use a full field and multiple views/motion for appearance acceptance, not only an attractive close specimen.

```sh
python3 tools/vegetation_study.py open --load content/vegetation/distance-01.ron
python3 tools/vegetation_study.py capture --camera overhead --time 0
python3 tools/vegetation_study.py capture --camera low --character --time 2.5
```

The helper builds the debug editor unless `--no-build` is supplied. `--output DIR` selects a fresh capture directory. Captures contain `viewport.png`, `editor.png`, `study.ron` and diagnostics. Replay loads an unsaved draft; it does not silently save a catalog. Matching pixels require the same renderer/assets/device. Reference originals live under `.editor/vegetation/references`; capture/study data is local. Study versions 1 and 2 remain readable.

Canopy controls isolate combined/ground/blade treatment. **Save canopy look** writes `content/vegetation/canopy-look.ron`; the game loads it at startup or **F1 → Advanced → Reload canopy look**. This is an artistic approximation, not a shadow solution. Shader-level banding studies remain experimental; normal-game controls were removed. The Animation workspace independently previews catalog models/clips and transport without changing gameplay actor authority.

## Explicit fixtures and catalog tools

Use fresh paths for disposable worlds:

```sh
cargo run -p yarra-world-cook -- create-road-demo tmp/roads.project.sqlite
cargo run -p yarra-world-cook -- cook tmp/roads.project.sqlite tmp/roads.runtime.sqlite
cargo run -p yarra-app-editor -- --project-db tmp/roads.project.sqlite --world-db tmp/roads.runtime.sqlite
python3 tools/hill_landscape.py prepare
python3 tools/hill_landscape.py game --view summit
python3 tools/hill_landscape.py editor --view valley
python3 tools/hill_landscape.py descent
```

`create-demo` provides the historical 32 m grass fixture; `create-mountain-fixture` and `create-hill-fixture` provide terrain fixtures. `create-island-fixture` writes the default island (and its views) to another path without cooking. The hill helper uses `tmp/hill-landscape`, preserves existing source, and supports `--recook`. It is separate from normal game content. Pure compiler fixtures remain available as `layered_meadow` and `cart_track` examples in `yarra-environment-compile`.

`export-vegetation PROJECT_DB CATALOG_RON` writes a new catalog file. `import-vegetation CATALOG_RON [PROJECT_DB RUNTIME_DB]` intentionally replaces the catalog and publishes; it is not a read-only preview. The old `demo` and `sync-demo-vegetation` reset commands no longer exist.

## Standalone gameplay and saves

The gameplay content tool runs without game/editor startup, cooked worlds or art assets. Start with the checked-in authored project:

```sh
cargo run --offline -p yarra-game-content -- validate content/gameplay/demo
cargo run --offline -p yarra-game-content -- demo content/gameplay/demo --locale uk
```

The demo loads definitions, actors, starting inventories/wallets, scenario actions and Fluent files from disk. It uses a potion, transfers equipped gear to a companion, stores an item in a chest, trades and completes a dialogue reward. It publishes and retains a content bundle, plays the scenario against that bundle, writes manual/quick/auto saves to a new temporary directory, reloads the manual slot and compares the complete state. It also reports how many dialogue graphs were loaded. Expected results are **75 player health**, **80 party gold**, **10 persuasion XP** and **3 saves**. Use `--locale en` for English, or `--save-dir /path/to/new-directory` to choose a fresh destination. Existing demo save directories are refused.

### Editing and validating gameplay content

Copy `content/gameplay/demo` to start a project. Source format **6** uses explicit package manifests and one asset directory per conversation:

```text
project.ron                         # content/world identity, packages, locale policy
scenario.ron                        # standalone seed and command exercise
packages/core/
  package.ron                       # stable package ID and declared dependencies
  items.ron, rules.ron, actors.ron   # package-owned domain definitions
  messages.ron                      # resource UUID, message/argument contracts, locale paths/reviews
  en.ftl, uk.ftl
packages/old_gate/
  package.ron
  conversations/gate/
    conversation.ron                # graph/bindings/resource paths
    graph.ron                       # exactly one conversation
    bindings.ron                    # local conditions/actions; campaign fact declarations
    messages.ron
    en.ftl, uk.ftl
```

Every path is relative to the **project root**, including paths inside conversation/resource manifests. Paths must remain inside that root after symlink resolution. Package manifests list catalog fragments, actor collections, an optional rules definition, conversations, resources and `quests`/`profiles`/`predicates`/`claims`/`objects`/`areas`/`triggers` file lists. Each quest/profile/named-predicate/claim/object/area/trigger file contains one definition; use empty lists when a package owns none. Shared catalog fragments use the same catalog ID/revision. The project has one rules definition. Cross-package mechanical/text references require a declared dependency (transitive dependencies are allowed). Missing dependencies, cycles, duplicate identities and ambiguous Fluent imports are errors. A conversation's action/condition keys are local: published `BindingId` combines the dialogue UUID and local `Key`. Facts remain explicitly shared campaign keys.

Keep IDs when moving/renaming files. Package/file enumeration order does not change fingerprints; choice display order remains meaningful. Authored text references use `Message((resource: "08080808-0808-0808-0808-080808080808", key: "item-healing_potion"))`; user names can use `Literal("Player name")`. Fluent keys only need to be unique within their resource/import scope. The editor will write these same RON/Fluent sources; no editor integration is installed yet.

`messages.ron` declares a `TextContract` with `id`, explicit `imports` and `messages`. Each message declares an `arguments` map whose values are `Text`, `Number` or `Select(["friendly", "hostile"])`. `locales` entries contain a canonical locale, FTL path and `reviewed` map. Source wording is required for every message contract. Static item/category/template/rule/quest/objective/topic/object/starting-name labels cannot require arguments. UI/dynamic text calls supply typed `localization::Arguments`. Dialogue read models supply `BoundText`; use `Localization::format_bound` to resolve localized actor names/text arguments and render the message in the requested locale.

`validate` checks all Fluent branches, references, attributes, cycles and argument/selector types, including unused term structure. Unknown functions and positional term arguments are rejected; no application functions are registered yet. Literal named term arguments and message/term attributes are supported. Imports merge only the explicitly declared scope, rejecting ambiguous names. A missing translated message or one of its dependencies falls back as a whole to a locale with a complete dependency closure. It never combines a translated line with a source-language term. Invalid syntax/types remain errors.

Translation reviews record each message's source dependency hash, including referenced terms/messages. Changing source wording marks affected translations stale without changing mechanical identity. `validate` prints stale keys and their current revisions; after a translator reviews the wording, record that revision in the locale's `reviewed` map in `messages.ron`. Missing/stale development translations are warnings. Add a locale to `project.ron`'s `shipping_locales` to require complete, reviewed coverage. Source locale is implicitly reviewed. Mechanical argument/import contract changes do change save compatibility.

Validation also runs the scenario against a disposable session and reports the failed step. It does not prove that every possible story branch succeeds. No validation/build command edits source or existing saves. Reads are bounded to 16 MiB per RON document, 2 MiB per Fluent source, 16 MiB combined translations and 64 MiB total source reads. Increment content/catalog revisions for mechanical releases. New-game scenario changes are separate from existing saves' state and RNG.

### Publishing mechanics and language packs

Publish each immutable database at a fresh path:

```sh
mkdir -p tmp/game-content
cargo run --offline -p yarra-game-content -- build content/gameplay/demo tmp/game-content/mechanics-v6.sqlite
cargo run --offline -p yarra-game-content -- build-language content/gameplay/demo en tmp/game-content/en-v1.sqlite
cargo run --offline -p yarra-game-content -- build-language content/gameplay/demo uk tmp/game-content/uk-v1.sqlite
cargo run --offline -p yarra-game-content -- validate tmp/game-content/mechanics-v6.sqlite --language tmp/game-content/en-v1.sqlite --language tmp/game-content/uk-v1.sqlite
cargo run --offline -p yarra-game-content -- demo tmp/game-content/mechanics-v6.sqlite --language tmp/game-content/en-v1.sqlite --language tmp/game-content/uk-v1.sqlite --locale uk
```

Content schema **7** contains one checksummed record per mechanical asset, text contracts and the tool-only scenario. It contains **no FTL**. Language-pack schema **1** stores one locale's resources, their contract hashes and review metadata. A wording fix uses `build-language` with a fresh pack path and requires no mechanical rebuild and does not affect saves. The caller explicitly selects packs; nothing is discovered implicitly.

`ContentRepository::open(path)` reads the manifest only. `headers(kind)` lists identities and checksums without payloads; `read(&AssetId::Item(id))` and `read_kind(kind)` return verified assets. A session uses it through the `ContentSource` port: `core()` once, then `dialogue(id)` per conversation. Text **contracts** are read explicitly with `AssetId::Text(resource_id)`; commands never read or parse wording.

`LanguageRepository::open` likewise decodes zero resources; `load(resource_id)` is an indexed, checksummed lookup in one locale. Compose `LanguageSource::new(content_repository, language_repositories)` with `Localization::with_source(source_locale, Box::new(source), limits)`. This is the runtime path. Formatting reads the requested scope/imports for the selected locale and its fallback candidates only. `Localization::scope` returns a handle pinning parsed Fluent data. Default runtime limits are 32 scopes, 32 imported resources per scope and 32 MiB charged cache bytes. Eviction cannot invalidate active handles; pinned/oversized requests return a budget error. Charges are conservative accounting, not measured RSS. Preparation is separately bounded by one scope budget. The eager authoring constructor allows 64 MiB. Replace the service/provider to activate another immutable pack generation. Use these synchronous readers on an I/O worker when integrating the game.

Inspect mechanics without instantiating a session, executing a scenario or reading packs:

```sh
cargo run --offline -p yarra-game-content -- inspect tmp/game-content/mechanics-v6.sqlite --item 01010101-0101-0101-0101-010101010101
```

`inspect` accepts repeated `--item` and `--dialogue` flags and prints each requested asset.

`LoadedProject::load_directory` eagerly validates authoring data. `materialize_bundle_for_tools` eagerly validates mechanical assets, fingerprints and scenario without packs. `materialize_with_languages_for_tools(bundle, pack_paths)` additionally validates all supplied wording. These are explicit tool APIs; gameplay reads published bundles through `ContentRepository`. `start` imports the authored seed; `run_scenario` also executes the exercise. Each start creates fresh playthrough/item-entry IDs. The demo imports setup/presentation for reporting, then runs commands against the indexed repository and retains content beside its saves.

### Quest-aware NPC interaction

Run the second authored scenario without replacing the inventory/trading regression exercise:

```sh
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard.ron
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard-refusal.ron
```

`LoadedProject::load_directory_with_scenario(root, relative_path)` and the CLI eagerly validate the project and execute the selected exercise in a disposable tool session. Scenario paths use the same project-root bounds as other sources. Runtime coverage in [`game_content/tests/narrative.rs`](../crates/game_content/tests/narrative.rs) runs these same steps against published SQLite content. The CLI does not yet load external exercises against a retained campaign or provide spatial movement.

The guard package owns two quests, a reusable readiness predicate, one interaction profile, a shared reward-claim definition and five conversation assets. The scenario starts with the required key already owned, changes attitude, activates both quests, checks opening priority and independent topics, then returns the key for objective completion, quest completion, attitude and XP. The refusal exercise interrupts between speakers, restarts, remembers a refusal, selects its follow-up, then lets the player return the key via an explicit topic. Quest/objective names and topic labels use the package's English/Ukrainian Fluent resource; dialogue lines can share that resource or own a separate one.

An actor template's `interaction: Some(profile_id)` attaches a profile; `None` explicitly declares no selector. A profile has 1–64 rules with a local ID, priority, tie order, optional topic label, condition and 1–16 positively weighted dialogue variants. Higher priority wins, then lower `order`; duplicate `(priority, order)` pairs are rejected. Automatic opening considers only rules without a topic. Every eligible topic is returned independently, keeping concurrent quests discoverable. Weights select within the winning rule. Derived `DialogueContract` records contain role declarations, history scope, repeat policy and line/choice IDs. Profile previews read these contracts and scoped history to filter ineligible variants; `unavailable_variants` explains repeat/cooldown exclusions. Full graphs remain outside the preview dependency closure.

Conditions support `All`, `Any`, `Not`, UUID-addressed `Named`, quest status, objective completion, directed attitude, items, facts, skill XP, scoped `History` counts and `Claimed`. History conditions reference a dialogue and an event (`Started`, `Completed`, `Interrupted`, `Line(id)` or `Choice(id)`); the dialogue contract supplies the scope and validates referenced IDs. Each tree is bounded; resolved named expansions are cycle-checked and share evaluation's depth/work budget. `Participant::Player` is the interacting actor and `Speaker` is the addressed NPC. Relationships are directed, clamped to −100…100, and resolve to neutral only after an indexed absence lookup. Quest progress similarly resolves to `NotStarted` only for a requested, defined quest. An unrequested record remains unavailable in a read model. Neither default is written by inspection.

Use `preview_interaction(participant, speaker)` for sorted candidates, observed leaf values and eligibility, `opening()` for the chosen rule, and `topics()` for available topics. Preview does not change state, generation or either RNG stream. Submit `Command::Talk { participant, speaker, topic: None, bindings: Default::default() }` to choose an opening or `Some(rule_id)` to request an eligible topic. Only the selected graph loads. The committed outcome includes `InteractionSelected`; its profile/rule/dialogue are persisted separately from graph progress. An active conversation resumes without rerolling or rebinding, including after save/load; a topic request during it is rejected. Greeting variation uses a saved narrative RNG separate from skill rolls.

`Command::Quest` and `Command::AdjustRelationship` are explicit domain commands. Dialogue `Action::Quest` and `Action::Relationship` can combine these changes with item/XP rewards in one transaction. Required objectives gate completion; failed/completed quests are terminal and duplicate transitions fail. There is no implicit quest auto-completion. World distance/access authorization belongs to the future application adapter.

### Conversation runs, history and rewards

Graphs declare `roles` (including `player` and `speaker`), `history_scope`, `repeat`, a start node and nodes containing ordered `lines`. A line has a graph-wide stable ID, speaker role, text reference and argument sources. Choices likewise have explicit arguments and repeat policy. `Talk` and `StartDialogue` automatically bind the two reserved roles; their `bindings` map supplies exactly the remaining declared roles. Missing/extra roles, reserved overrides and unknown actors reject the start atomically. At most 16 roles, 64 lines per node and 4,096 distinct line/choice IDs per graph are supported.

Use `conversation_view(ConversationKey)` for status, node, token, current line and available choices. A line includes the bound actor ID and `BoundText`; gameplay returns references and typed values, never rendered strings. Argument sources are `ActorName(role)`, `Attribute { role, attribute }` (derived attributes), static `Text`, integer `Number` and declared `Select`. Publication and bounded graph loading validate exact message argument names/types and declared roles. A `Text` argument must reference static text. `Localization::format_bound(locale, &line.text)` resolves actor-name/static references through the central Fluent service. Reading or formatting a line records nothing.

After presentation, submit `AdvanceLine { key, expected: view.token }`. This commits the line-history count and saved cursor. Choices appear only after every line at the node is acknowledged. A node with no choices completes at its last line. `Choose` also requires the view's `expected` token; use it when the input was shown, rather than fetching a new token just to retry an old input. Tokens contain run and step counters, rejecting duplicate/stale input across node loops and reopened runs. `InterruptDialogue` takes the same token, marks that run interrupted and records no completion or reward. A later start begins a fresh run if policy allows; an active saved run resumes at its saved line. The scenario steps `AdvanceLine`, `ExpectLine` and `Interrupt` use these same APIs.

Graph repeat policies are `Always`, `OnceCompleted` and `Cooldown { millis }` measured from the last committed start using saved logical time. An interrupted introduction with `OnceCompleted` can restart. Choices independently declare `Always`, `OncePerRun` or `OnceEver`. Graph history scope is `Playthrough`, `Player`, `Speaker` or `Interaction` (the ordered player/NPC pair). The state keeps bounded summaries of starts, lines, choices, completions, interruptions and last-start time; it never scans or saves an unlimited transcript. Scope applies to repeat eligibility and history predicates. Reusable graphs on different NPCs remain independent with `Interaction` scope. New starts replace the latest run record, preserve durable history, and increment the run token.

Package-owned `ClaimDefinition { id, scope }` assets use the same scope selectors and a stable UUID independent of dialogue identity. Wrap an atomic reward group in `Action::Claim { claim, actions }`. The first accepted group executes its actions and records the claim in the same transaction as inventory, quest, XP, history, cursor and RNG changes. Later attempts skip that group, including its random rolls; ordinary conversation choices can still repeat. Different graphs/NPCs can reference one shared playthrough claim, or use an actor/pair scope. `Condition::Claimed` can also hide a claimed offer. Nested claim groups share the action complexity budget. All effects and claims roll back on failure; previews never claim rewards. Inventory/rule conditions remain separate from the claim guard, so reopening a conversation need not grant the reward again.

[`conversation_runs.rs`](../crates/game_content/tests/conversation_runs.rs) exercises hub loops, scoped claims across graphs/NPCs, three-role localization, mid-line restore, remembered refusals, saved cooldowns, and stale inputs. These are standalone domain contracts. Spatial reach/access, proximity scheduling, recent-variant avoidance, background barks and role selection from the live party remain application/world follow-ups.

### Session and save APIs

`GameSession::new(content_source, state)` reads the always-loaded definitions, checks the state in full and loads the graphs of conversations that are in progress. Production uses `ContentRepository`; tools and tests use `ToolContent` over a loaded project. `apply(Command)` returns `CommandOutcome { header, events }`, or an error with state, clock and random streams unchanged. `state()` is the whole playthrough; `derived`, `conversation_view`, `preview_interaction`, `quote_trade`, `container_contents` and `next_movement` are read models. Absent records read as defaults through the state accessors (`quest`, `relationship`, `history`, `object`, `location`, `trigger`).

`LoadedProject::start` builds the authored starting state in a tool session and `run_scenario` also plays the scenario steps. To play against published content, take `project.start()?.into_state()` and open a session over a `ContentRepository`.

Before gameplay uses a published bundle, create `ContentLibrary::new(retention_directory)`, call `retain(bundle_path)` and open its returned identity through `library.open(&identity)`. Retained bundles are independent of source files and are not removed automatically. Saves reference mechanics; language packs are selected independently and are not part of save identity.

`SaveDirectory` supports manual slots, quicksave and autosave retention (1–32). A save is one `.save` file: a header line and the state as JSON, written beside the slot and renamed into place. Save format **8** rejects other formats; regenerate fixtures rather than migrate them. Restore with `library.load(&saves, slot)`, or resolve `saves.content_identity(slot)` yourself and call `saves.load(slot, content_source)`.

`HeadlessDriver::new(session, step_ms, trace_capacity)` shares the session's commands and read models. `submit`, `step` and `advance_until(max_steps, predicate)` use explicit logical time and a bounded event trace; no real-time sleeps are required. Failed commands add nothing to the trace. An unmet predicate returns an error after its step budget; steps already accepted stay accepted.

Run the standalone checks:

```sh
cargo test --offline -p yarra-game-content -p yarra-game-types -p yarra-gameplay -p yarra-localization -p yarra-save
cargo clippy --offline -p yarra-game-content -p yarra-game-types -p yarra-gameplay -p yarra-localization -p yarra-save --all-targets -- -D warnings
```

See [gameplay architecture](ARCHITECTURE.md#standalone-gameplay-foundations) for ownership and deferred features. The authoring tool and demo remain separate from game entities and UI.

## Validation and platforms

```sh
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets
cargo test --offline --workspace
python3 -m unittest discover -s tools -p test_grass_profile.py
```

`--streaming-smoke` explicitly installs `StreamingSmokePlugin`; ordinary game/editor composition contains no smoke state or exit system. It runs the demo-world traversal, cooling/ownership and second-world gameplay checks, then exits. Use the cooked overworld/interior fixture (zero initial gameplay objects, one interior object), not arbitrary authored worlds. It accepts both hierarchy and `--terrain-legacy`; traversal checks canonical destination cells and current source demand after rebasing. The existing 3/7.5/11-second checkpoints are readiness assertions, not performance measurements. It cannot share control/exit ownership with a profile, repro or capture. The default island is not a smoke fixture; run against a separately cooked historical demo. With unused database paths:

```sh
cargo run -p yarra-world-cook -- create-demo tmp/smoke.project.sqlite
cargo run -p yarra-world-cook -- cook tmp/smoke.project.sqlite tmp/smoke.runtime.sqlite
cargo run -p yarra-app-game -- --world-db tmp/smoke.runtime.sqlite --streaming-smoke --diagnostics off
```

Add `--terrain-legacy` to exercise the older renderer with the same fixture.

Native GPU/large-fixture tests are ignored by default and run explicitly for relevant changes. For example:

```sh
cargo test --offline -p yarra-engine mountain_cover_uploads_draws_moves_and_rebases -- --ignored --nocapture
cargo test --offline -p yarra-engine msaa_store -- --include-ignored --nocapture
```

Use [iOS setup](../ios/README.md) for device build/packaging and [performance](PERFORMANCE.md) for capture conditions. Passing CPU tests does not establish visual parity, smooth pacing or sustained power/thermal acceptance.

### Headless world-action exercise

Run the guard/gate sequence without the engine:

```sh
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard-gate.ron
cargo test --offline -p yarra-game-content --test world_actions
```

The guard package's `world/` directory declares separate gate/container, approach area and escort trigger assets. Publish the source to SQLite before runtime use, as above.

Use `Command::World` for logical object operations, position observations and movement reports. Call `world_work_pending`/`ProcessNext` from the coordinator or `HeadlessDriver::pump_world(limit)` in automation. A false pump result leaves queued work for the next tick. `next_movement(after_trigger)` polls one pending request at a time; send `StartMove` using its action ID and later `FinishMove` with an explicit outcome. The test adapter supplies these outcomes; real movement/pathfinding is separate. After loading, resume polling using saved IDs rather than creating replacement requests. `AdvanceTime` advances the logical clock; pumping processes overdue movement failures.

For normal object access, `Open` checks lock/destruction state, then `container_contents` exposes the bound inventory. Use `state().object(content, id)`, `.location(content, actor)` and `.trigger(id)` for read models, saved sequence status and diagnostics. Failed immediate effects do not partially consume rewards. Three consecutive failures suspend automatic attempts; `RetryTrigger` is an explicit recovery command after correcting the cause. Engine adapters must authorize access/control and report movement interruption. These APIs are not wired into graphical play yet.
