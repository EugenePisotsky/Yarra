# GP-013: native fullscreen GPU interval trace

September 16, 2026. This is **direct GPU execution attribution**, distinct from
powermetrics energy estimates and CPU command-encoding durations. See the
[optimization log](../../GRASS_OPTIMIZATION_LOG.md) and
[powered fullscreen baseline](../../GRASS_FULLSCREEN_120_20260916.md).

The recorder attached Metal System Trace to the same immutable game binary,
database, canopy and shader inputs as session `20260916-200703`, after 20 seconds
of warmup. Fullscreen surface 3456×2168, world 2592×1626, Balanced/current 72-root
density, 4× MSAA, low-walk and 120 fps cap. The three-second recording produced
3.870 seconds of trace data; analysis uses its interior seconds 1–3.

| Named GPU work / stage | Mean per encoder invocation | Observations |
| --- | ---: | ---: |
| Main opaque 3D render — vertex stage | 2.203 ms | 236 |
| Main opaque 3D render — fragment stage | 1.985 ms | 236 |
| Grass visible generation and finalization — compute | 0.516 ms | 237 |
| Shared blade curves/wind preparation — compute | 0.071 ms | 237 |
| Grass work scheduling — compute | 0.010 ms | 237 |
| UI/composite render — fragment stage | 0.286 ms | 237 |

These means use the **`metal-gpu-intervals`** table, filter to the game's PID,
and group GPU segments by encoder and hardware stage. Only encoders wholly inside
the interval enter the means. The exporter also exposes an encoder-creation table;
its CPU durations are deliberately not used. The trace's frame-number field is
not treated as an application-frame counter. Stages can overlap; their sum is not
live frame latency. The raw interval export and script reproduce the summary.

The main opaque pass includes grass, terrain and other opaque objects. This trace
does **not** split their draw/shader costs, identify a hardware limiter or measure
watts. Default/adaptive GPU performance state was used, with no shader counter set
or shader timeline. These are short instrumented moving-scene measurements, not
fixed-clock capacity or a sustained headroom estimate.

The immediate finding is useful: main rendering merits the next detailed capture;
scheduling is much smaller. Both vertex and fragment work matter. Do not call the
whole main pass “grass cost”, assume fragments alone dominate, or repeat a small
scheduler optimization before attributing the expensive draws. The next step is
a matching Metal frame capture with individual draw counters/limiters, followed by
one bounded implementation comparison and sustained validation.

## Evidence and recovery from the initial attempt

- [analysis.json](analysis.json): measured GPU stage intervals and method/limits.
- [metadata.json](metadata.json): controlled settings and recording context.
- `evidence.zip`: raw GPU-interval XML, analysis script, game/trace logs, input
  hashes and commands. Checksums and exact reanalysis verification: `manifest.json`.
- The complete native trace remains locally at
  `tmp/grass-gpu-attribution-20260916/short/fullscreen.trace` (about 98 MiB).
  Full Instruments metadata can contain the process environment and was excluded
  from this preserved bundle.

The initial launch-time 35-second trace exceeded the wrapper's 100-second deadline
while finalizing. It produced an unusable approximately 7.4 GiB partial trace;
offline export reported “Document Missing Template Error”. That partial trace was
removed, with failure metadata/commands/logs preserved here. Attaching for only
three seconds after warmup completed successfully. The failed attempt supplied
no performance conclusions and changed none of the earlier power results.
