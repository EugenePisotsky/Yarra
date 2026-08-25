# Yarra

A deliberately small starting point for the new Yarra project. The repository
is a Cargo workspace from the beginning:

```text
crates/
  app_game/  Executable and platform composition
  engine/    Reusable game scene, input, movement, and diagnostics
```

The current scene contains only:

- a flat, untextured ground plane;
- one movable cube;
- a fixed camera and one shadow-free directional light;
- a lightweight FPS/frame-time display.

Run the optimized baseline:

```bash
cargo run --release -p yarra-app-game
```

Left-click anywhere on the ground to move the cube. Rendering is uncapped so the
overlay exposes the scene's approximate performance floor at 1280 × 720.
