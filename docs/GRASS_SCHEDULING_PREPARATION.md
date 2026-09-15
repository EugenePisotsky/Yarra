# Grass scheduling and preparation — reverted September 15, 2026

**Status: reverted at the user's request.** The roughly 0.06 ms/frame saving did not justify the
extra queue memory, compute pipeline and scheduling logic. Preparation priority gave no demonstrated
benefit to the current all-paired field. The renderer uses the previous field-list scheduler and
preparation order again. The experiment-only tests were removed and the previous scheduler regression
was restored. Earlier shading early exits from `GRASS_SHADING_PERFORMANCE.md` remain in place.

Revert verification: the five affected source files match the pre-experiment snapshots exactly;
32 existing renderer unit tests and the release game build pass. Logs are archived as
`revert-unit-tests.log` and `revert-release-build.log` in the experiment directory below.

The results below describe the archived experiment, not the active renderer. Keep them as evidence
against repeating this approach without a materially different workload or expected payoff. Existing
draw-cost attribution is recorded in [RENDER_AUDIT.md](RENDER_AUDIT.md#physical-a17-pro-close-view-captures-2026-09-05)
and summarized in [PERFORMANCE_HANDOFF.md](PERFORMANCE_HANDOFF.md#what-the-evidence-establishes).

The combined changes reduced whole-game GPU time by about **1.3%** in the repeated low-walk runs
on this Mac. Compact scheduling removed **67–69%** of launched candidate slots in two fixed
views, with identical instance counts, at a cost of roughly **1 MiB** of additional queue memory.
Preparation priority is verified for mixed single/split grass; no standalone timing gain was
established in the current field, which emits only paired blades. Zoom timings were inconclusive.

## Archived implementation

### Compact candidate scheduling

The previous indirect launch was a rectangle: the largest visible field's candidate count in X,
with one row per visible field. Every smaller field paid for the unused tail of that row.
The scheduler now writes one eight-byte record per 64-candidate chunk, containing the field index
(and existing quarter-LOD flag) and candidate offset. One workgroup per source field handles its
visibility test and cooperatively writes the chunks. A final one-thread dispatch sets the indirect
launch dimensions. The generator reads each chunk and evaluates its candidates exactly as before.

The queue uses an eight-byte header and eight bytes per chunk, sized for all resident full-lattice
candidates when source data changes. Its allocation is retained across frames. Only the header
is cleared on generation; retired records are excluded using the live source count and queue count.
Large queues use a two-dimensional launch within the 65,535 workgroups/dimension limit. With the
normal 256-group row width, only the final row adds padding (at most 255 workgroups).

Field frustum/range culling, per-candidate culling, candidate acceptance caching, density rules,
LOD transitions, and stationary-camera generation reuse remain in effect. Balanced and Full
Reference still visit the full lattice. No CPU readback controls dispatch. Pipeline hot reload
invalidates the reuse key, including the new schedule-finalization pipeline.

The tradeoff is a larger scheduling buffer and one extra dispatch, in exchange for fewer empty
candidate workgroups. This is most relevant while the camera moves across unequal field sizes;
stationary frames already reuse generation. The sampled candidate-lane counter now reports the
actual tiled launch, including its padding.

### High-detail preparation priority

The preparation order changes from single-high, single-low, split-high, split-low to single-high,
split-high, single-low, split-low. Both high-detail bins get cached curves before low-detail work.
Allocation prefixes advance by the blades that actually fit. This lets a single blade use a spare
slot when a split pair cannot fit. Every unprepared instance receives a zero lookup index and
uses the existing vertex-shader calculation.

The 131,072-physical-blade cache budget, its 128-byte records, instance capacity, geometry, material,
wind and density settings are unchanged. Preparation still writes the lookup for every live
instance, including overflow, so a previous frame's cached index cannot leak into the fallback.

## Verification before revert

- Renderer unit suite: 32 passed; 12 native/offline tests are opt-in.
- Native scheduler regression verifies exact chunk identities without duplicates; unequal and
  zero-sized fields; invisible and retired fields; scene shrinkage; counter-free dispatch;
  authored quarter-LOD encoding; and two-dimensional launches. Allocations are reused between cases.
- An independent full-lattice GPU launch produces identical instance multisets to compact scheduling
  in Balanced, Full Reference and Authored modes, with caching on/off and a source shrink.
  The fixture emits respectively `[1167, 152, 267, 1640]`, `[1167, 191, 267, 3239]`, and
  `[871, 40, 247, 429]` instances in the four bins, with zero capacity drops.
- Native preparation readback checks high-bin priority, nonoverlapping/in-bounds allocations,
  odd capacities, telemetry and fallback indices seeded with invalid stale values.
  At capacity 1,701 it prepares `[1167, 0, 534, 0]` physical blades by bin; at 1,168 it prepares
  `[1167, 1, 0, 0]`, demonstrating the spare single slot.
- The existing frozen prepared/procedural comparison passes with wind, MSAA off/4× and forced
  three-blade overflow. Its existing tolerance is unchanged: maximum byte errors 1, 1, 22, 0,
  with mean errors no greater than 0.000175 (the larger error is a sparse MSAA edge).

## Benchmark method

Artifacts are archived under `.editor/vegetation/experiments/scheduling-preparation-01/`.
Before/after shader directories and release binaries preserve both scheduling ABIs. Preparation-only
runs use the original binary with only the preparation shader updated; scheduling-only runs use the
new binary and queue shaders with the original preparation shader. The first shading pass is present
in every variant. The runtime database and canopy settings are fixed and archived.

Timing uses `tools/grass_game_benchmark.py`, 2,400 frames per run, Balanced density, 1920×1080 world
rendering and 4× MSAA. Runs are sequential, without overlapping builds, GPU tests or another Yarra
process. Whole-game Metal HUD duration includes all rendering; it is not an isolated grass-pass
measurement or evidence of iPhone performance. The first five HUD packets are discarded.

Low-walk order is baseline, preparation, combined, scheduling, scheduling, combined, preparation,
baseline. The separate moving/zooming view uses combined, baseline, baseline, combined. These repeated
visits help expose drift, but do not remove thermal or clock variation.

## Timing results

### Low-walk: all sampled thermal states Nominal

| Order | Variant | Mean GPU ms | P95 ms |
|---:|---|---:|---:|
| 1 | baseline | 4.6837 | 5.29 |
| 2 | preparation | 4.7263 | 5.34 |
| 3 | combined | 4.6358 | 5.18 |
| 4 | schedule | 4.6590 | 5.22 |
| 5 | schedule | 4.6212 | 5.19 |
| 6 | combined | 4.6522 | 5.21 |
| 7 | preparation | 4.7105 | 5.25 |
| 8 | baseline | 4.7266 | 5.31 |

Mean of run means:

| Variant | GPU ms | Change from baseline |
|---|---:|---:|
| baseline | 4.7052 | +0.0000 ms |
| schedule | 4.6401 | -0.0651 ms |
| preparation | 4.7184 | +0.0132 ms |
| combined | 4.6440 | -0.0611 ms |

Combined: **4.7052 → 4.6440 ms**, a **0.0611 ms / 1.30%** reduction. Both combined visits and
both scheduling-only visits are below both baseline visits. This is a small whole-game result,
not a 67–69% frame-time improvement: removed dispatch slots were mostly cheap early exits, while
actual candidate evaluation, geometry and shading remain unchanged. Preparation alone varies
within the spread of the baseline visits; it does not demonstrate a speedup.

### Zoom: rejected as a reliable timing comparison

| Order | Variant | Mean GPU ms | Logged thermal states |
|---:|---|---:|---|
| 1 | combined | 5.1390 | fair, nominal |
| 2 | baseline | 5.9467 | fair |
| 3 | baseline | 7.6053 | fair |
| 4 | combined | 7.8544 | fair |

The same variant drifts by more than a millisecond between visits. Thermal state changes to Fair,
and these runs cannot separate the implementation from changing device conditions. Do not average
this series into a performance claim. A cooled, controlled repeat or target-device profiling is
needed before claiming a zoom-view gain or regression.

## Game workload counters

Separate 1,200-frame, fixed-camera captures enable the renderer's GPU counters. Counts remain
stable after streaming settles; these counter-enabled runs are not used for timing comparisons.

| View | Visible fields (both) | Candidate slots before | Candidate slots after | Reduction | Instances (both) |
|---|---:|---:|---:|---:|---:|
| grass-close | 60 | 4,120,320 | 1,294,336 | 68.59% | 114,191 |
| grass-overhead | 44 | 3,021,568 | 999,424 | 66.92% | 102,048 |

Both versions report zero capacity drops. Exact bin counts are `[0, 0, 5945, 108246]` for close
and `[0, 0, 8148, 93900]` for overhead. The current field's all-paired population explains why
high-bin priority has no practical allocation benefit in these views. The mixed-bin native test
covers the case where low-detail singles would previously consume the split-high cache budget.
Both versions prepare 131,072 physical blades; fallback counts are 97,310 and 73,024 respectively.

Retained source/scheduling capacity changes from **2,460,672 to 3,508,224 bytes** for 196 resident
fields: **1,047,552 bytes** more (a 1 MiB queue replaces a 1 KiB visible-field list). This grows
with resident candidate coverage; it is not a fixed one-MiB budget. The preparation allocation
remains 20,185,100 bytes including indirect arguments. Generation reuse continues in fixed views.

The first close baseline capture completed without Metal HUD samples while its window was in the
background. Its engine readbacks and screenshot completed normally. The local counter runner
accepts this specific missing-HUD condition and requires a settled readback and screenshot;
no GPU timing is inferred from that run.

Low-walk before/after full-field screenshots were visually inspected. Native exact instance
comparisons and frozen wind/MSAA comparisons provide the stronger correctness checks; separate
game launches can differ at equal-depth intersections and are not used as an exact pixel oracle.

## Artifacts and reproduction

The ignored experiment directory contains:

- `before-shaders/`, `after-shaders/`, `preparation-only/`, `schedule-only/`, `shader-changes.diff`;
- `before-game`, `after-game`, `before-renderer/`, `after-renderer/`, `runtime.sqlite`, archived settings;
- `runs/`, `matrix.json`, `matrix-with-thermal.json`, `counters/`, `counters.json`;
- `run-matrix.py`, `run-counters.py`, build/test logs and game screenshots.

The game wrapper records command lines and shader/binary/database hashes. The following commands
were used for the experimental version. Queue-specific tests now exist only in `after-renderer/`;
reproducing those checks requires the matching archived renderer and shaders.

```sh
cargo test --offline -p yarra-vegetation-render --lib
cargo test --offline -p yarra-vegetation-render gpu_scheduler_covers_candidates_once -- --ignored --nocapture
cargo test --offline -p yarra-vegetation-render scheduling:: -- --ignored --nocapture --test-threads=1
cargo test --offline -p yarra-vegetation-render prepared_blades_match_reference_with_wind_msaa_and_overflow -- --ignored --nocapture
cargo build --offline --release -p yarra-app-game
```

All listed checks and the release build passed. `git diff --check` passed. Native tests require
GPU access; the ordinary suite leaves them opt-in.
