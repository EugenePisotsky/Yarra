# Terrain readiness review — 2026-09-19

The review of `feat/environment` at `9877ad1` identified source admission stalls,
large-page decode exposure and the already recorded sustained GPU regression.
This follow-up distinguishes a confirmed scheduling defect from future capacity
work and tests one possible GPU optimization before committing to it.

## Retained scheduling correction

`attach_prepared_pages` sorted prepared pages, then truncated the list to two
before testing residency admission. When both pages exceeded the remaining
capacity, smaller pages behind them never got considered, even though they fit.
The new regression test failed against the original implementation.

Admission now scans the bounded prepared queue in priority order and limits actual
attachment attempts to two per frame. A rejected admission does not consume an
attachment slot. Existing CPU/GPU residency limits are unchanged. A second test
checks that a queue of fitting pages still attaches only two per update.

This fixes head-of-line blocking. It does not implement resident eviction by
priority or cancellation of lower-priority pending work. If every byte is retained
by still-demanded pages, or all 16 pending slots are blocked prepared pages, new
requests can still wait. Those are separate saturation-policy limitations.

## Actual source sizes

Read-only queries of the current published databases found:

| Source measurement | Default overworld | Hill landscape |
| --- | ---: | ---: |
| Largest decoded page | 25,694 bytes | 25,694 bytes |
| Sum of largest 16 decoded pages | 411,104 bytes | 411,104 bytes |
| Terrain cells | 256 | 2,304 |

The default overworld's entire decoded source payload is 11,869,626 bytes
(11.32 MiB), below the 64 MiB resident source limit. Its hierarchy source GPU
estimate plus all object dependency estimates is 32,343,872 bytes (30.85 MiB),
below the 256 MiB source admission limit. The hill contains more total data, but
streams a local subset. These figures exclude hierarchy geometry/material caches,
near materials, actual asset allocations, decoder scratch and process overhead.
They are source admission counters, not total memory measurements.

The 16-page staging limit remains a count limit: supported large payloads can
still consume hundreds of MiB before residency admission. Aggregate byte
reservation before decoding is appropriate before introducing such payloads.
It is not an explanation for current frame-time degradation, and neither it nor
priority-based resident eviction warrants a larger rewrite for the current scene.

## Neighbor-lookup GPU experiment: no retained shader change

The close-ground shader looks up neighboring page availability near cell edges.
A temporary shader replaces those lookups with full availability, to investigate
whether precomputing neighbor availability would justify additional cache data.
This diagnostic also removes missing-neighbor edge fades, so it is not a valid
production optimization or an appearance-equivalent implementation.

The existing release binary ran baseline → diagnostic → baseline with identical
world, density, geometry and settings: M2 Max, AC power, fullscreen, 2592×1456
internal pixels, 4× MSAA, normal prepass/native pacing and the repeating walking
route. Each run had 15 seconds warmup and 25 seconds measurement. No compilation
or other GPU test overlapped. Inputs were snapshotted by the profiler; the binary
predates the scheduling correction, which does not affect this unsaturated route.

| Run | Updates/s | Mean GPU ms | GPU MHz | Reported GPU W |
| --- | ---: | ---: | ---: | ---: |
| Baseline before | 120.00 | 5.79 | 1,278 | 20.52 |
| Neighbor lookups bypassed | 120.00 | 5.91 | 1,204 | 19.14 |
| Baseline after | 120.00 | 5.89 | 1,256 | 20.38 |

All runs passed the profiler's validity and target checks, stayed focused and
reported nominal application thermal pressure. None had an update over 12.5 ms.
Rootless macmon telemetry covers approximately 24 seconds of each measurement.

There is no demonstrated frame-time benefit. The lower diagnostic power at lower
clocks is suggestive but cannot establish a repeatable workload/energy saving from
one short sequence. It does not justify adding a neighbor cache now. GPU duration
alone is not a fixed-clock workload measure; reported power is system-wide.
Production shaders remain unchanged.

This AC-powered short test does not overturn the earlier
[ten-minute battery regression](../20260919-terrain-headroom/README.md).
That issue remains open. Further shader work should isolate close-material data
access and show a repeatable gain while preserving appearance, then repeat the
sustained workload. A basic sky/sun/day-night foundation can proceed; expensive
weather effects still need to fit a measured GPU budget.

## Validation and evidence

- 159 CPU tests passed across engine, terrain renderer, world, database and cooker.
- Native near-terrain image test passed.
- Native terrain streaming integration passed: movement, rebasing, contact/budget
  transitions, regional live editing and publication failure/retry.
- Rust formatting and `git diff --check` passed.
- Clippy completed with workspace warnings.

`summary.json` contains source sizes and compact GPU results. `evidence.zip`
contains source-query results, the diagnostic shader/configuration, profiler raw
logs, telemetry, input hashes, reports and test logs. Binaries and world databases
are identified by the profiler's hashes rather than duplicated. `manifest.json`
records each archived member's SHA-256.

After extracting the archive, its profiler report can be regenerated with:

```sh
python3 tools/grass_profile.py report PATH_TO_EXTRACTED_EVIDENCE/neighbor-probe
```
