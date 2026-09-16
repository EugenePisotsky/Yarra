# GP-022: skip overwritten distant-grass vertex work

**Retained as a bounded shader optimization.** The matching 96-root capture uses
**13.74% fewer main-pass vertex ALU instructions**, with the same vertex and
vertex-invocation counts. Twelve frozen shader comparisons are byte-identical.
**No reliable frame-time, power or heat improvement is established.** This closes
the implementation candidate selected in [GP-021](GRASS_EARLY_HEAT_20260916.md),
not the broader early-heat question.

## Change

In `assets/shaders/vegetation_debug_draw.wgsl`, production low paired geometry
with exactly zero morph skips the general cubic evaluation, transported frame,
camera opening and far-width calculation. The existing low-mesh branches already
replace those results; they still calculate the same positions, normals and side
vectors. Shared preparation, wind and final material outputs remain intact.

The guard requires low LOD, a paired ribbon and zero morph. Inspection disables
low LOD, so inspection, partially morphed/high geometry, single blades and broad
leaves retain the general path. No buffers, shader variants, draw calls, density,
LOD distances or MSAA settings were added or changed. Preparation capacity stays
at **131,072**, and the production authored-density configuration stays unchanged.
The 96-root database is an isolated profiling input.

## Matched evidence

M2 Max / 38 GPU cores; Xcode 26.6; same frozen release binary, 96-root database,
canopy and shaders as GP-018/019/020, except for this one draw-shader change.
Fullscreen **2592×1626 world / 3456×2168 presentation**, 4× MSAA, Balanced,
original preparation capacity, `low-walk`, frame 600 / render frame 601,
camera `(2.513, 1.730, -5.895)`, wind phase 5. Both diagnostic launches exited
automatically after capture, about 15 seconds after startup. They were sequential,
with no builds/native tests overlapping capture or profiling. No power collector
or sustained test was run.

Each captured frame was profiled twice, **A–B–B–A**, with **Medium** performance
state verified in Xcode. These are repeated profiles of two captures, not four
independent game runs.

| Main opaque encoder | Baseline | Candidate |
| --- | ---: | ---: |
| Submitted vertices | 1,480,956 | 1,480,956 |
| Vertex invocations | 1,115,304 | 1,115,304 |
| Vertex ALU instructions, mean of profiles | 6,979,861,120 | 6,020,571,136 |
| Vertex ALU change | — | **−13.74%** |
| Fragment invocations, range | 6,318,464–6,324,000 | 6,324,416–6,330,656 |
| Device reads, range (decimal MB) | 134.925–135.017 | 134.090–134.785 |
| Device writes, range (decimal MB) | 119.404–119.611 | 119.428–119.475 |

This encoder includes terrain as well as grass; 13.74% is a **main-pass vertex
instruction reduction**, not a whole-game or grass-only speedup. Fragment work
and memory traffic are essentially unchanged. First-profile vertex occupancy is
15.18% / 15.10%; no occupancy improvement is established. Separate captures have
nondeterministic compaction/preparation selection, so their pixel output is not
the strict image-equivalence test.

| Profile order | Shader | Main encoder ms | Sum of encoder ms |
| --- | --- | ---: | ---: |
| 1 | Baseline | 5.213 | 7.034 |
| 2 | Candidate | 5.496 | 7.574 |
| 3 | Candidate repeat | 5.664 | 7.531 |
| 4 | Baseline repeat | 5.727 | 7.432 |

The initial candidate is slower; repeated baseline main time rises by 0.514 ms
and the candidate times fall within the baseline range. This does not demonstrate
a speedup or isolate a stable regression. Summed encoder time also includes
variable unrelated work (for example the first pair's blit is 0.105 / 0.352 ms).
Medium is a replay performance setting, not proof that live clocks/power are
constant. Keep all four outcomes; do not substitute the fastest pair for them.

## Correctness and decision

- Renderer unit suite: **32 passed, 10 opt-in checks ignored**. The existing
  source-contract assertion was adjusted from a declaration to an assignment;
  its first failure and final passing log are both preserved.
- Extended native frozen shader A/B: **12 cases, zero differing bytes** at 256×256.
  Covers near, overhead and distant views; canopy off/on and envelopes; MSAA off/4×;
  fixed wind; all five inspection modes; preparation disabled and a three-blade
  arena forcing overflow. Assertions confirm all four topology bins and partially
  morphed high geometry occur. Restoring the candidate reproduces each image exactly.
- Existing native preparation/reference test passes with wind off/on, changed
  camera, 4× MSAA and overflow. Its small compute/vertex MSAA edge differences
  satisfy the existing bounded tolerance; this is separate from the exact shader A/B.
- WGSL parsing, native pipeline creation and `git diff --check` pass.

Retain the small branch because it removes measured redundant arithmetic without
extra memory or a visual tradeoff in the tested scenes. The user's expectation
of a modest overall benefit remains reasonable: generation, preparation, fragment
shading and most traffic remain. Do not claim cooler operation or increase the
density budget from this result. No further user-run test is needed to validate
this implementation. A matched powered early-load comparison remains necessary
before making an energy/heat claim; no new 15-minute run is requested.

## Reproduction and preservation

[Compact evidence and reproduction](performance/20260916-low-vertex/README.md)
contains the four raw counter exports, exact shader versions, commands, input
hashes, test logs, source diff and offline analyzer. Large GPU captures and frozen
binary/database copies remain in `tmp/grass-low-vertex-20260916/`.

The existing sustained presets deliberately reference older frozen shaders. They
will **not** automatically test this optimization; any later powered comparison
must explicitly snapshot baseline/candidate shader sets with matching other inputs.
