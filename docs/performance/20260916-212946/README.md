# Preserved evidence: GP-019 denser sustained candidate

M2 Max, 96 authored roots/m², Balanced, enlarged 524,288-blade preparation,
4× MSAA, fullscreen surface 3456×2168 / world 2592×1626. Sixty-second warmup,
15-minute measurement at a 120 fps target. AC, battery 100% charged at startup.

**119.600 average app fps; 119.266 in the last five minutes.** Initial thermal
pressure recovered by 7:05 into measurement. Three later brief stalls under
nominal pressure had lowest one-second app windows of 84.38, 94.06 and 77.31 fps;
HUD presentation samples also show the stalls. The user's observation was heat,
a louder fan ramp, then quieter/stabilized operation. No measured fan RPM or
temperature series is available. See the [interpretation and next test](../../GRASS_DENSITY96_SUSTAINED_20260916.md).

- [report.json](report.json), [report.md](report.md): original report, unchanged.
- [analysis.json](analysis.json): 10/30-second and five-minute windows, focused
  stall intervals, thermal transitions, input comparisons and user observation.
- [reanalysis/report.html](reanalysis/report.html): updated analyzer exposes
  short-window cadence loss and an app-cadence/thermal timeline. All original
  power/timing values remain identical. Original HTML is inside the ZIP.
- [user-observation.json](user-observation.json): verbatim user feedback and limits.
- `evidence.zip`: original raw power/game logs, session/run/input metadata,
  collector command/stderr, original reports, follow-up analysis and both analyzer
  versions, runner/tests/presets, canopy snapshot and comparison input hashes.
- [manifest.json](manifest.json): entry sizes/checksums, exact-reproduction results
  and local replay-input dependencies.

After extracting `evidence.zip` into a temporary directory, the original analyzer
at `analysis-tools/grass_profile_report.py` can regenerate the original report:

```sh
python3 -c 'import sys; from pathlib import Path; sys.path.insert(0,"analysis-tools"); import grass_profile_report as r; r.write_report(Path("."))'
python3 analysis-tools/analyze_sustained.py .
```

The analysis uses the extracted directory's name as its session label; keep the
name `20260916-212946` for an identical label. The updated analyzer is preserved at
`reanalysis-source/tools/grass_profile_report.py`; choose a separate output copy
of the raw inputs when regenerating it to retain the original reports.

Large executable/database/shader files remain local at
`tmp/grass-profiles/20260916-212946/inputs/density96-prepared120/`. The next control
preset uses identical hashes from `tmp/grass-draw-density-20260916/density96-prepared/inputs/`.
Those local dependencies are required for native replay; this compact archive
supports offline analysis without them. Prior evidence archives remain unchanged.
