# Grass shading early exits — September 15, 2026

Retained a bounded first optimization pass after reviewing the GPUOpen procedural-grass article.
The changes preserve the existing material response, geometry, population, wind and 4× MSAA.
They show a small low-camera saving on this Mac, not a demonstrated general or sustained mobile
performance improvement. The larger scheduling, preparation-storage and distant-material proposals
remain separate work.

## Changes

- `grass_canopy.wgsl` checks maximum envelope height, grass-boundary depth, the 96 m range,
  and zero distance attenuation before evaluating its two noise layers. Packed controls constrain
  the envelope so these rejected fragments already had visibility 1 in the original shader.
  The shared function serves both grass and ground.
- `vegetation_debug_draw.wgsl` evaluates the narrow GGX sheen only while its existing distance
  weight is positive. Its fade still spans 22–48 m. The broad lobe and every normal derivative
  remain outside the branch; the lighting model and fade curve are unchanged.
- Updated the existing shader-presence assertion for the conditional assignment, and added an
  opt-in native comparison that swaps compatible draw/canopy shaders in one frozen scene.

No new buffers, textures, draw calls, quality settings or production shader variants were added.

## Verification

- Release game build passed.
- Renderer unit suite: 32 passed; 10 opt-in native/offline checks ignored in the ordinary suite.
- Explicit native prepared/fallback comparison passed with wind, 4× MSAA and forced arena overflow.
- Explicit native terrain-variant check passed, including prepared rendering and 4× MSAA.
- The new frozen-scene comparison passed all five cases with **zero differing bytes** between
  original and optimized shader output at 256×256. Cases cover near, overhead, the 48 m sheen
  cutoff, distant grass, canopy off/on, thin/tall envelopes, fixed wind, and MSAA off/4×.
  A separate strength-zero control confirms canopy shading actually affects the fixture.
- Restoring the current shader after each reference render reproduced the first image exactly.
  Instance counts remained unchanged during every A/B pair.
- `git diff --check` passed.

Separate game launches are not suitable for a strict pixel-equivalence assertion: repeated baseline
captures also differ. In the sampled near-grass region, baseline/baseline mean byte error was 0.628,
versus 0.201 for baseline/optimized. The frozen-scene test removes that confound; the game captures
provide field-scale visual checks rather than a relaxed replacement for that test.

## Whole-game timing

All runs used one release binary, the same archived runtime database, the same canopy settings,
Balanced density, 1920×1080 world rendering, 4× MSAA and deterministic frame-based wind. Runs were
sequential, with no builds or native tests overlapping them and no other Yarra process running.
The benchmark discards its first five Metal HUD packets. These are whole-game HUD durations,
not isolated grass-pass time or power measurements; samples within packets are correlated.

### Initial series: 3,600 frames per run

| View | Order | Shader | Mean GPU ms | P95 ms |
|---|---:|---|---:|---:|
| Low | 1 | Original | 4.913 | 5.38 |
| Low | 2 | Optimized | 4.839 | 5.28 |
| Low | 3 | Optimized | 4.862 | 5.32 |
| Low | 4 | Original | 4.912 | 5.36 |
| Overhead | 1 | Original | 4.534 | 4.98 |
| Overhead | 2 | Optimized | 4.520 | 4.87 |
| Overhead | 3 | Optimized | 4.650 | 5.23 |
| Overhead | 4 | Original | 4.937 | 5.34 |

The low-camera mean of run means changes **4.912 → 4.851 ms**, a **0.061 ms / 1.25%** reduction.
Both original visits agree closely and both optimized visits are lower. This is a small result.
Overhead timings drift substantially; the final run records a transition to Fair thermal state.
Do not use that series' overhead average as an optimization result.

### Overhead repeat: 2,400 frames, reversed order

After completing the native shader check and a 30-second idle interval:

| Order | Shader | Mean GPU ms | P95 ms |
|---:|---|---:|---:|
| 1 | Optimized | 4.598 | 4.95 |
| 2 | Original | 4.548 | 4.90 |
| 3 | Original | 4.539 | 4.88 |
| 4 | Optimized | 4.543 | 5.01 |

Mean of run means: **4.544 ms original / 4.571 ms optimized**, a 0.027 ms increase. The two
optimized runs themselves differ by 0.056 ms and the last optimized visit matches the originals.
No meaningful overhead gain or reliable regression is established. Logged thermal samples are
Nominal in this repeat, which does not guarantee constant GPU clocks. The combined changes were
measured together; their individual timing contributions were not isolated.

## Artifacts and reproduction

Artifacts live in `.editor/vegetation/experiments/shading-perf-01/` (ignored local experiments):

- `before-shaders/`, `after-shaders/`, `shader-changes.diff`;
- `game`, `runtime.sqlite`, and archived `content/vegetation/` settings;
- `runs/`, `matrix.json`, `repeat-overhead/`, `repeat-overhead.json`;
- `unit-tests.log`, `release-build.log`, `native-preparation.log`, `native-terrain.log`,
  `native-shading.log`, and `game-image-comparison.json`.

Each game run includes hashes, its command, raw log, summary and screenshot. The unused
`canopy-only/` and `sheen-only/` shader sets are available for a future component comparison;
they are not measured results.

Run the frozen shader comparison against the saved baseline from the repository root:

```sh
YARRA_GRASS_REFERENCE_SHADERS="$PWD/.editor/vegetation/experiments/shading-perf-01/before-shaders" \
  cargo test --offline -p yarra-vegetation-render shading_matches_reference_in_frozen_scene \
  -- --ignored --nocapture
```

The reference directory must contain compatible `vegetation_debug_draw.wgsl` and
`grass_canopy.wgsl` files. The test swaps in-memory assets, leaving live shader files unchanged.
