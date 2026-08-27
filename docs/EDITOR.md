# Scalable world editor foundation specification

This document is the contract for reintroducing the Yarra editor on top of the page-oriented world
engine. It records both the first read-only viewport slice and the authoring architecture that later
terrain, vegetation, object, navigation, and encounter tools must use. Reserved concepts are design
constraints for future work, not a request to build every editor feature now.

## Goals

- Camera navigation remains immediate and numerically stable at any supported world cell.
- Opening a project does not deserialize, clone, reconcile, or spawn the complete world.
- Viewport cost is bounded by explicit page, memory, upload, and entity budgets rather than project
  size.
- The editor previews the same cooked page formats and renderers as the game.
- Mutable source data is loaded only for the cells and domains currently being edited.
- Selection, undo, saving, background derivation, and the outliner operate on stable IDs and database
  queries, not on a monolithic scene document.
- A slow tool or missing page may reduce detail or show a loading state, but must not stop the camera.
- Editor-only convenience state stays separate from runtime world data.

The editor does not need to sustain the game's frame rate. It may use lower reactive frame pacing,
bounded per-frame work, placeholders, delayed high-detail loading, and proxy views. It must not hide
unbounded work behind an acceptable frame rate on the current demo.

## What is retained from the legacy editor

The legacy editor has useful interaction and composition ideas:

- a separate editor executable and workspace shell;
- a free orbit/pan/fly camera;
- stable-ID selection, transform gizmos, and explicit tools;
- editor-only visibility and locking;
- an asset-browser/palette layout;
- distinct authoring and full-rate preview modes;
- reactive frame pacing: approximately 30 FPS while authoring, 60 FPS for live preview, and 5 FPS
  while unfocused.

Those behaviors can be ported incrementally. Their old document and residency model cannot.

## Workspace shell and workflow isolation

The editor is one executable with several substantial workspaces, not one world viewport that
accumulates unrelated tool panels. `World` is the default workspace. `Animation` is the next
registered workspace, and dialogue, quest, item, character, or other focused workflows may be
added through the same boundary when their data models exist.

The shell owns only cross-workspace concerns:

- the active typed workspace state and workspace switcher;
- an always-active transparent UI camera, independent from any 3D viewport camera;
- reactive frame pacing and the current workspace's full-rate-preview request;
- project identity, global diagnostics, and completion of already queued asynchronous writes.

Each workspace owns its own persistent UI/tool state, viewport camera and render layers, transient
input state, and Bevy systems. Workspace systems use state run conditions and may have explicit
enter/exit cleanup. Only the active workspace's viewport camera and input systems run. Switching
away from the world clears drag/gizmo transactions but preserves the logical camera, selection,
history, dirty object working set, and bounded resident pages so returning does not mean reopening
the project. In-flight save completion remains shell-level and is allowed to finish while another
workspace is visible.

```text
always-on editor shell + UI camera
  ├── World workspace (default)
  │     logical camera, streaming viewport, selection, object tools
  └── Animation workspace
        independent preview camera, catalog/tool state, transport and diagnostics
```

### Workspace windows and authoring tools

The shell maintains a typed registry of floating windows scoped to each workspace. The active
workspace contributes descriptors, the shell exposes them through its `Tools` menu, and the
workspace remains responsible for rendering their contents. Closing a window updates presentation
state only: it must not change source demand, pinning, commands, or derived invalidation.

The World workspace initially opens two movable, resizable windows constrained to the viewport:

- `World` on the left selects the active world space and authoring domain, then lists only the
  currently bounded visible-asset working set;
- `Inspector` on the right renders the active domain or stable-ID object selection.

`Assets`, `Navigator`, and `Diagnostics` are closed by default and can be opened from `Tools`.
`Assets` is the bounded definition palette, `Navigator` owns cursor-paginated project search and
overview jumps, and `Diagnostics` contains streaming, source, derivation, memory, frame, and preview
details that should not occupy the normal authoring layout.

Window identity is deliberately separate from `EditorToolRegistry`. Selecting Terrain, Grass, or
Visible assets in the World window activates the corresponding bounded tool contract. Merely
opening Assets, Navigator, or Diagnostics does not activate a tool or expand the camera-following
source query. Other workspaces can register their own windows without adding workspace-specific UI
branches to the shell.

The registered Animation workspace owns an independent 3D camera, persistent presentation-profile
and animation-set selections, and enter/exit-reset transport state. Its legacy preview code is not
ported wholesale because it assumes the old animation catalog and character model. Rendering the
selected model and browsing/playing real clips will consume the new character presentation model:
mesh/model choice, texture/material choice, locomotion and crouch sets, additional animation banks,
and equipment-dependent combat sets.

## Why the legacy world model is not the foundation

The old `ActiveScene` retained a complete `SceneDocument` and another complete saved copy. Undo kept
up to 128 more full document clones. Project loading read every area before constructing the world,
the active area created an ECS root per authored object, and the scene tree scanned all objects while
drawing UI. Its streaming system removed heavy visual children but left the full source document and
object roots resident.

That model appeared adequate only because the prototypes contained roughly twenty objects. Dense
settlements, painted terrain, and a large outliner would make memory, history, reconciliation, and UI
cost grow with the complete project. The old absolute `Vec3` camera also loses useful precision far
from the origin.

The new editor therefore is not a revived `ActiveScene`. It is a spatial client of persistent stores.

## Data ownership

The editor has two independent bounded working sets:

```text
project SQLite (mutable source of truth)
  -> ProjectEditorStore queries
  -> authoring-detail cache near camera + selected/dirty pinned cells
  -> in-memory edits and preview overlays
  -> granular transactions back to project SQLite

cooked runtime SQLite (immutable visual snapshot)
  -> existing async runtime page streamer
  -> bounded decoded/GPU residency
  -> terrain, ground cover, static-object, and proxy presentation
```

The cooked snapshot supplies broad visual context cheaply and through the same path used by the
game. It is never modified in place. Dirty authoring data overlays or temporarily replaces affected
runtime pages. Background cooking later publishes a new immutable generation, after which clean
preview pages may switch to it.

The source and runtime databases may temporarily describe different revisions. The editor UI must
make dirty/deriving/stale/failed states visible; it must not pretend every edit has already been
cooked.

## Logical coordinates and floating origin

Persistent positions use `WorldPosition`: world-space ID, integer cell, and cell-local `f32`
coordinates. The editor camera focus and streaming viewpoint use this logical representation rather
than an absolute `Vec3`.

`WorldOrigin` selects the cell represented by render-space origin. Page roots, the camera, gizmos,
and promoted authoring entities are positioned relative to that cell. The editor rebases when the
viewpoint moves beyond a small cell threshold. A rebase may discard and rebuild bounded presentation
pages; it never changes persistent positions.

```text
persistent WorldPosition
  - current WorldOrigin cell
  -> small render-space Vec3
```

Camera input updates the logical focus immediately, even when no cell descriptor or page is loaded at
the destination. The viewport may show only the grid and loading diagnostics until data arrives.
Requests carry revisions, and results for an obsolete space, index window, page demand, or edit
revision are ignored.

Navigation sensitivity uses a small close-range floor rather than shrinking all pan/fly/zoom input
with orbit distance. Zooming inward at the minimum orbit distance advances the logical focus along
the view direction instead of sticking at a clamp. At normal and large distances movement remains
proportional to distance.

The first implementation enables floating origin for the editor. Gameplay can adopt the same seam
later when its actor, camera, interaction fields, and physics state have an explicit rebase policy.

## Runtime visual context

The shared streamer is driven by explicit roles rather than by a required player entity:

- `WorldViewpoint` is the logical position around which indexes and preload pages are requested;
- `WorldViewCamera` marks a camera whose frustum drives visual page demand;
- `WorldOrigin` maps logical cells into render space;
- `WorldStreamingConfig` selects the editor or game policy;
- `ActiveWorldSpace` requests a transition without assuming that a playable character exists;
- public streaming diagnostics expose status and bounded residency without exposing internal page
  state.

The game synchronizes `WorldViewpoint` from its `WorldStreamFocus` actor. The editor owns the same
resource directly. Party members and NPCs never receive the stream-focus role and therefore do not
move either working set.

The editor policy initially loads terrain, static objects, and ground cover from the cooked snapshot,
but not gameplay-object activation pages, collision, navigation, or simulation. Later preview modes
may opt into additional domains explicitly.

Ordinary cooked objects remain page-batched. The editor must not create a permanent editable ECS
entity for every object in every visible cell. Hovered, selected, or actively edited stable objects
may be promoted to temporary editor entities; demotion returns them to the page/overlay
representation.

## Project editor store

The authoring API will be separate from the current eager `read_project_database` path, which remains
useful for cooking and whole-project validation. Its operations are asynchronous and query-shaped:

```text
query_world_spaces()
query_cell_descriptors(space, rectangle, domains)
load_cell(space, cell, domains)
query_objects(space, rectangle, filter, cursor)
load_object(stable_id)
apply_transaction(expected_revisions, commands)
```

Loaded source records retain their database revision. A write transaction compares expected
revisions and either commits atomically or reports a conflict. Dirty records and an active command
are pinned even if the camera leaves their cells; clean unselected records are evictable. Explicit
budgets cover cells, decoded blobs, promoted entities, pending queries, and background jobs.

Large dense blobs, such as terrain weights or vegetation masks, are loaded by domain and cell. Tools
must not fetch them merely because a cell name is visible in an outliner.

## Commands, history, and saving

Undo history stores commands, not document snapshots. A command identifies affected stable records
and carries the smallest reversible data:

- object creation/deletion or before/after transforms;
- metadata changes by stable ID;
- compact rectangles or patches for dense terrain/mask edits;
- transaction groups for one user gesture.

Long strokes may be chunked internally but remain one user-visible transaction. History has byte and
entry budgets and may checkpoint committed history. Saving writes only dirty records through bounded
database transactions. It does not serialize a full world or require all affected presentation pages
to be resident.

The object editor implements this through a stable-ID working set with three deliberately separate
states per touched placement:

- `base`: the last accepted project-database checkpoint;
- `current`: the result of local commands, or no record for a deletion tombstone;
- `runtime`: the placement still represented by the loaded cooked generation.

Selection contains only a stable ID. Changing or clearing selection therefore cannot discard a
command, and undo/redo can target a deselected object. Saving advances `base` but not `runtime`, so a
source-backed proxy or deletion tombstone continues to override stale cooked presentation until a
future cook publishes a matching runtime generation. A saved deletion can be undone: its inverse is
a revision-advancing placement recreation, not an attempt to resurrect an ECS entity.

`Cmd+Z` and `Cmd+Shift+Z` traverse the global command history (`Ctrl+Y` is also accepted for redo),
and `Delete`/`Backspace` records a deletion command. `Cmd+S` and the Save button create one
user-facing save-all intent. A coordinator drains every dirty region, object, and dense cell through
bounded domain transactions until all are clean; a failure or conflict stops the drain while
retaining local state for explicit resolution. Save & Publish uses the same drain and starts cooking
only after its final successful source transaction. The toolbar reports in-flight dirty records as
saving rather than asking the user to submit them again.

Palette placement creates a random stable ID at the logical viewpoint and remains local until
saved. One gizmo drag remains one command. Source revisions are checked inside each SQLite
transaction; one conflict rolls back every write in that transaction and preserves the local command
state for explicit resolution. Save is not cook: the broad runtime snapshot stays immutable until
the explicit publication stage adopts a validated generation.

Derived work is revisioned and cancellable. A result is accepted only if its input revisions still
match. Navigation, collision, thumbnails, cooked page previews, and other derived products follow the
same rule.

## Selection, outliner, and tools

Selection is a set of stable IDs plus an optional active item. ECS entities are disposable views of
that selection, never its identity. When selection leaves the visual working set, it stays selected
and its source records remain pinned; the viewport may show a proxy until full presentation returns.

The outliner is a paginated/filterable query view. It may show nearby cells, a named collection,
search results, or the current selection. It must not recursively materialize the complete project
tree on each UI frame.

Every tool declares:

- source domains it needs;
- its spatial query/pinning policy;
- the command type it produces;
- its preview overlay;
- derived jobs invalidated by the command;
- its loading, conflict, cancellation, and failure behavior.

Terrain and vegetation tools operate on bounded cell patches. Object tools query stable placements.
Global operations run as explicit batch jobs with progress and cancellation instead of masquerading
as interactive viewport work.

## Overview and very large views

Normal editing uses detailed pages near the camera. Zooming far enough to view a region or whole
world will eventually switch to separate overview products such as coarse terrain, density maps,
icons, or cell-status tiles. Increasing the detailed streaming radius is not an overview strategy.

Overview proxies are derived, revisioned data. Selecting a remote proxy may jump the logical camera
there and begin the normal local working set without first loading the intervening world.

## Frame and work budgets

The initial runtime profile retains bounded database requests, decode memory, GPU estimates,
attachments per frame, cooling, and LOD switches. Editor profiles may choose different values later,
but every queue and cache must remain bounded and visible in diagnostics.

The shell uses reactive frame pacing. Authoring can target 30 FPS, live preview 60 FPS, and an
unfocused editor 5 FPS. Database reads, decode, cooking, thumbnails, and validation run off the main
thread. Main-thread attachment and UI integration are metered per frame.

## Failure and recovery

- A missing or invalid runtime generation leaves the shell and logical camera usable and displays the
  error.
- A failed page shows a bounded placeholder/error state and can be retried without reopening the
  project.
- A stale async result is discarded.
- A source revision conflict preserves the local command and asks for explicit resolution.
- Dirty edits are journaled or transactionally saved before later crash-recovery work claims safety.
- Switching world spaces invalidates the previous visual demand and source queries without changing
  stable IDs.

## Current implemented slice

The first slice provides:

- a separate `yarra-app-editor` executable with a typed multi-workspace shell, dedicated UI camera,
  independent viewport-camera activation, 30 FPS authoring/60 FPS preview pacing, a default World
  workspace, and a registered Animation workspace with an independent preview lifecycle;
- workspace plugins that register their own resources, systems, viewport cameras, transient
  lifecycle, and contributions to one shell-owned egui frame; the Animation workspace already owns
  an independent 3D preview camera, persistent presentation selection, and transport state; the
  executable composition root is shell-only, while World and Animation are physical sibling
  modules rather than one workspace embedded in the other;
- a typed workspace-window registry and shell `Tools` menu; the World workspace uses floating,
  viewport-constrained World and Inspector windows by default, with optional Assets, Navigator, and
  Diagnostics windows, while window visibility remains independent from authoring-tool data demand;
- the shared runtime streamer decoupled from a mandatory player and game camera;
- explicit logical `WorldViewpoint`, `WorldOrigin`, and `WorldViewCamera` roles;
- an editor streaming policy with gameplay activation disabled and floating-origin rebasing enabled;
- a free orbit/pan/fly camera whose focus is a `WorldPosition`;
- a grid, world-space selection, logical coordinates, and page/memory diagnostics;
- read-only rendering of the existing cooked runtime snapshot;
- a separate read-only `ProjectReader` with bounded cell/object spatial queries and exact-object
  lookup, without constructing `ProjectDocument`;
- an asynchronous `ProjectEditorStore` working set that follows the logical viewpoint, retains
  source revisions, validates source/runtime world-space compatibility, replaces its 5×5-cell cache,
  caps object results, and rejects stale query results;
- bounded object-view queries that join placements to the small definition metadata plus the LOD0
  visual URI and dimensions needed by the editor without loading complete catalog tables;
- a filterable visible-assets tree in the World floating window, stable-ID-only selection, and an
  object working set that pins the source snapshots needed by active commands independently of
  bounded query-window eviction;
- ordered stable-ID multi-selection through Shift/Cmd-click, with one active item for the inspector
  and gizmo, blue active bounds, gold companion bounds, and grouped transform/deletion gestures as
  one undo command;
- promotion of the selected and source/runtime-divergent placements to disposable,
  floating-origin-relative editor proxies with source-backed LOD0 scenes; only the active selection
  receives the bounds frame and stock gizmo, and presentation is capped at 256 proxies within four
  cells of the logical viewpoint;
- temporary hiding of a matching streamed cooked visual after its authoring proxy scene is ready,
  plus immediate hiding for deletion tombstones, so local and saved source changes override the
  stale runtime snapshot without mutating or recooking it;
- mesh-triangle picking against the actually streamed visible meshes, resolved through their ECS
  ancestry to stable object roots, with projected source-asset bounds retained as the bounded
  fallback for unloaded or proxy-only records;
- Bevy's stock constant-screen-size world-space transform gizmo with move, yaw, and uniform-scale
  modes (`1`/`2`/`3`); an axis drag previews one shared delta from the active item across editable
  same-world companions and commits one atomic history command when the gesture ends; translation
  preserves companion offsets across cell boundaries, while yaw and scale apply in place around
  each object's own origin; its mesh
  layer renders through the clearing world camera instead of Bevy's non-clearing overlay camera,
  preventing retained control frames under reactive editor pacing while preserving the stock
  appearance, highlighting, and future rotate/scale modes;
- an object-transform overlay whose normalized cell/local coordinates and selected authoring visual
  remain separate from the unchanged cooked runtime data;
- a global command-sized transform/deletion history, capped at 128 undo entries, that operates by
  stable ID across selection changes and save checkpoints, with keyboard and compact-toolbar
  undo/redo;
- `Delete`/`Backspace` and inspector deletion as reversible tombstones; undoing a saved deletion
  recreates the source placement at a newer revision;
- a bounded 512-definition object palette exposed through the optional Assets window; placing its
  active definition at the logical viewpoint creates a UUID-backed, undoable placement command;
- `Cmd+S` and compact-toolbar save-all-dirty behavior through one coordinator that drains typed,
  bounded region/object/dense transactions; Save & Publish continues automatically into validated
  runtime publication only after every source batch commits;
- a bounded `ProjectWriter` object transaction that atomically creates, transforms, or deletes
  unique placements, compares expected source revisions, increments revisions, rolls back the whole
  transaction on conflict, and preserves local work on conflict or error; save-all chains bounded
  batches of at most 256 placements rather than constructing an unbounded transaction.
- an explicit tool registry: the Objects tool declares its source domains, spatial query and
  pinning policy, command kinds, overlays, invalidated derived products, and loading/conflict/
  cancellation/failure behavior; registered Terrain and Ground Cover tools use the same contract,
  and switching tools changes the bounded source domains queried by the project worker;
- bounded terrain-weight and ground-cover-mask spatial reads, separate clean/current dense working
  records, dirty/saving pinning, and an atomic mixed-domain writer capped at 64 unique records;
  every terrain/mask write compares an expected source revision and any conflict rolls back the
  complete batch;
- a first Ground Cover tool vertical slice: the World tree selects a typed layer/region, the
  Inspector and viewport toolbar expose Paint/Erase and a bounded radius, flat source terrain is
  ray-projected without runtime entity ownership, and one interpolated drag produces compact
  before/after mask rectangles in the same chronological undo/redo stack as object commands;
- stable-ID ground-cover region creation and conservative empty-region deletion in that same
  history; working regions overlay bounded project queries immediately, and a database-wide mask
  check inside the region write transaction prevents unseen coverage from being cascade-deleted;
- command history bounded by both entries and retained bytes, with checkpoint accounting; explicit
  conflict choices can either retain local object/dense intent on the latest compatible database
  revision or accept the database version; incompatible dense shape changes are never byte-rebased;
- an asynchronous atomic schema-4 sidecar journal that records dirty object presentation,
  reusable ground-cover preset/visual definitions, stable-ID region catalog snapshots, and dense
  `base`/`current`/`runtime` records, validates project identity, removes itself when every domain is
  clean, restores work without its original query window, and remains backward-compatible with
  schema-1 object-only, schema-2 object/dense, and schema-3 region journals;
- a bounded derived-work coordinator plus a four-slot background executor that scopes products by
  cell or bounded region, coalesces newer input revisions, cancels obsolete work, rejects late
  results, and publishes typed object, terrain, ground-cover, collision, navigation, and overview
  artifacts only after revision acceptance; ground-cover jobs call the same pure cell compiler as
  runtime cooking and retain the exact runtime page, species dependencies, bounds, and diagnostics
  rather than a source-mask summary;
- bounded disposable ground-cover presentation: accepted source-derived pages replace only their
  matching immutable cooked page, empty results allocate no GPU asset, origin rebases update the
  derived asset without recompilation, and only the current 7×7 runtime-index window is presented;
  dense records keep `base`, `current`, and `runtime` separately so saving does not make the stale
  cooked generation appear current;
- explicit background runtime publication through the shared whole-project cooker: only clean,
  conflict-free source can start a cook; the complete staging SQLite database is validated and
  atomically published, then the streamer reopens and verifies the exact generation ID before
  resident pages and clean object/dense runtime baselines are adopted;
- searchable cursor-paginated project Navigator and object-definition Assets queries on a dedicated
  bounded worker, including bounded back-cursor history and stale-result rejection;
- a hysteretic Overview mode with bounded 5×5 coarse-tile demand; each tile runs one bounded 16×16
  project-database query and produces concrete coarse heights, per-cell ground-cover density,
  object icons, and cell revision/status data; those proxies render independently from detailed
  runtime residency and support direct tile-to-local-view jumps;
- typed Authoring, Collision, Navigation, and Gameplay preview modes with declared domain and
  simulation policies; Collision renders bounded cell/object bounds, Navigation renders bounded
  cell connectivity, and Gameplay owns a transient fixed-step simulation host and a full-rate pacing
  request; all non-authoring roots are isolated and cleaned on mode or workspace exit;
- a real Animation workspace browser backed by the checked-in character presentation catalog; it
  renders the selected model through the runtime presentation resolver and plays the real Idle,
  Walk, and Jog GLTF clips with Play/Pause/Restart/speed transport while preserving explicit future
  crouch, additional-bank, and equipment-combat-set slots.

Not implemented yet:

- terrain brush gestures and ground-cover explicit strength, Fill, and Smooth modes (the ground-cover
  brush already has deterministic area sampling and adjustable hardness/falloff);
- catalog mutation tools for terrain surfaces, ground-cover layers, object definitions, or
  character presentation definitions; ground-cover presets/card visuals now support edit,
  duplicate, dependency-checked deletion, and region reassignment;
- shared-pivot rotation/scale and multi-object transform-field editing (the current gizmo applies
  yaw and scale deltas around each selected object's own origin, while inspector fields edit only
  the active item);
- production collision/navigation data generation, physics, AI, or the complete game-system stack
  inside Gameplay preview (the current hosts establish isolated rendering and fixed-step lifecycle);
- crouch/combat/equipment animation authoring and arbitrary clip banks beyond the currently cataloged
  locomotion set.

## Planned increments

1. Continue Ground Cover Phase 3 from the implemented procedural card generator/runtime atlas to
   direct R8 canvas painting, image import, and generated-to-painted conversion; then add terrain
   brushes on the same bounded patch seam. Details are in
   [`GROUND_COVER_AUTHORING_PLAN.md`](GROUND_COVER_AUTHORING_PLAN.md).
2. Add explicit shared-pivot modes and batch transform-field operations when their tool semantics
   are defined, retaining the existing command and revision-checked transaction boundary.
3. Replace source-summary Collision and Navigation previews with production derived data, then host
   the intended gameplay system subset inside the existing isolated fixed-step session.
4. Extend Animation with crouch/additional/combat banks as those catalog records are introduced,
   without adding gender- or weapon-specific branches to the workspace shell.

Each increment must be tested with a synthetic project substantially larger and denser than the
demo. Project size should affect database storage and query latency, not steady-state viewport entity
or memory counts.
