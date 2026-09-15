# Full-field appearance trial — September 14, 2026

The user rejected the isolated tuft comparison as an acceptance test and requested evaluation at field scale. This supersedes the small-patch-only gate in `GRASS_SHADOW_RESTART.md`. The latest request authorizes integrating the selected field into the game and checking its appearance and performance there.

## Playable game integration

The selected 72 roots/m² layout and revised palette are saved in
`content/vegetation/field-current.ron` and published to the normal project/runtime databases.
`field-baseline.ron` retains the previous 44-root catalog for the in-game comparison.

- The game starts with the published catalog and static canopy ground treatment.
- **G** switches between the published setup and the previous catalog with normal ground.
  Camera, sun, wind, geometry LOD, and density LOD policy remain the same across this switch.
- **O** uses the existing density policy cycle; the HUD reports the selected policy.
- Blade shadow marks default to **Off** to match the field comparison; **B** still cycles them.
- Geometry LOD remains enabled. The comparison's forced full blade geometry was an overhead-only
  diagnostic that could overflow the existing instance arenas; it is not included in this game build.

Ground coverage uses the same source-root retention, 0.65 m filter radius, and
`1 - exp(-density * 0.075)` transfer as the selected study. Terrain radiance is multiplied by
`mix(1, 0.10, cover)` before tone mapping. This remains an artistic approximation, not traced
visibility. Masks are generated on two background workers as terrain pages become resident,
with neighboring roots and a one-texel border to avoid page seams. They are cached through A/B
switches and removed on page unload. The HUD shows how many ground tiles are ready; initial
streaming, mask creation and first-use shader compilation should finish before comparing steady
frame timings. Walking into fresh pages also exposes those streaming costs.

```sh
cargo run --release -p yarra-world-cook -- import-vegetation content/vegetation/field-current.ron
cargo build --release -p yarra-app-game
MTL_HUD_ENABLED=1 target/release/yarra-app-game
```

Validation: release build and three coverage tests pass, including matching tile borders and
translated source pages. Both new and previous setups were rendered in the native game without
shader errors; the new view reached 49/49 ground tiles ready. The first native run exposed an
upper-edge float-rounding index error, fixed before the successful captures.

The existing rendering resolution and 4× MSAA are retained. This is a playable visual/performance
trial, not a claim that the Yōtei appearance has been matched.

### Gameplay LOD and missing-block correction

The user found that pulling back the orbit camera left the high-detail region behind the
character. That region was a camera-centered disk of roughly 10 m before per-root staggering.
The game now supplies the character's render-space position through `VegetationLodFocus`.
High topology uses an equal-area ellipse centered 2 m ahead, with 1.5× reach along the view
direction and reciprocal width. Its high-instance budget is unchanged. Editor clients without
a gameplay focus retain the camera-centered policy.

Balanced density now keeps all eligible roots near that focus and gradually restores its
existing distant thinning. The paired-root 0.65 retention and extra width compensation also
fade out toward the focus. This preserves near coverage without using full-reference density
across the visible field. No material, ground shading, or palette changed in this correction.

The straight bare strips in full-reference were consistent with overflowing the low-instance
arena: its append guard discards records after capacity is reached. In the low third-person
capture, full-reference emits 392,848 split low instances, exceeding even the former total low
capacity of 278,528. Low capacity is now 786,432 (851,968 total with the unchanged high bins).
This reserves an additional 15.5 MiB of instance storage plus 1.94 MiB of preparation indices.
Full-reference remains an expensive diagnostic; increasing capacity does not make it cheaper.

Native checks at camera distances 4, 9.7, and 17.6 m found no shader errors or capacity drops
in Balanced; full-reference at 4 m also had zero drops and no straight bare strips. At the same
4 m camera, Balanced emitted 124,238 roots / 1,478,262 indices versus full-reference's
404,852 roots / 4,003,788 indices. Both retained 12,004 high-detail roots. These are geometry
counts, not an FPS speedup claim. The two farther visual captures freeze wind; the low-camera
pair uses animated wind. The render audit camera transform is authoritative: the older orbit
HUD still reports its saved rig distance while a reproduction camera overrides that rig.
Release build, WGSL validation, and all 14 selected renderer tests pass (one existing ignored
test remains ignored). The interactive game is launched without reproduction flags, in Balanced
mode with the Metal HUD.

```sh
target/release/yarra-app-game --grass-density balanced --grass-counters --render-repro grass-follow --render-frames 1200 --render-snapshot /tmp/yarra-lod-balanced-follow.png
target/release/yarra-app-game --grass-density balanced --grass-counters --render-repro grass-follow-far --render-frames 1200 --render-snapshot /tmp/yarra-lod-balanced-far.png
target/release/yarra-app-game --grass-density full --grass-counters --render-repro grass-close --render-frames 1200 --render-snapshot /tmp/yarra-lod-full-close.png
```

### Shorter high-geometry range trial — September 15

`HIGH_TOPOLOGY_DISTANCE_SCALE` in `vegetation_debug_compute.wgsl` is now `0.65`.
This contracts the existing character-focused high-detail ellipse by 35% on both axes;
the stable per-root staggering and smooth geometry morph remain in place. A value of
`1.0` restores the previous range. This is applied after the CPU's capacity-derived
radius, so it reduces rather than expands the allowed high-topology population.

The near-field coverage region, density policy, blade topology, palette, gloss, and
canopy settings are unchanged. Roots switching to low geometry still follow the existing
low-LOD retention policy; the full-density near-field region is not shortened. The shader
is loaded from disk by both clients, so restart the game/editor to pick up the experiment.
Validation: a native Balanced third-person run completed without shader errors; the
captured field retained grass coverage and gloss. No before/after performance comparison
was made for this range adjustment.

## Verified Yōtei information

Eric Wohllaib's [GDC 2026 ambient lighting slides](https://media.gdcvault.com/gdc2026/Slides/Wohllaib_Eric_Ambient_Diffuse_Lighting_in_Ghost_of_Yotei.pdf), PDF pages 78–79 (printed slide numbers 77–78), explicitly state that procedural grass is excluded from the ray-tracing BVH. Grass impostor textures approximate terrain diffuse color for upward bounce lighting. The speaker identifies speckles on the terrain as grass **shadow impostors**, separate from the BVH. Thus, triangle-level grass shadows are not a prerequisite for our appearance work. These slides do not establish the exact material recipe responsible for the supplied screenshot.

Tsushima's earlier grass presentation separately describes shared clump variation, authored blade AO, a terrain shadow impostor and short-range screen-space shadows; see `GHOST_OF_TSUSHIMA_GRASS_TALK_NOTES.md`. Neither source justifies attributing our material approximation below to Yōtei.

## Coverage and palette controls

Artifact: `.editor/vegetation/experiments/field-coverage-05/comparison.html`.

The user rejected interpreting the last failure as simply excessive darkness: Yōtei has darker
areas that still look integrated. This revision keeps the ground shading rule and lighting while
separating three contributors: density LOD, geometry LOD, and source coverage. It also includes an
explicitly authorized color-only comparison.

- **Previous field:** 44 roots/m², production density and geometry LOD, field-ground-04 ground.
- **All 44 roots, current blade LOD:** `FullReference` density mode. This bypasses the extra 0.65
  paired-root retention factor as well as distance thinning, while retaining blade simplification.
- **All 44 roots, full blade shapes:** also disables geometry reduction in the isolated compute
  shader. The wider overhead view shows that restoring detail helps the distant field, but does
  not close the large openings or eliminate the uniform strip appearance.
- **72 roots, full blade shapes:** a moderate density control at the close/far overhead views.
  More overlap fills many openings. It does not establish that 72 is a final required density.
  Source placement changes when the candidate lattice changes; the ground retains the same
  source-coverage shading rule, so its mask also responds to density.
- **72 roots, revised color:** exactly the previous control's geometry and ground, with root RGB
  `(0.048, 0.092, 0.033)` and tip RGB `(0.11, 0.235, 0.058)` in the catalog's linear color space.
  Tips are less bright and root-to-tip color contrast is smaller. Exposure, light, gloss and
  occlusion settings are unchanged. This reduces the bright-strip effect; it does not supply the
  missing variation in illumination through the grass canopy.

Full-shape controls are restricted to bounded overhead views of the 64 m field. The first 72-root
wider overview exceeded the existing high-detail instance capacity; it is marked invalid and
excluded from the report. The capture tool now rejects capacity-clipped images. This is a
correctness check, not performance analysis. All displayed captures completed without shader
errors or capacity clipping. No Rust or production shader/catalog changes were needed for these
controls, and no performance tests were run.

The visual result supports investigating both coverage and material balance. It does not establish
that Yōtei uses a particular density, palette, or camera-distance ground effect. These are isolated
appearance controls, not an accepted replacement renderer.

```sh
python3 tools/grass_field_study.py prepare --output .editor/vegetation/experiments/new-coverage-field
python3 tools/grass_field_study.py capture --output .editor/vegetation/experiments/new-coverage-field --variants joined unthinned fullshape --views overhead
python3 tools/grass_field_study.py capture --output .editor/vegetation/experiments/new-coverage-field --variants joined fullshape filled palette --views top-close top-far
python3 tools/grass_field_study.py report --output .editor/vegetation/experiments/new-coverage-field
```

## Ground continuity revision — rejected

Artifact: `.editor/vegetation/experiments/field-ground-04/comparison.html`.

The user rejected field-canopy-03: dirty roots, bright ground and large gaps. The old ground mask
counted roots over a 0.22 m radius. Pulling roots into clumps emptied the mask between them, while
blades still extended over those spaces. Darkening lower blade sections increased this separation.

The new **Overlap** layout returns spacing to 0.6 m and reduces root attraction to 0.04, with
moderate shared height, shape and direction. It removes the extra canopy shader profile and returns
clump color variation to 0.12. The 44 roots/m² budget and blade topology remain the same.

**Overlap + shaded ground** uses exactly those blades with a new opt-in editor ground mode,
`CanopyGroundStudy` (`--study-ground canopy-ground`). It filters source root density across a
0.65 m footprint and converts density to approximate cover as `1 - exp(-density * 0.075)`.
Ground radiance is attenuated from 1 to 0.10 according to that cover, before tonemapping. This
includes ground highlights; the old albedo-only treatment did not. These constants are artistic
trial parameters, not a physical extinction estimate or a reconstruction of Yōtei. The footprint
does not know leaf orientation, height or sun direction. It preserves sufficiently large clear
areas, but cannot reproduce individual cast shadows or arbitrary canopy boundaries accurately.

Ground shading depends on source vegetation, not camera distance, wind or rendered LOD. Close
and farther overhead views keep the same angle, world target and lighting. They permit judging
distance without adding another brightness multiplier. The user's two reference screenshots do
not establish whether Yōtei explicitly darkens terrain with distance; projected coverage, filtering
and differences in framing remain possible explanations.

The trial contains the previous baseline/rejected captures plus ten new native captures: both
revised variants at the original three angles and the two overhead distances. The broader dark
ground reduces bright gaps. It does **not** solve the overly uniform blade illumination or establish
the desired soft clump appearance. No production catalog or grass shader was changed by this
revision. The new ground mode is confined to the editor study. The previous CPU bake timer was
removed; no performance analysis was performed.

Validation: editor builds; all 18 vegetation workspace tests pass, including a source-footprint test
that bridges small root-free gaps but preserves larger openings. Native captures validate the
extended ground shader. The field, close/far views, and both sun-facing directions were inspected.

```sh
python3 tools/grass_field_study.py prepare --output .editor/vegetation/experiments/new-ground-field
python3 tools/grass_field_study.py capture --output .editor/vegetation/experiments/new-ground-field --variants overlap joined --views overhead away toward top-close top-far
python3 tools/grass_field_study.py report --output .editor/vegetation/experiments/new-ground-field
```

## Previous trial — rejected

Artifact: `.editor/vegetation/experiments/field-canopy-03/comparison.html`.

All variants render a **64 × 64 metre field through the production vegetation renderer**, using the saved `distance-01.ron` catalog, its 44 roots/m² maximum budget, the same world seed, light and exposure, the actual scale character, and production LOD. Three cameras show the field from above, away from the sun and toward it. Captures use 1920 × 1080 and 4× MSAA. The study viewport now supports those settings through the existing study document; older documents retain their original settings.

1. **Current field:** current saved catalog and material, meadow ground.
2. **Clumps:** stronger shared direction, height and silhouette; moderate root attraction; clump spacing 0.85 m and color variation 0.25. No density increase. Changing clump assignment can slightly change stochastic retention, so equal density budgets do not promise identical instance counts.
3. **Clumps + canopy shading:** the same layout with a rest-space height profile relative to a representative clump crown. The lower skirt receives less diffuse and ambient light. Existing albedo and gloss remain. This is authored material shading, not measured occlusion, and can dim a geometrically exposed lower blade.
4. **Combined:** adds the existing coverage-based understory ground treatment, described in `GRASS_GROUND_MATERIAL_STUDY.md`.

Procedural shadow marks are off throughout to expose these contributions. No grass ray tracing, shadow-map caster, extra density or added blade topology is involved. World/character directional shadows still render normally. The study plane is flat; it does not reproduce the reference's terrain, weather, color grade or reconstruction pipeline.

The early field-01 canopy variant brightened both sides of every blade and remapped albedo toward crown height. It flattened the field and was dropped. Field-02 removed those changes but still used the old un-antialiased capture path. Field-03 uses the same revised appearance with matched 4× MSAA. These are local experiments, not changes to the saved game catalog or the user's unsaved editor draft.

## Reproduce

```sh
cargo build --offline -p yarra-app-editor
python3 tools/grass_field_study.py prepare --output .editor/vegetation/experiments/new-field
python3 tools/grass_field_study.py capture --output .editor/vegetation/experiments/new-field --views overhead away toward
python3 tools/grass_field_study.py capture --output .editor/vegetation/experiments/new-field --views overhead away --variants baseline understory --phase 0
python3 tools/grass_field_study.py capture --output .editor/vegetation/experiments/new-field --views overhead away --variants baseline understory --phase 1
python3 tools/grass_field_study.py report --output .editor/vegetation/experiments/new-field
```

The tool clones shader assets into each experiment and launches separate capture processes against unsaved study documents. It does not publish or overwrite live assets. Serve the resulting folder locally to use the comparison. The candidate is a visual trial, not an accepted replacement for grass shadow impostors or a claim that the Yōtei appearance is solved.

Validation: editor build and the 17 existing vegetation workspace tests pass. All 20 native captures
completed without shader errors and retain 1080p / 4× MSAA in their saved documents. The final field
comparison and both candidate wind poses were inspected visually. No performance measurements were
collected. The canopy profile itself is fixed in rest space; the existing moving geometry and
directional normal response still affect the visible field, so this does not establish a fix for all
wind-dependent brightness changes.
