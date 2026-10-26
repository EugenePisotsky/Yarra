# Yarra

A third-person game and its world editor in Rust on Bevy 0.20 (macOS and iOS, Metal): SQLite-backed
world authoring, a cook into immutable runtime generations, bounded streaming around the camera,
terrain on GPU clipmaps, GPU grass and trees, a shared atmosphere, and a Bevy-free gameplay layer
with RON/Fluent/Luau content.

## Documents

| Document | What it holds |
| --- | --- |
| [Architecture](docs/ARCHITECTURE.md) | Which crate owns what, how data moves, formats and versions |
| [Workflows](docs/WORKFLOWS.md) | Commands: setup, the world, editor, game, content, checks, measuring |
| [Decisions](docs/DECISIONS.md) | What was kept and what was rejected, and why |
| [Backlog](docs/BACKLOG.md) | Open work |
| [AGENTS.md](AGENTS.md) | Code style and the checks to run (for people and coding agents) |

Update these instead of adding reports; history is in Git.

## Run

Restore the ignored local assets ([assets](assets/README.md)), prepare the terrain textures and
build the world, Reteya ([its README](content/reteya/README.md)), from the repository root:

```sh
uv run --with numpy --with pillow python tools/import_terrain_surfaces.py
uv run python tools/compile_terrain_textures.py
cargo build --release -p yarra-world-cook
content/reteya/scripts/build_world.sh tmp/reteya
content/reteya/scripts/install_world.sh tmp/reteya
cargo run --release -p yarra-app-editor
cargo run --release -p yarra-app-game
```

In the editor, **Save** writes the project (`content/world.project.sqlite`); **Save & Publish**
also cooks the runtime the game reads (`assets/generated/world.runtime.sqlite`). From the command
line: `target/release/yarra-world-cook cook`.

In the game, move with WASD, a gamepad or a ground click; orbit with right-drag or the right
stick. **F1** opens the performance panel. `--help` lists every launch option.

On macOS `cargo run` starts the game from an app bundle beside its binary
(`target/release/Yarra.app`), which macOS treats as a game and gives Game Mode;
`open target/release/Yarra.app` launches the same bundle as an app.
