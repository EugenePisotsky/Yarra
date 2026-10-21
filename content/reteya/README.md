# Reteya, the world

The project's one world: the user's chosen version of Reteya (October 19, 2026). The game and the
editor open it by default, and tools, tests on real terrain and future adjustments use it and its
Houdini source. The island's shape and the route across the first island are what must survive engine
rewrites. Everything else — formats, cooks, renderers — may change; rebuild the world from these
inputs.

## What it is

- 10 km domain, 1 m heights, two islands. Starter island (SE): ridge to 466 m, gorge with the planned
  52 m bridge, the pass basin (245 m) and the journey from the landing (2910, 3150) to the port.
  Mainland: the central (556 m), north (425 m) and west (447 m) mountains, rivers and wetlands.
- Mountains carry 1 m Houdini detail with Canterbury-style ground: snow by the sun's aspect up to the
  crests, scree streaks and fans, rock outcrops, tussock.
- 16 ground masks bound to the Baltic terrain pack (`assets/packs/terrain/reteya-ground-layers.toml`).

## Where it lives

| What | Where | Tracked |
| --- | --- | --- |
| Route (roads) | `content/reteya/roads.ron`, made from the design guides by `scripts/guides_to_roads.py` | Git |
| Start views | `content/reteya/views/` (`start`, `journey`, `*-far` per mountain, `summit-*`, …) | Git |
| Build and helper scripts | `content/reteya/scripts/` | Git |
| Final terrain inputs: 1 m heights, 16 masks, forest floors | `assets/local/worlds/reteya-v13/heightfield.json` + `terrain/` | Local only |
| Forest plan and its extra input masks | `assets/local/worlds/reteya-v13/forest.json`, `planning/` | Local only |
| Houdini source, the only one | `assets/local/terrain/reteya-detail-v13/reteya_detail_v13.hiplc`: `/obj/ISLAND` (v09 = v08 geometry + ground stage) and `/obj/DETAIL` (1 m mountain patches); its `scripts/` and `data/` beside it, `design.json` with the route guides | Local only |
| The default world it builds | `content/world.project.sqlite` + `content/world.project.views/`, `assets/generated/world.runtime.sqlite` | Local only |

The local files are not in Git (licensed textures and large binaries live under `assets/local/`).
Back that folder up.

## Rebuild the world

```sh
cargo build --release -p yarra-world-cook
content/reteya/scripts/build_world.sh tmp/reteya      # about 4 minutes
content/reteya/scripts/install_world.sh tmp/reteya --replace
cargo run --release -p yarra-app-game
```

`install_world.sh` moves the build into the default paths; without `--replace` it will not overwrite
an existing default world. Edits made in the editor live only in the installed project, so a rebuild
from the inputs drops them.

`build_world.sh` imports the terrain into a new project, the 13 vegetation packs and the roads
without cooking, plants the forest (`tools/forest_plan.py`, keeping channels, wet ground and rock
open, with snow-laden spruce alone on snow) and cooks once. It stops at the first failure; a finished build leaves `BUILD_DONE`,
which `install_world.sh` requires. With `--replace`, the replaced world moves to
`tmp/world-backups/`. Two builds give the same generation; the October 20 build (project schema 33,
runtime 34) took about 4 minutes.

## How the inputs were made

1. Houdini: `/obj/ISLAND/OUT_LAYERS` exported at 4.88 m (`tools/houdini_export_heightfield.py`) and
   resampled to 1 m (`scripts/export_game.py`); the ground stage's masks resampled to 1 m the same
   way. The island was first designed in `~/Dev/Assets/reteya_island.hiplc`; its geometry has lived
   in the v13 file since v09, so that design project is history, not an input.
2. `/obj/DETAIL` rebuilds the mountain patches at 1 m (`scripts/rebuild_detail.py` in the Houdini
   folder) into `exports/detail/patches.json`; `scripts/merge_patches.py` lays them over the 1 m
   export.
3. `tools/forest_plan.py` plans the forest from the merged terrain; `tools/island_masks.py` derives the
   forest floors from the plan.
4. `scripts/guides_to_roads.py` turns the design's route guides into `roads.ron`
   (`guides_to_roads.py assets/local/terrain/reteya-detail-v13/design.json content/reteya/roads.ron
   OUT`; `import-roads` joins a road only at its start; junction arms must diverge by 30° or more).
