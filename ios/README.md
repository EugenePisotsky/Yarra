# iOS build and capture

From the repository root, restore local assets and initialize the world using [workflows](../docs/WORKFLOWS.md), then install the physical-device target:

```sh
rustup target add aarch64-apple-ios
```

Open `Yarra/Yarra.xcodeproj`, select a physical iPhone and run **Yarra**. Its build phase cooks `content/world.project.sqlite`, cross-compiles Rust and packages the runtime database/shaders/local runtime assets. Existing source edits are retained.

Use **Yarra Performance** for measurements: Release with LLDB console, Metal API validation/GPU capture/thread checkers disabled. The ordinary **Yarra** scheme remains for debugging/captures. Debug builds and the optional simulator (`aarch64-apple-ios-sim`) are unsuitable for GPU performance acceptance.

Both shared schemes enable `--render-audit` to open the touch-accessible Performance panel. Use its quality/features/compare controls; **Lock controls** holds player/camera still. The current controls, defaults and capture limitations are documented once in [performance](../docs/PERFORMANCE.md). The old automatic 160-second baseline was removed; use bounded F1 A/B comparisons or an explicit repro instead.

Keep `RENDER_AUDIT` and Metal HUD records together. `--render-console` also writes audit records to stderr for `devicectl --console`; normal OSLog is not included there. `--render-repro low-walk` enables this automatically and overrides ordinary presentation/camera settings. Verify effective resolution, MSAA and workload in the log. Optional grass diagnostic counters default off; enable them only for a specific comparison.

For one native GPU capture, use `MTL_CAPTURE_ENABLED=1` and:

```text
--render-repro low-walk --metal-capture NAME.gputrace
```

The app captures at extracted frame 600 to `Documents/MetalCaptures/NAME.gputrace` in its data container, then exits. Use an unused name. Transfer with `devicectl device copy from`, domain `appDataContainer`, identifier `com.pisotsky.Yarra`. Stop replay before live measurements; omit capture arguments/environment for sustained runs.

The [experiment ledger](../docs/EXPERIMENTS.md) preserves earlier phone findings and their limits. Sustained production-scene 60 FPS and heat remain open gates; desktop results do not establish phone acceptance.
