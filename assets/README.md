# Assets

```text
assets/catalogs/   tracked runtime catalogs (character presentations)
assets/packs/      tracked pack manifests: provenance, expected local files, import decisions
assets/shaders/    WESL shaders (layout in docs/ARCHITECTURE.md#shaders)
assets/local/      ignored: licensed sources and everything built from them
assets/generated/  ignored: the cooked runtime world, rebuilt from content/
```

A Git checkout does not restore `assets/local/`; back it up. What each folder holds and how to
rebuild it is in [Workflows](../docs/WORKFLOWS.md#local-assets). Nothing cooks until it is
restored.

## Vegetation packs

Every tree, shrub and ground plant comes from the vegetation kit in
[YarraVegetation](https://github.com/EugenePisotsky/YarraVegetation) (its generators, presets and
[authoring guide](https://github.com/EugenePisotsky/YarraVegetation/blob/main/TREE_AUTHORING.md)).
`tools/import_vegetation_bundle.py` turns a kit bundle into a pack: the tracked catalog
`assets/packs/yarra_<pack>/<pack>.catalog.ron` and the local files in `assets/local/yarra_<pack>/`:

```text
assets/local/yarra_longleaf/
  import.json, source/...
  runtime/pine_longleaf/pine_longleaf_lod{0,1,2,3}.{gltf,bin}, pine_longleaf_impostor_*.ktx2
  runtime/textures/
```

The packs: `yarra_longleaf` (pines), `yarra_spruces`, `yarra_birches`, `yarra_oaks`, `yarra_maples`,
`yarra_tall_forest` (tall broadleaf), `yarra_dead_trees`, `yarra_bay` and `yarra_shrubs` (shrubs),
and the ground cover `yarra_ferns`, `yarra_nettles`, `yarra_sorrel`, `yarra_lilies`. Import forms
that share textures together. Grass is not a pack of meshes: `assets/packs/grass/baltic.ron`
names what grows on each terrain surface.

## Terrain surfaces

`assets/packs/terrain/baltic.toml` lists the ground surfaces in texture-array order, their
Megascans sources (`~/Dev/Assets/terrain/sources`) and parameters. The tools build:

```text
assets/local/terrain/baltic/
  source/<surface>/{base_color.png,normal_material.png}   # tools/import_terrain_surfaces.py
  source/macro_variation.png, source/import-report.json
  runtime/{universal,astc}/{base_color_array,normal_material_array,macro_variation}.ktx2
                                                          # tools/compile_terrain_textures.py
```

The world's terrain catalog refers to the runtime files (UASTC for the Mac, ASTC for iOS).

## Characters

`assets/local/characters/female_main/female_main_locomotion.glb` is the player: body, skeleton
and animation clips, checked against the SHA-256 in `assets/packs/characters/female_main.toml`.
`assets/catalogs/character_presentations.catalog.ron` gives stable IDs to skeleton contracts,
models, animation banks, clips, movement sets and presentation profiles
(`engine::character_catalog`); it may reference ignored local files.

## Tree shading contracts

What the importer writes and the tree shaders (`shaders/tree/material.wesl`,
`shaders/lighting/pbr_lighting.wesl`) read:

- **Leaf cards are lit by the crown**, not by their own orientation: `NORMAL` is the smoothed crown
  surface normal, kept on both sides of a card. Leaves are rough with half the default
  reflectance (`KHR_materials_specular`): glossy sky reflection off crown normals turned shaded
  leaves grey.
- **Crown sky visibility** (`vegetation_occlusion: "crown_sky_v2"` from the kit, imported as
  `yarra_shading: "crown_v2"`): vertex colours hold the share of an overcast sky a card sees past
  the rest of its complete crown. The game multiplies the crown's sky light by it and by the shade
  of the crowns around, keeping 12% for light through the leaves; over the outer 30% of the
  shadow range it blends to a coarse self-shadow from the same values. Impostors carry the same
  values. The importer refuses the older `crown_sky_v1`: rebuild the bundle.
- **Bark sky visibility** (`"bark_sky_v1"`): bark vertex colours hold the same overcast sky
  visibility against the whole crown; the game lights bark by it (plus 12% through the leaves)
  instead of tinting its albedo.
- **Two-layer pine bark** (`yarra_bark: "blend_v1"`): every bark triangle gets the lower bark's
  material with the upper bark's textures, blended across COLOR_0 alpha
  (`shaders/tree/bark.wesl`): the upper bark shows first in the lower bark's furrows. Two
  materials met in a hard line.
- **Wind** (`yarra_wind`): TEXCOORD_1 holds flutter and branch; bark is opaque and rigid.

## Map symbols and lettering

`tools/render_world_map.py` reads two free sets from `assets/local/map/`:

```text
assets/local/map/homann/   K.M. Alexander's Homann Cartography Brushes 2.0 PNG Pack (CC0),
                           unpacked as distributed: Landforms/, Flora/, Settlements/, Cartouches/
assets/local/map/fonts/    IMFeENrm28P.ttf, IMFeENit28P.ttf, IMFeENsc28P.ttf: IM Fell English
                           roman, italic and small caps (SIL Open Font License)
```

Download the brushes from <https://kmalexander.com/free-stuff/fantasy-map-brushes/> and the fonts
from Google Fonts (`ofl/imfellenglish`, `ofl/imfellenglishsc`). CC0 needs no attribution; a
shipped game that includes the fonts must include the OFL text.
