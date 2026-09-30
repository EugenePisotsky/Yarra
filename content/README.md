# Authored content

`world.project.sqlite` is the current editable world: painted ground/grass layers,
shared presets, trees and a curved cart road on an 8 m grid. The editor opens it by
default. The game reads only its published `assets/generated/world.runtime.sqlite`.

For a fresh checkout:

```bash
cargo run -p yarra-world-cook -- init
cargo run -p yarra-app-editor
```

`init` creates a world only when the project is absent. Subsequent runs retain its
source and republish it. Normal **Save** writes source changes; **Save & Publish**
cooks and atomically replaces the runtime. The command-line equivalent is:

```bash
cargo run -p yarra-world-cook -- cook
```

The old `demo.project.sqlite` is retired from normal launches. Test fixtures require
explicit `create-demo` / `create-road-demo` commands and separate paths. The former
`demo` cooker and `sync-demo-vegetation` reset commands are removed. Vegetation is
edited through the editor; intentional catalog replacement uses `import-vegetation`.

Source and generated databases remain local and ignored by Git. Models live in
`source_assets`; placements reference `object_definitions`. Authored presets, layers,
coverage masks and road geometry are source records, not cooked render pages.

Standalone gameplay source lives in `gameplay/demo`: editable RON definitions and
scenario setup alongside English/Ukrainian Fluent resources. It is independent of
the current world database. Use `cargo run --offline -p yarra-game-content -- validate
content/gameplay/demo` to validate it; see the [gameplay workflow](../docs/WORKFLOWS.md#standalone-gameplay-and-saves)
for running the scenario, publishing SQLite bundles and save/load checks.

Gameplay source format 6 uses explicitly declared packages and one conversation per
asset directory. Graphs, local bindings, message contracts and source/translations
are grouped together. Text references contain a stable resource UUID and local key.
Mechanical SQLite schema 6 contains indexed definitions/contracts; separately
published language packs contain wording and review metadata. Both runtime readers
open without decoding all records. Fluent scopes and locale resources load on demand.

Runtime sessions resolve bounded content/state sets. Save schema 7 stores mutable
state and references a retained mechanical generation; independent wording updates
preserve save compatibility. The demo keeps mechanics beside its manual/quick/auto
slots. See the workflow for `build-language`, shipping-locale review checks and
explicit pack selection.

The guard package adds separate quest, named-predicate and NPC interaction-profile
assets. Run `cargo run --offline -p yarra-game-content -- scenario
content/gameplay/demo scenarios/guard.ron` for quest/attitude selection, concurrent
topics and atomic rewards. Runtime profiles fetch only condition dependencies and
the selected conversation. Selection and both RNG streams survive save/load.
Conversations now have role-bound lines, typed Fluent arguments, saved cursors,
explicit repeat policies and separate scoped history/reward claims. The additional
`scenarios/guard-refusal.ron` exercise demonstrates interruption, remembered refusal
and returning through a quest topic. Rebuild old publications and regenerate old
save fixtures; format compatibility layers are intentionally absent.

The guard package also owns `world/` object, area and trigger definitions. Run
`cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard-gate.ron`
for an area-triggered escort, pending movement and gate/container access after arrival.
Triggers and area bounds have SQLite indexes; saved pending events and sequences
survive checkpoints. Movement reports in this exercise are synthetic adapter inputs;
production navigation and graphical integration remain separate work.
