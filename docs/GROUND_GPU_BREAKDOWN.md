# Ground GPU breakdown — 2026-09-07

The prepared material reduces ground shader work. The earlier rejection based
on nearly unchanged live Metal HUD duration was premature. Keep this optimization
as the ground-material direction; do not restart the renderer or replace AA on
the strength of those HUD numbers.

The candidate remains opt-in (`--terrain-prepared`). This investigation changes
the evidence and evaluation workflow, not the default rendering policy. Memory,
visible streaming transitions and shimmer in motion still need acceptance.

## Matched current-branch captures

M2 Max, macOS 26.4.1, Xcode 26.0.1. Four captures used the existing release
executable and identical source/assets from the prior comparison. There were no
builds during collection and no historical `main` runtime comparison. Xcode
replayed one capture at a time, reporting **Medium** GPU performance state.

Both views: 2560×1440, native direct path, **4× MSAA**, Gaussian shadows, no
prepass, no grass, no UI, frame 600 with a fixed camera. Logs confirm focused
windows, nominal thermal state, 49 resident source pages and, for candidate
runs, 49 active prepared pages. Overhead is `--render-repro ground-overhead`;
the grazing view is `--render-repro ground-low`.

| Replay measurement | Reference | Prepared 4K | Observed change |
| --- | ---: | ---: | ---: |
| Overhead: main opaque pass | 1.309 ms | 0.775 ms | −40.8% |
| Overhead: sum of encoders | 1.657 ms | 1.079 ms | −34.9% |
| Low: main opaque pass | 1.914 ms | 0.615 ms | −67.9% |
| Low: sum of encoders | 2.443 ms | 1.024 ms | −58.1% |

These are **single captured frames replayed by the profiler**, not a repeated
live benchmark or a fixed-clock speedup guarantee. Some unchanged passes also
vary between replays. Encoder sums are not a live frame deadline; overlaps and
submission gaps matter. Do not substitute these numbers for yesterday's HUD
duration or derive battery/temperature results from them. Apple documents the
effect of [GPU performance state on profiling](https://developer.apple.com/documentation/xcode/optimizing-gpu-performance).

The instruction and sample counters independently confirm less shader work:

| Main opaque counter | Overhead reference → prepared | Low reference → prepared |
| --- | ---: | ---: |
| Fragment ALU instructions | 6.101B → 4.681B (−23.3%) | 5.048B → 3.793B (−24.9%) |
| Texture sample calls | 165.413M → 126.080M (−23.8%) | 124.453M → 89.856M (−27.8%) |
| Fragment invocations | 3,747,264 → 3,747,232 | 3,825,728 → 3,825,632 |
| Average pixel overdraw | 1.0165 → 1.0165 | 1.0378 → 1.0378 |
| Average anisotropic level | 2 → 2 | 6 → 6 |

These hardware counters include lighting/shadows and filtering work; they are
not the five-versus-twelve logical material lookup counts in the source shader.

The ground pass accounts for 79.0% / 78.3% of reference encoder time in the two
views. Its fragment stage accounts for 98.5% / 99.3% of that pass. Rendering the
three shadow maps is only 0.073 / 0.061 ms in these reference captures. This
separates shadow-map rendering from shadow sampling inside the ground fragment
shader; the latter is included in the expensive main pass.

Texture read is the largest reported fragment limiter (74.5% overhead, 88.5%
low for reference). Limiter percentages overlap and must not be added as shares
of execution time. Prepared ground actually reads more device memory in this
pass: 4.53 → 9.91 MB overhead and 13.71 → 36.70 MB low. Fewer filtered samples
and less shader arithmetic help despite those extra reads. This is not evidence
that reducing total DRAM traffic alone would solve the ground cost.

## AA and render-target storage

4× MSAA is retained throughout. The measured fragment invocation count is near
one screenful, not four screenfuls. This scene's expensive fragment shader is
not automatically executing four times per covered pixel because MSAA is 4×.
It still pays for multisample coverage, depth/color storage and resolving.

Xcode reports **Stored pre-resolve MSAA** for the main opaque color attachment
(57.25 MiB resource). The current Bevy opaque pass calls
`ViewTarget::get_color_attachment()` and stores multisample color even though
this ground frame has no subsequent color consumer before resolve output is
used. It also stores depth. Total main-pass device writes are about 55 MB
overhead / 46 MB low in both materials, including color and depth; the Xcode
resource size must not be added to those traffic counters.

A later change should resolve color and discard its multisample storage **when
there are no later consumers**, preserving 4× MSAA. This follows Apple's
[load/store guidance](https://developer.apple.com/documentation/metal/setting-load-and-store-actions).
Do not globally change opaque store actions: Bevy's later transparent pass
loads color/depth, and future effects may also consume them. A safe implementation
needs an explicit attachment lifetime policy or a combined final scene pass,
plus transparency/effect validation. No saving from that change is claimed yet.

## Next implementation priorities

1. Keep the prepared material and evaluate movement, streamed-page fallback
   transitions and finite pattern repetition. The previous still comparisons
   are close, but they do not settle these motion questions.
2. Reduce the candidate's memory overhead without losing texel density. The
   previous live HUD reported +73.31 MB for the 4K candidate. The smaller 2K bake
   was only timed with the HUD; its real pass timing and detail tradeoff remain
   unverified. Do not promote it based on the old 1–3% HUD differences.
3. Address unused MSAA storage with consumer-aware render passes. Keep 4× as the
   visual reference; introducing several new AA techniques is not required to
   make progress on the measured material cost.

The stock Bevy `RenderDiagnosticsPlugin` uses timestamp writes inside passes.
Its implementation only records pass GPU timestamps when the device exposes
`TIMESTAMP_QUERY_INSIDE_PASSES`. Metal stage-boundary timestamp support alone
does not satisfy that requirement. A trustworthy live pass logger requires
feature verification and potentially additional pass-boundary instrumentation;
no misleading CPU-as-GPU timing fallback was added.

## Reproduce and inspect

Capture from the release app with `MTL_CAPTURE_ENABLED=1`, the desired
`--render-repro` and `--metal-capture /absolute/new/name.gputrace`. Repeat with
`--terrain-prepared`. The capture automatically saves frame 600 and exits.
In Xcode, replay with profiling, then Performance → Counters → Share →
Export Encoder Counters. Stop replay before collecting another live capture.

```sh
python3 tools/summarize_gpu_counters.py \
  'tmp/ground-gpu-breakdown/reference Counters.csv' \
  'tmp/ground-gpu-breakdown/prepared Counters.csv' \
  --output tmp/ground-gpu-breakdown/overhead-comparison.json
```

The tool validates CSV widths, repairs Xcode's unquoted decimal-comma percentage
fields and preserves duplicate column names. It reports encoder sums explicitly,
not FPS. The same command with `reference-low` / `prepared-low` produces the low
comparison.

Evidence: [overhead counters](../tmp/ground-gpu-breakdown/overhead-comparison.json),
[low counters](../tmp/ground-gpu-breakdown/low-comparison.json), and the four traces,
raw CSV exports and capture logs in `tmp/ground-gpu-breakdown/`.
Executable SHA-256:
`1727883eceaf3012bb5d7b779752f8f67fd2f0a227684a62c9e30d42fc333da8`.
Original asset/source identity is in
[prepared build identity](../tmp/performance-review/prepared-build-identity.json).
