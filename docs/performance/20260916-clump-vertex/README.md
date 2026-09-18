# GP-023 preserved evidence

[Results and decision](../../GRASS_CLUMP_VERTEX_20260916.md): clump-lighting transfer
tried and reverted. Fifteen final frozen comparisons are byte-identical. Main
fragment ALU instructions fall 6.06%, vertex instructions rise 5.72%, combined
main ALU falls only 1.52%. Repeated timings overlap; no energy or heat claim.

- `evidence.zip`: baseline/candidate shader sets, rejected initial flat variant,
  candidate/test patches, capture script/dependencies, exact input hashes, canopy,
  commands/logs, four Xcode CSV exports, replay observations and native/unit logs.
- [analysis.json](analysis.json): matched inputs, per-profile counters, mean work
  counts, percentage changes and interpretation limits.
- [analyze.py](analyze.py), [summarize_gpu_counters.py](summarize_gpu_counters.py):
  offline reproduction, with no game launch, GPU work or sudo.
- [manifest.json](manifest.json): every ZIP entry's size and SHA-256.

From the repository root:

```sh
python3 docs/performance/20260916-clump-vertex/analyze.py
```

The output reproduces `analysis.json`. Native replay additionally depends on local
`tmp/grass-clump-vertex-20260916/{baseline,candidate}/frame.gputrace` and frozen
binary/database copies; these large files are not in the compact ZIP. The shader
baseline is checkpoint `75f8398` (already contains GP-022). The rejected candidate
patch applies to that checkpoint. Native A/B uses the archived test and sets
`YARRA_GRASS_REFERENCE_SHADERS` to the baseline shader directory.

The initial `native-shading-flat.log` records the failed MSAA check. The
interpolated and final guarded logs both pass all 15 cases. The restored unit log
confirms the renderer was checked after reverting the candidate.
