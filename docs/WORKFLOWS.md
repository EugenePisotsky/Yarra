# Workflows

Commands run from the repository root. [Architecture](ARCHITECTURE.md) describes ownership; [performance](PERFORMANCE.md) describes controlled measurement.

## Prepare, edit and publish

Restore local assets according to [pack contracts](../assets/README.md). Licensed sources and generated databases stay ignored. After restoring terrain inputs:

```sh
python3 tools/compile_terrain_textures.py
python3 tools/prepare_terrain_bake.py
cargo run -p yarra-world-cook -- init
cargo run -p yarra-app-editor
cargo run --release -p yarra-app-game
```

`init` creates source only if absent, then cooks it. `cook` requires existing source and never creates a demo. The editor defaults to `content/world.project.sqlite`; the game reads `assets/generated/world.runtime.sqlite`. A fresh public clone needs the local pack inputs before ordinary material cooking succeeds.

**Save** writes source edits. **Save & Publish** also cooks and atomically replaces the runtime. To publish from a terminal:

```sh
cargo run -p yarra-world-cook -- cook
```

Explicit paths: `init PROJECT_DB RUNTIME_DB`, `cook PROJECT_DB RUNTIME_DB`, editor `--project-db PROJECT_DB --world-db RUNTIME_DB`, game `--world-db RUNTIME_DB`. The editor validates the source/runtime pair before opening its window. Recook outdated runtime data; source incompatibility requires an explicit replacement decision, not an automatic reset.

## Game and F1

- Click/tap ground for a destination; WASD/left gamepad stick gives camera-relative movement.
- Right-drag, horizontal trackpad/two-finger gestures or right stick orbit. Wheel/pinch/vertical gestures zoom.
- Tab cycles demo world spaces only with `--debug-world-switch`; ordinary launches have no world-switch shortcut.
- F1 or **Performance** opens the panel; Escape closes it or cancels capture. `--performance-open` starts open. `--diagnostics panel` keeps F1 with frame/app timing only; `--diagnostics off` omits F1 and timing instrumentation. Full diagnostics remain the default.
- The persistent **FPS limit** button cycles Follow display → 30 → 60 → 120. `--fps N` accepts 0 or 15–240; Reset restores the launch cap, including custom values. VSync remains enabled.
- Quality controls resolution (100/75/50/33%), upscaler, MSAA, density and terrain/object detail. Auto/Spatial/Linear are normal choices; Temporal remains an explicit prototype.
- Weather follows a random sequence by default. `--weather clear|scattered|overcast|rain|storm` starts in a held preset; `--weather authored` shows the published profile unchanged. **F1 → Weather** forces presets (60 s, 10 s or instant blends), toggles the automatic sequence, starts the next change and accelerates the weather clock (1×/10×/60×/stopped). Precipitation and wetness are computed but rain is not rendered yet.

On macOS 14+, capped modes coordinate display callbacks and the Metal minimum presentation interval. Other platforms/older macOS use the timer fallback. This changes app pacing, not the display's system setting. The normal launch follows the display and uses 50% resolution with 4× MSAA.

F1 Advanced owns terrain macro variation, terrain page gizmos and canopy reload. Reset and A/B restore include those settings and the actual canopy values. B/G/H/U/V and the old X/P/O/L/K/I renderer handlers were removed. Normal grass always uses the published catalog; separate legacy overlays and banding controls are gone.

## World authoring

World and Inspector are the default windows. Tools opens optional Assets, Navigator and Diagnostics. Switching workspaces retains source drafts/history; opening a window does not automatically activate its authoring tool.

Navigation: right/middle drag orbits, Shift+right drag pans, wheel/pinch zooms, and right mouse + WASD/QE flies. Navigator bookmarks can be passed to either app as `--start-view FILE`; the file stores a logical view rather than a screenshot.

**Objects:** select visible objects or stable IDs in the bounded asset tree. Shift/Cmd-click builds a multi-selection. 1/2/3 selects move/yaw/uniform-scale gizmos. Delete/Backspace is undoable. Cmd+Z / Cmd+Shift+Z undo/redo; Cmd+S saves. The inspector affects the active object; gizmo operations can apply to selected companions. Source changes are represented by temporary proxies until publication.

**Environment:** choose World → Environment, select a layer, drag to paint, Shift-drag to erase. One stroke is one undo step. Inspector supports Nearby/All, layer creation/order, coverage and per-use settings. **Edit shared preset…** opens Presets with an isolated ground/grass/object preview. **Apply preset changes** and **Apply settings** are distinct undo steps.

**Collections:** Presets → Environment → New → Asset collection. Select registered tree/bush/rock assets, weights, scales, spacing, slope and road clearance. Paint a collection directly or include it in a composition. Layer density/seed control deterministic generated placements. Generated objects are not individually editable; use manual placement when independent identity/editing is needed.

**Forest assets:** Follow [asset setup](../assets/README.md) to export the Forest Tree Starter Kit, then run `cargo run -p yarra-world-cook -- import-assets assets/packs/forest_tree_starter_kit/summer.catalog.ron [PROJECT_DB]` (omit the optional project argument for the current world). This registers 11 summer trees and 8 shrubs, each with four authored mesh LODs. Restart the editor to refresh the palette; search “Forest” for manual placement or select the assets in a collection. Registration preserves existing world placements and does not publish automatically. Save & Publish after authoring. The normal renderer retains object pages within 192 m in all directions, subject to residency budgets, and uses their mesh LODs. Off-screen trees remain available to cast shadows after camera turns; Bevy still culls individual draws. Visible pages load before off-screen ones; there are no usable billboards yet. Legacy terrain diagnostics retain their old local object window.

Imported forest foliage now animates in both game and editor. It follows the shared wind enable/direction/strength and preview transport, with rigid bark and authored leaf weights. The existing game Wind control affects trees and grass together. `--upscaler metalfx-temporal` uses deformation-aware tree motion vectors automatically. To check the wind implementation, run `cargo test -p yarra-engine tree_wind`; with native GPU access, also run `cargo test -p yarra-engine tree_wind::gpu_tests -- --ignored`. The current local exports already include wind metadata; no world recook is needed for the shader/material update.

For tree LOD dropout regression, run `cargo test -p yarra-engine imported_tree_lod_materials_remain_drawable -- --ignored --nocapture` with native GPU access and the local forest pack. It switches an imported tree through all four LODs without Temporal AA and checks that bark and foliage both have prepared GPU materials and compiled color/depth/motion pipelines on every transition frame. Material conversion must finish before Bevy's asset events; wind bounds expand separately before culling. `--metalfx-timing-log` also logs every camera/history/target reset as `TEMPORAL_HISTORY_RESET`, so remaining full-view flicker can be distinguished from local LOD draw gaps.

For grass streaming/history regression, run `cargo test -p yarra-vegetation-render --features upscaling/metalfx temporal_offscreen_grass_streaming_preserves_history -- --ignored --nocapture` with native MetalFX access. It repacks offscreen pages while stationary, strafing and animating wind, exercises empty/resident and disabled/enabled transitions, and checks history plus depth-derived motion in prepared/procedural paths. Page residency must not reset whole-view history; catalog edits and camera cuts still do. `--metalfx-timing-log` includes `GRASS_TEMPORAL_HISTORY` events with source/catalog/visibility/discontinuity reasons alongside native reset/timing logs.

**Roads:** activate Roads, choose a style, then New cart road and place two points. Edit control points, tangents and widths; extend/split with the tool controls. Road styles define wheel/center/shoulder wear, retained grass, ground mixtures and rut/relief variation. Explicit junctions connect compatible 2–4-arm endpoints. Save checkpoints roads and painter changes together; publication derives ground, grass and relief from the same source.

**Atmosphere:** open World → Atmosphere for the active space's profile and preview time/weather. Apply authored profile edits through normal undo/save/publication. Preview transport and temporary quality controls do not rewrite startup time/weather merely by being adjusted. Clouds Off removes rendering/shadows, not the weather's ambient response.

Save conflicts indicate newer source revisions: resolve/reload the draft rather than forcing a stale overwrite. Recovery data under `.editor` is separate from saved source. Generated preview failures must remain visible as stale/error state; they are not publication success.

## Vegetation and animation studies

The Vegetation workspace renders isolated specimens/fields with camera, wind/time, catalog, palette, ground, density/LOD and reference-image controls. Shape and Colors edit the draft; **Save study** stores local reproduction settings, while **Save & Publish** applies authored catalog changes to the world. Linked image zoom is not camera movement. Use a full field and multiple views/motion for appearance acceptance, not only an attractive close specimen.

```sh
python3 tools/vegetation_study.py open --load content/vegetation/distance-01.ron
python3 tools/vegetation_study.py capture --camera overhead --time 0
python3 tools/vegetation_study.py capture --camera low --character --time 2.5
```

The helper builds the debug editor unless `--no-build` is supplied. `--output DIR` selects a fresh capture directory. Captures contain `viewport.png`, `editor.png`, `study.ron` and diagnostics. Replay loads an unsaved draft; it does not silently save a catalog. Matching pixels require the same renderer/assets/device. Reference originals live under `.editor/vegetation/references`; capture/study data is local. Study versions 1 and 2 remain readable.

Canopy controls isolate combined/ground/blade treatment. **Save canopy look** writes `content/vegetation/canopy-look.ron`; the game loads it at startup or **F1 → Advanced → Reload canopy look**. This is an artistic approximation, not a shadow solution. Shader-level banding studies remain experimental; normal-game controls were removed. The Animation workspace independently previews catalog models/clips and transport without changing gameplay actor authority.

## Explicit fixtures and catalog tools

Use fresh paths for disposable worlds:

```sh
cargo run -p yarra-world-cook -- create-road-demo tmp/roads.project.sqlite
cargo run -p yarra-world-cook -- cook tmp/roads.project.sqlite tmp/roads.runtime.sqlite
cargo run -p yarra-app-editor -- --project-db tmp/roads.project.sqlite --world-db tmp/roads.runtime.sqlite
python3 tools/hill_landscape.py prepare
python3 tools/hill_landscape.py game --view summit
python3 tools/hill_landscape.py editor --view valley
python3 tools/hill_landscape.py descent
```

`create-demo` provides the historical 32 m grass fixture; `create-mountain-fixture` and `create-hill-fixture` provide terrain fixtures. The hill helper uses `tmp/hill-landscape`, preserves existing source, and supports `--recook`. It is separate from normal game content. Pure compiler fixtures remain available as `layered_meadow` and `cart_track` examples in `yarra-environment-compile`.

`export-vegetation PROJECT_DB CATALOG_RON` writes a new catalog file. `import-vegetation CATALOG_RON [PROJECT_DB RUNTIME_DB]` intentionally replaces the catalog and publishes; it is not a read-only preview. The old `demo` and `sync-demo-vegetation` reset commands no longer exist.

## Validation and platforms

```sh
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets
cargo test --offline --workspace
python3 -m unittest discover -s tools -p test_grass_profile.py
```

`--streaming-smoke` explicitly installs `StreamingSmokePlugin`; ordinary game/editor composition contains no smoke state or exit system. It runs the demo-world traversal, cooling/ownership and second-world gameplay checks, then exits. Use the cooked overworld/interior fixture (zero initial gameplay objects, one interior object), not arbitrary authored worlds. It accepts both hierarchy and `--terrain-legacy`; traversal checks canonical destination cells and current source demand after rebasing. The existing 3/7.5/11-second checkpoints are readiness assertions, not performance measurements. It cannot share control/exit ownership with a profile, repro or capture. The current 8 m authoring world can exceed the fixed startup checkpoint; run against a separately cooked historical demo. With unused database paths:

```sh
cargo run -p yarra-world-cook -- create-demo tmp/smoke.project.sqlite
cargo run -p yarra-world-cook -- cook tmp/smoke.project.sqlite tmp/smoke.runtime.sqlite
cargo run -p yarra-app-game -- --world-db tmp/smoke.runtime.sqlite --streaming-smoke --diagnostics off
```

Add `--terrain-legacy` to exercise the older renderer with the same fixture.

Native GPU/large-fixture tests are ignored by default and run explicitly for relevant changes. For example:

```sh
cargo test --offline -p yarra-engine mountain_cover_uploads_draws_moves_and_rebases -- --ignored --nocapture
cargo test --offline -p yarra-engine msaa_store -- --include-ignored --nocapture
```

Use [iOS setup](../ios/README.md) for device build/packaging and [performance](PERFORMANCE.md) for capture conditions. Passing CPU tests does not establish visual parity, smooth pacing or sustained power/thermal acceptance.
