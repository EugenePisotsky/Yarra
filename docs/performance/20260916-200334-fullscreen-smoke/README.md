# Fullscreen profiling control check — September 16, 2026

GP-016 in the [optimization log](../../GRASS_OPTIMIZATION_LOG.md).
This is a brief tooling check, not sustained performance or thermal acceptance.

Command, run after building the release binary:

```sh
python3 tools/grass_profile.py run --no-build --power off --window fullscreen --size game --fps 120 --idle 1 --warmup 20 --seconds 12 --output tmp/grass-profile-fullscreen-smoke
```

- Borderless fullscreen; actual surface **3456×2168**, world **2592×1626** (75%).
- Monitor metadata reports 3456×2234, 120 Hz; use the observed surface for world
  sizing rather than assuming it equals the monitor dimensions.
- Low-walk, current density, Balanced, 4× MSAA, counters off, repro prepass off.
- Twelve measured seconds after 20 seconds of warmup: 120.00 app fps, HUD interval
  p95 8.33 ms, zero late updates, focus maintained and app thermal state nominal.
- No power collection. `CHECK` is expected for missing power telemetry; no validity
  errors or target misses. This run cannot establish watts, sustained heat, fan
  noise, or a higher-density budget. The short GPU duration is not a headroom claim.
- World pixels are 1.1433× and surface pixels 2.0325× the earlier windowed 1440p
  test. These pixel ratios are not performance multipliers.

[report.json](report.json) preserves the parsed result. `evidence.zip` contains
raw game logs, session/run manifests, input hashes, canopy and the relevant profiler
sources. `manifest.json` records checksums. Extracting the archive and running its
analyzer exactly reproduces the saved report. Large game/database/shader inputs
remain in local `tmp/grass-profile-fullscreen-smoke/inputs/current/`; their hashes
are preserved, but this bundle alone is not a complete replay package.

Follow-up: the user completed the [powered fullscreen baseline](../20260916-200703/README.md)
with the following command. Its sustained result is separate from this short check:

```sh
python3 tools/grass_profile.py suite tools/profiles/grass-fullscreen-120.json
```
