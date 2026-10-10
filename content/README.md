# Authored content

`world.project.sqlite` is the current editable world: the island imported from Houdini
(see [Terrain from Houdini](../docs/WORKFLOWS.md#terrain-from-houdini)) with its painted
ground and grass layers, shared presets and placed trees, on 32 m cells. The editor
opens it by default. The game reads only its published `assets/generated/world.runtime.sqlite`.

For a fresh checkout without a project, `init` creates and cooks the procedural island:

```bash
cargo run -p yarra-world-cook -- init
cargo run -p yarra-app-editor
```

`init` creates a world only when the project is absent. Normal **Save** writes source
changes; **Save & Publish** cooks and atomically replaces the runtime. The command-line
equivalent is:

```bash
cargo run -p yarra-world-cook -- cook
```

Test fixtures use the explicit `create-demo` / `create-road-demo` commands and separate
paths. Vegetation is edited in the editor; intentional catalog replacement uses
`import-vegetation`.

Source and generated databases remain local and ignored by Git. Models live in
`source_assets`; placements reference `object_definitions`. Authored presets, layers,
coverage masks and road geometry are source records, not cooked render pages.

Standalone gameplay source lives in `gameplay/demo`: editable RON definitions and
scenario setup alongside English/Ukrainian Fluent resources. It is independent of
the current world database. Use `cargo run --offline -p yarra-game-content -- validate
content/gameplay/demo` to validate it; see the [gameplay workflow](../docs/WORKFLOWS.md#standalone-gameplay-and-saves)
for running the scenario, publishing SQLite bundles and save/load checks.

Gameplay source format 11: `project.ron` lists package directories, and each file in a
package says what it holds by its name (`*.quest.ron`, `*.dialogue.ron`, `*.trigger.ron`,
`en.ftl`…). A package has one text resource, its Fluent files; content writes
`Message("key")` for its own package's text, and what a message takes comes from the
English file. Content names only characters declared in `characters.ron`. `build`
publishes an immutable SQLite bundle (schema 13) with one checksummed record per asset;
language packs are published separately and hold the wording.

At runtime the whole playthrough state is held in memory and saved as one snapshot file
per slot (save format 16). A session loads the always-needed definitions once and each
dialogue graph when a conversation needs it. A save loads with changed content, brought
in line with it; wording updates do not affect it. Old bundles and saves of another
format are rebuilt, not migrated.

The core package holds the rules: stats, classes, levels, status effects and abilities in
`rules.ron`, with their formulas in `scripts/rules.luau` and what the abilities do in
`scripts/abilities.luau`. The guard package adds quests, a named predicate, an
interaction profile, loot, conversations and `world/` objects and triggers. Four scenarios,
sharing the start in `scenarios/guard-start.ron`,
exercise them through the same commands the game uses:

```sh
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard.ron
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard-refusal.ron
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard-gate.ron
cargo run --offline -p yarra-game-content -- scenario content/gameplay/demo scenarios/guard-training.ron
```

Occupancy reports in these exercises are written into the scenario. In the game they come
from the engine: `cargo run --release -p yarra-app-game -- --story content/gameplay/demo`
plays the same triggers with shapes for `guard/approach` and `guard/gate_post`, painted in
the editor's Areas tool or stood in beside the guard.
