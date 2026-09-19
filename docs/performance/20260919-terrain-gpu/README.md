# Close-ground GPU regression — 2026-09-19

The release game remained expensive in high views after the CPU streaming fixes.
The user's screenshot showed 8.41 ms fragment work out of 9.56 ms GPU duration.
The new near-terrain shader was the main isolated regression in this investigation.

## Retained change

`terrain_near.wgsl` now returns an index from the material hash lookup and probes
only keys. Neighbour checks fetch just availability. Surface/macro functions
receive an index instead of a 304-byte material record. Each helper loads the
parameters it uses. Texture samples, material weights, grass density, terrain
geometry, lighting and near/far blending remain the same.

A separate experiment skipped exactly-zero-weight layers. It measured 6.89 ms
versus 6.70 ms for the preceding indexed-lookup variant in that route, so it was
removed. The final variant additionally changes the surface/macro helper arguments.
These are code/measurement findings; register spills or hardware cache behavior
were not independently measured.

## Strongest before/after pair

Runs **08** and **09** use the actual gameplay camera/controls and real-time wind,
normal event-loop/VSync behavior, the normal depth prepass, fullscreen presentation,
2592×1456 internal pixels, 4× MSAA and the same published world. The internal size
approximates the user's 3456×1942 surface at the game's normal 75% scale. The test
surface is the full display; its exact dimensions are recorded in each audit.
The audit HUD remains present in both runs. No compilation or other diagnostic
GPU run overlapped measurement. Each run has 12 s warmup and 20 s measurement.

| Metric | Before | After |
| --- | ---: | ---: |
| Application updates/s | 117.35 | 120.00 |
| Updates longer than 12.5 ms | 53 | 0 |
| Worst update interval | 17.59 ms | 8.84 ms |
| Metal HUD mean GPU duration | 8.51 ms | 5.92 ms |
| Metal HUD p95 GPU duration | 8.74 ms | 6.04 ms |

Both runs remained focused and reported nominal thermal pressure. This is about
30% lower measured GPU duration in this pair. It is not an equivalent GPU-power
reduction or a sustained thermal test. HUD samples overlap and correlate; the
actual application intervals provide the independent cadence evidence.

```sh
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 MTL_HUD_ENCODER_TIMING_ENABLED=1 \
target/release/yarra-app-game --terrain-lod --render-audit \
  --profile-native-pacing --profile-window fullscreen \
  --profile-warmup 12 --profile-seconds 20 --profile-fps 120 \
  --profile-size 2592x1456
```

The before run temporarily used the archived original near shader. The final
shader was restored before the after run and remains the working version. Source
or publication databases were not edited or recooked. No change to the normal
game's presentation defaults was made.

## Isolation and follow-ups

`summary.json` retains every run, including the unsuccessful optimization. In
runs 01–04, the same fixed high camera used the older profiling pacing override,
a windowed surface and no depth prepass. Mean GPU durations were:

- LOD path: 8.35 ms.
- LOD with grass drawing disabled: 10.75 ms. This is not an additive grass-cost
  estimate: occlusion and GPU frequency can change between runs.
- Previous terrain path: 4.12 ms. Source residency was 49 pages versus the LOD
  path's 256, so this is a reference smoke check, not a perfectly isolated A/B.
- LOD with only near-terrain shading bypassed: 3.77 ms. Grass, source residency
  and geometry stayed in place. This identifies the near shader as the main cost.

Runs 05–07 switched to fullscreen/native pacing/prepass and the fixed high camera.
They test the original shader, index-only hash lookups, and the rejected zero-weight
shortcut respectively. Runs 08–09 also restore the actual gameplay camera, rather
than overriding its transform with the fixed repro pose.

Run 10 uses `--render-repro grass-zoom --render-prepass` with the same fullscreen
native-pacing settings. It repeatedly moves between the high and closer oblique
views. Run 11 uses `grass-close` for the close third-person view:

| Follow-up | Updates/s | Updates over 12.5 ms | Mean GPU duration |
| --- | ---: | ---: | ---: |
| Repeated zoom | 120.00 | 0 | 6.06 ms |
| Close third person | 119.90 | 2 | 4.93 ms |

Both were focused with nominal reported thermal pressure. Two isolated longer
updates remain in the close-view run; this fix does not establish hitch-free
behavior everywhere. Complete results are in `summary.json`.

## Validation and tooling

41 CPU tests passed (17 game, 24 terrain renderer). The native near-terrain image
test passed and covers original-material agreement, mixed weights, normals,
canopy, rebasing, eviction and restoration after diagnostic variants. The release
game build and focused Clippy check passed; the latter permits existing lint
categories listed in `commands.txt` in the archive.

The audit's ground-only and shading controls now recognize composite terrain.
Previously they classified hierarchy meshes as other objects and hid them in
ground-only mode. A CPU regression test checks classification and visibility
restoration; GPU checks exercise flat, single-texture, unlit and production shaders.

`--terrain-near-off` is an audit/repro shader bypass that preserves cache residency
and logs `terrain_near=off`. `--render-prepass` enables the normal desktop prepass
in a repro, and `--render-ui-off` isolates rendering without the game/audit HUD.
Both choices are logged. Timed `--render-audit` runs can now retain normal camera
controls without selecting a repro. `--profile-native-pacing` preserves normal
event-loop/VSync settings; `--profile-fps` then specifies only the reference rate
for late-update classification, not a replacement frame cap.

`evidence.zip` contains logs, tests and shader snapshots. `manifest.json` records
their hashes. `summarize.py` reproduces `summary.json` from an extracted archive
using the repository's existing HUD parser. These short checks do not establish
mainstream-PC performance or guarantee sustained 120 fps in a complete world.

To reproduce the summary from the repository root:

```sh
unzip docs/performance/20260919-terrain-gpu/evidence.zip -d tmp/terrain-gpu-evidence
python3 docs/performance/20260919-terrain-gpu/summarize.py \
  tmp/terrain-gpu-evidence --output tmp/terrain-gpu-evidence/summary.json
```
