# Terrain allocation under pressure — 2026-09-19

This is correctness and bounded-allocation evidence, not a GPU performance or
thermal benchmark. No world publication or production density setting changed.

## Reproduced failures and correction

Three new CPU regressions failed against the previous planner: a camera-plane
visual error tied actor priority; vegetation displaced actor contact; and visual
refinement consumed the capacity needed by stitched contact edges.

The replacement uses separate actor/vegetation/camera-contact/visual priority
classes and schedules stitched-edge requirements alongside body refinement.
The runtime permits atomic retirement of lower-priority grass under budget pressure,
with the normal grass readiness gate, while continuing to reject actor-ground loss.
It also reserves selection work for the final complete seam pass.

## Checks

- 45 engine and 29 terrain renderer CPU tests pass. Four engine and three renderer
  tests are opt-in; the native and hill checks below were run explicitly.
- The native Metal test covers upload delays, cover swaps, morphs, rebasing,
  offscreen actor arrival, grass certification and publication rollback/retry.
  Its budget stress case tightens visual error thresholds for the 512 × 512 test
  view, then reduces the triangle limit. The active cover changes from **450,560
  to 260,096 triangles**, with zero blocked actors. Test duration was about 30 s;
  it does not measure steady-state GPU cost.
- The current hill publication, read without changing it, keeps actor contact at
  both 35° and 70° downward views under the normal **1,048,576-triangle** limit and
  an artificial **262,144-triangle** limit. With the normal limit all 44/44 and
  41/41 conservative grass guard regions fit. With the reduced limit 2 regions fit
  in each view. These are padded planning regions, not visible grass-page counts.
- Clippy completes with existing workspace warnings. New allocation/probe code
  adds no Clippy warnings. Formatting and diff whitespace checks pass.
- Release game and editor builds pass. Existing publications need no recook.

The first engine suite also exposed a test-harness omission from the earlier launch
bookmark change: publication tests did not initialize `WorldStartView`. The harness
now includes it. No production resource requirement was relaxed.

## Reproduction

Run from the repository root:

```sh
cargo test -p yarra-terrain-render -p yarra-engine --lib
cargo test -p yarra-engine --lib mountain_cover_uploads_draws_moves_and_rebases -- --ignored --nocapture
YARRA_TEST_WORLD_DB="$PWD/tmp/hill-landscape/runtime.sqlite" \
YARRA_TEST_START_VIEW="$PWD/tmp/hill-landscape/project.views/summit.ron" \
cargo test -p yarra-engine --lib published_landscape_contact_budget_probe -- --ignored --nocapture
```

The hill probe requires the existing hill fixture. It conservatively requests all
nearby leaf cells as meadow, including potentially empty painted areas. Its planning
viewport is 2560 × 1440; its printed CPU timings include final contact checks and
must not be interpreted as rendering times. The earlier 65 × 65 hill generation
was not recooked for this check.

`evidence.zip` contains the three failing pre-fix regressions, final CPU results,
native Metal results, hill probe, Clippy output and release build log. `manifest.json` records each
archived member's hash. Regional live editor integration remains a separate next
checkpoint.
