# Performance measurement

Use [EXPERIMENTS.md](EXPERIMENTS.md) before proposing an optimization. It records rejected approaches as well as retained ones. Current operation is in [WORKFLOWS.md](WORKFLOWS.md); implementation ownership is in [ARCHITECTURE.md](ARCHITECTURE.md).

## Environment milestone before broad profiling

The next accepted workload is one integrated environment scene, rather than isolated grass or tree density trials. Build and visually validate these together before resuming broad performance tuning:

1. A day/night cycle with stable sun/moon lighting and shadows.
2. Weather, particles and fog in the same scene.
3. Tree billboards and their transitions from mesh LODs.
4. A large traversable area with nearby vegetation, distant forests and mountains, covering every terrain/tree LOD and streaming boundary.

Then use repeatable camera routes and feature on/off comparisons to choose density, visual distance, LOD and shadow budgets. Continue bounded correctness checks while building the environment; these are not performance acceptance or a reason to start another optimization campaign.

## What a measurement means

**The in-game GPU markers are not an authoritative whole-frame timer.** The MetalFX investigation found Temporal bloom-on/off marker medians of 6.63/3.93 ms while Metal HUD reported 7.07/6.85 ms. Independent compute markers do not necessarily enclose all dependent graphics/MetalFX work. This affects normal sampled markers and F1 A/B reports, not just detailed probes. Use them for diagnostics; cross-check complete-frame comparisons with Metal HUD/runtime traces. Repair remains open.

| Signal | Valid use | Does not establish |
| --- | --- | --- |
| Application update intervals | CPU-side cadence, stalls, late windows | Actual displayed frames or isolated GPU cost |
| Metal HUD GPU duration | Matched whole-game descriptive comparison | Independent-frame statistics, per-feature cost, fixed-clock work |
| HUD presentation intervals | Detect uneven delivery hidden by average FPS | End-to-end input latency |
| Native MetalFX command-buffer elapsed | Reconstruction plus dependencies within that span | An additive kernel cost to sum with frame/pass times |
| Xcode replay counters | Shader instructions, invocations, traffic, allocations | Live speedup, power saving or sustained thermals |
| System power/clock/thermal telemetry | Compare workload under recorded device conditions | Per-process watts or a PC/iPhone performance prediction |
| Image/instance regression | Equivalence/correctness within exercised cases | Artistic acceptance or affordable runtime cost |

Equal capped FPS can hide very different work and power. Off/on differences can include changed occlusion, residency and GPU frequency; they are not automatically additive feature costs. CPU submission time is not GPU time. Missing/zero GPU samples are unavailable data, not free rendering. Unchanged passes drifting between replays invalidate a claimed fixed-clock speedup.

## F1 comparison

Overview shows cadence, diagnostic GPU/CPU spans, thermal/power mode, render dimensions and residency. Features toggles scene effects; Quality changes resolution/upscaler/AA/detail; Advanced exposes isolation modes and overlays; Compare records A/B.

1. Fix the viewpoint, time/weather, content, render size and FPS cap. Wait for loading and shader compilation.
2. Capture A. The panel closes and controls lock; it settles at least 2 seconds, then samples 10 seconds. Settling beyond 8 seconds or loss of focus cancels.
3. Change one setting; capture B. Detailed GPU probes pause during captures. F1/Escape cancels without erasing the previous slot.
4. Inspect conditions/warnings and the metric's limitations above. Restore A/B restores captured settings/FPS, not camera or weather.
5. Export writes `tmp/performance/comparison-<timestamp>/report.txt` and frame/GPU/CPU CSVs. `YARRA_PERFORMANCE_DIR` changes the root. Slots last for the session.

Reset restores F1's launch settings, including the loaded canopy values and terrain macro setting. Canopy reload and page gizmos live in Advanced and participate in captured settings; the old experiment hotkeys were removed. An externally changed cap cancels capture. Ten seconds is not sustained thermal acceptance.

Disabled means different things: grass Disabled skips render preparation/compute/draw but retains source streaming and joining for canopy/contact consumers; terrain/object switches hide draws while streaming/animation/grounding continue; Cloud Off skips passes but retains weather ambient response and shared state. These are isolation tools, not complete subsystem teardown. Closing F1 does not uninstall instrumentation. Choose its composition at launch:

| Mode | Installed behavior |
| --- | --- |
| `--diagnostics full` (default) | Existing F1, frame/app cadence, CPU system wrappers, supported GPU timestamps and Bevy render diagnostics. Detailed GPU spans and grass counters remain opt-in. |
| `--diagnostics panel` | F1 settings, Reset and A/B frame/app cadence; no CPU system wrappers, GPU timestamps or Bevy recorder. UI/export states that timing probes are disabled. |
| `--diagnostics off` | No F1 entities, panel sampling, CPU wrappers, GPU timestamp probes or Bevy recorder. Shared game settings still apply. |

`--timing-log`, `--gpu-timing-detail` and `--gpu-timing-off` require Full. The last disables both our GPU timestamps and the Bevy recorder; CPU system wrappers remain. Off rejects panel-opening flags, grass counters and input/native-timing logs. Explicit repro/profile routes, finite screenshots, native capture and their structured output remain available in every mode; Off is not a quiet-log switch. Ordinary renderer bookkeeping is retained. Record the mode in comparisons; no instrumentation-overhead improvement has been measured by this refactor.

## Controlled runner

Python 3.11+, the existing Rust toolchain, local assets and a cooked runtime are required. Build once before comparing. The runner snapshots the executable, runtime DB, shaders and canopy settings; other assets remain linked and must stay unchanged. It records actual physical surface/internal dimensions, hashes, commands and validity warnings.

```sh
# Bounded initial check without privileged telemetry.
python3 tools/grass_profile.py run --warmup 15 --seconds 30 --power off

# Rootless telemetry, using an existing local macmon executable.
python3 tools/grass_profile.py run --warmup 15 --seconds 30 --power macmon --macmon /path/to/macmon

# Rebuild reports offline from a recorded session.
python3 tools/grass_profile.py report PATH_TO_SESSION

# A configured comparison; inspect the preset's duration/settings first.
python3 tools/grass_profile.py suite tools/profiles/grass-on-off.json --power off
```

`--power required` uses a single local sudo authentication for `/usr/bin/powermetrics`; `off` collects no power. Do not silently reinterpret a telemetry failure as zero watts. `--no-build` reuses the binary and still records its hash. `run --help` and `suite --help` are the option reference.

Runner defaults are fullscreen, `--size game`, 60 FPS, 4× MSAA, Balanced density, 20 seconds warmup and 90 seconds measurement, plus an idle interval. **`size game` follows current game scale (50%), so rerunning an old preset does not reproduce historical 75% pixels automatically.** Specify internal `WIDTHxHEIGHT` for a fixed-pixel comparison. The September 16 fullscreen results used 2592×1626 internal / 3456×2168 output; later terrain tests used 2592×1456 internal. Record both dimensions rather than saying only “fullscreen.”

Keep window focus, device/power source, power mode, display, AA, world/camera, wind and density controlled. No overlapping builds, editor rendering, screen recording, GPU captures or Xcode replay. Reject unfocused/loading/invalid runs and keep the rejection reason. Use a return control (A/B/A) to reveal thermal/clock drift. Record subjective heat/noise separately.

Start with a short diagnostic. Longer heat/power runs need a specific unanswered question; duration alone does not prove equilibrium. Inspect per-second/30-second/final-window cadence and power, not just average FPS. `grass-stream` eventually stops; use `grass-soak` for repeated boundary-crossing movement. Historical long presets are reproduction tools, not mandatory tests for every edit.

## Launch controls and scenarios

The game parses and validates launch options once, before initializing Bevy. `cargo run -p yarra-app-game -- --help` lists accepted flags without opening a window. Unknown/retired flags, duplicate options, missing values and incompatible modes are errors.

Precedence is normal defaults → launch options → repro preset → profile presentation. Profile timing uses `--profile-fps`; combining ordinary `--fps` requires `--profile-native-pacing`, where profile FPS is only the reference deadline. Other `--profile-*` options require a timed or diagnostic profile. Frame/snapshot/prepass/UI repro options require `--render-repro`; smoke checks cannot share route/exit control with profiles or captures. The editor retains its separate launch interface.

| Purpose | Controls |
| --- | --- |
| Scene/presentation | `--world-db FILE`, `--start-view FILE`, `--fps 0\|15..240`, `--upscaler auto\|linear\|metalfx-spatial\|metalfx-temporal`, `--cloud-quality off\|balanced\|high`, `--grass-density balanced\|full\|authored` |
| Panel/logging | `--diagnostics off\|panel\|full`, `--performance-open`, `--render-audit`, `--render-console`, `--timing-log`, `--gpu-timing-detail`, `--gpu-timing-off`, `--metalfx-timing-log`, `--grass-counters` |
| Finite repro/output | `--render-repro NAME`, `--render-frames N`, `--render-snapshot PATH`, `--render-snapshot-frames N,N`, `--render-prepass`, `--render-ui-off`, `--metal-capture NAME.gputrace`, `--streaming-smoke` |
| Timed profile | `--profile-seconds N`, `--profile-warmup N`, `--profile-size game\|WIDTHxHEIGHT`, `--profile-surface WIDTHxHEIGHT`, `--profile-window fullscreen\|windowed`, `--profile-fps N`, `--profile-native-pacing`, `--profile-msaa 1\|2\|4`, `--profile-grass full\|off`, `--profile-bloom on\|off`, `--profile-temporal-bypass`, `--profile-diagnostic` |
| Grass references | `--grass-vertex-reference`, `--grass-placement-reference`, `--grass-candidate-reference`, `--grass-prepared-blades N`, `--canopy-look FILE` |
| Terrain/output/pacing references | `--terrain-legacy`, `--terrain-reference`, `--terrain-procedural`, `--terrain-prepared-universal`, `--terrain-near-off`, `--msaa-store-reference`, `--temporal-standard-output`, `--frame-pacing-timer` |
| Input/demo controls | `--trace-camera-input` on macOS, `--debug-world-switch` to enable Tab |

Repro names: `low-walk`, `grass-close`, `grass-away`, `grass-follow`, `grass-follow-far`, `grass-zoom`, `grass-top-down`, `grass-overhead`, `grass-stream`, `grass-soak`, `landscape`, `landscape-turn`, `landscape-descent`, `ground-low`, `ground-overhead`, `ground-walk`, `ground-stream`. Landscape routes require a start-view file. `landscape-turn` alternates the bookmarked heading and a 135° turn in place every 600 frames (ten seconds at 60 FPS), dwelling beyond source cooling to check off-screen caster retention. Use at least 1800 frames to include the return heading; snapshots at 660/990 and 1260/1590 compare early/late views after each turn. Repro presets override ordinary camera/presentation settings; inspect the logged configuration. `--terrain-lod` and `--terrain-prepared` are rejected obsolete switches; those paths are defaults. Removed experiment switches are `--grass-field-baseline`, `--grass-bands`, `--vegetation-v2-debug` and `--frame-pacing-display-only`. The previous grass catalog and duplicate overlays were deleted. Shader/reference APIs used by editor studies remain available.

Timed power runs reject frame-limited screenshots/GPU captures. `--profile-diagnostic` is for explicit finite capture presentation, not an ordinary power measurement. `--profile-native-pacing` preserves the normal scheduler; its profile FPS describes the reference deadline. Scripted profiles lock F1's cap.

Specialized tools remain under `tools/`: `grass_game_benchmark.py` isolates shader assets; `summarize_gpu_counters.py` analyzes exported Xcode CSVs. `vegetation_study.py` captures editor studies; ground, blade-band and shape-comparison tools produce visual fixtures, not accepted performance results. Prefer the controlled runner for new comparable performance sessions. Historical shader-injection and shadow/tuft runners were retired; see [refactoring](REFACTORING.md#completed-standalone-experiment-retirement).

## Input and native probes

```sh
cargo run --release -p yarra-app-game -- --fps 60 --trace-camera-input
cargo test --release -p yarra-upscaling --features metalfx temporal_motion_sampling_probe -- --ignored --nocapture
YARRA_TEMPORAL_IMAGES=/tmp/yarra-temporal-grass cargo test --release -p yarra-vegetation-render --features upscaling/metalfx temporal_grass_detail_during_strafe -- --ignored --nocapture
```

The first nonzero scroll starts a bounded 20-second input trace in `tmp/camera-input-<timestamp>-<pid>/`; normal exit saves a shorter trace. It observes only this game's window and removes its monitor after capture. `native.csv` records AppKit timestamps/deltas/phases; `frames.csv` records batches consumed, clamp/rejection state and requested/applied camera movement. Compare individual batches and cumulative displacement before calling an empty batch a lost event. Times exclude smoothing/presentation and are not end-to-end latency.

Temporal probes are explicitly ignored native correctness/image diagnostics. The grass strafe fixture reproduces unresolved blur; producing its images is not a passing visual acceptance threshold. For runtime cap transitions, `frame_pacing_switch` is a bounded example using the production pacing module. Physical-device build/capture instructions are in [iOS](../ios/README.md).

## Evidence policy

Append one compact entry to the experiment ledger: date/ID, question/change, decision, device/build/workload, observed metric, limitations and evidence location. Distinguish retained, rejected/reverted, superseded, diagnostic-only and unresolved. Keep before/after hashes and raw logs for consequential decisions. `docs/performance/` holds machine-readable summaries, manifests, bounded evidence archives and selected reproduction sources/images; it is evidence, not another set of guides. Historical narrative is recoverable from Git (`2afce93` predates consolidation).

Historical manifests describe the original capture bundles, including generated Markdown inside their evidence archives; they are not inventories of the five maintained guides. Keep their original hashes intact.

Do not create a new standalone Markdown report for each run. Update an existing experiment entry for follow-ups. Keep generated verbose reports local; preserve a compact machine-readable result/raw archive when warranted. Never convert instruction/counter reduction into an unmeasured FPS/power claim, or a short Mac result into mainstream-PC/iPhone acceptance.
