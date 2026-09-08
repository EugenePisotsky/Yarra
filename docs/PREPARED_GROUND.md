# Prepared ground

Prepared ground is the normal game and audit default. It does not require a world
database change. Use `--terrain-reference` to compare the original material.
Normal launches use **75% world resolution and 4× MSAA**, with native-resolution UI.
The terrain library remains opt-in for other clients, including the editor.

**Update, 2026-09-07:** current-frame Xcode profiles show substantially less GPU
work with this candidate. The earlier HUD-only rejection below was premature.
The main ground pass measured 1.31 → 0.77 ms overhead and 1.91 → 0.62 ms at the
low camera, retaining native resolution and 4× MSAA. These are replay encoder
measurements, not live FPS or thermal results. See
[GPU breakdown and next steps](GROUND_GPU_BREAKDOWN.md). At the time of these
captures, the candidate remained opt-in for memory and visual transition
evaluation; the activation update below supersedes that default.

**Update, 2026-09-08:** prepared ground now prefers the existing native ASTC 8×8
albedo bake when the device reports ASTC support. This keeps the 4096² dimensions,
four-cell period, mip chain and filtering. The desktop universal bake remains
the fallback, including when the optional native file is missing. The native
GPU test on M2 Max reports **44,739,296 → 11,184,896 bytes** for the shared albedo
array: a 32 MiB reduction. This is a lossy compression change and requires visual
comparison; it is not a claim about FPS or heat.

### Activation update, 2026-09-08

Shared albedo and page controls now activate independently. Once the shared bake is
resident, every eligible page uses that same albedo, including newly streamed pages,
pages awaiting control preparation, and pages outside the 24 MiB control budget.
The fallback shader evaluates the original weights and macro variation while
retaining the prepared albedo. Completing or evicting controls therefore no longer
replaces the fine surface pattern. This fallback also avoids the unused stochastic
transform table. The fully prepared shader keeps its existing sample count.

A texture set whose bake is still loading or unavailable uses the original albedo;
the change does not eliminate first-load transitions between different shared
texture sets. Cached and original controls also have different filtering/precision.
Explicitly switching to the reference material still changes the spatial pattern.

Normal game startup and audit Reset enable this path. `--terrain-prepared` remains
accepted for older capture commands; `--terrain-reference` opts out. Optional grass
statistics are now off by default on every platform, including audit startup.
`--grass-counters` (or the audit button) enables them; desktop readbacks are also
skipped when counters are off. Required placement/draw counters remain intact.

Validation passed: all seven terrain tests (including the native Metal test),
ten app tests, the macOS release build, and the iOS compile check. The native test
forces the control budget to zero and draws lit/unlit terrain at 4× MSAA. Compared
with cached controls, maximum byte error is 2 and mean error is 0.004745 in that
fixture; replacing a visible material with a newly streamed one then gives an
identical fallback image. Control restoration, source edits and reclamation also
pass.

A fresh 128 m streaming run without `--terrain-prepared` produced six images
identical in every original image channel to the earlier native-ASTC prepared
captures, including consecutive moving frames and the far point. Its return image
also matches its starting image exactly. Sampled controls peaked at 63 pages /
23.61 MiB; the last periodic allocation sample precedes the final cooldown, so it
does not establish the end-of-run allocation count. A separate 75%/4× run confirms
`--terrain-reference --grass-counters` restores the reference and produces GPU
readbacks. Normal launch and click-to-move passed without debug arguments. These
are correctness checks, not a new sustained frame-time or thermal result.

Evidence: [streaming results](../tmp/prepared-activation-2026-09-08/stream-result.json),
[reference/counter switches](../tmp/prepared-activation-2026-09-08/reference-counters-result.json),
and logs/images under `tmp/prepared-activation-2026-09-08/`.

### Current compression and streaming checks

Matched 2026-09-08 release runs on M2 Max used native 2560×1440, 4× MSAA,
Gaussian shadows, no prepass, no grass and no UI. The only comparison switch
was `--terrain-prepared-universal`; both runs used the same 4K bakes and executable.
No builds or GPU profilers ran during collection. The MSAA store policy was
Automatic in both runs.

| Resident payload (49 pages) | Universal bake | Native ASTC 8×8 |
| --- | ---: | ---: |
| Shared prepared albedo | 42.67 MiB | 10.67 MiB |
| Page controls | 18.37 MiB | 18.37 MiB |
| Combined prepared payload | 61.03 MiB | 29.03 MiB |
| Metal HUD graphics-memory display | 442.08 | 410.08 |

The HUD graphics-memory difference corroborates the 32 MiB payload reduction.
Those short runs were memory checks, not a frame-time or thermal benchmark;
the first run reported fair thermal state after compilation and the later run
reported nominal. That change must not be attributed to compression.

The 128 m out-and-back route generated and retired real pages. Sampled controls
ranged from 49 to 63 pages (maximum 23.61 MiB), under the 24 MiB payload cap,
and the shared albedo remained one allocation. The universal run's final sample
returned to 49 pages / 18.37 MiB; the native run's last periodic sample was just
before the final return. Both formats' frame-6500 images are byte-identical to
their frame-300 starting images after unloading and rebuilding pages.

Eleven matched image pairs, including consecutive moving frames and the farthest
point, show small compression differences. Over the lower half of each image
(excluding the sky), average RGB byte error is 0.735–0.833 / 255, with PSNR
45.85–47.18 dB. Reviewed starting and far-point images show no obvious new
compression blocks or page seams. These are sampled-frame checks, not exhaustive
validation of shimmer or every reference-to-prepared transition. These captures
predate the independent albedo/control activation update above.

The native GPU test verifies format selection, switching back, control reuse,
and removal of the old GPU image after each switch. Removing all eligible
materials also clears shared-albedo residency. A separate missing-native-file
run switched once to the universal bake and reached 49 active prepared pages;
the test restored the manifest afterward. All seven terrain tests and ten app
tests passed, including the native terrain test. The macOS release build and
`cargo check --locked -p yarra-app-game --target aarch64-apple-ios` also passed;
iOS runtime and sustained thermals remain unverified.

Evidence: [memory](../tmp/prepared-memory-2026-09-08/memory-comparison.json),
[streaming](../tmp/prepared-memory-2026-09-08/stream-comparison.json),
[image differences](../tmp/prepared-memory-2026-09-08/image-comparison.json),
[missing-file fallback](../tmp/prepared-memory-2026-09-08/missing-native-result.json).
Visual pairs: [starting universal](../tmp/prepared-memory-2026-09-08/universal-000300.png),
[starting native](../tmp/prepared-memory-2026-09-08/native-000300.png),
[far-point universal](../tmp/prepared-memory-2026-09-08/universal-003300.png),
[far-point native](../tmp/prepared-memory-2026-09-08/native-003300.png).
Raw logs, all 22 PNGs and the executable hash are in `tmp/prepared-memory-2026-09-08/`.

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
  cap. It uses the shared albedo as soon as that texture is ready, independently
  of control readiness. Control-budget overflow retains the shared albedo and
  evaluates original controls. Unsupported surfaces and missing bakes use the
  original material. Input edits invalidate controls; unloading releases them.
- The old stochastic-transform buffer is unnecessary for active prepared pages.
  Toggling back restores the reference resources. Resident controls are retained
  within the budget for warm A/B toggles.
- Shared albedo handles are released when no eligible resident material uses
  their source texture set. Changing compression drops the previous variant;
  normal selection loads only one variant. Readiness tracks the control image
  and selected albedo together, so format changes cannot reuse an old atlas's
  readiness signal.

The page payload is 393,040 bytes: about **18.37 MiB for 49 pages**. The prepared
two-layer albedo payload with mips is about **42.67 MiB with 4×4 block compression**
(such as BC7), or **10.67 MiB in ASTC 8×8** on supported devices, including M2 Max
and iOS. These are additional shared allocations while the reference textures
remain resident. Driver allocation overhead, other
textures and render targets are outside the control budget.

## Build and use

With the original local source textures present, run:

```sh
python3 tools/prepare_terrain_albedo.py
cargo build --offline --locked --release -p yarra-app-game
./target/release/yarra-app-game
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
`terrain_prepared_active` counts fully prepared pages;
`terrain_prepared_albedo_active` includes pages still evaluating original controls.
An enabled setting alone does not establish either path is ready.

On desktop, add `--terrain-prepared-universal` to disable the optional native
override for comparison. This keeps the manifest's primary image; on iOS that
primary image is already native ASTC. `terrain_prepared_albedo_bytes` reports the
selected shared textures' GPU payload including mips, separately from the page
control budget. `terrain_prepared_albedo_images` and
`terrain_prepared_astc8x8_images` report observed GPU allocations/formats.
These payload counters exclude driver overhead and resources pending destruction.

`--render-repro ground-stream` moves the camera and actual world streaming focus
128 m out and back over 6,000 frames after a 300-frame warmup, then holds at the
start. It uses controlled world-space position requests to exercise residency;
it is not a locomotion/controller test. Run for at least 6,600 frames so old pages
can leave the two-second cooling interval. The shorter `ground-walk` only moves
the camera inside the original preload ring.

For matched image sequences, combine `--render-snapshot /absolute/path/view.png`
with `--render-snapshot-frames 600,601,900,1800,3300,6300`. Multiple frames append
the frame number to the filename. Collect images separately from timing or memory
HUD runs, because screenshot readback adds temporary GPU work and allocations.

The current-branch comparison views use native resolution, 4× MSAA, no prepass,
no grass and no UI:

```sh
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 ./target/release/yarra-app-game \
  --render-repro ground-overhead --terrain-reference --render-frames 2400 \
  --render-snapshot /absolute/path/reference.png
# Repeat without --terrain-reference, using a different screenshot/log destination.
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
fallback, including albedo continuity during control eviction and new-page arrival.
Run explicitly on a native GPU with the local prepared assets present:

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
