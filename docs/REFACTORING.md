# Refactoring backlog

Rendering/editor/streaming source audit at `2afce93`, September 21, 2026; gameplay roadmap revised September 30, 2026. [Architecture](ARCHITECTURE.md) maps crate ownership and the current gameplay design. [Experiments](EXPERIMENTS.md) records retained/rejected approaches and performance limits.

For the rendering/editor/streaming cleanup below, keep existing crate boundaries initially and fix ownership/plugin composition first. The gameplay roadmap has its own contract-driven sequence; it does not require reopening those completed renderer changes.

## Gameplay roadmap

The first gameplay pass is on the `gameplay-foundations` branch. It proved the domain contracts headlessly but was built around a disk-backed working store and demand-loaded state, which made every feature touch many files. The plan below replaces that. Worlds, bundles and saves are disposable: change formats directly, bump versions and regenerate; no migrations or compatibility loading.

Target: a classic party RPG in the line of KOTOR / Dragon Age: Origins, Gothic and BG3 / DOS2.

| Step | Deliverable | Status |
| --- | --- | --- |
| 1. In-memory state | Domain crates merged into `gameplay`; whole playthrough in memory with journaled commands; snapshot saves; content loaded once except dialogue graphs, which load on demand | Done |
| 2. Thin slice in the game | Opt-in `--story` in `app_game`: a guard and a gate near the start, talk to the guard, the gate unlocks, quick save and load | Done |
| 3. Dialogue graph v2 | Flat node graph with a speaker, condition, actions and ordered children per node; any number of participants | Done |
| 4. Luau scripts and names | Names as identities in authored content; Luau in a `scripting` crate behind a trait; script conditions and actions beside the built-in ones | Done |
| 5. Events and areas | Event-driven triggers only; polygon areas painted in the editor; blocking and ambient dialogue modes | Done |
| 6. Character rules | Data-defined stats, classes, levels, skill ranks, modifiers, status effects; timed actions for combat | Done |
| 7. Consolidation | Fixes and cuts from a review of the branch: one driver in `gameplay`, limits only where they prevent real failure, saves that survive content edits, simpler types | Done |

### Step 1: done

`rules`, `actors`, `quests`, `inventory` and `dialogue` are modules of `gameplay`; `inventory_db` is removed. `GameSession` owns the state; commands run against a write journal (`tx.rs`) that gives rollback, per-command validation of changed records and change notifications. `save` writes one snapshot file per slot. `ContentRepository` lost its dependency indexes, budgets, cache accounting and leases: a session reads the always-needed definitions once and dialogue graphs on demand. Bundle schema 7, save format 8.

Evidence: the three authored scenarios (`guard`, `guard-refusal`, `guard-gate`) and the demo pass unchanged through the new session; 103 tests pass across the five crates with `clippy -D warnings`. Three item commands against 20,000 actors take about 0.1 ms in the dev profile. The demo loads 1 of 6 dialogue graphs. Rust in these crates went from about 19,000 to 15,500 lines.

Dropped with the storage layer: tests that injected SQLite write failures and asserted decoded-record counts. Their behavioural halves (rollback after a roll, claims not consumed by a failed plan, save/load mid-movement) remain.

Left for later steps: definition lookups are linear scans and inventory operations re-validate the catalog (index when content is restructured); effect expiry scans all actors per time advance.

### Step 2: done

`cargo run -p yarra-app-game -- --story content/gameplay/demo` publishes the project to a private bundle, starts `scenarios/island.ron` against it and adds a guard and a gate a few metres ahead of the start view. E talks, Space advances, 1–9 choose, Q leaves, F5/F9 quick save/load. Returning the key completes the quest and the gate swings open; loading an earlier save locks it again and resumes a conversation at its line. Without `--story` the game is unchanged.

[`story.rs`](../crates/app_game/src/story.rs) holds the session and turns input into commands; [`story/scene.rs`](../crates/app_game/src/story/scene.rs) is the Bevy side. The engine gained three small public hooks: `standing_character`, `PlayerMovementSuspended`, and the `PlayerControlled`/`TerrainGrounded` markers.

Deliberately not done here: the guard and gate are placed relative to the start instead of being authored in the world; the gate is a visual with no collision; the HUD is plain text. Reporting positions to the session came with step 5.

### Step 3: done

A dialogue is a flat set of nodes, the model used by BG3 and the Obsidian games. Each node has a speaker role, text, an optional inline condition, actions and ordered children ([`dialogue/mod.rs`](../crates/gameplay/src/dialogue/mod.rs), [`conversation.rs`](../crates/gameplay/src/conversation.rs)). After a node the first eligible child decides: a line plays, or every eligible choice is offered. Roles are `Required`, `Optional` or `Actor(id)`; named characters take part when they are in `SessionState::party`, and `Present(actor)` tests for a particular companion. `Relationship` conditions and actions can name `Actor(id)` besides `Player` and `Speaker`.

Conditions and actions moved into the nodes: `bindings.ron`, the per-dialogue binding maps, `BindingId` and the separate condition/action assets are gone; packages declare facts. Source format 7, bundle schema 8, save format 9.

Evidence: six tests in `gameplay/tests/session.rs` cover no reaction when alone, a companion reaction with the NPC answering it, a two-companion exchange, a bystander bound for one conversation, hub loops with per-run choices, and leaving the party. The authored reward conversation has a companion aside; the game slice starts with Mira in the party and she speaks it. All five authored scenarios pass.

Deviations from the plan, all deliberate:
- A run is still keyed by `(dialogue, participant, speaker)`, not an instance ID. It did not get in the way of multi-party conversations, and changing it would have touched every command and scenario step.
- `Player`/`Speaker` remain as participant names next to `Actor(id)`; conditions do not refer to arbitrary role names.
- The compiler does not resolve symbolic names yet: graphs still spell out UUIDs.
- Reactions are authored in the dialogue that hosts them; attaching them from a companion's package is not built.

### Step 4: done

**Names.** Authored content names things (`guard/gate`, `old_gate_key`) and the name is the identity: it hashes to the same 16 bytes the runtime already used, so state, saves and ID types did not change. Things created while playing keep random identities in UUID form. Names are remembered per process for messages and written back into saves; bundle storage keys use the raw bytes. `X::named("...")` is a `const fn`, so code and tests name the content they rely on. Renaming is find-and-replace, and it changes the identity.

**Scripts.** Luau rather than Lua, for its type annotations and sandbox. A module is a `.luau` file listed in its package and returning a table of functions; content uses `Script("module.function")` as a condition or an action. [`gameplay/src/script.rs`](../crates/gameplay/src/script.rs) defines what scripts may read (`ReadScope`) and do (`ActScope`); [`scripting`](../crates/scripting/src/lib.rs) translates Luau calls onto those, mostly by building the built-in `Action` and running it, so scripted and authored rules share one implementation.

- Conditions get read functions only and must return a boolean; an error fails the command and names the script and line.
- Effects apply at once through the command journal, so a script sees its own changes and a failure undoes everything.
- Each call reloads its module from bytecode: nothing kept in a script variable survives to the next call. Calls are sandboxed with a 16 MiB cap and a step budget; `math.random` and the `os` clock are removed, and chance comes from `game.roll`.
- Publication compiles every module and rejects references to functions that do not exist. Modules are part of the content fingerprint.
- `cargo run -p yarra-game-content -- script-api` prints the type definitions; a test keeps them equal to what scripts are actually given.

Evidence: the guard's reward is a script in the demo project and all five scenarios pass through it, from source and from a published bundle. Seven tests cover reads and effects, rollback with script and line in the error, read-only and bounded conditions, no state between calls, publication checks and the definitions file. The game builds for iOS with Luau included (compiled, not run on a device).

**Finished afterwards.**
- *Type checking.* `validate` and `build` run `luau-analyze` in strict mode when it is on `PATH` or named by `YARRA_LUAU_ANALYZE`, with the API definitions placed in front of each script and reported lines moved back to the authored file. Type errors fail; lints are warnings. Without the tool a note says scripts were only compiled. The stock analyzer has no option for a separate definitions file, hence the prefix.
- *Typed variables.* Boolean facts are gone. A package declares `variables` with an initial value (true/false, whole number or text) and a variable keeps that type. Content uses `Variable(variable, test: Is/AtLeast/AtMost)`, `Set` and `Add`; scripts use `game.get`, `game.set` and `game.add`. Only variables that were set are stored. A variable is scoped to the playthrough, or declared `scope: Actor` so that every actor has its own value: content names whose with `of: Some(Speaker)` and scripts pass the actor as the last argument.
- *Item keys.* Items and categories no longer carry a `key` beside their name.

Source format 8, bundle schema 9, save format 10.

Still open: derived-stat and check formulas in Luau belong to step 6.

### Step 5: done

**Triggers.** A trigger listens for world signals: an actor entering or leaving an area, a requested walk arriving or failing, an item acquired, a quest starting or changing, a variable changing, a dialogue completing. When one happens and the condition holds, its actions run as a unit: all take effect or none do, and a failure is reported and leaves the trigger free to fire later. Repeat is `Once`, `Always` or `Cooldown`. Nothing is polled: the command journal already knows what a command changed, so signals are derived from it and only those some trigger subscribes to are queued. "Maintained" triggers and scripted sequences are gone; a walk followed by an effect is two triggers, the second listening for `Arrived`.

**Areas.** Gameplay knows an area only by name ([`world.rs`](../crates/gameplay/src/world.rs)). Its shape is a ground polygon with an optional height range ([`gameplay_area.rs`](../crates/world/src/gameplay_area.rs)), painted with the editor's Areas tool and stored as one revisioned record in the world project. Publishing copies the set into the runtime outside the content hash, so repainting an area never recooks terrain. The engine hands the set to the game as `WorldCatalog::gameplay_areas()`.

**The engine's half.** The adapter reports `Observe { actor, position, areas }` for party members and for anyone asked to walk, and only when the set of areas changes, so standing still costs nothing. An actor on an edge stays inside until 0.25 m beyond it. `Move` asks the engine to walk an actor into an area; the walk ends when the actor is reported inside it, when the engine gives up, or when its time runs out. Occupancy, walks and queued work are part of the save.

**Dialogue modes.** A graph is `Blocking` (the panel; the world waits) or `Ambient` (spoken while play goes on; no choices allowed). `StartDialogue` as an action queues a conversation, so a trigger can start companion banter.

**In the game.** The `--story` slice reports occupancy, carries out queued work, walks the guard with the engine's `MoveIntent`, shows ambient lines above the panel and advances them on a timer, and keeps game time running except during a blocking conversation. Quick save records where everyone stands and quick load puts them back. The demo's two areas get stand-in shapes beside the guard until areas with those names are painted in the world.

Scripts gained `game.move_to` and `game.start_dialogue`. Source format 9, bundle schema 10, save format 11; world source schema 27, runtime schema 26, editor journal 15.

Evidence: 16 tests in `game_content/tests/world_actions.rs` (escort headless and from a published bundle, save mid-walk, failed and timed-out walks, signal order, repeat policies, a failing trigger staying armed, ambient banter with a companion); six story tests including the escort, the pause during a conversation and a load that restores the spoken line, the walk and positions; storage, cook and editor tests for areas (a save conflict changes nothing, an areas-only publish is incremental and recompiles no cell, a corner drag is one undo step across a save).

Costs: containment is a bounds check per area, about a microsecond per thousand areas per tracked actor, run only for the party and walking actors. The editor draws outlines for areas within 800 m of the camera.

Not done:
- Walking is a straight line to a point inside the area; there is no pathfinding. A walk without a timeout that gets stuck never ends.
- Companions do not follow the player yet, so only the player crosses areas in the slice.
- The guard, gate and companion are still placed relative to the start, not authored in the world.
- A variable subscription is per variable, not per actor.
- The Areas tool has no viewport labels; names are in the Areas window.

### Step 6: done

One model covers the player, companions and NPCs. How a game grants and spends points is data; its numbers are two script functions.

**Stats.** The rules declare each stat as `Primary` (stored per character, raised with attribute points), `Derived` (worked out by the rules script from primaries, level, class and skills) or a `Resource` capped by another stat. One resource is named as life. A modifier adds, multiplies or overrides, and lives where it comes from: an item or a status effect; `modifiers_of` lists them with their sources for a tooltip. A character's stats are worked out when what it is built of changes and stored with it, so reading one is a lookup and combat never calls a script to read a number ([`rules.rs`](../crates/gameplay/src/rules.rs), [`character.rs`](../crates/gameplay/src/character.rs)).

**Formulas.** The rules name two Luau functions: `derive(character)` returns every derived stat, `check(character, skill, difficulty, roll)` decides a skill check with dice from the saved random stream. The fixed attributes, the health field, skill experience counters and the hard-coded d20 are gone.

**Classes, levels, points.** A class gives starting stats, grants per level and at particular levels (attribute points, learning points, outright bonuses, abilities) and caps the rank a trainer can teach of each skill. Experience belongs to the party: every member is at least the level it has earned, and someone who joins catches up. Attribute points are spent freely (`SpendAttributePoint`); skill ranks are bought with learning points through the `Teach` action, which a dialogue puts behind a trainer; `Pay` and `Gold` take from and test the party's shared purse.

**Party.** Up to `party_size` members (four in the demo), one of them steered (`Control`), inventories per character, gold and experience shared. Actor roles are gone: the party and who is steered say what they said.

**Status effects.** Defined by the rules with modifiers and an optional periodic change to a resource. One of each per character; applying it again starts it over. Losing the last of the life resource is death: effects and actions end, and triggers can listen for `Died`.

**Actions in time.** An ability has a duration, a cooldown, resource costs and a script. A character lines up intents; one at a time is begun (costs paid), takes its duration, then takes effect. A repeating intent lines itself up again, which is a basic attack. There are no turns, only points in time, so the same model serves a paused-queue game and a real-time one; the choice is still open. Passing time visits only characters with something pending. A failing ability script costs its user the action and nothing else ([`combat.rs`](../crates/gameplay/src/combat.rs)).

**Lazy inventories.** A template names what the character wears and a loot table; a container names a table. Neither has an inventory until a command needs one. What is found depends on the playthrough and the owner only, from a random stream of its own, so the order of opening does not matter. Template equipment counts towards stats from the start.

**Typed refusals.** What a player can run into is a `Rejection` a UI can match on (locked, not enough gold or learning points, party full, on a dead target…), and it keeps its type when it comes from inside a script. Mistakes in content or code stay plain errors.

**Lookups.** Definition lists are kept in order of identity and found by bisection; item operations no longer re-check the whole catalog.

Source format 10, bundle schema 11, save format 12.

Evidence: 156 tests across the gameplay crates, plus 7 on the game's story slice. New: 15 on characters (formula-driven stats, effects over time, death, shared experience and points, trainers and class caps, the purse, party size and control, checks, load validation, unopened inventories and containers, typed refusals), 7 on timed actions (durations, queues and repeats, cooldowns and costs, death, refusals, a fight resumed from a save ends the same, a skirmish among bystanders), 7 on Luau (the authored formulas and abilities give the same results as the fixtures' Rust ones, formula mistakes are errors, a failing ability script). The demo has a trainer scenario (`guard-training`) and the game slice shows the sheet, lessons, points, a potion and sparring with a dummy.

Numbers, dev profile on this machine:
- Deriving a character's stats in Luau: about 9 µs. Spawning 20,000 characters 175 ms; checking all of them on load 130 ms.
- 200 characters trading blows among 20,000 idle ones: 0.23 ms per 100 ms step of game time.
- Three item commands against 10,000 item definitions: 0.09 ms (9.5 ms before bisection).

Not done, deliberately:
- No combat in the engine beyond the slice's dummy: no range or line of sight in the rules (the adapter decides who can reach whom), no animation, no AI choosing intents.
- Effects do not stack; a second application restarts the first.
- Stats are maps keyed by name, about 1 KB per character. Dense arrays indexed by the rules' order would shrink that without changing content.
- NPC levels are whatever their template says; only party members gain experience.
- Scenarios cannot start a party with experience.
- Variable subscriptions are still per variable, not per actor.

### Step 7: done

A review of the branch after step 6 found the core sound (the playthrough in memory, commands against a journal, names as identities, Luau effects running through the built-in actions, the Bevy-free session) and three kinds of debt: bugs, leftovers of the bounded SQLite-era design, and rules that live only in the game adapter. This step pays them before more content or engine work is built on top.

**Bugs in the rules crates: done.** A script's step budget now covers the rules it calls; each module load gets its own environment, so globals no longer survive between calls; a failed ability script no longer stalls its user's queue; the content fingerprint hashes identities as bytes, not as whichever names the process has met; a slot this build cannot read no longer stops saving or listing. Each has a regression test.

**One driver: done.** Rules that lived only in [`story.rs`](../crates/app_game/src/story.rs) are in `gameplay`, so scenarios reproduce them:
- A command carries out the work it gave rise to before it returns: triggers, the conversations they start, walks whose time ran out, each piece a step of its own. `ProcessNext`, the pumping in the game and the scenarios' `PumpWorld` are gone, and so are the caps on queued and per-command events that could refuse every command. Content whose triggers keep setting each other off is cut short after 256 steps and carries on with the next command; a scenario that does this fails.
- Conversations wait their turn in `state().floor`, one queue per mode, saved in the order they started. While a blocking one is on screen, `AdvanceTime` changes nothing.
- `Driver` (formerly `HeadlessDriver`) keeps the fixed time step for the game as well.
- `observed(actor)` says whom the engine reports: the party, walkers, and anyone a trigger's `Entered`/`Exited` names.

Fixed on the way: F5 sends `Record { positions }`, which leaves occupancy alone, instead of full observations that could set off a trigger before the snapshot; the game fails a walk it has nobody to carry out; it keeps reporting an actor the engine is still walking, so what the rules last heard stays true; action scripts see conversation bystanders; a conversation stops the player on the frame it opens. Save format 13.

Evidence: new tests for turns kept through a save and the world held still, runaway triggers, the observed set, recording positions and bystanders in action scripts; the world-action tests now check the events of the command itself. 164 tests across the gameplay crates, 7 on the story slice.

**Cuts.**
- Limits that refuse commands: done. Caps on how many of something content or a playthrough may have are gone: dialogues, text resources, actors, items, triggers, quests, rules, nodes, roles, arguments, trade lines, areas an actor stands in, dice in a check, the whole project's size. One could lock the game: a step of time with more than 10,000 things due failed, and every later step with it; it now carries on, and an ability must take at least a millisecond so a step always ends. Kept: guards against endless or exponential work (nesting depth, the predicate expansion budget, script steps and memory, 256 queued steps per command), guards against one mistake exhausting memory (the size of each file read, 4,096 stacks per inventory), value ranges and name formats, and rules of the game (party size, eight lined-up intents). The bundle's 2 MiB per record and the localization cache limits go with their rework.
- Saves survive content edits: done. A slot loads with whatever content is at hand. Loading works every character's stats out again with the current formulas instead of comparing them, keeps resources within their caps and raises party members to the level the party's experience now earns; then the whole state is checked, and what still does not fit (a removed item or class, an unreachable level) is refused with the reason. The game says when the content has changed since the save. A format bump still refuses.
- Dialogue graphs behind `GameContent`: done. They still load on demand and stay in a cache of the 32 used most recently, but the content reads them itself: the first command or read model that needs a graph reads it, and one that cannot be read fails what needed it. The pre-pass in `GameSession::apply`, the `unloadable` workaround for queued conversations and `&mut self` on read models are gone, and so is the session's type parameter: `GameSession` and `Driver` are plain types.

**Types and small refactors.**
- Owner kind and inventory role are enums instead of strings compared in 19 places: done.
- New items take identities from a counter in the inventory that makes them, instead of being renamed at commit: done. The count is part of the inventory record, so a replay makes the same items and an undone command gives the numbers back. The renaming, with its patching of equipment and events, is gone.
- Identities lose `Default`, which made a random UUID: done. Nothing relied on it; `new()` is now `random()`, which says what it does.
- Inventory refusals a player can meet are `Rejection`s, so a UI matches one vocabulary: done. Not enough money is `NotEnoughGold` with the amounts, as paying a trainer is; no room, an item that cannot be given up, an item not traded here and a stale offer have their own. The conversion happens where inventory errors enter gameplay; the rest stay mistakes.
- Per-command validation of changed records runs in debug builds only: done. It worked a character's stats out a second time on every equip or effect. The whole test suite passes with it switched off, so no command relies on it to refuse anything. A loaded save is still checked in full.
- Definitions are stored as maps by identity: done. Every list of definitions in `GameContent` (and the item catalog's) is a map, saved as a plain list as the state's records are, so authored files and bundles keep their shape. `sort`, the bisection lookups, the fingerprint's own sort list and the "listed in order" failure are gone; `keyed_map` and `KeyedMap::add` build and extend them. The rules' own lists stay lists, since their order is authored.
- Scenarios share a base start: done. A scenario can name a `base` whose starting conditions it plays from and then holds only steps; the four guard scenarios share `scenarios/guard-start.ron` instead of repeating 75 lines each.
- Test fixtures behind a feature: done. `gameplay::fixtures` and `inventory::fixtures` exist only with the `fixtures` feature, which the tests of `gameplay`, `scripting`, `save` and `game_content` turn on as a dev-dependency; the game does not get them.

**Claims folded into variables: done.** A once-only reward is a boolean variable and the new `If { condition, then, otherwise }` action, a general branch built on the existing conditions. Claim definitions, the claims set in the state and its journal entry, `Claimed`, `game.claim`, `RewardClaimed` and the claim asset kind are gone; the guard's reward is `guard/rewarded`. The pair scope claims had was used nowhere and was not carried over. Bundle schema 12, save format 16. Interaction profiles stay while graphs load on demand: they pick an opening without reading any graph.

Evidence: 232 tests across the gameplay crates and the game, clippy clean, the six demo scenarios pass. Each fix above has a regression test; the per-command checks were also run switched off. Bundle schema 12, save format 16.

**Later, with the next feature step.** The story adapter becomes app-side plugins (an `ActorId` to entity index, commands in as messages, `GameEvent`s out as messages, the HUD on its own) before companions follow or combat reaches the engine. The bundle keeps the `ContentSource` port but loses the hardening one author does not need. Rule names are interned into indices.

**A lighter authoring format: done.** Source format 11. `project.ron` lists package directories, and a package's files are found by name (`*.quest.ron`, `*.dialogue.ron`, `*.trigger.ron`, `en.ftl`…); an unrecognised `.ron` file is an error. `package.ron` is optional and declares only areas and variables. One NPC with one conversation now touches its template, its graph and two Fluent files instead of about nine. A package has one text resource; content writes `Message("key")` for its own text and `Message("core/key")` for another package's. Message contracts are read from the source-language Fluent instead of being declared again in `messages.ron`; translations are checked against them and fall back with a warning. Review hashes, `shipping_locales`, text imports, package dependencies, `conversation.ron` and the localization cache's charge accounting are gone, and a format no longer walks the Fluent syntax: each resource is parsed and checked once per locale. The localization crate stays about its size (reading contracts from Fluent replaces declaring them); the Rust across these crates is about 350 lines shorter and the demo content about 370.

**Named characters as content: done.** A package declares the characters content names in `characters.ron`: an identity, the template it is built from and optionally a name of its own. Triggers (their player, speaker and signals), conditions, actions and `Actor(id)` dialogue roles are checked against them, and the error names the record it was found in (`trigger guard/approach_banter: unknown character mria`). Scenario actors are declared characters, so a spawn no longer repeats the template; the game checks the characters its slice names when it opens. An actor's name is its playthrough name, else its character's, else its template's, worked out in one place instead of three. Bundle schema 13.

**Small items from the review: done.** The Areas tool ends a gesture left open when the World workspace is left, which held up saving. `to_render` takes the world space and gives nothing for another one; area outlines and walk targets use their area's space. `PlayerMovementSuspended` holds the player for named reasons, so one holder letting go does not free a player another still holds. An areas-only publish is a new runtime generation, derived in one place for the cook and both terrain bakes. The game warns about content areas the world lacks, a test pins the world's and the rules' area-name rules together, and one map deserializer naming the duplicate key replaces two.

### Working method

Work in small runnable batches. Replace superseded formats directly and regenerate fixtures. Run the headless scenarios first: [headless play](ARCHITECTURE.md#commands) through the same commands and read models as the game is a requirement for every step. Add a script only for a concrete case that built-in rules express badly.

## Performance roadmap

October 9, 2026. At native resolution the forest ran at about 43 fps uncapped on a cool M2 Max and lower once it heats (the user saw 35). Measurements and the fixes so far are in [Experiments](EXPERIMENTS.md#native-resolution--october-9). What remains, largest first:

- **Trees** cost at every resolution: about 3.7 ms of the forest's opaque pass, 3.2 ms of shadow cascades, 0.6 ms of Bevy's per-mesh GPU preprocessing and most of the 4 ms of CPU render preparation, for about 38,000 render entities (one scene per LOD, one entity per glTF primitive, so about 13 per tree). Impostors also show too close (from 166 m), and mesh LODs are too expensive to push them farther.
- **Temporal upscaling** adds about 4 ms of GPU and 1.5 ms of CPU at 50% scale: its depth and motion prepass draws the world again.
- **The sky composite** is one large shader for sky, sea, fog, aerial perspective, shafts and glare at every pixel (about 5.3 ms at native in the forest); stubbing its parts shows its size, more than any one part, sets its cost.
- **Terrain** shading at native (2.8–4.3 ms), mostly lighting.

### GPU-driven trees

Trees and shrubs would be drawn the way grass is: no render entity per tree, one instance list on the GPU, culling and LOD choice in compute, and a few indirect draws per form and LOD.

| Step | Deliverable | Gate |
| --- | --- | --- |
| 1. Prototype | Every resident tree drawn from one instance buffer with its current LOD's mesh, main view only, the entities still casting shadows; vertex pulling from a per-form merged buffer, wind and crown shading through the existing shaders and lighting adapter | **Done** (`engine::tree_wind::instancing`, the game's default since October 9; `--tree-entities` for the old path; the editor still draws entities): same image; see below |
| 2. LOD on the GPU | LOD choice from projected height (catalog thresholds as now) and timed fades with their state per instance in compute; impostors drawn by the same renderer, replacing the block meshes and the fade buffer; render entities for trees removed (rain shelter and other CPU users, and later trunk collision, keep a plain per-page list: collision is gameplay data and does not depend on how trees are drawn) | Same transitions and fade timing; CPU render prep and bin unpacking gone for trees |
| 3. Shadows | Cascades culled per cascade frustum and drawn through Bevy's shadow phase from the instance buffer, one LOD coarser than the main view's | **Done** (same instance buffer, `--tree-shadow-lod`): same image at the same LOD, entities out of every view; one LOD coarser saves a third of cascade time but lightens the forest; see below |
| 4. Temporal | Depth and motion for MetalFX Temporal from the same instance buffer (previous transforms for rebases, the previous wind pose) | **Done**: the main view's groups also go into Bevy's depth and motion prepass phases; trees are static, rebases reset the history (camera jumps over 8 m), and the shader's previous wind pose drives the motion; same sharpness in motion as entities |
| 5. Farther meshes | LOD2 in use and the impostor hand-off moved out, as far as the budget allows; impostor quality revisited with what is left | Impostors not noticeable at the distances the user plays at |
| 6. Retire | Old per-entity path, `ImpostorBatch`, `ImpostorFades` and the CPU LOD selection removed | Tests and docs |

Step 1 draws the main view's trees through the tree material's own pipeline, specialized with `TREE_INSTANCED` (transforms and fade tags from one storage buffer at group 4 instead of Bevy's mesh uniforms), one instanced draw per mesh and material; the entities moved to render layer 1, which the sun and moon also cast from. Broadleaf stand at 50% scale, alternating: 91.0, 90.6 fps as entities against 94.9, 95.2 instanced (GPU 9.5–10.0 → 9.4 ms, render prep 3.9–4.1 → 3.6–3.7 ms, main thread +0.15 ms for the gathering); the image matches (mean difference 0.25 of 255). The entities still cost most of their CPU in the three shadow views: left out of every view (no tree shadows), render prep fell to 2.1 ms and GPU time to 8.2 ms (92 → 115 fps), what step 3 can win back less the cost of instanced shadows.

Step 3 takes the entities out of the shadow views too: they move to a layer no camera or light sees, so Bevy no longer extracts, culls or queues them anywhere. Each frame every tree that fades over time is tested against the world view's cascades (a sphere round the tree, then each mesh as Bevy tests casters, near plane ignored), and the meshes of the LOD it casts from go into that cascade's groups of the same buffer, drawn through the tree material's shadow pipeline (Bevy's directional-light key, `TREE_INSTANCED`) with the same overlap of fading shadows as before. The main view's instanced pipelines now carry Bevy's dither key as the entities' do, so a fade dithers there as it did. Broadleaf stand, alternating, at 50% scale: 91.6, 91.8, 92.3 fps as entities against 102.9, 103.6, 95.1 instanced, render prep 4.0–4.2 → 2.1–2.2 ms, main thread +0.15 ms, Bevy's bin unpacking 0.58 → 0.08 ms of GPU; at native the frame is GPU-bound (22–25 ms) and fps stay within the heat drift (45.9, 40.1 against 44.1, 39.8). Casting from one LOD coarser (`--tree-shadow-lod 1`) cut the cascades from 0.61 to 0.41 of the sky composite's time (about 1.1 ms at native), but the coarser crowns let more light through: the broadleaf view 7% and the spruce view 5% brighter, the canopy most, against no change at the same LOD (mean difference at most 0.13 of 255 in four forest views). The same LOD stays the default until the look is judged. 

Step 4: without it, instanced trees drew no depth or motion into the prepass, so the temporal motion pass took the reprojected background's motion at their pixels and MetalFX smeared them as the camera moved (the user saw blurry trees). The main view's groups are now also queued into `Opaque3dPrepass` and `AlphaMask3dPrepass` with Bevy's prepass key for the view (plus the tag dither and the material read), through the tree material's prepass pipeline. Camera moving through the broadleaf stand at 6 m/s with MetalFX Temporal (`landscape-descent` on a temporary route): edge sharpness of the crowns 6.98 as entities, 5.82 instanced without the prepass, 7.03 with it. At 50% scale with Temporal, alternating: 71.6, 71.3 fps as entities against 77.4, 75.7 instanced, render prep 5.4 → 3.0 ms.

Expected: CPU render prep and the main thread's LOD work for trees mostly gone (about 3 ms CPU); 0.5 ms of GPU preprocessing gone; shadow cascades down by a third to a half through the coarser LOD; enough headroom for LOD2 and a farther hand-off. Risks: shadow-phase and prepass integration go through Bevy internals (grass has neither); the editor shares the streaming path; visual parity of crossfades and lighting must be checked view by view.

Open questions for the user:
- Trees deep inside a forest could stop casting into the far cascade, leaving their shade to the forest shadow map: to be judged on the image.
- Whether a lighter material far away (no normal map, no forest-shadow march) is acceptable, if it is not visible.

### After the trees

Target since October 9 (user): 50% render scale with MetalFX Temporal at a stable 60 fps. Broadleaf stand at that setting with instanced trees, Metal System Trace per frame: 13.7 ms, of which the GPU idles 2.2 ms. MetalFX Temporal 1.09 ms plus two unlabelled command buffers of its own (0.54 and 0.91 ms; they vanish with `--profile-temporal-bypass`) and a 1.4 ms wait between them: about 4 ms in all, against 0.45 ms for MetalFX Spatial. Shadow cascades 3.2 ms; main opaque pass 2.6 ms; grass 1.6 ms; depth and motion prepass 1.5 ms (1.8 ms on the start beach); Bevy's atmosphere LUTs about 1.4 ms (overlapping the cascades); bloom about 0.5 ms; sky composite 0.8 ms (1.4 ms on the beach).

MetalFX Temporal, examined: its two extra command buffers are its own (`MetalFX_Temporal_PreProcessing` and `_PostProcessing`, committed from MetalFX's thread to its own queue), and our queue waits for the post-processing before bloom and tone mapping. The post-processing is committed about 10 ms ahead, yet starts 0.7–3.9 ms after MetalFX's main pass ends: the wait is inside MetalFX, not ours. A scaler sized to the rendered region (b2c8cdf) dropped the pre-processing (0.5 ms) and made the main pass 1.09 → 0.81 ms, but the wait grew by as much and the frame stayed at 13.7 ms; fed the full-size textures, it also filled the sky and distant geometry with blocky stale history in motion, so it was reverted on October 10 (the scaler takes the full size and the rendered region as its input content). What remains is MetalFX's price at a 3456×2168 output.

Bevy's atmosphere LUTs, examined: the sky composite reads the transmittance, sky-view and aerial-view LUTs, so none can be dropped. Cutting each LUT's samples in turn (LUT pass time against the sky composite in the same trace, machine heating): unchanged 1.57, sky view 1.50, transmittance and multiscattering 1.45, aerial view 0.84. The aerial-view LUT is most of the pass (32×32 threads marching 32 slices each, so it mostly overlaps the shadow passes); cutting it gained about 0.5 ms of frame time on average in alternating runs (+3.5, +2.1, +0.2 fps on a hot machine). Not pursued for now: a rewrite of Bevy's LUT pass (a more parallel aerial view, skipping the dark moon, the sky view only when the sun or weather changes) could win at most about that.

A lighter prepass: terrain (`TerrainMaterial`, `TerrainCompositeMaterial`) and tree impostors leave Bevy's prepass (`enable_prepass` false); the main pass still writes their depth. After the opaque pass, `upscaling::temporal::CompleteTemporalMotion` writes the camera's motion at the final depth wherever that depth is nearer than the prepass's (static geometry in front, or nothing moving behind); moving geometry, drawn in both passes at the same depth, keeps its own, and grass then writes its motion over it. Impostors' sway is far below a pixel a frame. With the camera on the same route in both builds (`--render-frame-clock`), the image in motion matches the old prepass (mean difference 0.19–0.2 of 255, sharpness equal, motion vectors within 1/16 px on 99.6% of pixels). Measured at 50% with Temporal, alternating builds: terrain alone changed nothing (start beach 11.83–11.87 against 11.86–11.89 ms; forest 13.69 against 13.71 ms): its vertex work left the prepass, but the main pass lost the early depth test for it. Impostors: forest 13.13–13.14 against 13.60–13.70 ms (the near trees, still in the prepass, now hide the impostors behind them before shading), beach unchanged (11.79–11.94 against 11.85 ms: the prepass had been the early depth test for their alpha-tested cards, and that cost moved into the main pass). Less than the 1 ms hoped for: in this frame the prepass is mostly alpha-tested foliage, and drawing it once more is what keeps its main-pass shading cheap.

| Item | Idea | Expected |
| --- | --- | --- |
| Temporal motion | Camera motion from depth and reprojection for static geometry; only moving things (character, swaying foliage) written to the motion target, instead of a second draw of everything | **Done** (October 9): terrain and tree impostors left the prepass; about 0.5 ms in the forest, none on the beach (see below) |
| Sky composite split | Sea, sky and air as separate, smaller passes; or the air (fog, aerial perspective, shafts, glare) at half resolution for each pixel block's nearest and farthest depth, interpolated by depth at full resolution | About 2.5–3 ms at native, about 0.7 ms at 50% |
| Far terrain | Baked albedo and normal and lighter lighting where the detail cannot be seen | 0.5–1 ms at native |
| Dynamic resolution | Render scale held to the frame budget with MetalFX Temporal, as consoles do | Holds 60 fps as the machine heats |

### Bevy 0.20 features

October 10, 2026, on the upgrade to Bevy 0.20 (wgpu 30, Rust 1.99). In use from the upgrade itself: WESL shaders checked offline (`yarra-shader-check`, [Workflows](WORKFLOWS.md#shaders)), Bevy's weakly ordered render schedule, retained UI extraction and per-column change ticks in its mesh extraction.

Measured against the 0.19 build (same world, alternating runs, fullscreen at 50% scale, uncapped): with MetalFX Spatial 0.20 is about 6% faster (start beach 122 → 129 fps). With MetalFX Temporal it is 2–3% slower (beach 89 → 87, broadleaf stand 72 → 70 fps): pass times are unchanged, but the GPU waits longer for MetalFX's own post-processing command buffer (about 1.9 ms in 95% of frames against 1.3 ms in 58%, Metal System Trace). Bevy now finishes every pass's commands in parallel when it submits (encoding 3.0 → 0.5 ms, submission 0.3 → 2.0 ms), which may delay MetalFX's helper thread; submitting the frame early, right after MetalFX, did not help. The render thread's own CPU time fell by about 0.5 ms. Images match the 0.19 build within run-to-run noise in twelve look-capture views (`tmp/bevy020/cap.sh`).

| Feature | Fit for this game | Decision |
| --- | --- | --- |
| Mesh shaders (`MeshPipelineDescriptor`, `draw_mesh_tasks`) for grass | wgpu 30 exposes them on this M2 Max (256 vertices and primitives per meshlet), but Apple accelerates mesh shading in hardware only from M3, and Bevy offers a bare pipeline: no material, prepass, shadow or motion-vector integration, all of which grass has. Grass is bound by fragments (about 0.7 ms of the native opaque pass, 1.6 ms at 50%); a mesh shader would only fold the blade-preparation compute pass into the draw | Not now. A one-day spike measuring prepare + vertex time against a task/mesh shader on M2 and an M3+ machine first, if grass vertex work ever becomes a top item |
| Real leaves instead of cards (mesh shaders, meshlets) | A broadleaf has 10⁵ leaves and a pine about 10⁶ needles, against a few thousand cards per crown now: 30–300× the triangles, drawn again into four cascades, mostly smaller than a pixel beyond 30 m (quad and MSAA waste). Cluster culling does not simplify foliage (aggregate geometry needs voxel or card LODs anyway) | No. Cheaper lever for the same cost centre: cutout meshes fitted to the leaf silhouette (fewer discarded fragments, which on a tile-deferred GPU also defeat hidden-surface removal), baked in Houdini |
| Compressed vertex attributes (`MeshAttributeCompressionFlags`, Bevy's `VERTEX_*_COMPRESSED`) | Tree shadow cascades (3.2 ms) are mostly vertex work; positions, normals and UVs at 16 bits halve their fetch. `tree_wind.wesl` would have to decode them, and the custom wind and card attributes would be quantized by us | Candidate after measuring the cascades' vertex-fetch limiter in a Metal trace |
| Synchronous pipeline compilation on macOS (unchanged in 0.20) | WESL composition costs more than naga_oil did: startup compiles about 310 ms of pipelines against 210 ms, and a variant first needed mid-game stalls its frame for longer | Candidate: compile the known variants while the world loads (the pipeline log now names each pipeline's shaders and definitions) |
| WESL modules | The sky composite (1,358 lines) and the grass draw and compute shaders (about 1,100 lines each) can now be split into modules without name clashes | Do it with the planned sky composite split (sea, sky, air as separate passes) |
| Catching panics (`FallbackErrorHandler`) | A panic in an editor tool loses unsaved authoring | Candidate for the editor: log and continue |
| `despawn_all` | Faster batched despawns when streaming unloads object and grass pages | Candidate if page unloads show in `YARRA_SPIKE_LOG` |
| Schedule randomization (`shuffle_seed`) | Finds order-dependent bugs in streaming and gameplay systems | Use in tests when such a bug is suspected |
| Texture compression (`ctt`: BC7/ASTC with mip chains) | KTX2 UASTC is transcoded at load; BC7 on the Mac and ASTC on iOS would skip that | Later, with the iOS build |
| Solari (path tracing), BSN, Feathers widgets, pan-orbit camera | Path tracing is beyond the frame budget and has no macOS denoiser; the editor UI is egui | No |

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
