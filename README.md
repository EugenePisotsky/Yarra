# Yarra

Yarra now has a minimal SQLite-backed world path while keeping gameplay small:

```text
crates/
  app_editor/ Separate bounded world-editor viewport and shell
  app_game/   Executable and platform composition
  engine/     Bevy gameplay, rendering, and bounded page streaming
  vegetation/ Renderer-neutral V2 species, population, and field contracts
  vegetation_compile/ Deterministic field compilation and CPU placement reference
  vegetation_render/ GPU placement diagnostics and indirect-draw integration
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

Run the editor foundation:

```bash
cargo run -p yarra-app-editor
```

The editor opens in a typed World workspace and can switch through its shell to an isolated
Animation workspace with its own 3D camera, catalog-backed model/clip browser, and transport
lifecycle. The shell has an always-on UI camera, while workspace input,
viewport cameras, transient interactions, and preview frame-rate requests have separate lifecycles;
world selection, history, dirty edits, and camera state survive a workflow switch. The World
workspace renders the same cooked runtime pages as the game, but owns an independent logical
viewpoint and floating render origin. Right-drag or middle-drag orbits, Shift+right-drag pans, the
mouse wheel or trackpad pinch zooms, and holding right mouse while using WASD/QE flies the camera.
Close-range navigation keeps a minimum useful sensitivity, and zooming inward at minimum orbit
distance advances the focus instead of becoming stuck. Its shell exposes
two default movable World and Inspector windows plus a Tools menu for Assets, Navigator, and
Diagnostics. The World window switches between Terrain, Grass, and the bounded visible-assets
tree; the Inspector follows that context or the active stable-ID object selection. Logical
coordinates, origin state, and bounded page/memory details live in optional Diagnostics rather than
occupying the viewport permanently. Objects, Terrain, and Ground Cover are registered bounded
tools; window visibility is presentation-only, while tool activation controls source demand. Dense terrain/mask
queries and revision-checked writes are in place, while brush gestures and publishing a newly cooked
runtime generation remain later increments. A project worker follows the camera with a bounded 5×5
domain-specific source query, reports source revisions, and discards stale results without loading a
`ProjectDocument`. Its visible-assets tree selects by stable ID and promotes selected or edited
placements to disposable floating-origin-relative editor proxies with source-backed LOD0 visuals.
Matching cooked instances are hidden while authoring proxies are ready, and deletion tombstones hide
stale cooked instances without mutating runtime data. Cyan
placement handles retain source asset bounds as a fallback, while visible streamed meshes use
triangle-accurate picking resolved back to stable object roots. Shift/Cmd-click builds an ordered
multi-selection: the active item gets a blue bounds frame and gizmo, companions get gold frames, and
gizmo transforms and deletion each remain one undo step. The active object gets
Bevy's stock world-space move/yaw/uniform-scale control (`1`/`2`/`3`). The stock mesh layer is composited by the
clearing world camera so reactive rendering cannot retain old control frames. Gizmo gestures and
the transform inspector produce bounded stable-ID commands; gizmo deltas apply to same-world
companions, while inspector fields remain active-item-only. Cmd+Z/Cmd+Shift+Z undo and redo across
selection changes, Delete/Backspace creates a reversible tombstone, and Cmd+S atomically saves dirty
placement changes only when their source revisions still match. A bounded definition palette places
new UUID-backed objects at the logical viewpoint through the same command and save pipeline.
The underlying cooked runtime snapshot intentionally remains unchanged until a later explicit cook; see
[`docs/EDITOR.md`](docs/EDITOR.md) for the complete architecture contract and legacy-editor analysis.

For a physical iPhone build, open
[`ios/Yarra/Yarra.xcodeproj`](ios/Yarra/Yarra.xcodeproj) and run the `Yarra`
scheme. Its build phase cooks the current world, cross-compiles the Rust
executable, and packages only runtime assets. Use the Release configuration for
performance measurements; see [`ios/README.md`](ios/README.md).

The current scene contains:

- SQLite-streamed relief terrain pages with shared height/normal sampling;
- streamed V2 vegetation fields rendered through GPU candidate generation and procedural topology;
- sparse streamed instances of the local evaluation tree with four mesh LODs;
- one catalog-selected animated character with controller-owned Idle/Walk/Jog locomotion;
- a following camera and bounded cascaded directional shadows;
- FPS/frame-time and page residency diagnostics.

Authored placements reference stable object definitions rather than models
directly. The cooker resolves visuals into camera-driven render pages and emits
separate gameplay pages only for proximity-activated definitions. The runtime
requests those gameplay pages for the player's current cell and immediate
neighbours, then fetches all page definitions in one SQLite query.

Tap or left-click anywhere on the ground to set a movement target, or use WASD /
the left gamepad stick for direct camera-relative movement. A circular ground
marker shows the active target. Swipe horizontally with two fingers, right-drag,
or use the right stick to orbit. Swipe vertically or use the mouse wheel for
smooth zoom between the default high-angle view and a close third-person view.
Press Tab to move between the demo overworld and interior. A transition removes
the previous area's residency set before requesting pages for the destination.
VSync follows the display's refresh rate; on a 120 Hz display, every frame has
an 8.33 ms deadline.

The actor root owns movement while a stable presentation-profile reference
selects a presentation-only imported scene, compatible animation bank, and
movement tuning. Camera follow and world streaming are explicit leader roles,
so future party followers and NPCs can reuse the same motor and animation output
without affecting either. The character/animation spec also records the reserved
extension points for alternate meshes, crouch, actions, equipment, and combat;
see [`docs/CHARACTERS.md`](docs/CHARACTERS.md).

Tree LOD is selected from projected logical-pixel height rather than world
distance, so camera zoom, third-person perspective, and elevation affect it
correctly. The current thresholds are LOD0 at 320 px, LOD1 at 160 px, LOD2 at
80 px, and LOD3 below that, with 12% hysteresis. The HUD shows active LOD counts
and the projected-size range of resident trees.

Vegetation V2 stores one generation-level species/population catalog plus compact,
terrain-independent field pages. The runtime joins each resident field page with the
matching streamed terrain surface, generates candidates on the GPU, finalizes an indirect draw,
and renders species-driven cubic ribbons without creating one database row, vertex stream, or Bevy
entity per blade. A GPU view scheduler compacts visible page-field work and writes the indirect
candidate dispatch; four draw bins cross single/split topology with high/low geometry selected from
species-authored projected-pixel thresholds. Run with `--vegetation-v2-debug` and press `X` to switch
from geometry to accepted-instance, parent/child, and candidate-outcome diagnostics. Press `P` to
cycle full, frozen-draw, compute-only, and scheduler-only profiling workloads; frozen draw is intended
for a fixed camera immediately after the full mode. Procedural
instances are 32 bytes, capacity pressure uses distance-prioritized stable seed buckets, and a dense
short split-blade population fills beneath the taller curved ribbons. High geometry converges onto
low samples and uses a stable nested density transition. Compact vegetation fields and their terrain
relief stay in a rotation-invariant three-cell source shell while GPU work remains view-culled.
Persistent GPU page slots, a measured far
representation, a full broad-leaf family, wind, interaction, material filtering, and shadows are the
next renderer milestones; the former card/ribbon pipeline and its editor/database schema have been
removed completely.

Run the automated live traversal check with:

```bash
cargo run -p yarra-app-game -- --streaming-smoke
```

It verifies initial residency, teleports five cells, waits for the cooling
deadline, enters the second world space, and asserts that no page-owned roots
from either previous working set survived.

See [`assets/README.md`](assets/README.md) for why the evaluation asset itself is
kept out of Git.
