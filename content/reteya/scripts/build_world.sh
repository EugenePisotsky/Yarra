#!/bin/zsh
# Builds a fresh Reteya world from the preserved v13 inputs (assets/local/worlds/reteya-v13) and
# the tracked route: import the 1 m terrain and its 16 ground masks, the 13 vegetation packs, the
# roads, plant the forest plan, cook. Usage: build_world.sh [OUT_DIR] (default tmp/reteya).
# The terrain texture pack must be compiled (tools/import_terrain_surfaces.py,
# compile_terrain_textures.py, prepare_terrain_bake.py) and target/release/yarra-world-cook built.
setopt pipefail
cd "${0:A:h}/../../.."
IN=assets/local/worlds/reteya-v13
W=${1:-tmp/reteya}
P=$W/world.project.sqlite
R=$W/world.runtime.sqlite
COOK=target/release/yarra-world-cook
if [[ -e $P ]]; then echo "$P exists; choose an empty folder" >&2; exit 1; fi
mkdir -p $W
packs=(yarra_bay/bay yarra_longleaf/longleaf yarra_spruces/spruces yarra_birches/birches
  yarra_oaks/oaks yarra_maples/maples yarra_tall_forest/tall_forest yarra_dead_trees/dead_trees
  yarra_shrubs/shrubs yarra_ferns/ferns yarra_nettles/nettles yarra_sorrel/sorrel yarra_lilies/lilies)
assets() { for pack in $packs; do $COOK import-assets assets/packs/$pack.catalog.ron $P || return 1; done }
FOREST=(uv run --with numpy --with scipy --with pillow python tools/forest_plan.py $IN/planning/heightfield.json $W/forest
  --roads content/reteya/roads.ron --keep-open tributary --keep-open wetland --keep-open bedrock
  --keep-open riverbed --keep-open mud --keep-open wet_bank --keep-open snow)
$COOK import-heightfield $IN/heightfield.json $P $R &&
  assets &&
  $COOK import-roads content/reteya/roads.ron $P $R &&
  $FOREST --apply --project $P | tail -2 &&
  $COOK cook $P $R &&
  mkdir -p $W/world.project.views && cp content/reteya/views/*.ron $W/world.project.views/ &&
  echo BUILD_DONE || echo BUILD_FAILED
