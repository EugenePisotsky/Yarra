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

Gameplay source format 8 uses explicitly declared packages and one conversation per
asset directory: the graph (with its conditions and actions inline), message contracts
and translations are grouped together. Text references contain a stable resource UUID and a local key. `build`
publishes an immutable SQLite bundle (schema 9) with one checksummed record per asset;
language packs are published separately and hold the wording and review metadata.

At runtime the whole playthrough state is held in memory and saved as one snapshot file
per slot (save format 10). A session loads the always-needed definitions once and each
dialogue graph when a conversation needs it. Saves are tied to the content they were
made with; wording updates do not affect them. Old bundles and saves are rebuilt, not
migrated.

The guard package adds quest, named-predicate, interaction-profile and `world/` object,
area and trigger assets. Three scenarios exercise them through the same commands the
game will use:

```sh
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard.ron
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard-refusal.ron
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard-gate.ron
```

Movement reports in these exercises are synthetic adapter inputs; engine integration is
the next step in the [roadmap](../docs/REFACTORING.md#gameplay-roadmap).
