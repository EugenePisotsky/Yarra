# Workflows

How to set up, build, run, check and measure Yarra. Commands run from the repository root on an
Apple silicon Mac. [Architecture](ARCHITECTURE.md) says which crate owns what;
[decisions](DECISIONS.md) records what was tried and why it was kept or dropped.

- Build the apps and tools with `--release`; only the [checks](#checks-before-committing) use the
  debug profile, and `target/debug` grows to tens of GB.
- Check free space (`df -h .`) before full builds, test runs and traces.
- `tmp/` is ignored scratch space for trial worlds, captures, logs and traces.

## Setup

### Tools

| Tool | For |
| --- | --- |
| Rust 1.99.0, clippy, rustfmt (pinned in `rust-toolchain.toml`) | everything |
| [uv](https://docs.astral.sh/uv/) | Python tools; `uv run --with …` fetches NumPy, SciPy, Pillow |
| `sqlite3` | `install_world.sh`, database copies |
| Khronos KTX 4.4.2 (`ktx`) | terrain textures, vegetation imports |
| Xcode | Metal, `xctrace`, iOS |
| Houdini (`hython`, in `/Applications/Houdini/Current/Frameworks/Houdini.framework/Versions/Current/Resources/bin`), Blender, `luau-analyze` | terrain and tree authoring, detail props, script type checks |

```sh
rustup toolchain install 1.99.0 --profile minimal --component clippy,rustfmt
```

The scripts find `ktx` on `PATH`, at `../yarra/.tools/ktx/4.4.2/bin/ktx` beside this checkout, or
by `--ktx PATH` (`compile_terrain_textures.py` also reads `KTX_BIN`).

### Local assets

Licensed sources and everything made from them stay out of Git under `assets/local/`
([asset layout](../assets/README.md)); the manifests in `assets/packs/` say what each folder holds.
Nothing cooks until they are restored. Back `assets/local/` up.

| Under `assets/local/` | What | Restore |
| --- | --- | --- |
| `worlds/reteya-v13/`, `terrain/reteya-detail-v13/` | Reteya's inputs, its Houdini source | backup only |
| `terrain/baltic/` | terrain texture arrays | [Terrain surfaces](#terrain-surfaces) |
| `yarra_*/` (13 packs) | trees, shrubs, ground cover | [Creating and revising trees](#creating-and-revising-trees) |
| `details/*.glb` | stones, pebbles, cobbles, twigs | [Grass and ground details](#grass-and-ground-details) |
| `characters/female_main/` | player model | backup; check `shasum -a 256` against `assets/packs/characters/female_main.toml` |
| `map/` | map brushes and fonts | [download](../assets/README.md#map-symbols-and-lettering) |

### Terrain surfaces

```sh
uv run --with numpy --with pillow python tools/import_terrain_surfaces.py   # [--only KEY ...]
uv run python tools/compile_terrain_textures.py                              # [--pack baltic]
```

`assets/packs/terrain/baltic.toml` lists the surfaces in texture-array order with their Megascans
folders (`~/Dev/Assets/terrain/sources`) and parameters. The import writes
`assets/local/terrain/baltic/source/`, the compile `runtime/{universal,astc}/*.ktx2`. After
changing a surface, re-import it (`--only`), compile and rebuild the world: a world keeps the
surface parameters it was created with.

## Build and install the world

Reteya is the one world ([its README](../content/reteya/README.md)): built from the local inputs and
the tracked route, then installed as the default world the game and editor open.

```sh
cargo build --release -p yarra-world-cook
content/reteya/scripts/build_world.sh tmp/reteya           # about 6 minutes
content/reteya/scripts/install_world.sh tmp/reteya         # --replace over an existing world
target/release/yarra-world-cook cook                       # no-op: 0 changed terrain cells
```

- `build_world.sh [OUT_DIR]` needs the compiled terrain textures and the release cook binary, and
  refuses a folder that holds a project. It imports terrain, the 13 vegetation packs and the
  roads without cooking, plants the forest and cooks once. Success leaves `BUILD_DONE`; failure
  prints `BUILD_FAILED`.
- `install_world.sh BUILT_DIR [--replace]` requires `BUILD_DONE` and moves the build to the
  default files: `content/world.project.sqlite`, `content/world.project.views/` (start views),
  `assets/generated/world.runtime.sqlite`. With `--replace` the old world moves to
  `tmp/world-backups/<date-time>/`.
- Editor edits live only in the installed project: a rebuild drops them.

### Rebuild after a format change

No migrations: formats change in place and the world is rebuilt. The versions are
`PROJECT_SCHEMA_VERSION` and `RUNTIME_SCHEMA_VERSION` in `crates/world/src/lib.rs`.

| What changed | Do |
| --- | --- |
| Project schema, an importer, the route, the forest plan | build and install again with `--replace` |
| Runtime schema or cooked output only | `target/release/yarra-world-cook cook` (a runtime it cannot continue is cooked afresh) |
| Sources edited without new revisions | `target/release/yarra-world-cook cook --full` |

### Cook and imports

```sh
target/release/yarra-world-cook cook [PROJECT_DB RUNTIME_DB] [--full]
target/release/yarra-world-cook import-heightfield MANIFEST [PROJECT_DB RUNTIME_DB] [--no-cook]
target/release/yarra-world-cook import-roads content/reteya/roads.ron [PROJECT_DB RUNTIME_DB]
target/release/yarra-world-cook import-grass assets/packs/grass/baltic.ron [PROJECT_DB RUNTIME_DB]
target/release/yarra-world-cook import-assets CATALOG_RON [PROJECT_DB]
target/release/yarra-world-cook create-demo NEW_PROJECT_DB        # or create-road-demo
```

No paths means the default world; no command means `cook` (there is no `--help`). `cook`
recompiles only changed cells and never creates a project. Imports cook unless given `--no-cook`;
`import-assets` only registers a catalog whose files are under `assets/`. `create-demo` and
`create-road-demo` write the two fixtures the unit tests build in code.

### Roll back an install

`install_world.sh --replace` moves the replaced world to `tmp/world-backups/<date-time>/`. To go
back, close the game and editor and move those files back to `content/` and
`assets/generated/`. A backup opens only with a build of the same schema versions.

### Terrain from Houdini

Only to change the island. The one Houdini source is
`assets/local/terrain/reteya-detail-v13/reteya_detail_v13.hiplc`; [how the inputs were
made](../content/reteya/README.md#how-the-inputs-were-made) lists the steps from its export to
`assets/local/worlds/reteya-v13/`. The export, one `--mask` per ground mask the stage writes:

```sh
H=/Applications/Houdini/Current/Frameworks/Houdini.framework/Versions/Current/Resources/bin
$H/hython tools/houdini_export_heightfield.py \
  assets/local/terrain/reteya-detail-v13/reteya_detail_v13.hiplc tmp/export \
  --node /obj/ISLAND/OUT_LAYERS --start 2910 3150 --mask sand --mask rock --mask scree
```

Sculpt in Houdini, not the editor: a re-import replaces heights (only in changed cells) and keeps
objects, roads and painted layers.

## Editor

```sh
cargo run --release -p yarra-app-editor
cargo run --release -p yarra-app-editor -- --start-view content/world.project.views/summit.ron
cargo run --release -p yarra-app-editor -- --project-db tmp/roads.project.sqlite \
  --world-db tmp/roads.runtime.sqlite                  # a create-road-demo fixture, cooked
```

It takes only these three options, and checks that project and runtime describe the same world
before opening a window (cook if not). Right drag orbits, Shift + right drag pans, wheel or pinch
zooms, right mouse + WASD/QE flies.

**Workspaces** (top-left menu; **Tools** opens the workspace's windows):

- **World**: the World window picks a tool; Inspector shows the selection; Tools adds Assets,
  Navigator, Diagnostics, Areas and Atmosphere.
  - *Visible assets*: click, Shift- or Cmd-click to select; 1/2/3 move, yaw, scale; Delete.
  - *Environment*: pick a layer, drag to paint, Shift-drag to erase. Layers that read an imported
    mask are rewritten by each import and cannot be painted.
  - *Roads*: a style, **New cart road**, then points, tangents, widths and junctions.
  - *Areas*: named places for gameplay (`guard/gate_post`): **Draw new area**, click corners,
    Enter closes, Backspace drops a corner, Esc cancels.
  - *Atmosphere*: sky, weather presets and sequence, fog and mist; the preview is not saved.
- **Presets**: shared ground, grass and object presets and road styles in an isolated preview.
  An unapplied draft blocks saving.
- **Animation**: previews character models and clips.

**Save** (Cmd+S) writes source changes to the project. **Save & Publish** also cooks, validates
and atomically replaces the runtime, which the game reads (**Publish** when nothing is unsaved;
`target/release/yarra-world-cook cook` from a terminal). Cmd+Z / Cmd+Shift+Z undo and redo.
**Source conflict**: the project changed underneath; reload rather than force.

**Journal:** unsaved edits are journalled to `content/world.project.editor-journal.ron`. After a
crash the next launch restores them as unsaved changes (Diagnostics says what; undo history starts
empty). A journal of another project or version is refused. To discard it, close the editor and
delete the file.

## Game and F1

```sh
cargo run --release -p yarra-app-game
cargo run --release -p yarra-app-game -- --start-view content/world.project.views/journey.ron
cargo run --release -p yarra-app-game -- --help        # every option, no window
cargo run --release -p yarra-app-game -- --upscaler metalfx-spatial    # FXAA instead of Temporal
```

**The play setup** is native fullscreen, 50% render scale and MetalFX Temporal (both default),
with the day clock and weather running (default in play). The window opens at 1280×720:
fullscreen it with the green button. Measurements should match this setup.

**Controls:** WASD or left stick moves; click or tap the ground to walk there; right drag,
two-finger horizontal scroll or right stick orbits; wheel or pinch zooms.

**F1** (or **Performance**) opens the panel, Escape closes it. Always shown: **FPS limit** (Follow
display → 30 → 60 → 120), **Movement** (Normal / Fast 10×), **Reset launch settings**. Pages:
Overview, Features (effects on/off), Quality, Weather (presets, clocks, time jumps), Wind
(session-only), [Compare](#f1-compare), Advanced.

| For | Launch options |
| --- | --- |
| Where | `--start-view FILE`, `--world-db FILE` |
| Time, weather | `--time HH:MM`, `--day-clock on\|off`, `--weather auto\|authored\|clear\|scattered\|overcast\|rain\|storm` (on snow ground, overcast, rain and storm are light snow, snow and a blizzard) |
| Presentation | `--fps 0\|15..240` (default 60, 0 follows the display), `--resolution-scale 1\|0.75\|0.5\|0.33`, `--upscaler auto\|linear\|metalfx-spatial\|metalfx-temporal`, `--cloud-quality off\|balanced\|high`, `--tree-shadow-lod 0..2` |
| Panel, logs | `--diagnostics off\|panel\|full` (default full), `--performance-open`, `--timing-log` |
| Gameplay | `--story DIR` |

Start views: `start`, `journey` (has a walking route), `landing`, `*-far`, `summit-*`,
`forest-*` and more in `content/world.project.views/`. Keep a log with `2>&1 | tee tmp/game.log`.

## Gameplay content

`content/gameplay/demo` is the authored gameplay project; it runs without the game, a world or art
([architecture](ARCHITECTURE.md#gameplay-crates)). Build the tool once:
`cargo build --release -p yarra-game-content`.

### Editing and validating gameplay content

```text
project.ron                         # identity, source locale, packages, scenario (format 11)
scenario.ron, scenarios/            # starting conditions and steps
packages/guard/                     # a package is named after its directory
  package.ron                       # optional: areas it names, variables it declares
  actors.ron, items.ron, loot.ron, characters.ron, rules.ron (core only)
  gate.quest.ron, guard.profile.ron, ready.predicate.ron
  conversations/reward.dialogue.ron, world/gate.object.ron, world/escort.trigger.ron
  scripts/guard.luau                # module named after its file
  en.ftl, uk.ftl                    # the package's text per locale
```

A file says what it holds by its name (an unrecognised `.ron` is an error). Names such as
`guard/gate` are identities. Text is `Message("key")`; the source locale's Fluent file decides
which messages exist. Copy the demo to start a project.

```sh
target/release/yarra-game-content validate content/gameplay/demo
target/release/yarra-game-content scenario content/gameplay/demo scenarios/guard.ron
target/release/yarra-game-content script-api            # the Luau host API, for editors
```

- `validate` runs `scenario.ron` in a throwaway session and type-checks scripts when
  `luau-analyze` is on `PATH` or named by `YARRA_LUAU_ANALYZE`.
- Scenarios `guard`, `guard-refusal`, `guard-gate`, `guard-training` share `guard-start.ron`;
  `island.ron` is the game slice's start.
- Focused tests: `cargo test -p yarra-game-content -p yarra-game-types -p yarra-gameplay
  -p yarra-localization -p yarra-save -p yarra-scripting`.

### Standalone gameplay and saves

```sh
G=target/release/yarra-game-content; mkdir -p tmp/game-content
$G demo content/gameplay/demo --locale uk --save-dir tmp/game-content/saves-1
$G build content/gameplay/demo tmp/game-content/content-1.sqlite
$G build-language content/gameplay/demo uk tmp/game-content/uk-1.sqlite
$G validate tmp/game-content/content-1.sqlite --language tmp/game-content/uk-1.sqlite
$G inspect tmp/game-content/content-1.sqlite --item healing_potion --dialogue guard/reward
```

`demo` publishes the project, plays its scenario against the bundle and writes fresh save slots.
Bundles, packs and save folders are never overwritten (new paths each run); other formats are
refused, not migrated.

### The story slice in the game

```sh
cargo run --release -p yarra-app-game -- --story content/gameplay/demo
```

A guard, a gate and a training dummy stand a few metres ahead of the start view. Near the guard
E talks and T asks about his offer; Space/Enter continues, 1–9 replies, Q leaves; near the dummy F
strikes, G power-strikes, V stops; H drinks; Z/X/C spend points on strength/wisdom/vitality; F5/F9
quick save and load. Areas `guard/approach` and `guard/gate_post` are stand-ins until painted in
the editor. Each run publishes its own bundle under `$TMPDIR/yarra-story/` and deletes it on exit;
saves stay in `$TMPDIR/yarra-story/saves`. To check it without a keyboard, walk a route and read
the `Story:` lines:

```sh
target/release/yarra-app-game --story content/gameplay/demo \
  --start-view content/world.project.views/journey.ron --render-repro actor-walk \
  --render-walk-speed 1.5 --render-frames 1800 --render-snapshot tmp/story/walk.png \
  --render-snapshot-frames 900,1700 2>&1 | grep 'Story'
```

## Vegetation and assets

Every tree, shrub and ground-cover plant comes from the YarraVegetation kit, checked out at
`~/Dev/Assets/vegetation`. Judge forms in the game against photos.

### Creating and revising trees

Author shapes in YarraVegetation
([authoring guide](https://github.com/EugenePisotsky/YarraVegetation/blob/main/TREE_AUTHORING.md)):
build and validate the forms, refit LOD coverage after changing foliage, bake impostors:

```sh
cd ~/Dev/Assets/vegetation
hython scripts/build_shrubs.py && hython scripts/validate_shrubs.py     # per family
hython scripts/fit_lod_coverage.py shrub_rounded                        # then rebuild
uv run --with numpy --with pillow python scripts/bake_impostor.py outputs/spruce_forest/current \
  --leaf-grow 0 --fill 0 --cell 256 --source lod0.glb                 # shrubs, bay: --cell 160
cd -
```

Here, import all forms of a pack in one run (an import replaces the pack, keeping the old one as
`<dir>.previous-<time>`), register, cook. The import welds each LOD's vertices and prints the
counts before and after:

```sh
V=$HOME/Dev/Assets/vegetation/outputs
python3 tools/import_vegetation_bundle.py --bundle $V/spruce_forest/current \
  --output assets/local/yarra_spruces --catalog assets/packs/yarra_spruces/spruces.catalog.ron \
  --lod-screen-heights 480 240 --impostor-screen-height 200
target/release/yarra-world-cook import-assets assets/packs/yarra_spruces/spruces.catalog.ron
target/release/yarra-world-cook cook
```

| `<dir>`, catalog | Forms (one `--bundle $V/<form>/current` each) | Heights |
| --- | --- | --- |
| `yarra_birches`, `birches` | `birch_` + `leafy sparse bare crown pendulous double triple` | T |
| `yarra_oaks`, `oaks` / `yarra_maples`, `maples` | `oak_` / `maple_` + `forest spreading sparse` | T |
| `yarra_tall_forest`, `tall_forest` | `tall_broadleaf_forest` | T |
| `yarra_longleaf`, `longleaf` | `pine_longleaf`, and with `_half_bare _nearly_bare _one_sided _tall_bole _broad _leaning _flat_top` | T |
| `yarra_spruces`, `spruces` | `spruce_forest`, `spruce_snow` | T |
| `yarra_dead_trees`, `dead_trees` | `dead_` + `upright spreading split slender double triple` | T |
| `yarra_shrubs`, `shrubs` | `shrub_` + `rounded spreading sparse medium_rounded medium_spreading medium_upright` | S |
| `yarra_bay`, `bay` | `bay_upright` | S |
| `yarra_ferns`, `ferns` | `fern_` + `upright spreading sparse` | `140 45` |
| `yarra_nettles`, `nettles` / `yarra_lilies`, `lilies` | `nettle_` + `young mature patch` / `lily_` + `open full patch` | `100 35` |
| `yarra_sorrel`, `sorrel` | `sorrel_` + `open full patch` | `80 25` |

`--output assets/local/<dir> --catalog assets/packs/<dir>/<catalog>.catalog.ron`; always pass both
(they default to the birches). Heights: T is `--lod-screen-heights 480 240
--impostor-screen-height 200`, S is `480 180` with `60`; ground cover has `--lod-screen-heights`
only and no impostor.

- `assets/local/<dir>/import.json` records the last import's settings, `--canopy-blend` included.
- Register again even with unchanged file names (memory estimates change). Registration keeps
  placements; new forms need [planting](#forest-plan). Restart the game to see new meshes.

### LOD lab

```sh
target/release/yarra-app-game --start-view content/world.project.views/start.ron \
  --lod-lab yarra_spruces/spruce_forest --lod-lab-stand yarra_longleaf/pine_longleaf \
  --lod-lab-spacing 7
target/release/yarra-app-game --start-view content/world.project.views/start.ron \
  --lod-lab yarra_spruces/spruce_forest --lod-lab-capture tmp/lod-lab/spruce --resolution-scale 1
uv run --with numpy --with pillow python tools/lod_lab_sheet.py tmp/lod-lab/spruce
```

The form (`pack/asset`) stands at the start view's focus, drawn as the game draws it; the panel
sets distance, bearing, pitch, what draws, wind and sun. LOD distances follow the window's height,
so judge fullscreen (captures are). For snowy trees, `content/reteya/views/snow-lab.ron` stands on
open snow, nothing within 84 m, facing snowy spruces at every LOD's distance: from it the lab's
switch captures (`--lod-lab-capture DIR --lod-lab-pitch 9`) show each switch before and after, and a
look capture the bands as play sees them. For frames without the forest, run `forest_plan.py` with
`--clear --project` on a copy of the project (`sqlite3 … ".backup …"`), cook the copy to its own
runtime and pass that as `--world-db` (not tried yet).

### Forest plan

```sh
uv run --with numpy --with scipy --with pillow python tools/forest_plan.py \
  assets/local/worlds/reteya-v13/planning/heightfield.json tmp/forest \
  --roads content/reteya/roads.ron --keep-open tributary --keep-open wetland \
  --keep-open bedrock --keep-open riverbed --keep-open mud --keep-open wet_bank --snow snow
# check tmp/forest/preview.png; add --apply to plant it in the default world, then cook
```

This is the plan `build_world.sh` plants. `--apply` replaces the kit's trees and shrubs on the
landmass at the start (or `--at X Z`) in `--project` (default world) and adds `forest-*` views.
Reteya's forest-floor masks come from this plan (`tools/island_masks.py`): a different plan needs
new masks and a rebuild ([Reteya README](../content/reteya/README.md)). `--snow` plants snow
ground last with its own random stream, so the rest of the plan, and the masks, stay as they were.

### Grass and ground details

```sh
target/release/yarra-world-cook import-grass assets/packs/grass/baltic.ron
/Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup \
  -P tools/import_detail_props.py -- PACK_DIR assets/local/details/NAME.glb [--albedo L]
```

`assets/packs/grass/baltic.ron` (fields documented in its header) defines grass types, ground
details and what grows on each surface. The game reloads looks live; placement and the ground's
grass colour need `import-grass`, which cooks. Stones, pebbles, cobbles and twigs are made from
Megascans 3D packs in `~/Dev/Assets/props/sources`; `--albedo` sets the mean linear luminance
(twigs: 0.1). Ferns, nettles, lilies and sorrel use the vegetation packs' LOD1 files.

### World map

```sh
uv run tools/render_world_map.py assets/local/worlds/reteya-v13/heightfield.json \
  content/maps/reteya.map.json assets/generated/maps/reteya.png
```

About 25 s and 3 GB. `content/maps/reteya.map.json` holds the title, compass and named features;
`--no-lettering` leaves names for the game to draw; brushes and fonts are in `assets/local/map/`.

## Checks before committing

```sh
cargo fmt --all && cargo run -p yarra-tidy -- --write     # format, then add blank lines
cargo fmt --all --check
cargo run -p yarra-tidy                                   # lists missing blank lines, fails
cargo clippy --workspace --all-targets
cargo test --workspace
cargo run -p yarra-shader-check
uv run --with numpy --with pillow python -m unittest discover -s tools -p 'test_*.py'
```

Judge rendering changes in the running game against [captures](#captures-and-measurement) of the
same views before and after.

### Shaders

```sh
cargo run -p yarra-shader-check                  # every variant in crates/shader_check/entries.ron
cargo run -p yarra-shader-check -- --parse-all   # every .wesl file parses
cargo run -p yarra-shader-check -- shaders/sky/composite.wesl --each-flag \
  --defs "ATMOSPHERE,CACHED_CLOUDS,CLOUDS,DUAL_SOURCE_BLENDING,MIST_NOISE_PERIOD=2048"
```

Shaders are WESL in `assets/shaders/`; the game composes a variant only when a view first needs
it. `--each-flag` flips each `@if` flag in turn, `--print` shows the composed WGSL. Refresh
`entries.ron` from the `FRAME_SPIKE kind=pipeline` lines of a `YARRA_SPIKE_LOG` run.

### Native GPU tests

Ignored by default; run those a change touches, with GPU access:

```sh
cargo test -p yarra-engine -- --ignored --nocapture          # trees, wind, depth
cargo test -p yarra-terrain-render -- --ignored --nocapture  # clipmap material
cargo test -p yarra-atmosphere -- --ignored --nocapture      # terrain shadow search
cargo test -p yarra-upscaling --features metalfx -- --ignored --nocapture
cargo test -p yarra-world-cook two_kilometre -- --ignored --nocapture    # after cook changes
```

Some need local packs (`wind_depth_prepass_preserves_colour_coverage`,
`connected_spruce_lods_compile_colour_depth_and_motion`); a missing one is not a pass.

## Captures and measurement

Build first (`cargo build --release -p yarra-app-game`), and say what a long run answers before
starting it.

- **Metal HUD GPU milliseconds are not a cost**: macOS clocks the GPU to the load. Use work
  counts (triangles, instances, overdraw, texture MB), `--timing-log` for CPU spans, and Metal
  System Trace per pass, divided by an untouched pass in the same trace.
- Measure the play setup. Profiles, repros and captures hold the clock and use authored weather
  unless told; work keyed to the sun shows only with `--day-clock on`.
- Heat moves results 5–30% within a run: alternate A/B/A/B, and do not trace while Houdini or a
  build runs (`pgrep -fl houdini`).
- **A slowdown in every pass at once is the GPU's clock, not the frame's work.** After minutes of
  near-full load (a jog through the forest into the meadow) macOS holds the GPU in its Medium
  performance state, which runs each pass about twice as long; standing still, it returns to
  Maximum within a minute or two. Read the state from the same Metal System Trace (below) before
  chasing a drop, and compare costs at Maximum. Fixed-size passes (`atmosphere_luts`) show the
  clock; MetalFX slows less, so a ratio to it understates a drop.
- The Mac is the working machine, not the target (PS5-class hardware at 60 fps): look for
  headroom and real work, not perfect play on the Mac.

### Benchmark and frame budget

```sh
cargo build --release -p yarra-app-game
python3 tools/bench.py --note "what this measures"                 # all views, ~3 min each
python3 tools/bench.py --views meadow,forest-pine --note "grass off" -- --profile-grass off
```

`tools/frame_budget.toml` holds the frame budget (GPU busy ms per frame at the play setup with
the GPU at Maximum) as lines of passes, and the views it is measured in. Each view runs uncapped
from 10:00 in its weather; a 3 s Metal System Trace gives each line's ms, printed against its
budget (`!` over), with fps, CPU main and render preparation. Views run 90 s apart, since about
six minutes of uncapped load drop the GPU to Medium; a view whose GPU left Maximum anyway is
flagged and left out of the history. The trace's state table covers only its first half second,
so when it shows no lasting state the clock is read from MetalFX Temporal's own pass (1.06–1.09 ms
at Maximum). Passes listed as `background` (the cloud volume's sweep) are charged only the time
they run alone: overlapping the opaque pass, an even split charged the clouds up to 1.4 ms of it,
while turning them off saves 0.2. Rows append to `tools/bench_history.csv` with the commit and note: a change's cost is
the difference from the previous row of the same view, and heat moves a row by a few percent, so
judge sub-0.3 ms changes by an A/B/A of that view. Flags after `--` reach every run:
`--profile-objects off` (trees and rocks) splits the opaque line, leaving the terrain;
`--profile-terrain off` does not, since the forests the terrain hid are then drawn instead.
Traces stay in `tmp/bench/` until the next run.

### Look captures

```sh
target/release/yarra-app-game --start-view content/world.project.views/start.ron \
  --resolution-scale 1 --gpu-timing-off --look-capture tmp/look/start \
  --look-variants "tony|EV 13:tone=tony,ev=13;neutral|EV 13:tone=neutral,ev=13,ambient=1.5"
uv run --with numpy --with pillow python tools/look_sheet.py tmp/look/start
```

After one load (`--look-settle`, default 25 s) each variant draws 5 s, wind and weather still,
then a fullscreen screenshot (keep the display awake). `--gpu-timing-off` avoids lost GPU counters
between quick launches; `row|column` names lay out the sheet.
Keys: `ev`, `tone` (`tony agx neutral filmic aces boring reinhard none`), `ambient`, `sun`,
`canopy`, `auto`, `fog`, `particles`, `shafts`, `bloom`, `phase` (0.34 morning, 0.5 noon), `moon`, `tilt`,
`wind`, `wet`, `lightning`, `burst`, `haze`, `mist`, `depth`, `air`.

### Repro routes and snapshots

```sh
target/release/yarra-app-game --start-view content/world.project.views/overlook.ron \
  --render-repro landscape --render-frame-clock --render-ui-off --render-frames 900 \
  --render-snapshot tmp/repro/overlook.png --render-snapshot-frames 600,840
```

Routes: `low-walk`, `grass-close`, `grass-away`, `grass-follow`, `grass-follow-far`, `grass-zoom`,
`grass-top-down`, `grass-overhead`, `grass-stream`, `grass-soak`, `landscape`, `landscape-turn`,
`landscape-descent`, `actor-walk` (walks the view's `route`; `journey.ron` has one, along the
road). `landscape*` and `actor-walk` need `--start-view`. `--render-frame-clock` steps by frames,
so two builds see the same camera. Snapshots follow the 300-frame warm-up; `--render-frames` is at
least 900. `actor-walk` jogs as the player does (`--render-walk-speed` overrides, F1's fast walk
is about 35 m/s), with the game's follow camera turned to look along the route ahead.

To replay play as reported, keep the play settings a repro would otherwise change:
`--resolution-scale 0.5 --upscaler metalfx-temporal --day-clock on --weather auto`.

### Timed profiles

A timed profile runs the play setup unless told otherwise: fullscreen, the internal size the
render scale gives (`--profile-size game`, 1728×1084 at 0.5 on a 3456×2168 display), the day
clock running. Walking the journey:

```sh
mkdir -p tmp/prof
target/release/yarra-app-game --start-view content/world.project.views/journey.ron \
  --render-repro actor-walk --render-walk-speed 35 \
  --profile-seconds 40 --profile-warmup 10 --profile-fps 60 --profile-native-pacing \
  --upscaler metalfx-temporal --weather auto --timing-log 2>&1 | tee tmp/prof/walk.log
grep 'GRASS_PROFILE event=sample' tmp/prof/walk.log \
  | sed -E 's/.*late_updates=([0-9]+).*/\1/' | paste -sd+ - | bc       # late frames
```

Each second logs `GRASS_PROFILE event=sample` (update FPS, percentiles, `late_updates`);
`--timing-log` adds `PERFORMANCE_TIMING` (`main_work_ms`, `render_prep_ms`, GPU spans). For
GPU-bound comparisons drop the route and use `--profile-fps 0` from a fixed view, checking
`update_fps`. Fullscreen may pace to the display: give `--profile-surface WIDTHxHEIGHT` to run
windowed at a fixed size.

### Frame spikes and work counts

```sh
YARRA_SPIKE_LOG=8 target/release/yarra-app-game --upscaler metalfx-temporal 2>&1 \
  | tee tmp/spikes.log | grep FRAME_SPIKE
RUST_LOG=info,wgpu=error,naga=warn,yarra_engine::trees::gpu=debug \
  target/release/yarra-app-game 2>&1 | grep TREE_GPU_COUNTS
```

`FRAME_SPIKE kind=interval` is an update over 5% late for the cap; other kinds name the slowest
systems of frames over the threshold (ms) and each pipeline compile. Even gaps between drops mean
something periodic. `TREE_GPU_COUNTS` gives meshes, triangles and impostors in view and per
shadow cascade.

### Metal System Trace

```sh
target/release/yarra-app-game --start-view content/world.project.views/forest-pine.ron \
  --upscaler metalfx-temporal &
sleep 25                                          # load, warm up, fullscreen the window
mkdir -p tmp/trace
xcrun xctrace record --template 'Metal System Trace' --attach yarra-app-game --time-limit 3s \
  --output tmp/trace/a.trace
xcrun xctrace export --input tmp/trace/a.trace --output tmp/trace/a-intervals.xml \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]'
xcrun xctrace export --input tmp/trace/a.trace --output tmp/trace/a-states.xml \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="gpu-performance-state-intervals"]'
rm -rf tmp/trace/a.trace; rm -f $TMPDIR/instruments*.ktrace; pkill -x yarra-app-game
```

`python3 tools/summarize_gpu_intervals.py a-intervals.xml [b-intervals.xml]` gives the GPU's busy
time per frame and each pass's share of it, and their ratios to a reference pass in the same trace
(`--reference`), which is what to compare between traces. `--states a-states.xml [b-states.xml]`
adds the share of each trace's time the GPU spent in each performance state: compare traces only
at the same state. The GPU runs passes side by side, so a
pass's own interval is not its cost: a few rows of clouds traced beside the opaque pass last as
long as the opaque pass. The tool splits each moment among the passes running then; a pass's
true cost is clearest as the busy time it adds, between builds with and without it. Keep recordings to 3–11 s; `export --toc` lists tables,
`--instrument 'Metal GPU Counters'` adds limiters, occupancy and the shader compiler's spills: export
the `graphics-compiler-spill-events` and `metal-application-encoders-list` tables and pass them to
`--spills`. A pass that spills, or sits near 13% fragment occupancy, is short of registers. Each recording leaves about 1 GB of `instruments*.ktrace` in `$TMPDIR`: delete them.
`tools/summarize_gpu_counters.py reference.csv candidate.csv --output result.json` compares
Xcode's exported encoder counters (replay work, not live time).

### A/B against a baseline build

The game reads shaders, packs and catalogs from `./assets` first, so a baseline binary needs its
own assets root, and its own runtime copy when the format changed:

```sh
R=$PWD; mkdir -p tmp/ab/base                  # before the change (or after git stash)
cargo build --release -p yarra-app-game && cp target/release/yarra-app-game tmp/ab/base/
git archive HEAD assets | tar -x -C tmp/ab/base     # tracked shaders, packs, catalogs
ln -s $R/assets/local tmp/ab/base/assets/local      # ignored local files, shared
cp -c assets/generated/world.runtime.sqlite tmp/ab/base/
cd tmp/ab/base && ./yarra-app-game --world-db $PWD/world.runtime.sqlite \
  --start-view $R/content/world.project.views/forest-pine.ron --upscaler metalfx-temporal; cd $R
```

Run the candidate from the repository root with the same options, alternating with the base.

### F1 Compare

Fix view, time, weather, render size and FPS cap. **Capture A** (settles ≥ 2 s, samples 10 s),
change one setting, **Capture B**, **Export** to `tmp/performance/comparison-<timestamp>/`
(`YARRA_PERFORMANCE_DIR` moves it). Its GPU markers are diagnostics; ten seconds says nothing
about heat.

### Metal capture

```sh
MTL_CAPTURE_ENABLED=1 target/release/yarra-app-game --render-repro low-walk \
  --metal-capture $PWD/tmp/captures/low-walk.gputrace
```

One frame (from frame 600) to a new absolute `.gputrace` path; the game then exits.

## iOS

Device builds, packaging and phone captures: [ios/README.md](../ios/README.md) (from a built
default world, plus `rustup target add aarch64-apple-ios`).

## Launch options

Every game option, generated from `crates/app_game/src/launch/flags.rs` (a test keeps this table
in step; `--help` prints the same with descriptions):

| Purpose | Controls |
| --- | --- |
| Scene and presentation | `--world-db FILE`, `--start-view FILE`, `--story DIR`, `--fps 0\|15..240`, `--resolution-scale 1\|0.75\|0.5\|0.33`, `--time HH:MM`, `--day-clock on\|off`, `--weather auto\|authored\|clear\|scattered\|overcast\|rain\|storm`, `--upscaler auto\|linear\|metalfx-spatial\|metalfx-temporal`, `--cloud-quality off\|balanced\|high`, `--tree-shadow-lod 0..2` |
| Panel and logging | `--diagnostics off\|panel\|full`, `--performance-open`, `--render-audit`, `--render-console`, `--timing-log`, `--gpu-timing-detail`, `--gpu-timing-off`, `--metalfx-timing-log`, `--grass-counters` |
| Scripted routes | `--render-repro NAME`, `--render-frames N`, `--render-snapshot PATH`, `--render-snapshot-frames N,N`, `--render-walk-speed MPS`, `--render-frame-clock`, `--render-temporal-view motion\|depth`, `--render-ui-off` |
| Captures and checks | `--metal-capture PATH.gputrace` |
| Timed profile | `--profile-seconds 2..3600`, `--profile-warmup 1..600`, `--profile-size game\|WIDTHxHEIGHT`, `--profile-surface WIDTHxHEIGHT`, `--profile-window fullscreen\|windowed`, `--profile-fps 0\|15..240`, `--profile-native-pacing`, `--profile-grass full\|off`, `--profile-objects on\|off`, `--profile-terrain on\|off`, `--profile-bloom on\|off`, `--profile-auto-exposure on\|off`, `--profile-fog on\|off`, `--profile-particles on\|off`, `--profile-shafts on\|off`, `--profile-tree-touch on\|off`, `--profile-temporal-bypass`, `--profile-diagnostic` |
| Look captures | `--look-capture DIR`, `--look-variants LIST`, `--look-settle SECONDS` |
| LOD lab | `--lod-lab PACK/ASSET`, `--lod-lab-stand ASSET`, `--lod-lab-stand-count N`, `--lod-lab-spacing METRES`, `--lod-lab-capture DIR`, `--lod-lab-screenshot FILE`, `--lod-lab-yaws DEGREES,...`, `--lod-lab-scale 0.25..4`, `--lod-lab-pitch DEGREES`, `--lod-lab-distances METRES,...`, `--lod-lab-settle FRAMES` |
| Output and pacing fallbacks | `--temporal-standard-output`, `--frame-pacing-timer` |
