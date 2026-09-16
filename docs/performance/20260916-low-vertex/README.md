# GP-022 preserved evidence

See [results and decision](../../GRASS_LOW_VERTEX_20260916.md). Retained shader
branch: 13.74% fewer main-pass vertex ALU instructions, unchanged vertex counts,
12 byte-identical frozen-image comparisons. No demonstrated sustained speedup,
power saving or heat reduction.

- `evidence.zip`: baseline/candidate complete shader sets, input hashes, canopy,
  capture commands/logs, four Xcode encoder CSVs, observed replay settings/order,
  capture helper and its dependencies, implementation diff and test logs.
- [analysis.json](analysis.json): matched-input checks and per-profile counters.
- [analyze.py](analyze.py), [summarize_gpu_counters.py](summarize_gpu_counters.py):
  exact offline analysis tools. Run `python3 docs/performance/20260916-low-vertex/analyze.py`
  from the repository root; it reads the ZIP and prints the analysis without
  launching the game or accessing a GPU.
- [manifest.json](manifest.json): ZIP-entry sizes and SHA-256 hashes. Large GPU
  capture directories and game/database copies remain local at
  `tmp/grass-low-vertex-20260916/{baseline,candidate}/`.

To repeat the native image comparison with these shader versions, extract the
ZIP, install its candidate shader set into a compatible checkout, and set
`YARRA_GRASS_REFERENCE_SHADERS` to the extracted `baseline/shaders` directory:

```sh
cargo test --offline -p yarra-vegetation-render shading_matches_reference_in_frozen_scene -- --ignored --nocapture
cargo test --offline -p yarra-vegetation-render prepared_blades_match_reference_with_wind_msaa_and_overflow -- --ignored --nocapture
```

The captured binary/database are dependencies of native capture reproduction,
not of offline counter analysis. Replay order is A–B–B–A, all Medium; only two game
frames were captured. The initial unit log contains a source-string assertion
failure corrected before native validation; `unit-tests-final.log` is the passing
suite. Both are retained instead of overwriting the first attempt.
