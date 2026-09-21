# Yarra

A Rust/Bevy game and world editor with SQLite-backed authoring, bounded world streaming, hierarchical terrain, procedural grass and shared atmosphere rendering.

## Start here

| Guide | Purpose |
| --- | --- |
| [Architecture](docs/ARCHITECTURE.md) | Crate ownership, data flow and invariants |
| [Workflows](docs/WORKFLOWS.md) | Setup, editor/game controls, save/publish and fixtures |
| [Performance](docs/PERFORMANCE.md) | F1, launch controls, repeatable profiling and measurement limits |
| [Experiments](docs/EXPERIMENTS.md) | What we tried, retained/rejected decisions and measured effects |
| [Refactoring](docs/REFACTORING.md) | Cleanup status, priorities and acceptance gates |

These are the five maintained project documents. Update them instead of adding individual design/handoff/run reports. Raw captures and machine-readable summaries remain under `docs/performance/`; earlier prose is in Git.

## Run

Restore the ignored local assets described in [asset setup](assets/README.md), then prepare and initialize the world from the repository root:

```sh
python3 tools/compile_terrain_textures.py
python3 tools/prepare_terrain_bake.py
cargo run -p yarra-world-cook -- init
cargo run -p yarra-app-editor
```

**Save** updates source; **Save & Publish** also updates the game's runtime. `init` creates missing source without resetting existing edits. To publish and play:

```sh
cargo run -p yarra-world-cook -- cook
cargo run --release -p yarra-app-game
```

Default files are `content/world.project.sqlite` and `assets/generated/world.runtime.sqlite`, both ignored. The current authored world uses 8 m cells; old 32 m grass fixtures are separate workloads. See workflows for explicit paths, recovery and disposable fixtures.

Move with WASD/gamepad or a ground click/tap; orbit with right-drag/two-finger horizontal scrolling/right stick. **F1** opens the unified Performance panel. Normal presentation uses 50% physical world resolution, 4× MSAA and native-resolution UI; Auto selects MetalFX Spatial where supported, otherwise Linear. Temporal remains an explicit prototype. **F1 → FPS limit** selects Follow display / 30 / 60 / 120; `--fps 60` chooses a launch cap. Optional grass counters default off. `--diagnostics panel` keeps F1 without timing probes; `--diagnostics off` omits F1 and instrumentation. Full diagnostics remain the default. Run with `--help` for validated launch options.

For physical-device packaging and capture, see [iOS](ios/README.md). For validation and bounded measurements, use the workflows/performance guides. Historical performance observations are not acceptance for a different device, scene or resolution.
