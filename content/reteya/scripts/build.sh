#!/bin/zsh
# Builds tmp/reteya-v13: the v12 world (cloned) with the v13 1 m detail patches (every mountain of
# both islands) from
# assets/local/terrain/reteya-detail-v13 merged into masks_1m/: forest plan and floors,
# re-import (only changed cells), plant, cook. The retuned terrain textures must be compiled
# first (import_terrain_surfaces / compile_terrain_textures).
setopt pipefail
cd /Users/eugenepisotsky/Dev/Sources/Hobby/YarraProject
W=tmp/reteya-v13
P=$W/world.project.sqlite
R=$W/world.runtime.sqlite
COOK=target/release/yarra-world-cook
FOREST=(uv run --with numpy --with scipy --with pillow python tools/forest_plan.py $W/forest_input/heightfield.json $W/forest
  --roads $W/roads.ron --keep-open tributary --keep-open wetland --keep-open bedrock --keep-open riverbed
  --keep-open mud --keep-open wet_bank --keep-open snow)
uv run --with numpy python $W/scripts/merge_patches.py tmp/reteya-v09/masks_1m/heightfield.json \
    assets/local/terrain/reteya-detail-v13/exports/detail/patches.json $W/masks_1m &&
  $FOREST | tail -2 &&
  uv run --with numpy --with scipy python tools/island_masks.py tmp/reteya-v08/game_1m/heightfield.json $W/forest_floors \
    --forest $W/forest/forest.json | tail -1 &&
  python3 - <<'PY' &&
import json
from pathlib import Path
W = Path('tmp/reteya-v13')
m = json.loads((W / 'masks_1m/heightfield.json').read_text())
floors = ('pine_forest', 'spruce_forest', 'broadleaf_forest'); ff = (W / 'forest_floors').resolve()
m['masks'] = [{'name': e['name'], 'file': str(ff / f"{e['name']}.u8") if e['name'] in floors else e['file']} for e in m['masks']]
(W / 'import').mkdir(exist_ok=True)
(W / 'import/heightfield.json').write_text(json.dumps(m, indent=2) + '\n')
PY
  $COOK import-heightfield $W/import/heightfield.json $P $R &&
  $FOREST --apply --project $P | tail -1 &&
  $COOK cook $P $R &&
  echo BUILD_DONE || echo BUILD_FAILED
