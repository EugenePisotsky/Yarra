#!/bin/zsh
# Renders reviews (content/characters/reviews/<name>.json; all of them by default) into
# tmp/characters/<name>/ and lays out each one's sheet.png. Usage: render_reviews.sh [NAME ...]
# Cycles on the GPU takes about 6.5 s a view; all the reviews take about 15 minutes.
setopt pipefail errexit
cd "${0:A:h}/../../.."
reviews=(${@:-${(f)"$(ls content/characters/reviews | sed 's/\.json$//')"}})
# Variants side by side on a sheet's row: one view per variant reads best in a grid.
typeset -A columns=(bodies 1 outfits 1 brows 4 layers 1 skins 3 expressions 4 faces 3)

for review in $reviews; do
  out=tmp/characters/$review
  rm -rf $out
  BLENDER_USER_RESOURCES=$PWD/.tools/blender /Applications/Blender.app/Contents/MacOS/Blender -b \
    --python-exit-code 1 --python content/characters/scripts/render_views.py -- \
    content/characters/reviews/$review.json $out | grep -E "RENDERED|RENDER_DONE"
  uv run -q --with pillow python content/characters/scripts/sheet.py $out \
    --columns ${columns[$review]:-3} --height 300
done
