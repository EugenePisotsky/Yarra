# Linear composition: one isolated change

Explicit Linear previously enlarged the scene into a full-resolution RGBA8 image,
then sampled that image again during UI composition. The game now samples the scene
image directly using its existing linear sampler. This removes the separate
`linear upscale` pass and leaves only a 1×1 placeholder for the unused intermediate.
At 3456×1942, the removed image's pixel storage was approximately 25.6 MiB.

This applies to explicit Linear only. Startup defaults and other backends are
unchanged. Resize and switching back to a separate backend restore the correct
image dimensions, requests and presentation handles.

## Matched comparison

The saved pre-change release example and rebuilt example both used the actual
game presentation code: 75% resolution (2592×1457), 3456×1942 physical output,
4× MSAA, an empty scene, no audit probes, and normal VSync pacing. Each ran for
8 seconds of warmup and 8 seconds of measurement. Both remained focused, rendered
at approximately 120 FPS and completed without errors.

| Version | Metal HUD GPU median | Mean GPU clock |
| --- | ---: | ---: |
| Before | 1.43 ms | 444 MHz |
| Direct Linear composition | 1.42 ms | 444 MHz |

**This comparison found no meaningful GPU-time improvement.** Removing the extra
pass simplifies Linear presentation and eliminates an intermediate allocation,
but does not identify or fix the dominant performance problem. These short Mac
empty-frame runs do not establish sustained gameplay or iPhone performance.

[Raw results](linear-composition/results.json) include executable hashes and exact
commands; HUD logs, telemetry and the bounded runner are alongside them. Overlapping
HUD packets are descriptive samples, not independent-frame confidence intervals.

Validation: all 33 app tests passed; release game and example built successfully.
Tests cover direct Linear startup, switching into Linear, resizing without changing
settings, and switching back to a separate upscale backend.

Launch the changed path with:

```sh
cargo run --release -p yarra-app-game -- --upscaler linear
```

The normal game still starts at 50% resolution. The comparison's 75% setting is
available through F1 → Quality → Resolution.
