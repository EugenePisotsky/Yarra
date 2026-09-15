# Fixed tuft lighting experiment — September 14, 2026

This is an isolated visual experiment, not a production integration. The editor's unsaved density draft was not touched. No performance study was run.

## Result

The native comparison is at `.editor/vegetation/experiments/tuft-ambient-02/comparison.html`.

- Additional ambient occlusion alone has a modest effect under this direct sun.
- Actual blade-to-blade sun shadows provide much stronger separation between overlapping leaves and darken covered interiors.
- This still does **not** reach the reference appearance. The crowns are regular, individual blades are visibly angular, and the point-directional sun produces hard shadows. It would be misleading to treat this as an accepted grass solution or evidence of Yōtei's exact implementation.

The first fixture (`tuft-ambient-01`) used wide radial root disks. Their empty centers made sparse starbursts. The second concentrates the same root count into compact crowns, with the existing upright profile inside and arching profiles outside. This is a layout correction, not an increase in geometry between lighting comparisons. Neither fixture changes the production placement code.

## What the comparison holds fixed

Four tufts contain 128 roots / 256 blades. All three lighting versions use the same vertices, material, exposure, sun, sky and camera. Geometry comes from the production generator using the saved `distance-01.ron` profiles (copied into the artifact folder). Wind, view-opening and procedural shadow marks are disabled. The neutral plane is a simple diagnostic ground, not the game's terrain material. No game tone-mapping pass or temporal antialiasing is used.

The test exports the actual blade triangles, including hidden surfaces, into a CPU-built BVH. The diagnostic fragment shader traces 64 fixed upper-hemisphere sky directions and one sun direction against that geometry. Rays stop at 4 m. Sky visibility uses two-sided leaf cosine weighting; it attenuates ambient light only. The combined version also attenuates direct light when a triangle blocks the sun. The ground receives the same geometry-derived visibility. No clump ID, color noise, procedural stripe or painted dark patch determines these shadows.

Limitations: opaque blockers, hard sun, finite sky samples and ray bias; no multiple scattering or translucency through blockers. Existing authored root/tip AO remains in the baseline material, so this experiment adds neighbor visibility to that approximation. Wind stability is not established by these frozen views. This implementation is test-only and is not proposed as a runtime algorithm.

## Reproduce

From the repository root, choose a fresh output folder and copy the saved study into it:

```sh
mkdir -p .editor/vegetation/experiments/tuft-ambient-new
cp content/vegetation/distance-01.ron .editor/vegetation/experiments/tuft-ambient-new/source-study.ron
YARRA_TUFT_AMBIENT_OUTPUT="$PWD/.editor/vegetation/experiments/tuft-ambient-new" \
YARRA_SHADOW_STUDY_SOURCE="$PWD/.editor/vegetation/experiments/tuft-ambient-new/source-study.ron" \
cargo test --offline -p yarra-vegetation-render capture_tuft_ambient -- --ignored --nocapture --test-threads=1
python3 tools/grass_tuft_preview.py .editor/vegetation/experiments/tuft-ambient-new
```

The extractor requires NumPy and Pillow. Serve the output directory with a local HTTP server to open `comparison.html`, or inspect its PNGs directly.

Validation completed: Naga shader validation, native Metal capture of both views, finite visibility values, and selected sun/sky visibility checked against independent CPU triangle intersections. The comparison remains subject to visual acceptance.
