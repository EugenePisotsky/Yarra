# Prepared ground candidate

This is a switchable implementation on the current branch. It does not require a
world database change. The reference material remains the default while the
candidate is evaluated. Normal game and audit startup now use **4× MSAA**.

**Update, 2026-09-07:** current-frame Xcode profiles show substantially less GPU
work with this candidate. The earlier HUD-only rejection below was premature.
The main ground pass measured 1.31 → 0.77 ms overhead and 1.91 → 0.62 ms at the
low camera, retaining native resolution and 4× MSAA. These are replay encoder
measurements, not live FPS or thermal results. See
[GPU breakdown and next steps](GROUND_GPU_BREAKDOWN.md). The candidate remains
opt-in while memory and visual transitions are evaluated.

## What changes

For a two-surface lit page, the prepared fragment path uses five logical material
texture samples: two albedos, two packed normal/material samples, and one combined
weight/macro control sample. The reference uses twelve selected-path samples.
This count excludes lighting/shadow sampling and is not a GPU speedup estimate.

- Each shared albedo array layer is baked in linear light into a seamless 4096²
  periodic triangular lattice. Four lattice cells per axis preserve approximately
  the original texel density. It changes the spatial pattern and introduces a
  finite repeat period; visual inspection is required.
- Each resident page gets a 272² RGBA control texture: 256² interior, eight texels
  of halo, and four mip levels. RG stores weights; BA stores a linearly decodable
  16-bit macro signal. Every mip evaluates the original macro inputs at its own
  footprint, including the halo. Fine macro detail is prefiltered at page scale.
- Macro strength, contrast and enable remain draw-time controls. Normals,
  roughness, AO, two-layer blending, lighting, shadows and mesh geometry retain
  their current paths. Normal textures retain their original UV mapping.
- The renderer prepares at most two pages per frame, with a 24 MiB page payload
  cap. It switches a material only after the control and shared albedo are on the
  GPU. Unsupported surfaces, missing bakes and budget overflow use the reference.
  Input edits invalidate controls; unloading a material releases its control.
  Switching a fallback page to its prepared albedo changes the fine pattern;
  streaming transitions still need visual acceptance in motion.
- The old stochastic-transform buffer is unnecessary for active prepared pages.
  Toggling back restores the reference resources. Resident controls are retained
  within the budget for warm A/B toggles.

The page payload is 393,040 bytes: about **18.37 MiB for 49 pages**. The prepared
two-layer albedo payload with mips is about **42.67 MiB with 4×4 block compression** (such as BC7), or
**10.67 MiB in ASTC 8×8** on iOS. These are additional shared allocations while
the reference textures remain resident. Driver allocation overhead, other
textures and render targets are outside the control budget.

## Build and use

With the original local source textures present, run:

```sh
python3 tools/prepare_terrain_albedo.py
cargo build --offline --locked --release -p yarra-app-game
./target/release/yarra-app-game --render-audit --terrain-prepared
```

The Python tool needs NumPy, Pillow and the KTX tools used by
`tools/compile_terrain_textures.py`. It tests periodic continuity, generates
wrapped mip chains and validates the compressed outputs. Intermediate PNGs stay
under ignored `source/prepared`; compressed arrays stay under ignored `runtime`.
Regenerate these bakes whenever their source albedos change. The build report
records source/output hashes. The tracked mapping is
`assets/packs/terrain/prepared.terrain-prepared`; iOS packaging includes it.

Use **Ground material: reference / prepared** in the audit to switch. The control
is independent of the flat/unlit diagnostic modes and of AA. The audit log records
requested state, allocated pages, active pages, bytes and cumulative builds.
An enabled setting with zero active pages is a fallback, not a measured candidate.

The current-branch comparison views use native resolution, 4× MSAA, no prepass,
no grass and no UI:

```sh
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 ./target/release/yarra-app-game \
  --render-repro ground-overhead --render-frames 2400 \
  --render-snapshot /absolute/path/reference.png
# Repeat with --terrain-prepared and a different screenshot/log destination.
# ground-low fixes a grazing view; ground-walk moves through the same low view.
```

Screenshots are taken at frame 600. Frame-limited runs allow compilation warmup
but are **short comparisons, not sustained thermal acceptance**. Exclude startup
and screenshot frames from timing. Check actual render dimensions, focus, AA,
thermal state and active-page counts in the logs. Stop builds and other GPU runs
before measuring. No historical branch comparison is needed.

## Validation

The native terrain test compiles and draws the prepared lit and unlit variants
with 4× MSAA, reads back packed macro texels including the halo, and checks
stationary reuse, reference restoration, source-image edits, eviction and budget
fallback. Run explicitly on a native GPU with the local prepared assets present:

```sh
cargo test --offline --release -p yarra-terrain-render -- --include-ignored --nocapture
```

## Initial HUD observations on 2026-09-06 (reassessed above)

The implementation passed validation, but the live HUD did **not** demonstrate a
useful duration improvement on the M2 Max. That observation does not establish
that the material failed to reduce GPU work: the 2026-09-07 pass profiles show
that it did. The default remains the reference material. The local assets are
the visually compared 4096² bake; the 2048² variant is preserved in the scratch
evidence.

Matched current-branch runs used 2560×1440, 4× MSAA, native direct rendering,
Gaussian shadows, no prepass, no grass and no UI. All had 49 resident source
pages; candidate runs confirmed 49 active prepared pages and no repeated builds.
Compilation and native GPU tests had finished before timing began.

| View / material | Mean GPU duration | Metal HUD graphics memory |
| --- | ---: | ---: |
| Overhead, reference | 4.66 ms | 368.77 MB |
| Overhead, prepared 4K | 4.61 ms | 442.08 MB |
| Moving low view, reference | 4.47 ms | 368.77 MB |
| Moving low view, prepared 4K | 4.35 ms | 442.08 MB |
| Moving low view, prepared 2K | 4.33 ms | 410.08 MB |

The observed mean differences are about 1–3%, with substantially more graphics
memory. All runs remained near the 120 Hz VSync limit. The first overhead
reference transitioned from fair to nominal thermal state; the other timing
runs reported nominal. These are short observations, not statistically proven
speedups or evidence of sustained phone performance. GPU frequency/power was not
measured, so no power saving or fixed-frequency throughput claim follows.

Timings use Metal HUD report headers 900–2300, excluding startup and the optional
frame-600 screenshot. HUD timing pairs can overlap/repeat; they are not treated
as independent frame samples. The parser only collapses identical consecutive
packets. The raw logs and
[summary](../tmp/performance-review/prepared-live-summary.json) preserve the
selection, timings, memory, focus, AA and residency evidence.

The overhead and low-camera **still images** retain similar broad colour,
blending and surface detail. No obvious page seams were seen in these fixed
views. This does not establish equivalent shimmer, streaming transitions or
repetition during extended play. Comparisons:
[overhead](../tmp/performance-review/overhead-comparison.png),
[low view](../tmp/performance-review/low-comparison.png).

A separate existing-audit probe at a different, fixed camera, with UI visible
and 4× MSAA, measured approximately 2.99 ms for reference lit ground with Gaussian
shadows, 2.95 ms with hardware 2×2 filtering, and 2.61 ms for the surface-unlit
diagnostic. The latter also skips packed normal/material sampling and fog. These
are not isolated lighting timings and must not be subtracted from the matched
views above. Neither probe identified a large saving from shadow filtering.
See [probe details](../tmp/performance-review/ground-lighting-probe-summary.json).

Validation completed: release build, seven terrain tests including the native
GPU test, nine app tests, compressed-asset validation and periodic bake checks.
The GPU test's final readback shutdown warning occurs after assertions pass;
runtime preparation emitted no errors. Build/source hashes are recorded in
[build identity](../tmp/performance-review/prepared-build-identity.json).

The follow-up [current-frame investigation](GROUND_GPU_BREAKDOWN.md) identifies
ground fragment shading as the dominant cost and confirms reduced shader work.
The live HUD/replay discrepancy has not been isolated to a particular cause;
submission gaps, overlap and GPU performance state remain relevant. iPhone
thermal acceptance remains untested in this Mac-only iteration.
