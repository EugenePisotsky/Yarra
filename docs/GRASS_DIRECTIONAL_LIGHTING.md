# Directional grass lighting — visual trial, 2026-09-14

The user wants blades to catch light at different orientations when looking away from the
sun, while preserving the existing sun-facing gloss. The first trial was rejected: it mostly
added dark strips, including on exposed blade faces. Performance work is deferred until the
user accepts the visuals.

## Current correction

The material previously reused its strongly rounded, cylinder-like gloss normal for diffuse
lighting. Reducing diffuse wrap and ambient fill amplified the dark side of that artificial
cross-section. A controlled capture with procedural shadow marks disabled retained the dark
longitudinal stripes, isolating the normal response as a contributor.

- Body lighting now uses the physical curved blade plane with only 18% of the rounded gloss
  normal, fading that small fold out with distance. Gloss retains its existing rounded normal.
- Diffuse wrap is 0.18 and the exposed sun-energy ceiling is 1.20. This gives sun-facing surfaces
  more light rather than relying on deeper darkening for contrast.
- The existing distant illumination filter retains 35% of the individual blade response, with
  65% gradually blending toward the stable clump response between 20 and 72 metres.
- Sky fill uses the absolute vertical component of the blade plane, ranging from 0.75 to 1.0.
  A viewer-facing normal flip no longer changes the sky fill between a bright and dark hemisphere.
  Ambient intensity and exposure still matter; zero ambient contributes zero fill.

The ambient peak ceiling is 0.60, with authored AO scaling it from 0.40 to 0.85. Sun gloss,
transmission, real directional shadow reception, blade colors, geometry, density and wind are
unchanged. The Medium procedural shadow marks remain available as before; they are synthetic
marks, not measurements of neighboring occluders. Their on/off comparison is retained separately.

This remains a visual trial. It does not add actual grass self-shadowing, ground shadow casting,
or environment reflections, and should not be presented as matching the reference yet.

## Review and validation

Local original images and isolated shaders are in
`.editor/vegetation/experiments/directional-light-01/`:

- `before-shaders`: original implementation.
- `final-shaders`: rejected first lighting trial (historical name).
- `sheet-shaders`: current correction.
- `final-away-no-marks` / `sheet-away-no-marks`: same camera, roots, light and marks-off setting.
- `sheet-away` / `sheet-toward`: current correction with the existing Medium marks.
- `game-sheet-away`: corrected shader rendered in the actual game, with Medium marks and 4× MSAA.

`correction.html` compares these native captures. The shader parser validates every blade-band
variant, the existing production-lighting source contract passes, and native captures render
without shader errors. No performance measurements were run for this correction.
