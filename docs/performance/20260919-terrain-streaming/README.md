# Terrain/grass streaming responsiveness — 2026-09-19

The editor became unresponsive after navigating with `--terrain-lod`; the user
also observed degradation in the release game. The diagnosis and implementation
are recorded in [DISTANT_WORLD_RENDERING.md](../../DISTANT_WORLD_RENDERING.md#streaming-responsiveness-correction-2026-09-19).

This is a CPU/streaming investigation on a MacBook Pro M2 Max, not a power or
thermal experiment. Grass density, the terrain shader and world publication were
not reduced or changed for the correction.

## Evidence

`evidence.zip` holds raw CPU samples, the isolated contact-check benchmark logs,
editor before/after logs, focused test/check output, and release-game logs.
`manifest.json` lists each archived file's SHA-256. `game-comparison.json` contains
the aggregate application update measurements from the valid before/after pair.

The initial dev-build contact check redundantly scanned whole fields per sample;
the dense 257×257/33×33 synthetic case fell from 1913.325 to 1.421 ms. These are
single-run flat-grid measurements and must not be described as whole-frame gains
or applied to release builds, which already excluded debug assertions.

The editor replay left and returned to the field three times using native pixel
scroll panning. Menus opened while grass was loading; at each complete load the
fresh certificate count increased by exactly 253, then stopped. The final native
CPU sample spans the return/loading interval. The before/after CPU samples have
different durations; their stack-node counts are not comparable timing totals.
The user's exact repeated zoom gesture remains a manual confirmation step.

## Commands and controls

CPU tests and microbenchmark:

```sh
cargo test --offline -p yarra-engine -p yarra-vegetation-render -p yarra-world -p yarra-vegetation --lib
cargo test --offline -p yarra-engine profile_surface_certification -- --ignored --nocapture
```

Native Metal checks (run sequentially):

```sh
cargo test --offline -p yarra-vegetation-render contact_gate_clears_frozen_roots_and_elevated_views_cull_grass -- --ignored --nocapture
cargo test --offline -p yarra-engine mountain_cover_uploads_draws_moves_and_rebases -- --ignored --nocapture
```

Release route, identical invocation for the existing pre-fix release binary and
the rebuilt release binary:

```sh
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 target/release/yarra-app-game \
  --terrain-lod --render-repro grass-stream \
  --profile-warmup 2 --profile-seconds 12 --profile-fps 60 \
  --profile-size 2560x1440
```

This uses 2560×1440 internal rendering, 4× MSAA, the repro's disabled depth
prepass, a windowed surface and a 60 fps cap. It is a short streaming smoke check,
not the normal fullscreen 120 fps configuration. Both valid runs report focus
throughout measurement. Their update averages were 59.77/59.91 fps and worst
updates 47.97/33.32 ms. Do not infer a sustained or GPU-power improvement from
this single pair. The archived `game-after-build-overlap.log` is deliberately
excluded: compilation overlapped that intermediate run.

Validation: 107 CPU tests, both native Metal tests, workspace check and focused
Clippy with existing lint exceptions passed. The readiness test verifies exact
restored instance data, image agreement, frozen-draw safety, elevated culling,
and no readiness-driven source repacks or acceptance-cache rebuilds. One initial
strict byte-for-byte image assertion caught four one-unit channel differences
after GPU append-order changes; exact instance equality and the existing strict
candidate-cache image tolerance are the final checks.
