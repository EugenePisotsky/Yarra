# Early heat buildup and optimization priority

**User steering after GP-020:** reducing the heat buildup during the first few
minutes takes priority over the brief late pacing dips. The timing trace is
deferred. No new game run, trace, fan setting or production configuration change
was made for this follow-up. GP-021 reuses the preserved power/game logs.

## What the existing evidence says

For comparison, use **game elapsed minutes 1–4**, before any of the three runs
reports elevated thermal pressure. All intervals here are nominal; that does not
mean no heat is being generated or that the case remains cool.

| Configuration | GPU W | CPU W | CPU+GPU W | First elevated pressure, game elapsed |
| --- | ---: | ---: | ---: | --- |
| 66 roots, original preparation | 20.05 | 4.26 | 24.30 | 4:11 |
| 96 roots, original preparation | 20.79 | 4.33 | 25.12 | 4:46 |
| 96 roots, enlarged preparation | 22.14 | 4.32 | 26.47 | 4:59 |

These are whole CPU/GPU subsystem estimates at fullscreen 120 fps, not isolated
grass watts or total laptop power. The matched 96-root pair changes only buffer
capacity. The 66-root comparison also has a different binary/database. Initial
temperature, cooling response and charging state were not identical or fully
measured. Therefore the first warning's timestamp alone cannot rank efficiency:
the higher-power candidate actually reports pressure later in these sessions.

The important behavior is sustained GPU-heavy work before the thermal warning,
not a new expensive task starting at minute four. Assets/source/prepared terrain
are ready by the first positive audit at approximately 5.3 seconds for the older
baseline and 11 seconds for the two 96-root builds. Residency then stays stable.
This does not identify every CPU/GPU task, but it supplies no evidence of a
minute-four asset-loading burst as the cause.

The previous “warmup” needs precise interpretation: for its first 60 seconds,
the timed profiler holds the low-walk camera and wind phase still. `ProfileClock`
starts advancing the route at measurement start, and repro wind uses that clock.
Preparation reuse during this stationary minute lowers work. It is asset warmup,
not a full minute of the later moving workload at thermal equilibrium. The
minutes 1–4 comparison above avoids mixing it with the moving scene. Historical
timing/power measurements remain valid; do not change old presets/reports silently.

The user's hotter case followed by a fan ramp and recovery is consistent with
heat accumulating before cooling catches up. We have no fan RPM or temperature
series establishing the exact controller behavior. For software work, the useful
objective is lower power/energy at the same 120 fps, resolution and accepted image,
including the period before thermal clock reduction.

## First implementation candidate

**Follow-up:** implemented and retained as [GP-022](GRASS_LOW_VERTEX_20260916.md).
Short native validation confirms 13.74% fewer main-pass vertex ALU instructions
and identical output in 12 frozen shader comparisons. Repeated replay timings do
not establish a speedup; power/heat benefit remains unmeasured. The reasoning below
records the proposal before that implementation.

Investigate an explicit path for the **existing fully low-detail paired grass**
in `assets/shaders/vegetation_debug_draw.wgsl`, preserving the current geometry,
coverage, shading outputs and animation.

- Generation explicitly packs `lod_morph = 0` for low LOD in
  `vegetation_debug_compute.wgsl`.
- The vertex shader initially evaluates general cubic positions/derivatives,
  transported frames, view opening and width compensation. Later paired-geometry
  branches replace position and shading-frame values using the low representation
  when the morph is zero. That is a concrete opportunity to avoid calculating
  intermediate results which do not contribute to the final low-detail output.
- The direct capture attributes most grass work to the distant paired draw.
  This targets measured expensive work; it does not remove blades, change the
  previously restored silhouette, or enlarge the preparation arena.
- Compiler optimization may already eliminate some of this work. Implementing
  the branch is a candidate, not a demonstrated instruction or energy saving.
  Shared curve information still required for low normals/shoulders must remain.
  Near/morphing, inspection, broad-leaf and single-blade paths require preservation.

Use the existing frozen prepared/fallback and LOD GPU tests to check output
equivalence, then a short matched frame capture to check vertex instructions,
register/occupancy behavior and memory traffic. GPU duration alone is insufficient.
Only a promising candidate warrants a new powered heat-ramp comparison. It is
not necessary to run another 15-minute baseline now. A genuinely thermal outcome
cannot be certified by a few-second capture.

If this candidate is ineffective, next examine a compact representation for
low-detail preparation and distant material cost. These are separate experiments.
The larger 128-byte-per-blade arena already traded repeated arithmetic for more
traffic and showed no power benefit; increasing its capacity again is not the
heat plan. Reducing distant geometry/coverage or material features is a quality
tradeoff requiring visual review, including the prior rejected far-LOD attempts.

Lowering frame rate or internal resolution remains an available operating-mode
tradeoff, not an optimization at the requested 120 fps/image. Keep it separate
from code changes. Removing MSAA previously harmed image stability, and its
unnecessary color-store traffic is already fixed; do not count either as an
untested quality-preserving opportunity. Fan control is not part of this plan.

## Evidence

[Reproducible analysis](performance/20260916-early-heat/analysis.json) and
[offline script](performance/20260916-early-heat/analyze.py) read the existing
checksummed GP-015/019/020 reports and zipped raw logs. Running the script launches
no game and needs no sudo. Source report/log hashes are stored in the output.
The script uses the current `tools/grass_profile_report.py` power-window function.
No renderer optimization or new measured saving is claimed in GP-021.
