# Terrain surfaces

This records the terrain implementation that exists today. It is deliberately
not a complete editor or world-rendering roadmap.

## Ownership

Terrain data has three different owners:

- Project SQLite stores reusable surface definitions, texture-set metadata,
  one terrain profile per world space, cell palettes, and painted weight maps.
- The world cooker validates borders and emits small page-local palettes and
  control maps into the runtime SQLite database.
- `assets/local/terrain/` stores licensed source images and derived KTX2 GPU
  textures. They remain ignored until redistribution rights are known.

A terrain surface is semantic data such as “uncut grass”: stable ID, editor
name, tile size, optional albedo anti-tiling, normal response, and roughness
range. A texture set maps these surface IDs to array layers. Consequently,
cells never depend on array-layer numbers directly and a pack can be rebuilt
without rewriting painted cells.

## Cell painting

The project schema permits eight local surface slots per cell and two RGBA
weight pages. The first renderer supports one or two surfaces; this keeps the
initial shader and its cost easy to understand without constraining the future
editor format.

The demo uses two surfaces and a 65-by-65 endpoint-inclusive weight map for a
32-metre cell, giving a half-metre interval between control samples. Adjacent
cells therefore share their border samples exactly. The cooker rejects
mismatched borders rather than allowing visible seams. A
constant one-surface cell carries no weight texture at all.

The demo blend is generated from world coordinates only, so it remains
continuous across page boundaries. It contains broad pure-green and pure-dry
regions separated by irregular soft borders; averaging both materials at every
texel would destroy their individual character. An editor can later replace
those bytes with painted values without changing the runtime page or shader
contract.

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

There is no terrain geometry LOD, heightfield, cliff projection, or
far-terrain renderer yet. These should be added only when a real scene
demonstrates the need; they are not hidden in this foundation.
