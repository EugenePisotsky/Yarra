# Yarra

Yarra now has a minimal SQLite-backed world path while keeping gameplay small:

```text
crates/
  app_editor/ Separate bounded world-editor viewport and shell
  app_game/   Executable and platform composition
  engine/     Bevy gameplay, rendering, and bounded page streaming
  environment/ Environment compositions, layers and spatial source contracts
  environment_compile/ Pure ground/vegetation/object compiler and offline acceptance fixture
  vegetation/ Renderer-neutral V2 species, population, and field contracts
  vegetation_compile/ Deterministic field compilation and CPU placement reference
  vegetation_render/ GPU placement diagnostics and indirect-draw integration
  world/      Bevy-free coordinates, IDs, page domains, and payload ABI
  world_db/   Strict authoring/runtime SQLite schemas and readers
  world_cook/ Authoring database to immutable runtime generation
```

The data flow is deliberately one-way:

```text
content/world.project.sqlite
  -> yarra-world-cook
  -> assets/generated/world.runtime.sqlite
  -> dedicated read-only database worker
  -> async page decode
  -> bounded Bevy attachment and explicit removal
```

The current authoring world contains an overworld and interior, painted meadow layers,
stable tree placements and a curved cart road. Its 8 m cells resolve the physical wheel
tracks. The editor opens `content/world.project.sqlite`; the game opens its published
`assets/generated/world.runtime.sqlite`. These are the shared defaults for cooking,
profiling and iOS packaging too. Both local databases are ignored by Git.

On a fresh checkout, initialize the editable world and its runtime once:

```bash
cargo run -p yarra-world-cook -- init
```

Then open the editor normally:

```bash
cargo run -p yarra-app-editor
```

Save updates the source; **Save & Publish** also updates the game's runtime. To publish
from the command line:

```bash
cargo run -p yarra-world-cook -- cook
```

`init` creates source only when absent; neither command resets existing edits. `cook`
requires existing source and never silently creates a demo. Explicit paths work with
`init PROJECT_DB RUNTIME_DB`, `cook PROJECT_DB RUNTIME_DB`, or the editor's
`--project-db` / `--world-db` options. Project schema 22 stores typed presets, layers,
coverage and roads. Incompatible source databases must be replaced explicitly; the
editor now reports missing/incompatible or mismatched databases before opening its
window. `create-demo` and `create-road-demo` remain explicit disposable test-fixture
commands, separate from normal launches. See
[environment authoring](docs/ENVIRONMENT_AUTHORING_ARCHITECTURE.md) for the design.
In the editor, choose
**World → Environment**, select a layer, and drag on terrain. Shift-drag erases;
each drag supports Undo/Redo. The Inspector browses **Nearby / All** layers, creates
and reorders them, and shows selected-layer coverage. Quick settings group each
composition use; Reset restores its inherited default. **Edit shared preset…** opens
the **Presets** workspace for reusable defaults, child references and duplication,
with an isolated 8 m / 16 m ground, grass and object preview. **Apply preset changes** and
**Apply settings** are separate undo steps; **Save & Publish** updates the game runtime.

**Presets → Environment → New → Asset collection** creates a paintable collection of
trees, bushes or rocks from registered assets. Configure selection weights, scale ranges,
spacing, slope and road clearance there; map layers expose density and seed. Collections
also work inside compositions. Placement is deterministic across cells and uses the
existing object LOD renderer. The local library currently has one tree; generated
objects are render-only and are not individually editable. See
[painting objects](docs/EDITOR.md#painting-trees-bushes-and-rocks) for the workflow.
Runtime schema is **17**; recook an older runtime before launching the updated editor.

Run the game:

```bash
cargo run --release -p yarra-app-game
```

Normal game launches use **75% world render resolution and 4× MSAA**, with UI
rendered at native resolution. Prepared ground is enabled and optional GPU statistics
are off. No audit or profiling arguments are needed. `--terrain-reference` selects
the original ground material; `--grass-counters` enables GPU statistics.

For fullscreen grass tests at the normal world scale, run
`python3 tools/grass_profile.py run` in a local terminal. It records the actual
world/surface resolutions and collects sustained power, GPU clocks and frame
delivery with one sudo authentication for `powermetrics`; `--power off` skips privileged telemetry. See
[`docs/GRASS_PROFILING_TOOLS.md`](docs/GRASS_PROFILING_TOOLS.md) for 60/120 fps and
grass-on/off suites, repeatable input snapshots, and offline reports.
Track measured results, retained/reverted experiments and next tests in
[`docs/GRASS_OPTIMIZATION_LOG.md`](docs/GRASS_OPTIMIZATION_LOG.md).

The grass-shadow playtest starts at **Medium** on desktop.
Press **B** to cycle Off / Subtle / Medium; `--grass-bands off` starts without it.
The editor's Grass study → **Colors…** provides live root/tip color and clump-variation
controls. Save study keeps a local experiment; Inspector → Save & Publish applies
catalog colors to the runtime used by the game.

Run the editor foundation:

```bash
cargo run -p yarra-app-editor
```

The new authoring architecture is documented in
[`docs/ENVIRONMENT_AUTHORING_ARCHITECTURE.md`](docs/ENVIRONMENT_AUTHORING_ARCHITECTURE.md):
reusable environment compositions, painted layers, shared ground/vegetation derivation,
and editable curved cart roads. The pure compiler and its two-cell fixture are
implemented together with source persistence, the layer/preset Inspector and paint UI. Run the offline fixture with
`cargo run --offline -p yarra-environment-compile --example layered_meadow > /tmp/meadow.svg`.

The next rendering foundation is specified in
[`docs/DISTANT_WORLD_RENDERING.md`](docs/DISTANT_WORLD_RENDERING.md): hierarchical
terrain LOD, independent distant visibility, cheaper ground materials, and later
cliff/forest proxies. Terrain precision, bounded production cooking and hierarchy products are implemented;
visible terrain LOD and distant materials remain planned. Meshlets are out of scope.

The first curved-road source/compiler fixture is also available:
`cargo run --offline -p yarra-environment-compile --example cart_track > /tmp/cart-track.svg`.
It demonstrates cart wheel tracks, a retained grassy middle, patchy wear and gradual
shoulders. **World → Roads** now authors curves with movable points, tangent and width
handles, extension, splitting, undo/redo, recovery and live ground/grass previews.
**Presets → Road styles** authors shared wheel/center/shoulder wear, grass retention,
irregular edges, linked Ground material mixtures, and road/rut relief with seeded depth variation,
with a straight/curved test preview.
Explicit road junctions connect two–four arms using the same style, blend wear and relief,
and move their connected points together. Save checkpoints roads and painter changes together; Save & Publish cooks them for the game.
Use a fresh schema-22 project; older source databases are not migrated automatically. Journal schema is 11.

For an isolated road experiment, create a disposable 8 m fixture; it has the same
grid as the current authoring world. The 32 m grass fixture remains available through
`create-demo` for explicit regression work:

```bash
cargo run -p yarra-world-cook -- create-road-demo tmp/my-roads.project.sqlite
cargo run -p yarra-world-cook -- cook tmp/my-roads.project.sqlite tmp/my-roads.runtime.sqlite
cargo run -p yarra-app-editor -- --project-db tmp/my-roads.project.sqlite --world-db tmp/my-roads.runtime.sqlite
```

Choose **Roads**, select a style, then click **New cart road** and place two points.
Use **Road style library…** to create or edit shared styles.
See [road authoring](docs/EDITOR.md#roads) for controls and current limits.
Old testing-world formats need no backwards compatibility or import path.

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
