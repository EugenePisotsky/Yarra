# Reteya, the island to keep

The user's chosen version of Reteya (October 19, 2026): the island's shape and the route across the
first island are what must survive engine rewrites. Everything else — formats, cooks, renderers — may
change; rebuild the world from these inputs.

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
| Houdini source | `assets/local/terrain/reteya-detail-v13/reteya_detail_v13.hiplc`: `/obj/ISLAND` (v09 = v08 geometry + ground stage) and `/obj/DETAIL` (1 m mountain patches) | Local only |

The local files are not in Git (licensed textures and large binaries live under `assets/local/`).
Back that folder up.

## Rebuild a world

```sh
cargo build --release -p yarra-world-cook
content/reteya/scripts/build_world.sh tmp/reteya      # about 9 minutes
cargo run --release -p yarra-app-game -- --world-db tmp/reteya/world.runtime.sqlite \
  --start-view tmp/reteya/world.project.views/start.ron
```

`build_world.sh` imports the terrain into a new project, the 13 vegetation packs, the roads, plants
the forest (`tools/forest_plan.py`, keeping channels, wet ground, rock and snow open) and cooks. A fresh
build on October 19 matched the iterated test world (`tmp/reteya-v13`) view for view.

## How the inputs were made

1. Houdini: `/obj/ISLAND/OUT_LAYERS` exported at 4.88 m (`tools/houdini_export_heightfield.py`) and
   resampled to 1 m (`~/Dev/Assets/reteya_island/scripts/export_game.py`); the ground stage's masks
   resampled to 1 m the same way.
2. `/obj/DETAIL` rebuilds the mountain patches at 1 m (`scripts/rebuild_detail.py` in the Houdini
   folder) into `exports/detail/patches.json`; `scripts/merge_patches.py` lays them over the 1 m
   export.
3. `tools/forest_plan.py` plans the forest from the merged terrain; `tools/island_masks.py` derives the
   forest floors from the plan.
4. `scripts/guides_to_roads.py` turns the design's route guides into `roads.ron` (`import-roads` joins
   a road only at its start; junction arms must diverge by 30° or more).
