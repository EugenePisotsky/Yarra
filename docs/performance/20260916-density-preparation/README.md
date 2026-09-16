# GP-017 / GP-018 preserved evidence

See [findings, limits and next command](../../GRASS_DENSITY_DIAGNOSTICS_20260916.md)
and the [optimization log](../../GRASS_OPTIMIZATION_LOG.md).

- Actual measured baseline catalog: **66 roots/m²**, correcting earlier 72 labels.
- Fullscreen frame-600 replay isolates grass as 64.88% of profiled replay work;
  paired low-detail draw 62.21%, grass vertex 42.12%, grass fragment 22.76%.
- Isolated 66/72/96/128 density databases reproduce the original baseline tables
  and change only catalog/runtime metadata. Sampled capacity drops are zero.
- Matched 96-root preparation comparison: 131072 → 524288 physical blade capacity;
  7.218 → 5.660 ms summed encoder work, 5.343 → 3.747 ms main encoder, 42.7% fewer
  main-pass vertex ALU instructions and identical submitted vertices. +48 MiB.
  No sustained energy claim.
- Short frozen-candidate check: fullscreen 2592×1626 world / 3456×2168 surface,
  118.33 updates/s over 15 seconds, HUD interval p95 8.34 ms, nominal app pressure.
  CHECK solely for intentionally absent power telemetry. Long acceptance pending.

The 184 KB evidence ZIP contains raw encoder CSVs for all three captures, native
game/diagnostic logs, exact commands, results, input hashes, exported catalog,
variant table checks, screenshots' comparison metrics including an unchanged
reference repeat, the smoke run/report, and scripts/tool source.

The initial capture folder is called baseline72 but actually contains the
unchanged **66-root** runtime catalog. This historical folder name was retained
to avoid changing recorded paths.

[Replay comparison](preparation-comparison.json),
[visual/counter validation](preparation-validation.json),
[density observations](density-diagnostics.json) are also directly readable.

The manifest verifies every ZIP member, ZIP checksum and all 14 full-resolution
screenshot hashes. Full PNGs, three native Metal traces, binaries and database
copies remain in the local tmp/grass-draw-density-20260916 directory. No process
environment or Instruments TOC is included. Preserve the local frozen inputs
until the next powered run completes. Xcode replay was stopped before all live
diagnostics and before handoff.

The original earlier powered-session evidence is unchanged; separate
DENSITY_ERRATUM.md files point to the catalog correction.
