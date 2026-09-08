# MSAA color storage

The game camera now permits resolving and discarding multisample color after
the opaque pass when no supported later pass needs it. The resolved image is
still written. **4× MSAA, shader quality and native resolution are unchanged.**
Depth is still stored. This reduces unnecessary attachment writes; it does not
remove the allocated MSAA texture or turn it into a memoryless texture.

## Measured result — 2026-09-07

Matched current-branch release captures on M2 Max, macOS 26.4.1, Xcode 26.0.1:
prepared 4K ground, overhead camera, native 2560×1440, 4× MSAA, Gaussian shadows,
no prepass/grass/UI, frame 600. Both runs had 49 active prepared pages, focused
windows and nominal thermal state. Only the storage policy differed. No builds
or other trace replays ran during capture; Xcode reported Medium performance
state during each replay.

| Main opaque counter | Preserve | Automatic | Change |
| --- | ---: | ---: | ---: |
| Device-memory bytes written | 55,049,600 | 35,646,976 | −35.25% |
| Texture device-memory bytes written | 25,439,808 | 6,187,648 | −75.68% |
| Depth device-memory bytes written | 28,261,952 | 28,150,720 | −0.39% |
| Device-memory bytes read | 9,873,792 | 9,889,408 | +0.16% |
| Color pixels stored | 7,372,800 | 3,686,400 | −50% |
| Fragment invocations | 3,747,264 | 3,747,264 | unchanged |
| Texture sample calls | 126,079,808 | 126,079,744 | effectively unchanged |

The confirmed saving is **19.40 MB of device writes per captured ground frame**.
Texture writes are a subset of total writes; do not add these rows. Depth remains
the bulk of the remaining attachment traffic. Xcode's "Stored pre-resolve MSAA"
insight appears in the reference and disappears with automatic storage. Its
57.25 MiB resource-size estimate is not the measured traffic saving.

Separate game screenshots at frame 600 are **byte-identical at 2560×1440**.
They were collected separately from the GPU captures to avoid adding screenshot
readback work to the profiled frame.

The exported replay timings are 1.037 → 0.575 ms for the opaque pass and
1.520 → 0.933 ms summed across encoders. **Do not interpret this as a measured
44.5% pass speedup.** The same automatic trace initially replayed at 1.87 ms
total, then 0.933 ms after restarting Xcode to fix a disabled CSV export dialog.
Both reported Medium performance state, and unrelated passes also varied.
This run establishes eliminated writes and preserved output, not a reliable
live frame-time, power or thermal improvement. iPhone runtime behavior remains
unmeasured.

Evidence: [counter comparison](../tmp/msaa-color-store/comparison.json),
[replay observations](../tmp/msaa-color-store/replay-observations.json),
[image comparison](../tmp/msaa-color-store/image-comparison.json), and the two
traces, raw CSV exports, PNGs and capture logs in `tmp/msaa-color-store/`.
Executable SHA-256:
`9f180a6b440f9d6013f6a16d5885cf7820e23fcdd303ca02c125846b67bd28f0`.

## Lifetime policy

`MsaaColorStorePolicy::Automatic` is enabled on the main game camera. Unmarked
cameras and `Preserve` cameras retain Bevy's original store behavior. Use
`--msaa-store-reference` for a same-executable comparison.

The automatic policy retains multisample color if any of these apply:

- The target is single-sampled.
- Another camera shares the same multisample texture, even if it has no draws.
- The transparent or transmissive phase is nonempty, or its entry is unavailable.
  Gizmos use the transparent phase and therefore also retain color.
- The wireframe phase has draws.
- Atmosphere, order-independent transparency or volumetric fog is enabled.

Opaque and alpha-mask draws, including the skybox, stay in the original opaque
pass. UI consumes resolved color and does not require preserving the multisample
attachment. Any future custom pass that loads multisample color must set the
camera policy to `Preserve` or extend the consumer check.

## Bevy adapter

`crates/engine/src/msaa_store.rs` adapts Bevy 0.19.1's opaque pass. Its only render
attachment change is `StoreOp::Discard` for eligible multisample color. It keeps
the original depth store action, viewport, draw phases and diagnostic span.
The Metal encoder is labeled `main_opaque_pass_3d_resolve_only` when this path is
selected; the reference label remains `main_opaque_pass_3d`.

The plugin replaces the original system in its existing, uninitialized Core3d
schedule graph slot. This preserves ordering edges attached to Bevy's original
system type set. Startup assertions reject an incompatible schedule. Recheck
the adapter against upstream when upgrading Bevy; the adapted source's MIT
license is included in `third_party/BEVY-MIT.txt`.

The audit log's `msaa_store_policy` records the requested policy. Inspect the
captured encoder label to see whether the automatic policy actually discarded
color in that frame.

## Validation and measurement

The native Metal regression test passed on M2 Max, with byte-identical stored
and automatic images in nine cases: opaque/alpha-mask/UI, depth prepass,
transparency, screen-space transmission, gizmos, sub-viewport, single sample,
resized 4× target, and shared target with an empty later camera. It also checks
the actual discard decisions, fresh readback, pipeline errors, and ordering of
opaque → transmissive → transparent passes. All 17 ordinary engine tests passed.
The macOS release game build and iOS `aarch64-apple-ios` compile check passed.
Atmosphere, OIT and volumetric fog are conservatively excluded by component
checks; they were not exercised as visual scenarios in this test.

Run the native test with:

```sh
cargo test --offline --locked --release -p yarra-engine msaa_store \
  -- --include-ignored --nocapture
```

For a matched capture, use `--render-repro ground-overhead --terrain-prepared`
for both runs and add `--msaa-store-reference` only to the reference. Keep 4×
MSAA and the same executable/assets. Compare exported Xcode encoder counters:

```sh
python3 tools/summarize_gpu_counters.py reference.csv automatic.csv \
  --label-alias main_opaque_pass_3d_resolve_only=main_opaque_pass_3d \
  --output comparison.json
```

Aliasing only groups comparable pass names; raw encoder labels remain in the
JSON. Replay timings are not live frame latency or battery/thermal measurements.
