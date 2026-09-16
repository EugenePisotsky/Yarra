# Preserved evidence: GP-015 fullscreen baseline

M2 Max, September 16, 2026. Fullscreen 3456×2168 surface / 2592×1626 world,
normal 75% scale, current 72-root authored density, Balanced thinning, 4× MSAA,
120 fps target, 60-second warmup and 900-second continuous measurement.

**119.946 app fps overall; 119.897 in the final five minutes.** Pressure was
temporarily elevated and returned to nominal around 6:10 into measurement.
The report's `CHECK` is retained. The user said fans were not loud and did not
check case heat. See the [interpretation](../../GRASS_FULLSCREEN_120_20260916.md)
and [optimization log](../../GRASS_OPTIMIZATION_LOG.md).

- [report.json](report.json): original parsed report, one-second app cadence,
  power samples, settings, resolution/display audit and input hashes.
- [analysis.json](analysis.json): 30-second/five-minute groups, HUD pacing,
  thermal transitions/durations, input comparisons and user observation.
- [comparison-timeline.json](comparison-timeline.json): timing of the earlier
  windowed 120 visits and this continuous run, including warmup. This comparison
  does not isolate the cause of different thermal responses.
- `evidence.zip`: unedited raw power/game logs, collector command/stderr,
  session/run/input manifests, canopy snapshot, original HTML/Markdown reports,
  analyzer and follow-up analysis script, comparison input hashes and user response.
- `manifest.json`: sizes, SHA-256 checksums and round-trip verification.
- [follow-up-context.json](follow-up-context.json): user's later clarification that
  they normally keep other heavy workloads closed and the game focused. Added
  separately so the original evidence archive stays unchanged.

The archived analyzer exactly reproduces the original report. Running the archived
`analysis-tools/analyze_sustained.py` on that regenerated report also exactly
reproduces `analysis.json`. No analyzer or game changes were made in this turn.

The executable, database and shader/asset snapshot remain in local
`tmp/grass-profiles/20260916-200703/inputs/fullscreen120/`; hashes are preserved here.
This bundle supports offline reanalysis after temporary-directory cleanup, but
does not contain all large inputs needed to replay the uncommitted build.
