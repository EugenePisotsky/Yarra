# Preserved evidence: GP-011

Session recorded September 16, 2026, on M2 Max: 1440p60, 4× MSAA, Balanced grass,
low-walk, grass on/off/off/on. See the
[optimization log](../../GRASS_OPTIMIZATION_LOG.md) and
[interpretation](../../GRASS_ON_OFF_20260916.md).

- `report.json`: full parsed results, per-second application cadence, power samples,
  settings and input hashes. All four visits pass the current report checks.
- `analysis.json`: group averages, paired differences, 30-second windows,
  comparison of input hashes with the earlier suite, and the user's confirmation
  that there were no intentional setup changes between suites.
- `evidence.zip`: unedited power/game logs, collector command/stderr, session/run/
  input manifests, canopy snapshots, original HTML/Markdown reports and the analyzer.
- `manifest.json`: SHA-256 hashes, byte sizes, archive scope and verification.

The analyzer preserved as `analysis-tools/grass_profile_report.py` exactly
regenerated `report.json` from the archived logs. No report correction was needed
for this session. The power spread between off visits is an analysis caveat;
individual-run `OK` labels do not establish repeatability of a comparison.

The larger executable, database and shader/asset snapshots remain locally under
`tmp/grass-profiles/20260916-192957/inputs/`. Their hashes are preserved here.
This archive supports reanalysis independently of temporary-directory cleanup;
replaying the exact workload still requires those larger inputs. Git HEAD alone
does not reconstruct the uncommitted build used in the session.

To regenerate with the current workspace analyzer, from the repository root:

```sh
python3 -m zipfile -e docs/performance/20260916-192957/evidence.zip /tmp/yarra-grass-20260916-192957
python3 tools/grass_profile.py report /tmp/yarra-grass-20260916-192957
```

No game launch or sudo is required. Preserve the original archived result if a
later analyzer deliberately changes interpretation.
