# Preserved evidence: GP-009 / GP-010

Session recorded September 16, 2026, on M2 Max. Experiment: native-world 1440p,
4× MSAA, Balanced grass, low-walk, 60/120/120/60 fps. See the
[optimization log](../../GRASS_OPTIMIZATION_LOG.md) and
[detailed interpretation](../../GRASS_POWER_BASELINE_20260916.md).

This directory preserves the measurements outside the ignored `tmp/` tree:

- `report.json`: complete analyzed results after the presentation-pacing correction,
  including per-second app cadence, all parsed power samples, settings and hashes.
- `evidence.zip`: original power log, collector command/stderr, session manifest,
  every game log/run manifest, per-variant input manifests and canopy snapshots,
  the original generated report, and the corrected report's Python analyzer.
- `manifest.json`: SHA-256/byte size of each archived entry and the archive/report
  themselves, source location, archive scope and verification result.

Archive member `analysis-tools/grass_profile_report.py` is the analyzer used for
the corrected report, captured when preserving this session. The original
`report-original.*` is retained to show the earlier average-FPS classification.
Raw game and power logs were not edited. Regeneration from the archived logs was
checked against `report.json` with the archived analyzer.

Large executable, runtime database, shader/asset snapshots and GPU traces are not
included here. The full run inputs remain locally at
`tmp/grass-profiles/20260916-190422/inputs/`, with their hashes in the reports and
input manifests. This archive supports reanalysis of the observations; replaying
the exact game workload still needs those inputs. The recorded Git HEAD had
uncommitted changes and is not by itself a reproduction of that executable.

To reanalyze with the current workspace tools, from the repository root:

```sh
python3 -m zipfile -e docs/performance/20260916-190422/evidence.zip /tmp/yarra-grass-20260916-190422
python3 tools/grass_profile.py report /tmp/yarra-grass-20260916-190422
```

This only reads captured measurements and writes reports; it does not launch the
game or require sudo. A later analyzer may deliberately produce different warnings;
retain this archived result when documenting that correction.
