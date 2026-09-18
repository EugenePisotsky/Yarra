# GP-023: move clump lighting from fragments to vertices

**Tried and reverted.** The candidate preserves the image in 15 frozen comparisons,
but reduces combined main-pass vertex/fragment ALU instructions by only **1.52%**:
fragment instructions fall **6.06%**, while vertex instructions rise **5.72%**.
Repeated replay timings do not demonstrate a speedup. No power or heat improvement
was measured. The small net reduction does not justify promoting this transfer of
work into the already expensive vertex stage for the current dense-field target.
This is a conservative implementation decision, not proof of an energy regression.

The pre-experiment checkpoint, including GP-022, tools, canopy settings and prior
evidence, was committed and pushed to `origin/feat/vegetation` as
`75f839868e54cc77784dd74f3176cdc7804ddb4b`. Production shaders are restored exactly
to that checkpoint. The additional image-test cases and this experiment record
remain as follow-up changes; the rejected shader is preserved in the evidence.

## Candidate and correctness

`stable_clump_normal` reconstructs a tangent frame, applies clump trigonometry and
normalizes a lighting direction using values constant across each render unit.
The candidate calculates that direction in `geometry_vertex`, carries it through
location 6 (previously surface normal plus clump variant), and renormalizes it in
the fragment shader. It preserves the existing four-float interpolator and its
perspective interpolation. A uniform guard skips the new vertex work for legacy
and unlit diagnostic lighting. No new buffer, material approximation, density,
geometry, wind, MSAA or preparation-capacity change is involved.

An initial three-float, flat-interpolated version failed the existing strict image
check at 4× MSAA: maximum byte difference 40, mean 0.017651 in case 1. It was not
profiled or accepted. Restoring the original interpolator shape/mode and adding
fragment renormalization passed; those adjustments were made together, so the
failure was not isolated to one of them. The final guarded candidate also passed.

The final **15 frozen shader A/B cases are byte-identical** at 256×256, with
unchanged counts and exact candidate-image reproduction after each swap. Coverage
includes near/overhead/far views, MSAA off/4×, canopy settings, all five shape
inspection modes, disabled preparation and forced overflow, plus three added
cases: Medium blade bands, legacy lighting and unlit diagnostic lighting.
The fixture includes single/paired, high/low and partially morphed geometry.

The renderer unit suite passed before profiling and after restoration:
**32 passed, 10 opt-in tests ignored**. Offline shadow fixtures were adapted during
the experiment and restored with the production shader. The additional native
image-test coverage is retained. `git diff --check` passes.

## Matched capture and counters

Same frozen release binary, 96-root database and canopy as GP-022, original
131,072-blade preparation capacity. The baseline shader **already includes GP-022**;
do not add the two experiments' percentage changes.

M2 Max, 38 GPU cores; fullscreen 2592×1626 world / 3456×2168 presentation;
4× MSAA, Balanced, `low-walk`, frame 600 / render frame 601;
camera `(2.513, 1.730, -5.895)`, wind phase 5. Both game launches exited after
capture. Input hashes match except for the draw shader. No builds or native tests
overlapped capture/profiling. There was no sustained test or power collector.

Two profiles per capture, order **A–B–B–A**, all **Medium** in Xcode 26.6:

| Profile | Main opaque encoder ms | Sum of encoder ms |
| --- | ---: | ---: |
| Baseline | 4.857 | 6.718 |
| Candidate | 5.150 | 6.987 |
| Candidate repeat | 5.398 | 7.322 |
| Baseline repeat | 5.778 | 7.616 |

The baseline itself drifts by 0.921 ms in the main encoder. Candidate timings lie
inside that range. These are profiles of two captured frames, not four independent
live runs; neither a reliable speedup nor a stable regression is established.
Summed encoder work is not live frame latency or power.

| Main opaque encoder counters | Baseline mean | Candidate mean | Change |
| --- | ---: | ---: | ---: |
| Vertex ALU instructions | 6,020,569,216 | 6,365,033,216 | +5.72% |
| Fragment ALU instructions | 9,582,022,784 | 9,000,944,768 | −6.06% |
| Combined VS + FS ALU instructions | 15,602,592,000 | 15,365,977,984 | **−1.52%** |
| Submitted vertices | 1,480,956 | 1,480,956 | unchanged |
| Vertex invocations | 1,115,304 | 1,115,304 | unchanged |

Fragment invocations are 6.318–6.324 million in the baseline and approximately
6.327 million in the first candidate profile; see raw exports for every repeat.
First-profile device reads are 134.515 / 132.306 MB, writes 119.503 / 117.701 MB.
First-profile vertex occupancy is 15.11% / 15.12%; fragment occupancy 29.07% / 28.89%.
The main encoder includes terrain, so these are not grass-only percentages.
The arithmetic sum is a work counter, not an energy model: instruction types,
inactive lanes and execution overlap matter. Separate captures also have
nondeterministic compaction/preparation selection; frozen A/B is the image proof.

## Decision and preservation

Revert this candidate rather than claim an improvement from fragment savings
alone. It might behave differently in a more pixel-heavy scene, but that is
unmeasured. No user-run heat test is requested for this rejected version. The
current renderer retains GP-022; its power benefit remains unmeasured as before.

[Preserved evidence](performance/20260916-clump-vertex/README.md) includes the
candidate patch, rejected initial variant, exact shader sets, native test logs,
four raw counter exports, input hashes and a reproducible offline analyzer.
The local large captures remain in `tmp/grass-clump-vertex-20260916/`.
