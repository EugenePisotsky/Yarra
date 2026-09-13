# Grass shadow restart — 2026-09-09

Both the raised depth-noise sheet and the clump-mask caster failed visual review. The user rejected
continuing either approach, especially from above. Both implementations and their controls have been
removed. There is no replacement caster selected or implemented. This supersedes the earlier plan
that prescribed a canopy grid, dithered depth, and broad shadowing as an acceptable endpoint.

## What the restart preserves

The goal is convincing, stable grass shading with a small added cost. Optimization is central;
bounded allocations and a short Mac timing comparison are insufficient evidence of a good solution.
iPhone remains the eventual target, but phone testing and the broader grass optimization are deferred.
Keep the existing 4× MSAA quality reference and render settings during comparisons. Do not increase
blade density, shadow resolution, cascade count, or receiver sample count to hide artifacts.

The current visible grass pipeline, ordinary directional-shadow reception, root color and root AO
remain intact. Reproduction cameras and the rejected experiments' evidence remain useful. The failed
source reduction is not retained as an architectural prerequisite: mean height and leaf area do not
by themselves describe the grass's occlusion geometry.

## What failed in the reasoning

The sheet discarded silhouette and height structure, then tried to recover it with random depth.
The replacement stamped unrelated clumps into the light map. Its seeds and cell centres were
independent of the visible grass's actual roots, grouping and curves. Facing cards toward the sun
provided a convenient projected area but did not make them a representation of that grass. Stable
randomness alone did not solve spatial correspondence. From above, exposed ground makes that
mismatch particularly easy to see. This is an inference from the implementation and the user's
rejection, not a new measurement of the exact rejected view.

Compensating receiver bias solved a real integration defect, but it could not make the chosen
representation look correct. The native test measured whether a shadow existed and survived a bias
change; it did not test whether it resembled grass. Small timing differences did not compensate for
visual failure. The previous final near/far screenshots were not sufficient acceptance evidence.

## Separate the visual requirements

| Effect | What must be explained | Existing support |
| --- | --- | --- |
| Root depth and color | Darker lower blade regions without a painted-looking ground pattern | Authored root-to-tip color and AO |
| Blade-on-blade detail | Irregular, narrow dark portions with plausible shape, placement and motion | No accepted approximation |
| Grass onto ground/other objects | Shadows tied to where grass actually exists, including boundaries and sparse patches | No grass caster after removal |
| Trees/objects onto grass | Ordinary world shadows | Existing directional receiver; preserve it |

In `vegetation_debug_draw.wgsl`, root color and AO interpolate along curve parameter `t`, not actual
world height. A bent tip can be close to the ground but retain tip AO. This is a useful diagnostic
question for the restart, not evidence that changing it alone reproduces inter-blade shadows.
The current receiver also reduces ambient/body color according to its shadow visibility. Any new
visibility approximation must be inspected separately from albedo and root AO to avoid compounded
blackening. Do not change ordinary tree-shadow behavior as a side effect of this study.

The [Ghost talk notes](GHOST_OF_TSUSHIMA_GRASS_TALK_NOTES.md#slides-45-46-grass-shadows-without-rendering-every-blade-into-every-light)
distinguish broad terrain-impostor shadows from separate short-range detail. Their source is Eric
Wohllaib's [GDC 2021 presentation](https://gdcvault.com/play/1027033/). They do not establish Yōtei's
implementation or justify reproducing only one part of a combined result.

AMD's [procedural grass sample](https://gpuopen.com/learn/mesh_shaders/mesh_shaders-procedural_grass_rendering/#pixel-shader)
uses relative height to darken roots and noise for broad color variation. That is an example of
inexpensive shading, not evidence that generic noise reproduces blade-shaped occlusion. Copying its
noise would repeat the mismatch with the requested reference.

## Next experiment: establish the signal before choosing its representation

Use a small deterministic patch of the actual authored ribbons, including the current short split
species. Keep the visible placement, curve, width, grouping and light the same across every option.
Start with two crossing blades to isolate a partial shadow, then a bounded patch with a sparse edge,
an empty gap and a mixture of upright and bent blades. Do not replace the content with generic masks.

For this comparison only, a small offline geometry-derived shadow reference can establish where
occlusion should occur and how it changes with light and wind. Bound it to at most 256 blades and a
single patch; it must never become a full-field runtime shadow path. Reuse the current curve formula
or validate exported geometry against it instead of silently substituting another blade model.
This answers the specific question the failed experiments skipped: what spatial information can be
removed while retaining a convincing result from both above and beside the patch?

Inspect root AO, sun visibility and final shaded color separately. Compare fixed low-sun and higher
sun states, stopped wind and two deformed poses. Include a neutral ground material so terrain color
variation cannot conceal a failure. A useful illusion need not reproduce every real shadow, but must
remain plausible when the camera changes and when sparse gaps reveal the ground.

Candidate families, with no winner assumed:

| Candidate | Potential advantage | Main question / reject condition |
| --- | --- | --- |
| Material-local occlusion on visible blades, using existing blade coordinates and stable identity | No extra geometry, shadow pass or render target; can target the requested marks directly | May look painted and cannot cast onto ground; reject if detached from sun/motion or if per-fragment cost is excessive |
| Precomputed visibility derived from the same authored clump used by visible geometry | Can retain actual overlap at a bounded lookup cost | Current procedural fields are not repeated authored clump meshes; requires a shared geometric definition, not another independent atlas of stamps |
| Coarsened geometry from the actual source roots/curves | Can cast onto ground and blades with spatial correspondence | Must survive simplification and have affordable generation, vertex, fill and cascade cost; no duplicate full-field blade pipeline |

Prefer investigating the first candidate for blade detail because it fits the existing draw. It is
an unproven hypothesis and does not fulfil ground casting. Ground shadows stay a distinct requirement;
no design may rename material variation as a completed shadow impostor. A separate ground solution
should be chosen only after the small-patch comparison shows what information it must retain.

Do not add screen-space tracing, a grass prepass, temporal history, or a world volume as an automatic
fallback. Each would need a separate total-cost justification given the current renderer's problems.

## Visual and cost gates

First inspect the same patch from the actual gameplay overhead angle, vertical top-down, low grazing,
and near views. Then orbit and zoom out/back with fixed light and wind. A candidate fails if it shows
unrelated stamps, a camera-following boundary, darkening in empty gaps, an obvious texture on the
blade, or large mean-brightness changes at LOD/cascade transitions. Passing a few stills is not enough;
check motion and record the same locations across views.

The existing `grass-overhead` is oblique, and `grass-zoom` follows gameplay pitch. A new
`--render-repro grass-top-down` (v8) places the camera vertically at 18 m with frozen wind. It is a
baseline/inspection route, not a replacement for the gameplay overhead case. `grass-stream` remains
available for later source-lifetime checks.

Before implementing a promising candidate across the field, declare its work in units: extra
vertices, fragments, texture reads, varyings, allocations, writes and update frequency. A tiny shader
addition still runs over many overlapping grass fragments; one lookup is not automatically cheap.
No runtime cost is claimed for candidates that have not been implemented and measured.

Only a visually useful small-patch candidate advances to a short current-scene Mac on/off check with
unchanged density, actual render resolution, AA, wind and camera path. Inspect the entire added work,
including bandwidth and startup/streaming spikes. Extended comparisons are justified only after that
check. There is no phone test or new full-field caster in this reset.

## Reset verification

The game/editor `cargo check --offline --locked` and the release game build pass after removal.
A 900-frame `grass-top-down` run exited successfully with no shader errors or grass-caster diagnostics;
its frame-600 capture is `tmp/grass-shadow-restart-2026-09-09/baseline-top-down.png`. This clean baseline
still shows directional grass grouping and exposed terrain, so the patch study must isolate those
existing visual contributions too. The capture is a baseline, not a new shadow result or a timing
measurement. The removed source, original integration diff and prior report are archived alongside it.
