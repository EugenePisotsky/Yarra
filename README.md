# Yarra

Yarra now has a minimal SQLite-backed world path while keeping gameplay small:

```text
crates/
  app_game/   Executable and platform composition
  engine/     Bevy gameplay, rendering, and bounded page streaming
  world/      Bevy-free coordinates, IDs, page domains, and payload ABI
  world_db/   Strict authoring/runtime SQLite schemas and readers
  world_cook/ Authoring database to immutable runtime generation
```

The data flow is deliberately one-way:

```text
content/demo.project.sqlite
  -> yarra-world-cook
  -> assets/generated/demo.runtime.sqlite
  -> dedicated read-only database worker
  -> async page decode
  -> bounded Bevy attachment and explicit removal
```

The authored demo contains two independent world spaces: a 4,096-cell overworld
and an 81-cell interior. Cells in both areas may use the same coordinates because
the world-space ID is part of every persistent cell and page key. Only the cells
demanded by the active area's camera/player working set become Bevy entities.
The derived runtime database is ignored; the authoring database remains source
content.

Cook the runtime generation after cloning or changing authored content:

```bash
cargo run -p yarra-world-cook -- demo
```

The command creates the demo authoring database only if it does not exist. It
then cooks and atomically publishes a new immutable runtime generation.

Run the game:

```bash
cargo run --release -p yarra-app-game
```

The current scene contains:

- SQLite-streamed flat terrain cells;
- sparse streamed instances of the local evaluation tree;
- one movable cube;
- a following camera and bounded cascaded directional shadows;
- FPS/frame-time and page residency diagnostics.

Authored placements reference stable object definitions rather than models
directly. The cooker resolves visuals into camera-driven render pages and emits
separate gameplay pages only for proximity-activated definitions. The runtime
requests those gameplay pages for the player's current cell and immediate
neighbours, then fetches all page definitions in one SQLite query.

Left-click anywhere on the ground to set a movement target, or use WASD / the
left gamepad stick for direct camera-relative movement. A circular ground marker
shows the active click target. Swipe horizontally with two fingers, right-drag,
or use the right stick to orbit. Swipe vertically or use the mouse wheel for
smooth zoom between the default high-angle view and a close third-person view.
Press Tab to move between the demo overworld and interior. A transition removes
the previous area's residency set before requesting pages for the destination.
VSync follows the display's refresh rate; on a 120 Hz display, every frame has
an 8.33 ms deadline.

Run the automated live traversal check with:

```bash
cargo run -p yarra-app-game -- --streaming-smoke
```

It verifies initial residency, teleports five cells, waits for the cooling
deadline, enters the second world space, and asserts that no page-owned roots
from either previous working set survived.

See [`assets/README.md`](assets/README.md) for why the evaluation asset itself is
kept out of Git.
