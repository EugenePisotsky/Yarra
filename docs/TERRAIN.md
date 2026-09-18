# Terrain surfaces

This records the terrain implementation that exists today. It is deliberately
not a complete editor or world-rendering roadmap.

The unified painter direction, terrain treatment rules and curved-road source model
are described in [Environment compositions and spatial authoring](ENVIRONMENT_AUTHORING_ARCHITECTURE.md).
Environment storage, cooking, painting, curved-road editing and road relief are implemented.
Automatic terrain rules remain planned work. Road relief derives a new surface from original
heightfields for preview and cooking; source heights are never repeatedly offset.

## Ownership

Terrain data has three different owners:

- Project SQLite stores reusable surface definitions, texture-set metadata,
  one terrain profile per world space, heightfields, and environment compositions,
  layers and R8 coverage masks. Cell palettes and render weights are derived output.
- The environment compiler validates shared source-mask borders and the cooker emits small page-local palettes and
  control maps into the runtime SQLite database.
- `assets/local/terrain/` stores licensed source images and derived KTX2 GPU
  textures. They remain ignored until redistribution rights are known.

A terrain surface is semantic data such as “uncut grass”: stable ID, editor
name, tile size, optional albedo anti-tiling, normal response, and roughness
range. A texture set maps these surface IDs to array layers. Consequently,
cells never depend on array-layer numbers directly and a pack can be rebuilt
without rewriting painted cells.

## Cell painting

The runtime format permits eight local surface slots per cell and two RGBA
weight pages. The first renderer supports one or two surfaces; this keeps the
initial shader and its cost easy to understand without constraining the future
editor format.

The demo uses two surfaces and a 65-by-65 endpoint-inclusive weight map for a
32-metre cell, giving a half-metre interval between control samples. Adjacent
cells therefore share their border samples exactly. The cooker rejects
mismatched borders rather than allowing visible seams. A
constant one-surface cell carries no weight texture at all.

The demo source masks are sampled from continuous world-coordinate functions.
Dry-meadow, green-meadow and clearing compositions derive both ground and grass
from those masks. Ground weights are no longer independently editable source.
Painting updates layer masks and previews their compiled ground and grass, without
changing the runtime page or shader
contract. Empty coverage produces the world's explicit base material.

## Rendering

`yarra-terrain-render` owns a Bevy PBR material. A resident terrain page creates
one tiny weight image and one material, both released when the page leaves
residency. Base color and packed normal/AO/roughness arrays plus the macro map
are loaded through Bevy's asset cache and shared by every page.

Base color is sampled as sRGB. Packed normal/AO/roughness and macro variation
are explicitly loaded as linear data; allowing Bevy's sRGB default on those
maps corrupts both the reconstructed normals and the centered macro signal.

The shader currently performs:

- world-aligned texture sampling at each surface's metre tile size;
- optional three-sample stochastic albedo anti-tiling per surface;
- normalized two-surface blending;
- octahedral normal decoding and PBR roughness/AO response;
- endpoint-correct weight sampling at cell borders;
- optional three-band macro variation.

Press `V` in the demo to toggle the complete macro treatment. The active mode
is shown in the diagnostics HUD.

The demo meadow uses the procedural-grass branch's 7.7, 31.5, and 235 metre
macro scales, 2.5 signal contrast, 0.395 albedo exposure response, and 1.6x
surface tile-size multiplier. Macro variation remains a centered exposure
change; it does not alter hue or saturation.

These numbers are authoring values for the current demo meadow, selected to
reproduce that reference appearance. They are not canonical engine defaults.
Terrain profiles and surface records already store the macro scales, contrast,
albedo response, tile size, anti-tiling choice, normal strength, and roughness
range as data. The editor should expose those values per world-space profile or
surface as appropriate; changing them must not require modifying the renderer.

The portable runtime pack uses UASTC KTX2. iOS selects native ASTC files from
the same texture-set record. Both contain offline mip chains. See
`assets/packs/terrain/temperate_meadow.toml` and
`tools/compile_terrain_textures.py` for the local pack contract.

Anti-tiling intentionally applies only to albedo. Normal/AO/roughness remain a
single sample, matching the useful default from the legacy renderer while
avoiding the much larger cost of stochastic sampling for every texture. The
demo lighting and exposure use the procedural meadow reference values.

Streamed heightfields with shared height/normal sampling are implemented; see
[the terrain integration checkpoint](GROUND_COVER_ARCHITECTURE.md). Terrain geometry
LOD, cliff projection and a far-terrain renderer remain separate future work.
