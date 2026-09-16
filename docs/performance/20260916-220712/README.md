# Preserved evidence: GP-020 matched preparation control

M2 Max, 96 roots/m², default preparation capacity 131,072, fullscreen 120 fps,
4× MSAA, Balanced. World 2592×1626, surface 3456×2168. Sixty-second warmup and
900-second measurement. Same binary/database/canopy/shaders as GP-019;
only preparation capacity changes.

**119.557 average app fps; 119.394 in the final five minutes.** Pressure returns
to nominal at 7:26 into measurement. Brief late stalls recur, including a 73 fps
one-second app window. Final-five-minute GPU power is 19.48 W versus 21.01 W with
enlarged preparation; GPU duration is longer (6.44 versus 4.90 ms). This pair does
not support promoting the enlarged arena for energy. See the
[comparison and decision](../../GRASS_PREPARATION_POWER_COMPARISON_20260916.md).

- [report.html](report.html), [report.json](report.json), [report.md](report.md):
  original reports, unchanged. HTML includes cadence and power/thermal timelines.
- [analysis.json](analysis.json): 10/30-second and five-minute groups, thermal
  transitions, focused event windows, nearby HUD packet summaries and user feedback.
- [comparison.json](comparison.json): matched settings/hashes, whole-run and final
  five-minute comparison, power differences and explicit limitations.
- [user-observation.json](user-observation.json): user's mostly-similar end-of-run
  heat/fan impression, uncertain earlier fan ramp and larger GPU-time impression.
- `evidence.zip`: raw power/game logs, original reports, all session/run/input
  metadata, collector command/stderr, canopy snapshot, archived analysis scripts,
  runner/tests/presets and the prior session JSON used in the comparison.
- [manifest.json](manifest.json): sizes/checksums, successful reproduction checks
  and local native-replay input dependency.

Original reports remain unchanged. The archived report analyzer reproduced the
original report exactly; follow-up and paired analysis also reproduced exactly.
All ZIP entry checksums and the local game/database/canopy/shader hashes passed.

For offline reproduction, extract the ZIP into a directory named
`20260916-220712`, enter it, and run:

```sh
python3 -c 'import sys; from pathlib import Path; sys.path.insert(0,"analysis-tools"); import grass_profile_report as r; r.write_report(Path("."))'
python3 analysis-tools/analyze_sustained.py .
python3 analysis-tools/compare_preparation.py .
```

Large replay inputs remain local at
`tmp/grass-profiles/20260916-220712/inputs/density96-default120/` and are not inside
this compact bundle. Prior-session raw logs remain in the separate GP-019 archive;
its report/analysis/user-response/session JSON used here are included under
`comparison-source/`. No fan RPM or temperature series was collected. This is one
chronological pair, not a repeated energy-regression estimate.
