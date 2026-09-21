# Empty-frame isolation, 2026-09-20

The user's screenshots showed 4.31 ms with native-sized Linear presentation and
4.00 ms at 33% with Auto selecting MetalFX Spatial. The game panel simultaneously
reported 1.43/1.53 ms. Terrain, objects, grass, clouds and bloom were disabled;
the first screenshot retained sky/haze and Gaussian shadows. The user then
reported that disabling sky/haze barely changed the result.

These screenshots establish expensive residual work and misleading panel timing.
They do not establish a 4 ms minimum for an empty Bevy renderer.

## Controlled empty-frame results

The new `crates/app_game/examples/empty_frame.rs` installs a camera with no world,
geometry, atmosphere, vegetation or Performance panel. Its composite modes reuse
the actual `game_render.rs`, rather than an approximation of the presentation path.
The plain mode installs neither the game presentation nor the upscaling plugin.

Release build, M2 Max, 3456×1942 physical output, 4× MSAA, normal VSync pacing,
8-second warmup followed by 8 seconds of measurement. The existing game was
temporarily suspended during each comparison and resumed in the runner's cleanup.
There were no other diagnostic game instances running during accepted runs.

| Empty-frame configuration | Input size | Metal HUD GPU median | Mean GPU clock |
| --- | --- | ---: | ---: |
| Stock Bevy HDR camera | 3456×1942 | 0.79 ms | 444 MHz |
| Game presentation, Linear at 33% | 1152×648 | 1.25 ms | 444 MHz |
| Game presentation, Spatial at 33% | 1152×648 | 2.74 ms | 451 MHz |
| Spatial + three empty shadow cascades + depth prepass | 1152×648 | 2.94 ms | 444 MHz |

All four accepted runs were focused, at approximately 120 FPS, without render
errors. [Results and provenance](empty-frame/results.json) include exact commands;
raw HUD logs and telemetry are beside that file. These are short diagnostic
observations, not sustained thermal or production-scene benchmarks. HUD packets
overlap; medians are descriptive and not independent-frame confidence intervals.

The comparison demonstrates approximately 1.5 ms additional empty-frame cost
for Spatial versus Linear in these low-clock runs. The 33% input does not reduce
the 6.7-million-pixel Spatial output or native UI composition target. The regular
HDR resolve, tone mapping, camera output, reconstruction/copy and composition
passes can run without any scene meshes. Empty shadows and prepass added only
about 0.2 ms here; they do not account for the full-game screenshot's remaining gap.

**The full game's roughly 4 ms is not completely attributed.** These results
remove world streaming, mesh preparation, the actual panel and its instrumentation.
They demonstrate a much lower bare-renderer baseline, not a fix for the full game,
nor an additive per-pass breakdown of the screenshot.

## Clocks and timing limitations

Separate live samples at 22:49 local time found 444–629 MHz GPU clocks, compared
with around 1390 MHz in the earlier heavy Temporal runs. Those live readings were
not synchronized to the screenshots or settings changes. Do not extrapolate a
full-clock frame time from them, or equate similar milliseconds across different
clock states with identical GPU work.

Apple documents that the HUD's command-buffer GPU time can include idle gaps;
encoder GPU time is a different aggregate. The screenshots also showed 3.72 ms
encoder GPU time, so the high reported cost cannot simply be dismissed as a
command-buffer-span artifact. See [Apple's HUD documentation](https://developer.apple.com/documentation/xcode/monitoring-your-metal-apps-graphics-performance/).

The game's independently submitted timestamp markers still cannot be treated as
complete frame GPU timing. The example optionally imports those exact probes.
An initial diagnostic import exposed a hard-coded module-name exclusion and
caused a schedule cycle; changing the exclusion to `module_path!()` fixed that
reuse issue without changing which game systems are excluded. Subsequent probe
runs were unfocused, ran around 60 FPS and emitted no Metal HUD packets, so they
were excluded. **No probe-overhead conclusion is drawn from those runs.**

## Reproduction

Build once; compilation must finish before timing:

```sh
cargo build --release -p yarra-app-game --example empty_frame --offline
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 target/release/examples/empty_frame --plain --msaa 4
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 target/release/examples/empty_frame --scale 0.33333334 --upscaler linear --msaa 4
MTL_HUD_ENABLED=1 MTL_HUD_LOG_ENABLED=1 target/release/examples/empty_frame --scale 0.33333334 --upscaler metalfx-spatial --msaa 4
```

Each run exits after approximately 16 seconds. The accepted measurements used a
Retina-enabled app wrapper under `tmp/empty-frame-20260920/`; verify the logged
surface is 3456×1942, the window stays focused and the HUD is available before
accepting another run. `--shadows`, `--prepass` and `--probes` add individual
diagnostic components; `--direct` uses the game presentation's existing direct
path. No normal gameplay settings or upscaling defaults changed in this step.

Validation: release example built; all 32 app tests passed; four bounded GPU runs
completed successfully. No production performance improvement is claimed.
