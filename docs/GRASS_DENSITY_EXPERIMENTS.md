# Grass density experiments — 2026-09-10

Status: **selected checkpoint: `distance-01`, with the extended detail range accepted for continued grass work on September 13.** The earlier `curvature-01` sampling change was rejected as a fix for the visible LOD knees. The published `lod-width-01/thin20` catalog remains unchanged (see the September 13 follow-ups below).
The user found the first candidate
too sparse and too uniform in height. The September 10 candidate was
`budget_pairs_arches`: fuller paired arches, broader blade shoulders, less concentrated roots,
and **45 roots/m² instead of 60**. Its local coverage and GPU measurements justify keeping it
for review. This is an improvement over our baseline, not a claim that the reference look is solved.

On September 11, at the user's request, `length-05/layered_budget` was applied to the local project
catalog and cooked into runtime generation `0e5ea029c3d811d2` for in-game testing. Only
`short_split_fill` and its three referenced species were merged; other populations and assemblages
were preserved. This is still an uncommitted experiment, not an accepted performance baseline.
Pre-apply source/runtime databases, before/after catalog RON, and the temporary typed apply helper
are backed up in `.editor/vegetation/experiments/game-apply-length-05/`. The apply used the editor's
compare-and-swap catalog writer and the normal `yarra-world-cook demo` publication path; the
published catalog was read back and checked against the intended merge. Presets, captures, shader
snapshots and raw measurements remain local under `.editor/vegetation/experiments/`. Existing
unrelated work on grass shadows/performance documentation has been preserved.

## Portable checkpoint

The selected full catalog and close-camera setup are tracked in
[`content/vegetation/distance-01.ron`](../content/vegetation/distance-01.ron).
This preserves the 44-roots/m² population and its three weighted blade profiles; those authored
settings otherwise live only in the ignored project/runtime databases. The snapshot uses production
LOD, a 16 m field and wind disabled at time zero for static comparison. It has no local reference ID.

```sh
python3 tools/vegetation_study.py open --load content/vegetation/distance-01.ron
```

Loading restores the full catalog as an unsaved editor draft. Enable wind and press Play for motion
review. Use the inspector's Save & Publish to apply that catalog to a local project/game; this
replaces the full vegetation catalog, so review other authored populations before saving. The source
and runtime databases stay local. `sync-demo-vegetation` restores the older code-authored fixture
catalog, not this preset. Shader changes are shared by game and editor.

The following sections retain the experiment history. Capture paths, raw timings and before/after
shader snapshots under `.editor/` are local evidence, not files included with this checkpoint.

## What changed

- Ribbon taper uses `1 - t²` instead of `(1 - t)^0.72`. The same root width and pointed tip now
  retain more leaf area through the shoulder. No extra vertices or sections; no fractional power
  for the ribbon taper.
- The candidate's two rest-curve variants have more upright root tangents and shorter tip handles.
  Lateral variation is 0.08 and pair spread is 0.45 radians. Pairing extends through the existing
  0.69 m maximum height. Height/width bounds and LOD thresholds remain unchanged.
- Root attraction is 0.05, group spacing 0.6 m, shared orientation weight 1.5, radial weight 0.75,
  independent random weight 1.5. Root density is reduced by 25% to pay for the additional paired
  curves. Changing density/group spacing changes sampled root positions; the seed remains fixed.
- Ribbon wind rotates the whole resting cubic around its anchored root. Pitch is bounded to
  -0.10…0.18 radians, further restricted near the ground; yaw is bounded to ±0.07 radians.
  The control points and side axis receive the same rotation, preserving the centreline's length
  and curvature. The old per-vertex curl/bob/flutter path is skipped for ribbons. Broad leaves
  retain their previous wind path. This remains an experimental response, not a physical model.
- `--play` allows repeatable native wind playback without UI clicking. GPU tests explicitly enable
  the readback counters they assert on; these had become opt-in in the production renderer.

No added passes, buffers, textures, render resolution, MSAA, topology sections, or cache capacity.
The two shader changes currently affect all ribbon populations; the catalog experiments target
`short_split_fill`. Taller species still need artistic review before treating this as a default.

## Static coverage and work

Same 1280 × 720 target, MSAA off, Balanced density LOD, fixed seed/camera, wind off, hidden character.
Coverage uses the central 80% of the `top_down` neutral-ground view (pitch 89°, distance 4.7 m,
50° FOV): pixels differing by more than one RGB byte from ground `[46,46,38]`. It is a **screen
coverage proxy**, not physical leaf area, a reference-game measurement, or an occlusion metric.
Lighting/material settings are unchanged; the ground texture cannot hide gaps in this measurement.

| Experiment | Coverage | Vertex inputs | Submitted indices |
|---|---:|---:|---:|
| Original baseline | 36.29% | 65,646 | 167,826 |
| Root redistribution only, original taper | 36.74% | 64,404 | 164,730 |
| New arches + distribution, original taper | 41.22% | 63,702 | 163,884 |
| New arches + distribution + quadratic taper | 44.75% | 63,702 | 163,884 |
| All pairs at 60 roots/m², quadratic taper | 58.32% | 63,702 | 148,638 |
| **All pairs at 45 roots/m² — working candidate** | **50.07%** | **47,772** | **111,468** |

An alternative taller height distribution reached 47.40%, but submitted 169,482 indices, slightly
above baseline. It is not the selected candidate. The all-pairs 60/m² version looked denser but
was slightly slower in the animated low-view check, so it is also not the selected candidate.
`distributed_arches` remains a conservative alternative with fewer prepared curves.

| View, wind off | Baseline vertex inputs | Candidate vertex inputs | Baseline indices | Candidate indices |
|---|---:|---:|---:|---:|
| Overhead, 16 m field | 65,646 | 47,772 | 167,826 | 111,468 |
| Low, 64 m field | 417,370 | 249,102 | 890,754 | 340,014 |
| LOD, 128 m field | 1,151,038 | 690,750 | 2,341,062 | 782,454 |

All recorded captures have zero dropped instances. The preparation allocation remains 18,153,484
bytes. At 128 m both versions exceed the 131,072-blade preparation cache: baseline has 54,270
fallback blades and the candidate 83,894. This is extra fallback arithmetic, not discarded grass.
It is why fewer vertices alone was not sufficient evidence to accept the candidate.

## Animated performance smoke checks

Native local Metal HUD, whole editor, fixed camera/field/lighting, wind **playing**, debug editor
binary, no simultaneous GPU tests/builds. Each run lasts 25 seconds; the first five HUD packets
are excluded. Consecutive duplicate packets are removed; equal samples within a packet are retained.
Exact shader hashes, samples and logs are saved beside each result. These measurements are **not
isolated grass GPU busy time, iPhone results, or evidence of sustained thermal performance**.

| Run | Mean GPU duration | Median | p95 |
|---|---:|---:|---:|
| 64 m baseline, first run | 3.02 ms | 2.97 ms | 3.48 ms |
| 64 m baseline, repeat | 2.88 ms | 2.94 ms | 3.44 ms |
| 64 m conservative arches, bounded wind | 2.62 ms | 2.70 ms | 2.95 ms |
| 64 m all pairs, 60/m², bounded wind | 3.10 ms | 3.12 ms | 3.49 ms |
| **64 m candidate, 45/m², bounded wind** | **2.32 ms** | **2.58 ms** | **2.90 ms** |
| 128 m baseline | 5.45 ms | 5.48 ms | 5.96 ms |
| **128 m candidate** | **4.25 ms** | **4.00 ms** | **6.01 ms** |

The candidate improves local medians; the 128 m tail is essentially unchanged. This supports
continuing the experiment under the current budget, not promising a particular production speedup.

## Review and reproduction

Open the main candidate with animated wind:

```sh
python3 tools/vegetation_study.py open --no-build \
  --load .editor/vegetation/experiments/density-04/working-wind.ron --play
```

Use the reference picker to restore the supplied Yotei views and character scale. For a static
apples-to-apples comparison with our own baseline:

```sh
python3 tools/vegetation_study.py open --no-build \
  --load .editor/vegetation/experiments/density-04/comparison-overhead/study.ron
```

The baseline image is frozen original output; replaying a baseline RON alone uses the current
shaders and therefore does not reproduce the original image. Shader backups are in `density-01/shaders`
(original) and `density-04/shaders` (candidate). `density-02` captured the taper change before the
wind change. `measurements.json` contains the static measurements and their method.

`tools/grass_density_experiment.py prepare --baseline CAPTURE/study.ron --output NEW_DIRECTORY`
creates the catalog variants and archives the current shaders. Its `capture` action accepts
`--variants`, `--view`, and `--ground`; it rejects shader mismatches with the experiment manifest.
The generator targets the current short-grass catalog schema and fails if a target field is ambiguous.

For matched performance, close other GPU workloads and run sequentially:

```sh
python3 tools/grass_study_benchmark.py \
  --study .editor/vegetation/experiments/density-04/budget_pairs_arches-lod-meadow/study.ron \
  --shaders .editor/vegetation/experiments/density-04/shaders \
  --output .editor/vegetation/experiments/new-performance-run --seconds 25
```

This helper temporarily installs the supplied draw/blade shaders, runs only the editor process it
starts, enables wind playback, then restores the previous shaders even on normal error/interrupt.
Do not edit shaders or run another renderer concurrently with it.

Validation: 62 editor and 19 renderer tests pass. The explicit native
`prepared_blades_match_reference_with_wind_msaa_and_overflow` test also passes: zero byte differences
in all four prepared/reference image comparisons, including 4× MSAA and a deliberately tiny cache.
Native candidate captures at wind phases 0, 1 and 2 seconds retain identical workload counts and
show bounded movement of the same arches. Ordinary tests leave the four optional GPU tests ignored;
the preparation test above was run separately.

Remaining visual work: review the candidate in motion, inspect close blades and LOD transitions
while moving the camera, and tune any overly broad/plastic appearance separately. Strong highlight
streaks and visible gaps are still present. More occupancy by itself is not final artistic acceptance.

## September 11 — longer blades and varied canopy height

The user's close and low reference comparisons rejected 50% coverage as still too sparse and
the two similar arches as too uniform. This pass changes **catalog studies only**; the renderer,
wind shader and per-unit topology remain the same as the first experiment.

The first controlled suite held roots, widths, curves and sections fixed. Changing the length
distribution alone increased overhead coverage from 50.07% to 59.23%. Scaling blade extents by
1.35 reached 58.65%; combining that scale with the taller distribution reached 68.57%. These
are the same neutral-ground screen measurements as above, not estimates of Yotei's blade count.
The authored `height` parameter measures the root-to-tip extent before bending, not canopy height.
Long low sweeps can therefore cover gaps without making all the grass stand taller.

A wide upright/sweeping curve range (`length-02/varied_arcs`) reached 67.00%, but was rejected in
the low camera because too many blades rose into the foreground view. The restrained
`length-03/layered_arcs` reached 70.16%. It reduces height coherence from 0.58 to 0.20 and silhouette
coherence from 0.53 to 0.12: less averaging of independent root/group samples preserves a wider
distribution instead of pushing most samples toward the middle.

The main variety study uses the existing weighted species selection within **one population**:

| Share of roots | Form | Root-to-tip extent bounds |
|---|---|---:|
| 65% | Long low sweeps | 0.38–1.02 m |
| 25% | Middle arches | 0.30–0.85 m |
| 10% | Shorter, higher arches | 0.24–0.62 m |

Each root still emits a pair, sharing the same fixed vertex budget. This substitutes forms at
existing roots; it does not add three populations on top of each other. Each form has its own two
correlated curve variants. Colors and widths remain unchanged. Two extra profile records are
stored as catalog/GPU metadata; the instance and preparation allocations are unchanged. The
complete reproduction is `length-05/layered_budget.ron`.

Larger bounds slightly increased the number of roots visible at some camera edges, so the final
study reduces density from 45 to **44 roots/m²**. Measured vertex and index counts are now below
the previous candidate in all three fixed views. Final neutral-ground overhead coverage is
**69.72%**, compared with the previous candidate's 50.07%:

| Wind-off view | Previous vertices | New vertices | Previous indices | New indices |
|---|---:|---:|---:|---:|
| Overhead | 47,772 | 44,622 | 111,468 | 104,118 |
| Low, 64 m | 249,102 | 247,290 | 340,014 | 339,258 |
| LOD, 128 m | 690,750 | 685,950 | 782,454 | 779,526 |

There are zero dropped instances. At 128 m, fallback curves fall from 83,894 to 81,982;
the 18,153,484-byte preparation and 11,010,048-byte instance allocations remain unchanged.
Shader hashes are identical to the first candidate. `length-measurements.json` records the
captures, counts and coverage; the `length-*` directories preserve intermediate trials.

Fresh 25-second animated Metal HUD comparisons, with the existing review editor suspended and
resumed rather than closed, produced:

| View / candidate | Mean | Median | p95 |
|---|---:|---:|---:|
| 64 m previous | 2.81 ms | 2.79 ms | 3.25 ms |
| 64 m varied lengths | 2.62 ms | 2.70 ms | 3.10 ms |
| 128 m previous | 4.03 ms | 3.69 ms | 5.87 ms |
| 128 m varied lengths | 4.69 ms | 5.29 ms | 6.12 ms |
| 128 m single-profile layered arches | 4.77 ms | 4.57 ms | 6.44 ms |

The near check shows no slowdown. The large-field mean increases about 16% over the previous
candidate despite fewer vertices, and uses part of the earlier performance savings. It is below
the earlier original-baseline mean of 5.45 ms, but that was a separate session. **This is a visual
candidate, not an accepted performance improvement.** Coverage/fragment work and the profile mix
need to be considered before promoting it. These remain local whole-editor measurements, not
isolated grass timings or mobile/sustained performance.

Returning to one profile did not remove the far-field cost in the isolation run. This points
toward the increased covered/overlapping blade area as a concern; it does not isolate a GPU stage
or establish a precise cause. Keep the cheaper previous candidate available while evaluating LOD.

Review the new forms against Yotei with the character and bounded wind:

```sh
python3 tools/vegetation_study.py open --no-build \
  --load .editor/vegetation/experiments/length-05/working-wind.ron \
  --select-reference bottom_straight --character --play
```

The `review-top_down_close` and `review-bottom_straight` folders contain native comparisons at
wind phase 1. The older single-profile candidate remains reproducible under `density-04`.

Generate this suite from the first paired candidate with:

```sh
python3 tools/grass_density_experiment.py prepare --suite length \
  --baseline .editor/vegetation/experiments/density-04/budget_pairs_arches.ron \
  --output NEW_DIRECTORY
```

The benchmark helper now skips rewriting identical shaders and optionally accepts
`--pause-editor-pid PID`. It checks that the PID is this repository's debug editor, pauses it only
if it was running, and resumes it on exit. This keeps unsaved UI state out of the measured GPU
workload. All presets were loaded and validated by the native editor; no new renderer code or
topology tests were needed for this catalog-only pass. These measurements preceded the local
in-game publication described above; no changes have been committed or pushed.

## September 11 — thinner blades and the nearby topology band

The first in-game review found the blades too wide, a conspicuous nearby LOD ring, and a possible
performance regression. The next trial keeps the three-profile length/height mixture and 44 roots/m²,
but reduces each profile's half-width range by 20%, from 0.008–0.020 m to 0.0064–0.016 m.

Inspection found two actual width discontinuities at the high/low topology boundary. The low bin
applied density coverage compensation only after switching, and its partially retained density
fade band did not match the high bin's retirement width. Preparation now interpolates toward the
exact low-bin width and compensation using the existing morph. A new native GPU regression calls
the real `prepare_blade` shader on both sides of the boundary for 99 density/rank cases; it checks
effective widths and root/tip positions, including partially retained blades.

Drooping tips also made the low triangles lie almost on the soil, losing the height of the arch.
The trial lifts the simplified tip to at least the cubic midpoint's height along the surface normal,
and approaches that position through the existing morph. This is a representation approximation;
one low triangle still cannot reproduce the curved silhouette. No vertices or render units are added.

The high-detail boundary now uses an independent stable seed with a wider radius multiplier range
(0.50–1.20 instead of 0.84–1.12). It is independent of density retirement rank. Expected disk area is
0.763 of the authored budget disk rather than 0.967, spreading the transition while spending less
high geometry overall. Bin capacities, density thresholds, maximum source field, and buffer sizes
are unchanged. The visible band is softened in the native comparison captures, not claimed eliminated
under every camera, motion, or lighting condition.

Matched frozen study captures using the exact previous camera/field/wind settings:

| View | Previous vertex inputs | Revised vertex inputs | Previous indices | Revised indices |
|---|---:|---:|---:|---:|
| Top down, 16 m | 44,622 | 44,604 | 104,118 | 104,076 |
| Low, 64 m | 247,290 | 234,990 | 339,258 | 307,902 |
| LOD, 128 m | 685,950 | 676,110 | 779,526 | 751,158 |

All have zero dropped instances; preparation/instance allocations remain 18,153,484 / 11,010,048
bytes. The neutral-ground coverage proxy falls from 69.72% to **62.45%**, as expected with thinner
blades; it remains above the original 36.29% and first candidate's 50.07%. These percentages measure
one fixed screen region, not physical grass density or coverage in the reference game.

`tools/grass_game_benchmark.py` measures the actual release game using isolated shader copies,
an explicit read-only runtime database, and a repeatable camera path. It does not swap live assets
or overwrite the project. Runs use 1920×1080 world rendering, 75% scale, 4× MSAA, wind on, GPU counters
off, 3,600 frames, nominal reported thermal state, and no competing Yarra session. Five startup
Metal HUD packets are discarded. Logged GPU durations cover the whole game; they are not isolated
grass timings or phone/thermal qualification. Packet samples are correlated and do not establish a
statistical confidence interval.

First matched runs (mean / median / p95, milliseconds):

| View | Original catalog + original shaders | First in-game candidate | Revised width + LOD |
|---|---|---|---|
| Moving low view | 3.93 / 3.89 / 5.03 | 4.03 / 4.03 / 4.67 | 3.76 / 3.69 / 4.82 |
| Fixed overhead | 3.92 / 3.97 / 5.00 | 3.92 / 3.94 / 4.84 | 3.95 / 3.97 / 4.68 |

The first original low-view run overlapped a small CPU-only trial utility build during startup;
do not use its tiny difference from the first candidate as evidence of a regression. Overhead shows
no material mean difference. A second close-view pair measured 4.03 / 4.05 / 4.71 ms for the first
in-game candidate and 3.92 / 3.90 / 4.84 ms for the revision. Across both runs, revised mean GPU time
is 3.76–3.92 ms versus 4.03 ms; the close-view p95 is slightly worse, not improved. Treat this as a
modest mean-cost improvement, not a claim that frame-time tails or phone performance are solved.
The earlier 128 m editor regression remains valid for that separate workload;
fewer vertices alone never establishes equal GPU cost, because blade coverage and overlap also change.

Native shader parsing and all 19 default renderer tests pass. The 99-case LOD GPU test passes.
Prepared-versus-fallback rendering also passes wind, MSAA, and forced-overflow cases (maximum byte
difference 1, mean at most 0.000004). Captures, shader hashes, benchmark logs, before/after catalog RON,
and geometry/coverage measurements are under `.editor/vegetation/experiments/lod-width-01/`.

The revised catalog was saved through the compare-and-swap writer and locally published as
generation `7e8831432854a354` with the normal cooker for another game review. Source/runtime snapshots immediately before this application
are `lod-width-01/project-before.sqlite` and `runtime-before.sqlite`. The original pre-experiment
snapshots also remain under `game-apply-length-05/`. Everything remains uncommitted and unpushed.


## September 12: curved paired LOD experiment (`geometry-02`)

The user rejected the previous visible ring, coarse long-blade curvature, and overhead LOD.
This trial changes the representation instead of extending the high-detail disk. It is still
**uncommitted and awaiting visual review**; the overhead field still lacks the reference density.
The project/runtime catalog remains generation `7e8831432854a354` (44 roots/m², thin20 widths).
No density slider, render resolution, MSAA, arena capacity, or source-field size was increased.

### Geometry and transition

- High pairs use **five segments on the main blade and three on the companion**, with shared
  pointed tips: **18 unique vertex inputs, 42 indices**, exactly the prior pair budget.
  The companion is 80% of the sampled length and its control handles blend 75% toward the authored
  curvature from the straight chord. This is a global paired-ribbon experiment, not a catalog
  setting yet. The main spends three segments before the low shoulder and two after it; the
  editor's curve markers show that actual sampling. For the two dominant rest-curve endpoints,
  the largest centreline turn decreases from about 29°/25° to 23°/24° (without wind or morph).
- Low pairs use a bent main kite plus a companion triangle: **7 vertex inputs, 9 indices** versus
  the previous 6/6. Production low retention and its fade band are multiplied by **0.65**, paying
  for the added triangle: expected low index work is 97.5% of the old value, low vertex work 75.8%.
  Full-density reference mode remains a diagnostic ceiling and does not receive that reduction.
- The low kite preserves the ribbon taper's integrated width (2/3 of root width); density-budget
  compensation restores approximate leaf area from the removed roots. These are coverage
  approximations, not guarantees of view-dependent pixel coverage. Low blades become wider.
- The high vertices now move **onto the actual low triangles**, rather than sliding samples along
  the cubic and bunching them together. Normals, cross-section coordinates and ribbon-side vectors
  converge to the low mesh too. Both paired blades include their low mesh anchors exactly.
- Low shoulder/root width vectors reuse spare fields in the existing **128-byte prepared blade**.
  Full-detail blades skip the extra morph work. Broad leaves keep their legacy per-blade sample
  limits and low triangle mapping; the shared paired low bin still uses the reduced retention.
- High-detail radius, its stable per-root staggering, population spacing thresholds, wind response,
  and authored widths are unchanged. Horizontal reach was already included in projected extent;
  overhead failure was not caused by projecting vertical height alone.

The first six/two-segment trial was rejected locally: the two-segment companion remained angular,
close coverage fell to 56.4%, and the low kite lost too much area. Its captures/shaders remain in
`trial-*` for diagnosis. `revised-*` tested five/three segments before moving the extra sample into
its tighter root bend. `final-shaders` is the current version. `optimized-shaders` tried skipping redundant low-vertex width/opening and normal interpolation work. Its rendered LOD-boundary tests passed, but timings did not improve reliably, so that optional optimization was reverted.

### Static work and coverage

Same studies/cameras as the September 11 measurements, 1280×720, wind off, Balanced density.

| View | Before vertex inputs | Trial vertex inputs | Before indices | Trial indices |
|---|---:|---:|---:|---:|
| Close top, 16 m | 44,604 | 44,604 | 104,076 | 104,076 |
| Low, 64 m | 234,990 | 190,960 | 307,902 | 302,808 |
| LOD, 128 m | 676,110 | 527,015 | 751,158 | 736,557 |

Zero capacity drops in these captures. Instance allocation stays **11,010,048 bytes**; preparation
allocation stays **18,153,484 bytes**. At 128 m, fallback preparation decreases from 81,790 blades
to 9,676. Close top-down coverage is **59.34% versus 62.45% before**, using the same neutral-ground central-80% metric. The shorter companion trades some coverage for shape quality. Counts do not measure shading/overdraw cost; gameplay timings are recorded separately.

### Verification and reproduction

`paired_lod_boundary_matches_rendered_shape_and_lighting` draws the actual high mesh at morph zero
and the actual low mesh at identical roots, testing low/overhead views with animated wind and both
full/fading density. Mean differences are below 0.003 of one 8-bit channel value; overhead cases
have no channel differences above two. The existing 99-case preparation test also passes. This
checks the **discrete bin boundary**, not perceptual invisibility of the whole morph annulus.

Cached versus uncached preparation passes, including 4×MSAA and forced cache overflow. One thin
silhouette pixel differed by up to 19 byte levels with MSAA, touching clear pixels on both sides
(mean 0.00013); compute/vertex arithmetic can cross a multisample coverage boundary. The test now
permits at most four such clear-adjacent pixels, with mean below 0.001 and max below 33, only in
MSAA cases. Interior and non-MSAA checks remain strict. Candidate-cache/source-lifetime tests pass.
The CPU suites contain 62 editor and 19 renderer tests.

The previous executable and shaders are archived as `game-before` and `before-shaders`, alongside
`runtime-before.sqlite`. Do not mix that executable with the new index-count shaders.
`tools/grass_game_benchmark.py` now accepts `--binary` and `--view grass-top-down` so each side uses
its matching geometry ABI without replacing live assets. Final native captures and measurements
are under `.editor/vegetation/experiments/geometry-02/`.


### Gameplay GPU checks

Native release game, isolated shader assets/database, 3600 frames per run, `low-walk` and
`grass-top-down`, wind on, 1920×1080 3D target, 75% resolution setting, 4×MSAA, counters off.
Whole-game Metal HUD durations after discarding five startup packets, not grass-pass timings.
Runs were sequential, without simultaneous builds, editor captures, or GPU tests.

| Build/view | Mean GPU ms | Median ms | p95 ms |
|---|---:|---:|---:|
| Before, top | 4.41 | 4.54 | 5.13 |
| Before, top repeat | 4.58 | 4.63 | 5.47 |
| Final, top | 4.29 | 4.40 | 5.60 |
| Before, low | 4.24 | 4.10 | 6.03 |
| Final, low | 4.43 | 4.45 | 5.38 |

The earlier five/three sample arrangement (`revised-*`) measured 4.27 ms overhead and 4.12 ms low.
Final overhead is lower on average but has a slightly worse tail; final low is **about 4.5% slower
on average** in this run despite fewer vertices. HUD reporting frequency and total run wall time
varied between runs (final top has 1,568 post-startup samples; final low 3,920). These desktop smoke
checks do **not** establish a performance win or an accepted regression-free/mobile baseline.
The extra morph arithmetic and wider low geometry remain costs to audit. Geometry/coverage trade-offs
must be judged in motion before accepting this experiment; the user requested keeping it uncommitted.


A subsequent arithmetic trial (`optimized-*`) measured 4.45/4.56/5.33 ms mean/median/p95 at low
angle and 4.49/4.18/7.62 ms overhead. It did not establish an improvement, and its overhead tail
was much worse. That trial was archived and **reverted to `final-shaders`**. No catalog changes or
commits were made. The selected geometry experiment's roughly 4.5% slower low-view average is
still unresolved; it is available for visual testing, not accepted as the performance baseline.

## Shape diagnosis — September 13 (no new production tuning)

The next step is measurement, not another LOD-radius or density change. The study now has a
`Shape diagnosis` selector with production, current, full, low, morph-weight and cause views.
The comparison retains the production candidate acceptance and the original packed morph,
seed, species and density target. All active inspection views draw the retained instances through
the high bins; only the displayed geometry morph changes. This deliberately costs more in the
study and is limited to a 4 m or 16 m field. It is not a production performance proposal.
The normal game defaults to inspection off. No production index ranges, bin sizes, allocations,
curve parameters, densities or LOD thresholds were changed for this diagnostic work.

`Full shape` means the current authored high mesh (5 sections on the main ribbon, 3 on its
companion), not a smooth analytic reference. `Low shape` morphs this high mesh to the existing
low endpoint. Density fades and density width compensation still use the original production
morph. View opening can be disabled independently without editing the catalog. Cause colors:
green means morph >= .999, orange means the budget bound is smaller, blue means projected size
is smaller. The per-instance cause uses the limits evaluated during actual GPU generation; it
is not inferred from the truncated instance seed.

First frozen comparison: `.editor/vegetation/experiments/shape-diagnosis/comparison.html`.
Inputs are the previously captured geometry-02 catalog, 16 m field, seed 1513603485, no wind,
1280 × 720, no MSAA, default close game camera (4 m distance, 10° pitch, .9 m target, 45° FOV).
This is a controlled approximation of the user's screenshot, not its exact saved camera pose.

| View | Retained units | Vertex inputs | Indices | Capacity drops |
| --- | ---: | ---: | ---: | ---: |
| Production draw | 4,234 | 61,054 | 132,354 | 0 |
| Each shape inspection | 4,234 | 76,212 | 177,828 | 0 |

Observations from the actual captures:

- Corners remain in the foreground with full shape forced. The high mesh itself inadequately
  approximates some of these strongly curved profiles; moving the LOD boundary cannot fix that.
- Current versus full differs farther into the field. The cause view is predominantly orange
  where shape is being reduced, confirming the budget bound's role in that region.
- Disabling view opening materially reduces apparent leaf area and changes the silhouettes.
  Corners remain. These images do not prove or disprove a per-row opening flip; do not report
  that hypothesis as an established bug.
- Current-shape and actual production images are close: mean byte difference 0.004240/255,
  0.0119% of RGB channels differ by more than 2. They are not pixel identical: high-versus-low
  subdivision, compaction order and raster edge cases remain. Do not call this exact raster parity.

Validation: native GPU test `shape_inspection_preserves_production_population_and_density`
compares sorted complete instance records against production, exercises high and low source
bins, all five inspection modes, preparation/fallback, opening isolation, and return to production.
It passes with zero capacity drops. Both native LOD-boundary tests pass, including 99 prepared
width/endpoint cases. Editor tests (62) and renderer CPU tests (19) pass.

The next geometry experiment should first compare how the existing high-detail samples are
placed on the cubic, and what curvature that fixed 5/3 split can represent acceptably. A smooth
curve/edge error measurement for selected blades is still needed; these field images are not
that measurement. After establishing an acceptable high shape, test a low approximation against
it before adjusting its admission policy. Keep geometry/vertex counts fixed, and measure shader
cost as well as counts. No frame-time performance claim is made by this diagnostic capture.

Top-down follow-up: `.editor/vegetation/experiments/shape-diagnosis/top/comparison.html`,
using the actual CLI capture tool end to end. The top preset is 89° pitch, 4.7 m distance,
50° FOV; all seven views retain 2,478 units, 44,604 vertex inputs and 104,076 indices,
with no drops. Current versus full has mean byte difference 0.013377/255; only 0.08793%
of RGB channels differ by more than 2. The visible field is overwhelmingly full-shape green.
Thus, at this particular top-down setup, forcing high shape cannot explain away the remaining
appearance problem as a low-bin switch. This is not a claim about every gameplay overhead pose.
The measurements are image differences, not geometric-error or quality scores.

The native editor captures and report input validation were checked. HTML JavaScript passed a
syntax check; the embedded browser could not preview the local HTML because its file-URL policy
blocked navigation. No alternate browser route was attempted. Open the local report directly
for linked zoom/pan. A shader snapshot and image-difference JSON accompany the captures.


## September 13 — fixed-budget curve sampling (`curvature-01`)

The full-shape diagnostic showed that the high mesh itself was visibly angular. This experiment
reallocates the existing paired-ribbon mesh rather than changing density, authored curves, wind,
LOD thresholds, retention, buffers or preparation capacity. The local project/runtime catalogs
remain untouched; runtime generation is still `7e8831432854a354`.

- Before: five main sections and three companion sections, each with a wide root.
- After: five main sections and four companion sections, both with a pointed root. The two
  saved root vertices buy one extra width row on the companion. Both arrangements use exactly
  **18 distinct indexed vertex inputs and 14 triangles per high pair**. The low pair remains
  seven vertex inputs / three triangles.
- Main parameters before the authored power are `[0, .128, .292, .5, .768, 1]`; companion
  parameters are `[0, .183, .423, .723, 1]`. The main's `.5` shoulder still matches the existing
  low kite. These schedules were evaluated against the three current grass profiles; they are
  not a universal optimum for arbitrary authored curves.
- The companion's first width row moves down to form the low triangle's base as its shape
  simplifies. Its extra root triangle degenerates. Position, color, AO and rounded-side
  interpolation use the same remapped parameter at the boundary. Preparation reuses an unused
  ribbon flutter-phase word for that first-row parameter; no record grows.
- Broad leaves share the static index template. Their vertex mapping preserves the former
  nondegenerate triangles and tip-side attributes across all valid authored section counts.
  Single ribbons retain their original topology and width profile.

`tools/grass_curve_sampling.py` compares the intended cubic centerlines against the polygonal
samples for 27 normalized rest curves: three species and nine variant blends. The companion's
existing 75% curvature blend is included. Distances are normalized by each blade's own height;
this does not measure wind, lateral curvature, opened ribbon edges or screen-space appearance.

| Maximum centerline error per curve | Before median | After median | Before worst | After worst |
| --- | ---: | ---: | ---: | ---: |
| Main | .013842 | .012300 | .016912 | .014808 |
| Companion | .030421 | .012750 | .038488 | .013941 |

The median error falls about 11% on the main and 58% on the companion. The visual improvement
is modest across the full field, clearest on companion bends. Corners remain where production
LOD has already simplified the blades; this experiment does not solve that remaining shape loss.

### Coverage tradeoff

Pointed roots with the previous quadratic width profile exposed too much ground. The selected
pair uses `1 - t³` to retain width farther along the body, without exceeding the existing maximum
half-width. This recovers coverage without extra roots or geometry. A quartic trial restored more
area than necessary and was not selected. Single ribbons keep `1 - t²`; broad leaves are unchanged.

All masks use the same saved camera, catalog, roots, wind-off state, 1280×720 target, no MSAA,
no character, and forced full shape. Grass is unlit magenta; coverage counts pixels with
R > 180, G < 40 and B > 180, independent of the textured ground. This measures visible coverage,
not physical leaf area, overdraw or reference-game blade counts.

| Mask | Before | Pointed / quadratic | Selected / cubic | Rejected / quartic |
| --- | ---: | ---: | ---: | ---: |
| Close view, lower half | 78.82% | 74.43% | 78.13% | 80.01% |
| Top-down, whole image | 54.65% | 50.34% | 54.73% | 57.02% |

The linked report `.editor/vegetation/experiments/curvature-01/comparison.html` contains twelve
captures: before/after for close/top, current/full/production. Each before/after pair has matching
saved inputs and identical emitted counts, vertex inputs and indices, with zero capacity drops.
Full/current inspection is intentionally more expensive than production; do not use those counts
as production timings. `baseline-shaders`, `renderer-before.rs`, `editor-before`, `game-before`
and `runtime-before.sqlite` preserve the start-of-turn version; `final-shaders` is the selected
experiment. Old captures cannot be reproduced by loading their RON through the new binary alone.

Validation: 62 editor and 20 renderer CPU tests pass. Five explicitly run native preparation/cache/
LOD/inspection tests pass, including 99 width/endpoint cases, animated wind, 4×MSAA and forced
preparation overflow. LOD raster-boundary mean byte errors remain below .003; cached/reference
MSAA retains the existing sparse coverage-edge difference (max 19, mean .000130), without relaxing
thresholds. `git diff --check` and targeted Rust formatting pass. These measurements preceded the September 13 checkpoint request.

### Performance of the selected curve experiment

Native release game, same runtime database, wind on, 3600 frames per run, 1920×1080 3D target,
75% setting and 4×MSAA. Whole-game Metal HUD, not an isolated grass pass or mobile result.
Each run contains 6,664 samples after removing the first five HUD packets. Runs are sequential
with no simultaneous editor, build or GPU test. Both views were repeated in reverse A/B order
because the first final runs had slower tail frames.

| Run | Mean GPU ms | Median ms | p95 ms |
| --- | ---: | ---: | ---: |
| before-low | 4.317 | 4.35 | 5.19 |
| before-low-repeat | 4.091 | 4.11 | 5.50 |
| final-low | 4.212 | 4.21 | 5.61 |
| final-low-repeat | 4.082 | 4.12 | 5.64 |
| before-top | 4.220 | 4.30 | 5.22 |
| before-top-repeat | 4.243 | 4.35 | 5.41 |
| final-top | 4.308 | 4.44 | 5.73 |
| final-top-repeat | 4.365 | 4.49 | 5.34 |

Low-view means are approximately unchanged on the repeat (4.091 → 4.082 ms). Top-down is
consistently about 0.09–0.12 ms slower (roughly 2–3%). Tail timings vary by run; the reversed
top-down pair does not retain the earlier p95 regression. This is a small **measured top-down
cost**, despite unchanged vertex counts, not a performance win or a regression-free acceptance.
The smoother companion and restored coverage are available for review; keep the trial uncommitted.

Open the native before/after comparison, with actual production LOD:

```sh
python3 tools/vegetation_study.py open --no-build \
  --load .editor/vegetation/experiments/curvature-01/final-close-production/study.ron \
  --reference .editor/vegetation/experiments/curvature-01/Before_curve_sampling.png
```

The release game and debug editor have been rebuilt with the new index layout. The current
shader files are live in both applications; no catalog republish is needed. Rebuild other game
configurations before testing, since an older executable has a different high-pair index layout.


### Visual rejection and corrected metric — September 13

The user's `Screenshot 2026-09-13 at 15.57.48.png` still shows the original sharp-knee problem.
The companion centerline-distance improvement was not an adequate acceptance criterion. Treat
`curvature-01` as an unsuccessful fix for this issue, not a promoted improvement. Its code remains
available for reproduction; this follow-up changes diagnosis only, without another rendering tweak.

The longer blade's draw path explicitly morphs every high sample onto one of two straight lines:
root → shoulder and shoulder → tip. Their joined shoulder becomes a hard knee even while the
high-topology bin is still drawing the blade. At the fully simplified endpoint, reallocating the
high samples cannot change that knee. The offline tool now records the maximum direction change
between adjacent main-blade segments through the same morph, using the same 27 rest curves.

| Main geometry morph (1 = full) | Old worst turn | New worst turn |
| --- | ---: | ---: |
| 1.00 | 28.81° | 29.46° |
| .75 | 28.64° | 30.98° |
| .50 | 36.88° | 38.39° |
| .25 | 44.87° | 45.59° |
| .00 | 52.52° | 52.52° |

These are rest-space centerline angles, not measurements of the marked screenshot. A 52.52°
direction change is a 127.48° interior angle. Perspective and ribbon edge opening can change
its screen appearance. The latest sample schedule slightly worsens this metric through much
of the morph, despite lowering centerline-distance error at full detail.

For the isolated 44-roots/m² study, the high budget radius is
`sqrt(32768 × .5 / (π × 44)) = 10.887 m`. Per-root staggering chooses .5–1.2 of that radius,
and morphing starts at .68 of the chosen radius. The earliest onset is therefore **3.702 m
from the camera on the ground plane**. With the character roughly 4 m from the camera, nearby
roots can already be morphing. This is the study calculation, not telemetry of the exact game
screenshot; the game's maximum overlapping population density can change its budget radius.

The next representation experiment must evaluate visibly resolved main-blade corner angles
through the production morph, including the low endpoint. High-detail sampling alone does not
address the two-segment target. Any alternative must keep the overall geometry budget, and must
be judged at the user's close camera before accepting a numerical centerline improvement.


## September 13 — extend detail distance (`distance-01`)

The user explicitly requested moving the transition farther away and accepting the initial
performance cost, then finding other savings if needed. This changes the range policy only:
per-blade topology, density settings, authored curves, screen-size thresholds, wind, buffers and
runtime catalog are unchanged. The previous sampling experiment remains the same on both sides
of this distance comparison; this does not independently fix full-detail blade corners.

The per-root boundary multiplier changes from `.50–1.20` to `1.10–1.30` of the existing budget
radius. Each root starts morphing at `.90` of its boundary instead of `.68`. At the current
44 roots/m² study density, the base budget radius is 10.887 m:

| Distance rule | Before | After |
| --- | ---: | ---: |
| Earliest budget-driven curvature reduction | 3.70 m | 10.78 m |
| Range of per-root morph start distances | 3.70–8.88 m | 10.78–12.74 m |
| Range of per-root low-topology boundaries | 5.44–13.06 m | 11.98–14.15 m |

These are ground-plane distances from the camera, not distances from the character. At the
four-metre third-person setting the earliest budget transition is roughly seven metres beyond
the character. The screen-size bound remains active; sufficiently small projected blades can
simplify earlier. Overlapping population density also changes the CPU-derived budget radius.
The native cause capture confirms that the close foreground is full shape after this change.

The existing 50% CPU budget factor and maximum 1.30 boundary multiplier imply an outer-disk
root estimate of 84.5% of the high-bin capacity, before root-placement variance. The instance
arena is still 11,010,048 bytes, preparation arena 18,153,484 bytes. No capacities grow. More
roots retain high topology and avoid low-density retirement, so actual geometry work increases.

### Static work checks

Matched saved catalog/camera, wind off, 1280×720, no MSAA. Close is a 16 m patch; wide and overhead
are 64 m. The overhead study uses an 18 m orbit distance. Actual production LOD is enabled.

| View | Vertex inputs before → after | Indices before → after | High pairs before → after |
| --- | ---: | ---: | ---: |
| close | 61,054 → 89,131 | 132,354 → 207,129 | 2,856 → 4,907 |
| wide | 206,576 → 240,250 | 319,680 → 408,930 | 2,868 → 5,305 |
| overhead | 264,886 → 365,448 | 535,758 → 806,160 | 10,351 → 17,834 |

All eight before/after captures (including cause views) have zero capacity drops. The production
captures also have zero preparation fallbacks. `distance-01/comparison.html` contains the linked
before/after images; `workload.json` contains the counters. Exact shader snapshots and a copy of
the unchanged runtime database are in the same local experiment directory.

Validation: 20 renderer CPU tests and five native GPU tests pass, including placement/cache
agreement, shape inspection, wind, MSAA, overflow and both LOD boundary checks. Raster-boundary
thresholds were unchanged. The release game is rebuilt. The shader change is shared by game and
editor and requires no catalog republish. `--render-repro grass-close` and the matching benchmark
option reproduce the minimum-distance game camera at four metres, 10° pitch, .9 m focus height.
The camera rig's normal HUD label does not track the render-audit pose override; audit logs contain
the actual camera transform. This is a fixed camera benchmark, not a walking/streaming route.

### Matched whole-game performance

Native release game on the local Mac, 3600 frames per run, wind enabled, 1920×1080 3D target at
75% resolution setting, 4×MSAA, GPU readback counters off. Identical executable and database
on both sides; isolated shader directories. Close runs animate wind using the deterministic frame
clock; top-down holds wind at phase zero. Close runs are before/after; top runs are after/before.
All runs have 6,664 Metal HUD samples after removing five startup packets. No other Yarra app,
build or GPU test ran concurrently. These are whole-game timings, not isolated grass GPU time.

| View / version | Mean GPU ms | Median ms | p95 ms |
| --- | ---: | ---: | ---: |
| grass-close / before | 3.254 | 3.18 | 4.86 |
| grass-close / after | 3.484 | 3.34 | 5.02 |
| grass-top-down / before | 3.371 | 3.09 | 4.92 |
| grass-top-down / after | 3.637 | 3.56 | 5.09 |

The close mean increases by 0.230 ms (7.1%); top-down by 0.267 ms (7.9%). This is a measured
cost of retaining detail farther away, not a performance improvement. The user explicitly
prioritized extending the range and measuring the cost, so the increased range remains active
for play testing. Future optimizations must not silently restore the old near-field collapse.
These measurements preceded the September 13 checkpoint request.
