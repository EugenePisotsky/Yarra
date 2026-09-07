# iOS

Open `Yarra/Yarra.xcodeproj`, select a physical iPhone, and run the `Yarra`
scheme. The Xcode build phase cross-compiles the Rust executable and packages
the cooked runtime database, shaders, character GLB, and imported tree and
terrain runtime files.

Use the Release build configuration for performance measurements. The Debug
configuration is intended only for iteration and validation.

Select **Yarra Performance** for measurements: it runs Release with LLDB attached
so the Xcode console receives audit and Metal HUD logs, while Metal API validation,
GPU capture, and thread checkers are disabled.
The Metal HUD should no longer display the validation/capture badges. The original
**Yarra** scheme remains available for GPU debugging and captures.

The shared Run scheme enables `--render-audit`, a touch panel for ground/grass,
shader, shadow, prepass, resolution, and compute comparisons. Gameplay controls
work by default; **Lock controls** freezes the player and camera for measurements.
Uncheck the argument in Edit Scheme → Run → Arguments to remove the audit UI. See
[`docs/RENDER_AUDIT.md`](../docs/RENDER_AUDIT.md) for findings and the measurement
sequence. The target is sustained 60 FPS; use Metal HUD GPU time and thermal state
to distinguish costs when multiple modes all show 60.

For the combined scene resolution test, **Scene: Current** now shows both ground
and grass on iOS. Tap **Resolution** to cycle **100% → 75% → 50% → 100%**; the UI
stays at native resolution. Scaled rendering selects Composite automatically;
leave that path selected when returning to 100% for a consistent comparison.

For grass shimmer, **AA** cycles **off → 2x MSAA → 4x MSAA → off** on the 3D
camera, independently of resolution. Start at 50% and compare without moving the
camera; leave wind on to expose flickering blade edges. The log includes the
camera's `msaa_samples` and `requested_msaa_samples` (1 means off). Startup/Reset
and the automatic baseline use AA off; the baseline restores your AA setting
when it finishes. MSAA is an edge-quality comparison, not a proven fix for
specular sparkle or a performance optimization.

The iOS game camera and audit Reset baseline now default to **Depth prepass: off**.
The current scene has no effects that need the copied prepass depth texture, and
the flat-ground comparison measured less GPU duration with this pass disabled.
Use the existing toggle to reproduce older captures, which started with it on.
This change still needs a sustained production-scene run on the phone.

For the next baseline investigation, rebuild/run **Yarra Performance**, wait for
the scene to load, then tap **Baseline test (160s)** once. Leave the phone in the
foreground without touching it until the button returns. The test switches
automatically between Clear/Flat, the audit composite/direct rendering, and UI
drawing on/off, at native resolution with shadows and prepass off. It includes
return comparisons and restores your previous settings at completion.
The game UI disappears for 60 seconds during the test; the Metal HUD is separate
and stays visible. Save the entire console as `tmp/logs5.txt`, including
`RENDER_BASELINE`, `RENDER_AUDIT`, and `metal-HUD` lines. A screenshot of the Metal
HUD during the first blank-screen interval (roughly 65–75 seconds after starting)
also supplies Encoder GPU/stage timings absent from the CSV.

**Render: direct** is now the startup/Reset default at 100% resolution. It restores
the game camera's window target and disables the audit's second camera/composite.
Choosing a scaled resolution switches back to composite, preserving native UI.
The completed `log5.txt` Clear comparison measured a GPU-duration difference of
about 2.3 ms for the composite path and another 1.7 ms for visible debug UI. These
are workload-specific differences: `log6.txt` with grass Full measures only about
0.22 ms between Direct and Composite. They are not fixed pass costs or proven
thermal savings. Clear with Direct and hidden UI was
about 0.97 ms; Flat ground was 1.36 ms. Full production-ground thermal performance
still needs measuring with the diagnostic rendering overhead removed.

Keep `RENDER_AUDIT` lines alongside `metal-HUD` when saving the console. Audit
records include initial settings, switches, a snapshot every five seconds, and
thermal/Low Power Mode changes. They include real elapsed time, application frame
counts, camera/resolution, and source residency counters so tests can be compared
without relying on what the on-screen HUD happened to show.

For `devicectl --console`, add `--render-console` to the game arguments. This
also sends audit records to stderr; iOS's normal OSLog output is otherwise not
included in that console stream. `--render-repro low-walk` enables it automatically
and runs a repeatable low-camera route at 75% resolution, 4× MSAA and no prepass.
The route is an explicit diagnostic override; restart without the argument to
restore manual camera control. Grass diagnostic counters default off on iOS
because their readback is unavailable. Use `--grass-counters` with repro to
enable them for an explicit comparison; Metal HUD remains independent.

For a single physical-device GPU capture, launch with `MTL_CAPTURE_ENABLED=1`
and arguments `--render-repro low-walk --metal-capture NAME.gputrace`. The app
captures at extracted application frame 600 into `Documents/MetalCaptures/NAME.gputrace`
inside its data container, then exits. Transfer the directory with `devicectl
device copy from`, domain `appDataContainer`, identifier `com.pisotsky.Yarra`.
The output name must be unused. Stop Xcode's GPU replay before launching the
live game. Omit capture arguments and its environment variable for sustained
performance tests. Reference switches are `--grass-vertex-reference`,
`--grass-placement-reference`, `--grass-candidate-reference`, and `--terrain-procedural`.

For ground tests, select **Scene: Ground**, then tap its shading button to cycle
**Production → Surface unlit → One texture → Flat unlit → Production**. The modes
separate full PBR shading, terrain albedo blending, ordinary texture sampling,
and drawing an untextured surface. Hold each for about 20 seconds after shader
compilation, keep the camera fixed, and finish with Clear. Ground shader and
terrain macro settings are included in the log.

The physical-device Rust target is installed with:

```sh
rustup target add aarch64-apple-ios
```

The simulator is optional and is not suitable for GPU performance testing. An
Apple Silicon simulator additionally requires:

```sh
rustup target add aarch64-apple-ios-sim
```
