# Terrain GPU headroom and sustained movement — 2026-09-19

The user reported that LOD terrain still slowed after about ten minutes of walking.
Restarting immediately did not recover performance, while the previous terrain
renderer remained smooth. Their screenshots showed 9.28 ms GPU duration with LOD
versus 4.48 ms without it, with fragment shading dominating both. The viewpoints
and residency differed, so those screenshots identify a problem but are not a
controlled two-times shader-cost measurement. Persistence across restart makes
accumulating process state less likely; it does not by itself prove thermal
throttling or establish the GPU frequency.

## Retained changes

- Evaluate close ground first. Where its weight is exactly one, skip the baked
  root and material hierarchy that would be blended away. Loading, edge, distance
  and pixel-footprint fades still evaluate and blend the baked fallback.
- Use the mesh's interpolated geometry normal as the close material's basis,
  matching the original detailed renderer. Previously the lower-resolution baked
  normal field was used even under fully detailed ground. This is a deliberate
  basis correction, not a claim of byte-identical old shading on every slope.
- Compute derivatives before the availability branch and explicitly supply them
  when sampling fallback textures.
- Read near-material fields directly instead of assigning the whole storage
  record to a local struct. Handle the common first-slot hash hit outside the
  collision loop; collisions retain the complete search and key validation.

Grass density, terrain geometry, texture resolution, material sample count within
the detailed surface, lighting, and render resolution were not reduced. An exact
zero-weight surface shortcut was tried again after the earlier changes, but was
slower in this route and removed. A suspected prepared-texture eligibility issue
was also ruled out: runtime surface lists are already filtered per page.

## Short isolation runs

Release M2 Max game, actual default gameplay camera, 2592×1456 internal pixels,
3456×2168 fullscreen surface, 4× MSAA, normal prepass and native event-loop/VSync
behavior. Each run has 12 s warmup and 40 s measurement. Metal HUD encoder timing
is enabled. All runs remained focused and reported nominal thermal pressure.

| Run | Mean GPU ms | p95 GPU ms | Updates/s | Updates over 12.5 ms |
| --- | ---: | ---: | ---: | ---: |
| LOD before this follow-up | 5.94 | 6.05 | 119.97 | 1 |
| Previous renderer | 4.65 | 5.02 | 119.95 | 2 |
| Evaluate close ground first | 5.59 | 5.73 | 120.00 | 0 |
| Also read fields directly | 5.40 | 5.52 | 120.00 | 0 |
| Also handle first probe directly, retained | 5.31 | 5.41 | 119.97 | 1 |
| Also skip zero-weight surface, rejected | 5.47 | 5.59 | 119.95 | 2 |

The combined short comparison is about 11% lower measured GPU duration. Small
individual differences are not independent energy measurements and should not be
treated as precise power savings. Source residency remains different between
renderers: LOD retains 256 vegetation pages at this location; legacy retains 49.

## Sustained comparison protocol

`grass-soak` repeatedly walks a 12 m-radius circle through the authored meadow at
about 2.5 m/s. Each 30 s cycle crosses page boundaries, moves the character/detail
and streaming focus, and retains the high oblique gameplay camera. The earlier
one-way stream route eventually stopped moving and was unsuitable for this test.

`tools/profiles/terrain-soak-120.json` runs legacy for one minute, LOD for ten
minutes, and legacy for another minute, each following 15 s warmup, with no idle
gap. The same binary, world, render size, grass settings, prepass and native pacing
are used. Inputs are snapshotted by the existing profiler. No compilation or other
GPU diagnostic overlaps the measurements. The runner enables ordinary HUD logs;
these suite runs do not enable the encoder-timing overlay used in the short tests.

The user confirmed both their LOD and legacy runs were on battery. This suite was
also on battery (confirmed by native `pmset`, not the sandbox's inconsistent
virtualized reading). Initial power collection was disabled: noninteractive sudo
required authentication. Rootless collection was discovered and started late in
the LOD run; it must not be interpreted as covering the initial slowdown.

## Sustained result: the problem remains

| Measurement | Updates/s | Mean GPU ms | Updates >12.5 ms |
| --- | ---: | ---: | ---: |
| Legacy before, 60 s | 119.92 | 4.59 | 5 |
| LOD, first minute | 119.96 | 5.57 | 2 |
| LOD, minute four | 115.10 | 7.30 | 292 |
| LOD, minute six | 110.99 | 9.13 | 544 |
| LOD, final minute | 116.94 | 7.20 | 184 |
| Legacy immediately after, 60 s | 119.95 | 4.58 | 3 |

The complete LOD run averaged 116.15 updates/s, with 2,310 updates above 12.5 ms
and 137 full sampling windows below 114 updates/s. Its worst approximately
one-second window was 92.54 updates/s. These are application cadence statistics,
not independently counted presentations or a 1% low. Minute buckets assign each
sample to its end time, so boundaries are approximate.

LOD retained 256 vegetation pages and 272 mesh assets throughout. Source revisions,
repacks and candidate-cache build counts stayed fixed after loading. Image counts
were 99–100. Metal memory stayed around 731–732 MB; process memory fell after
loading and then remained around 1.2 GB. This run gives no evidence of accumulating
resident content or an application memory leak causing the slowdown. It does not
exclude every possible driver/runtime issue.

The retained shader improvement is real in the short test but **does not solve
the sustained regression**. Both renderer paths used the same repeating route,
resolution, grass quality and release binary; their different source residency
and rendering architectures remain part of the comparison.

## Partial power and frequency evidence

[macmon 0.8.2](https://github.com/vladkens/macmon/tree/v0.8.2) can sample Apple's
private IOReport/SMC interfaces without sudo. A temporary release binary was
downloaded from its official release and SHA-256 checked against release metadata;
no global installation or system configuration change was made. The release
archive hash is `588d5bde79885ba36f693e5150911c10c3ad208a2e418a3f2aa827ac84a2d973`.

Collection ran from 23:40:46 to 23:44:46 UTC on September 18 (local September 19).
Whole in-window samples cover about 106 s at the end of LOD and 58 s of the
following legacy measurement; there are no power samples for the first legacy
run or for the initial LOD degradation.

| Sampled portion | GPU MHz | GPU active % | GPU W | CPU W | GPU °C | Fans RPM |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Late LOD only | 1028 | 95.9 | 16.17 | 5.04 | 65.99 | 3452 / 3730 |
| Legacy after | 1258 | 81.5 | 17.11 | 5.28 | 66.93 | 3455 / 3730 |

The faster path did not simply receive less power: its sampled GPU frequency and
power were higher, while activity was lower. This is consistent with different
workload headroom interacting with dynamic power/frequency behavior. It does not
prove a specific thermal limit, battery cap or driver policy. These are system-wide
counter estimates, not per-process watts, and were not cross-validated against
`powermetrics`. Thermal pressure was `fair` in both sampled portions. Temperatures
and fan readings cannot reconstruct the missing earlier transition.

A later 30 s run with the pre-follow-up shaders averaged 6.31 ms after a short
pause. It had only 5 s warmup and an initial cadence loss, with no overlapping power
samples. It is archived as context, not an optimization comparison or evidence
that sustained behavior was fixed.

## Short check with complete rootless telemetry

A subsequent LOD → legacy → LOD suite used 15 s warmup and 30 s measurement per
variant, with the same settings/route as the sustained suite. It validates the
integrated collector and provides a short comparison with clocks recorded from
the beginning. All three runs passed report validity/target checks with nominal
application thermal pressure; every run had at least 27.98 s of power coverage.

| Run | Updates/s | GPU ms | GPU MHz | GPU active % | GPU W | CPU W |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| LOD before | 120.00 | 5.58 | 1361 | 88.6 | 22.59 | 6.16 |
| Legacy | 119.83 | 4.53 | 1273 | 82.0 | 18.01 | 5.30 |
| LOD after | 120.00 | 5.57 | 1368 | 89.3 | 23.33 | 6.18 |

The LOD path still has extra cost at normal frequency: in this short sequence it
used about 4.6–5.3 W more reported system GPU power. This is not an attribution of
every watt to terrain, or a guaranteed sustained power difference. The short
result also cannot overturn the observed ten-minute cadence failure. Mean GPU
temperatures were 69.1, 72.4 and 82.6 °C respectively; an averaged temperature
alone is not a reliable substitute for the actual frequency and frame timeline.

## Repeat and inspect

To repeat with power/frequency telemetry, run from a local terminal:

```sh
python3 tools/grass_profile.py suite tools/profiles/terrain-soak-120.json
```

The default requests one local sudo authentication and elevates only Apple's
`powermetrics`. An existing macmon 0.8.2 executable allows the same suite without
sudo:

```sh
python3 tools/grass_profile.py suite tools/profiles/terrain-soak-120.json \
  --power macmon --macmon tmp/terrain-gpu-headroom/macmon
```

The temporary path above is local to this investigation; use `--macmon /path/to/macmon`
or an installed `macmon` on another machine. The profiler never silently switches
collectors. It records the executable version/hash and exact finite command,
preserves raw JSONL, validates measurement coverage, and adds temperature/fan
choices to the report timeline. Missing active-residency fields are not replaced
with macmon's frequency-scaled utilization. Collector pauses/missing lines do not
become fabricated long intervals. The macmon timestamp follows sample collection
([source](https://github.com/vladkens/macmon/blob/v0.8.2/src_app/main.rs)); alignment
remains approximately one second.

Keep the game focused. `--power off` explicitly opts out of power measurements.
No further long user run is required for this finding: the regression is reproduced.
The next rendering investigation should isolate near-material bandwidth/occupancy
against the old uniform-per-material path under recorded GPU clocks. Preserve
distant coverage and close appearance; reducing density or resolution would not
resolve this regression.

## Validation

42 CPU tests passed across the game and terrain renderer; 28 profiler/report tests
passed. The native GPU image test checks fully available near material independent
of fallback color/normal, partially loaded edges, distance fades, agreement with
the original shader on a slope, mixed surfaces, canopy, rebasing and eviction.
The release game build passed. No publication change or recook is required.
Clippy passed for both packages/all targets with the existing workspace warning
allowlist. Rust formatting and `git diff --check` passed.

The profiler now supports `terrain_lod`, `native_pacing` and `prepass` settings.
The report verifies them against the actual audit/configuration and accepts
expected residency changes on the repeating movement route. The old defaults and
the interpretation of existing report artifacts are preserved.

`evidence.zip` preserves raw game logs, shader variants, measurement metadata and
input hashes, the partial and complete rootless captures, reports and validation
logs. Binaries and world databases are identified by hash, not duplicated in the
archive. `manifest.json` records every archived member's SHA-256. Regenerate the
minute summary after extracting it with:

```sh
unzip docs/performance/20260919-terrain-headroom/evidence.zip -d tmp/terrain-headroom-evidence
python3 docs/performance/20260919-terrain-headroom/summarize_soak.py \
  tmp/terrain-headroom-evidence/soak
python3 tools/grass_profile.py report tmp/terrain-headroom-evidence/rootless-check
```
