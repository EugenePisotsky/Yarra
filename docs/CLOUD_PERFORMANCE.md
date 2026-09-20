# Cloud performance — 2026-09-20

Current Balanced uses the [bounded sky cache](#bounded-sky-cache-follow-up) below.
The first optimization improved short-run GPU timing but did not address the
user's reported sustained heating. Neither experiment certifies quiet fans.

## First optimization: resolution and redundant work

The first cloud renderer left little room within a 120 Hz frame's 8.33 ms budget.
This follow-up reduces its cost in three places:

- Reuse the 256² ground-shadow texture in cloud-field coordinates. Wind moves the
  lookup, while density, layer, seed, active light direction and GPU resource or
  shader changes rebuild the texture. Fixed-time gameplay normally builds it once;
  a continuously moving sun still rebuilds it. Inactive lights skip integration.
- Default to Balanced: quarter width/height for cloud raymarching, versus High's
  original half width/height. This uses one-quarter of High's cloud pixels, with
  softer fine edges. The 48 view steps, lighting samples and shadow resolution stay
  unchanged. Terrain and vegetation resolution are unaffected.
- Blend scattering and transmission directly into the current HDR target, avoiding
  the previous full-scene read and ping-pong copy. Full-resolution depth still
  preserves foreground silhouettes, including MSAA coverage.

Quality is a session render preference, available under the editor's temporary
Atmosphere preview controls and through the game's `--cloud-quality` argument:

```sh
cargo run -p yarra-app-game -- --cloud-quality balanced
cargo run -p yarra-app-game -- --cloud-quality high
cargo run -p yarra-app-game -- --cloud-quality off
```

Balanced is the default. Off disables cloud rendering and direct cloud shadows;
it retains the authored weather's ambient response, so it is a rendering-cost
comparison rather than a clear-weather preset.

## Measurement

Sequential native Metal runs on the M2 Max used a copy of the published overcast
world, the same fixed camera near the character, grass enabled, UI disabled,
2592×1456 internal resolution, MSAA 4, and a 120 FPS target. Each run had 25 seconds
of warmup and 15 seconds of measurement. The existing landscape reproduction mode
disables the depth prepass; normal macOS gameplay enables it. These runs therefore
compare the change under matched conditions, rather than reproducing every setting
of the reported game window. They use the workspace's optimized debug profile.

GPU times below are the median of whole-game Metal HUD samples during measurement,
not isolated cloud-pass timings. App FPS is the mean of the logged update windows.

| Run, in execution order | GPU median | App FPS | Thermal state |
| --- | ---: | ---: | --- |
| Previous renderer | 7.03 ms | 116.00 | nominal |
| Optimized Balanced | 6.11 ms | 119.73 | nominal |
| Optimized Off | 5.19 ms | 119.87 | nominal |
| Optimized Balanced repeat | 6.36 ms | 116.72 | nominal |
| Previous renderer repeat | 9.39 ms | 82.84 | nominal → fair |
| Optimized High | 10.47 ms | 79.49 | fair |

The first nominal comparison reduced whole-game GPU time by about 0.92 ms (13%).
Relative to Off, the added GPU cost was approximately 1.84 ms before and 0.92 ms
for the first Balanced run. These differences are estimates across separate runs;
GPU clocks were not locked. The Balanced repeat shows normal variability even
before the thermal state changed. The final two runs cannot establish a controlled
before/after comparison or High's performance. These results support a useful
saving, not a guarantee of sustained 120 FPS or a measurement of startup stalls.

Local evidence is under `tmp/cloud-performance/` (ignored development artifacts):
`comparison.json`, each run's `run.json` with commands and binary/shader hashes,
`summary.json`, and `game.log`. `baseline-1` was an early instrumentation experiment
and is excluded. Pass timestamp instrumentation was removed after inconsistent
Metal results; none of the table's runs enabled it.

## Validation

- `cargo test -p yarra-atmosphere -p yarra-app-editor -p yarra-app-game`:
  154 passed, one existing manual check ignored.
- `cargo check --workspace --all-targets` and formatting checks passed.
- Native captures with the depth prepass enabled covered previous/optimized
  overcast, Balanced/High, scattered clouds, and night with MSAA disabled. No shader
  validation failures or foreground cloud bleed were observed. The comparison
  captures freeze cloud wind; actor animation can differ between frames.

The database and asset copies used for measurement and capture are disposable.
These checks do not modify the project's source or published runtime data.

## Bounded sky cache follow-up

Balanced now stores cloud radiance/transmission in a 512² stereographic hemisphere
atlas. It refreshes four strips, at most 32 strips/second, instead of tracing the
visible sky every frame. Camera rotation samples this world-direction atlas every
frame, so turning does not expose missing history or wait for cloud updates.
Radiance uses a fixed scale in RGBA16F; current exposure is applied during the
composite, avoiding stale exposure and overflow at the maximum authored sun lux.

The first image, shape/seed changes, texture/shader replacement, camera teleports
and resume after a pause refresh the complete atlas. Ordinary wind, translation
and lighting changes refresh progressively. At 120 FPS, the complete refresh takes
about 0.13 seconds; slower frame rates can lengthen it. A delayed frame submits
one update without a catch-up loop. Fine angular detail is softer than High, and
rapid wind/lighting previews can show differences between strips. High retains
per-frame half-resolution rendering. Terrain, grass, MSAA and the frame-rate target
are unchanged.

The three timed runs totaled **215 seconds** (3 minutes 35 seconds), honoring the
requested bounded test. Each used 10 seconds of warmup, then 70/80/35 seconds of
measurement respectively. This time the depth prepass was enabled, matching normal
macOS gameplay. Other scene/resolution settings match the earlier experiment.
No builds or other Yarra processes overlapped these timing windows; a build and
brief correctness capture occurred between the first and second runs. Starting
temperatures and clocks were not controlled.

| Run, in execution order | GPU median | GPU mean | App FPS | Thermal state |
| --- | ---: | ---: | ---: | --- |
| Previous per-frame Balanced | 6.45 ms | 6.46 ms | 119.73 | nominal |
| Cached Balanced | 4.64 ms | 4.95 ms | 119.69 | nominal → fair |
| Clouds Off | 5.92 ms | 5.96 ms | 100.38 | fair throughout |

Cached Balanced reached fair thermal pressure around 71 seconds. The Off control
started while the machine was already hot and remained at fair pressure. Its
higher GPU time and lower FPS cannot be interpreted as clouds being free, nor as
proof of what caused the initial heating. The 1.81 ms median reduction between
the two Balanced runs is an observed timing difference, not a measured energy
saving. GPU watts, temperature and fan RPM were not collected. **The reported
thermal problem remains unverified as resolved.** No further stress runs were made.

Renderer counters confirm a median of about 1.96 million cloud target pixels
scheduled per second, versus about 28.3 million for the previous 648×364 target
at 120 FPS. This is roughly 93% fewer scheduled pixels, including cheap empty-sky
pixels; it is not a 93% claim about GPU time, density samples or power. The local
counter is logged as `CLOUD_WORK ... traced_pixels=...` at debug level.

The follow-up passed 156 editor/game/atmosphere tests (one existing manual check
ignored), workspace all-target compilation, and formatting. New tests cover
refresh budgets at 30/60/120/240 FPS, camera cuts, coordinate wrap seams, edits,
pause/resume, and avoiding catch-up bursts. A native Metal overcast capture verified
the cached path and foreground silhouettes. Prior night/scattered capture results
above apply to the earlier renderer, not a new visual qualification of the cache.

Local evidence: `tmp/cloud-performance/cached-comparison.json`, `sustained-before/`,
`sustained-cache/`, `sustained-off/`, and `visual-cache-2/`. Each timing folder retains
the command, binary/shader hashes, raw logs and summary. Source and runtime world
databases remain unchanged by these tests.
