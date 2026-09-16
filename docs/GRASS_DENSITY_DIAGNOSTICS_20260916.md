# Fullscreen grass draw and density diagnostics — September 16, 2026

GP-017 corrects the source-density record and isolates the expensive grass draw.
GP-018 tests a larger blade-preparation arena without changing default quality.

## Corrected baseline identity

The actual project and runtime catalogs both contain **66 roots/m²** in
`short_split_fill`, not 72. Their payload is identical, SHA-256
`d75b7fb8cb8113d357f93237bb2c5ea8c7a8f5983270e15ccbae04c359f6942c`.
This is also the recorded catalog hash in the earlier September 16 powered
sessions and the September 15 shading/scheduling investigations. Their timings,
watts, focus and thermal observations remain valid; the source-density label was
wrong. Nominal paired blade forms are 132/m² before acceptance/LOD.

The checked-in `field-current.ron` has 72 and also different species colors.
The HUD had a hard-coded “new 72-root field” label. It now displays the loaded
catalog density. Old screenshots retain that misleading label. Original reports
and evidence archives are preserved; this document is their density erratum.

The first new capture folder is still named `baseline72` because it was created
before decoding exposed the discrepancy. Its actual density is **66**.

## Direct draw attribution

Captured frame 600: camera (2.513, 1.730, -5.895), wind phase 5 seconds,
world 2592×1626, surface 3456×2168, fullscreen, Balanced, 4× MSAA, no prepass,
prepared terrain, current canopy, low-walk. Xcode 26.6 replay reports **Medium**
GPU performance state. The game completed capture after settling to 49 source
and prepared terrain pages. Startup briefly lost focus; it recovered before
capture. This is an offline workload diagnosis, not a live cadence measurement.

| Attributed work | Share of Xcode profiled replay work |
| --- | ---: |
| Main opaque pass, all objects | 75.64% |
| Grass pipeline within that pass | 64.88% |
| Grass vertex work | 42.12% |
| Grass fragment work | 22.76% |
| Far paired grass draw | 62.21% |
| Near paired grass draw | 2.67% |
| Terrain opaque pipeline | 10.34% |
| Other opaque pipelines together | about 0.43% |

The four grass draw calls map to single high/low, paired high/low in renderer
submission order. The two single-root draws are negligible in this field.
Percentages are rounded values transcribed from Xcode's Performance navigator.
They describe a single profiled replay; they are not frame deadline shares,
per-pass watts or a cross-hardware density budget.

The main encoder is 4.884 ms in this replay. Its vertex and fragment ALU limiter
readings are 70.58% and 83.28%, versus fragment texture-read limiter 17.61%.
These are overlapping limiter readings, not additive execution-time shares.
The main encoder has 813,506 vertex-shader invocations, 5,959,872 fragment-shader
invocations and average pixel overdraw 1.409. These counters include terrain and
objects; the finer grass percentages above come from the pipeline/draw navigator.

This supports investigating repeated vertex calculations. It does not support
assuming the whole main pass is grass, that only fragment overdraw matters, or
that scheduler work is the main opportunity.

## Isolated density variants

`tools/grass_density_variants.py` exports the catalog from a read-only project,
backs up the project into an experiment directory, changes only the target
population density, and cooks separate runtime databases. It first proves that
the unchanged baseline reproduces every logical runtime table. For candidates,
only the vegetation catalog and runtime metadata may differ. Original database
tables are verified unchanged. The RON reference is not used as the source.

Native diagnostics at 66, 72, 96 and 128 use matching fullscreen settings and
frame-based camera/wind. Screenshots at frames 600 and 900 are separate from
power timing. Three settled telemetry samples per variant show:

| Authored roots/m² | Emitted roots, observed range | Blade preparation fallbacks | Nominal scaled paired high-detail radius |
| --- | ---: | ---: | ---: |
| 66 | 113,697–113,962 | 96,322–96,852 | 5.78 m |
| 72 | 119,482–119,678 | 107,892–108,284 | 5.53 m |
| 96 | 156,127–156,589 | 181,182–182,106 | 4.79 m |
| 128 | 200,987–201,764 | 270,902–272,456 | 4.15 m |

Radius is derived from the current paired arena budget and 0.65 topology scale;
stable staggering, the focus ellipse and morphing still apply. The separate
12–26 m near-density region is unchanged. Thus these are density variants under
the existing adaptive geometry policy, not fixed-high-detail-distance comparisons.

All settled samples have focus, 49 source pages, nominal application thermal state
and zero instance capacity drops. Readback is sampled and asynchronous; the
variant samples occur at slightly different route poses. These ranges do not
certify every frame, thermal equilibrium or sustained 120 fps.

The 131,072-blade preparation arena fills. Overflow preserves rendering by
recomputing blade curves/root wind in the vertex shader. Increasing density
increases this repeated work. Raising arena capacity is a measurable tradeoff,
not an automatic win: it adds preparation work, storage reads/writes and memory.

Screenshots show increased coverage at higher densities without obvious holes.
They are retained for artistic review; numeric density alone does not establish
that the higher setting is worth its cost.

## GP-018 — larger preparation arena at 96 roots/m²

An explicit diagnostic argument, `--grass-prepared-blades 524288`, expands the
existing preparation arena. Default remains 131,072. No shader algorithm,
geometry topology, density retention, range or material quality was changed.
The argument is bounded and checked against the device's storage-buffer limit.

The matched pair uses the **same binary, database, canopy and shader hashes**,
frame 600, camera and wind phase. Both replay with Medium performance state.
The initial 66-root capture used an earlier binary before the new controls/HUD
label; use the matched 96-root pair for the optimization comparison.

| Xcode replay measurement | Default 131,072 | Candidate 524,288 |
| --- | ---: | ---: |
| Summed encoder time | 7.218 ms | 5.660 ms |
| Main opaque encoder | 5.343 ms | 3.747 ms |
| Blade preparation encoder | 0.086 ms | 0.162 ms |
| Main-pass vertex shader invocations | 1,115,304 | 1,115,304 |
| Main-pass vertex ALU instructions | 6.980 billion | 3.997 billion |
| Preparation ALU instructions | 0.124 billion | 0.276 billion |
| Main-pass fragment ALU instructions | 9.581 billion | 9.576 billion |
| Preparation arena plus dispatch | 19.25 MiB | 67.25 MiB |

Summed replay work falls 21.6%, the main encoder 29.9%, and its vertex ALU
instructions 42.7%. Unchanged small passes still vary, so do not project the
21.6% onto live FPS, watts or every camera. The independent instruction reduction
supports the mechanism: curves/root wind are calculated once per prepared blade
rather than repeatedly for fallback vertices. Main-pass vertex count is identical
(1,480,956), and fragment invocations differ by only about 0.09%.

The tradeoff is real: preparation time nearly doubles, main-pass device reads
rise about 22 MB, and preparation device writes rise about 22 MB in the exported
counters. A faster replay can still have a different energy outcome. The larger
arena is an experiment, not a new shipping default or a recommendation for iPhone.

Separate matched live counter runs show about 188–189k fallback blade forms
with the default arena and **zero** in the candidate samples. All sampled
instance-capacity drops are zero. Samples occur at different route poses;
do not subtract their emitted root totals as if they were the same frame.

The two screenshot comparisons exclude UI, comparing a 2500×1400 world region.
At frame 600, candidate/reference mean absolute RGB differences are about
0.003/255; an unchanged reference repeat is about 0.002/255. At frame 900 they
are about 0.54/255 versus 0.50/255 for the unchanged repeat. About 1.56% versus
1.47% of frame-900 pixels differ by more than 8 channel values. The frame-based
moving capture is therefore not bit-exact even against itself. Visual inspection
found no obvious changed silhouette or coverage, but this is not proof of
pixel identity or acceptance for all views.

The exact frozen candidate preset was checked with 20 seconds warmup plus
15 seconds measured, counters and capture off: **118.33 average updates/s**,
HUD interval p95 **8.34 ms**, nominal application thermal state, correct
2592×1626 world / 3456×2168 fullscreen surface and 70,516,748 preparation bytes.
There were 17 late updates and a 42.4 ms maximum update interval. No requested
setting/focus errors or target misses under the runner's tolerances. The report
is CHECK only because power telemetry was intentionally disabled. This checks
the command and workload; it is not perfect pacing or long-run acceptance.

## Next powered run

**Completed as GP-019, session `20260916-212946`.** The instructions below retain
the candidate protocol. [Sustained results](GRASS_DENSITY96_SUSTAINED_20260916.md):
119.600 fps overall, temporary thermal pressure and later brief stalls under
nominal pressure. The matching default-preparation 96-root control subsequently
completed as [GP-020](GRASS_PREPARATION_POWER_COMPARISON_20260916.md): both stall,
and enlarged preparation has no measured energy win in this pair. Keep it opt-in.

From the repository root:

```sh
python3 tools/grass_profile.py suite tools/profiles/grass-fullscreen-96-prepared-120.json
```

This is 30 seconds idle, 60 seconds warmup and **15 minutes measured** at 120 fps,
fullscreen, normal 75% world scale, 4× MSAA, Balanced, **96 roots/m²**, enlarged
preparation. It uses frozen inputs already saved under
`tmp/grass-draw-density-20260916/density96-prepared/inputs`; no rebuild or catalog
publication is required. Other linked assets must remain stable. Existing
powermetrics authentication is the same single local sudo step.

Run after the machine has settled from profiling. Keep the game focused as in
the earlier tests and note fan noise near the end. Return the report path.
Inspect first/last five-minute windows, thermal timeline, power/clocks and pacing.
This asks whether the denser candidate sustains 120 with acceptable heat/noise.
A precise power-saving claim will still require a matched default-arena repeat;
the earlier 66-root run is a different density.

The lower high-detail radius at 96 remains an explicit quality tradeoff of the
existing policy. Broader views/traversal, a fixed-near-detail comparison and native
1440p60 on named mainstream PC hardware remain open.

## Evidence and verification

[Preserved evidence](performance/20260916-density-preparation/README.md) includes
raw encoder CSVs, structured replay comparison, catalog export/variant manifests,
logs, exact commands, input hashes, counter and visual metrics, and scripts.
Native traces, binaries, databases and full screenshots remain local under
`tmp/grass-draw-density-20260916` and are listed with hashes. Do not remove that
directory while the remaining native timing diagnostics are open.

Checks: 16 app tests, 21 Python runner/report tests; real catalog recook/table
equivalence; native captures, four density runs, matched preparation/repeat
screenshots and candidate preset check. No production catalog was changed.
