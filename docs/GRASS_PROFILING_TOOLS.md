# Controlled grass profiling

Record results and optimization decisions in the
[grass optimization log](GRASS_OPTIMIZATION_LOG.md). Its evidence-preservation
convention keeps important sessions available after local `tmp/` cleanup.

Use the profiling runner to compare **sustained power, clocks and frame delivery** on
this Mac. Similar GPU milliseconds can hide different energy use as the GPU changes
its performance state. The tool does not lock clocks or convert Mac results into PC
performance. The shipping target remains 1440p60 on named mainstream PC GPUs.

## Start here

From the repository root, in a local terminal:

```sh
python3 tools/grass_profile.py run
```

This builds the release game, snapshots its inputs, asks for sudo authentication
once if needed, records ten seconds with the game closed, then runs **borderless
fullscreen on the primary display, normal 75% world scale, 4× MSAA, Balanced
density, 60 fps**. It holds the starting camera for 20 seconds of warmup and
measures a 90-second low-camera route. Both actual world pixels and presentation
surface pixels appear in the report. Keep the game focused and leave other
GPU-heavy applications closed.

The fullscreen default was added after the first two powered suites. Those used
a 1280×720 logical window, a 2560×1440 physical presentation surface and an
explicit 2560×1440 world target. They were genuine 1440p world tests, but do not
establish fullscreen performance. Normal 75% scale means 75% of each **physical**
surface dimension; 1920×1080 was specific to the smaller window, not a fixed game
resolution. `--size game` follows that scale and the fullscreen aspect ratio.
`--size WIDTHxHEIGHT` instead fixes internal pixels independently of the surface.

The runner needs Python 3.11+ and the project's existing Rust toolchain and cooked
runtime database. It uses only Python's standard library. The build is offline and
locked; on a fresh checkout, set up dependencies and cook the world first as described
in the project README. `--no-build` reuses the current release binary and still
records its hash. Run from this Mac's desktop session, not a headless shell.

For a sustained heat test, allow more time:

```sh
python3 tools/grass_profile.py run --warmup 60 --seconds 600
```

A duration alone does not prove thermal equilibrium. Inspect the power/frequency
timeline and thermal states. Keep power source, macOS power mode, display setup,
brightness and ambient conditions consistent between sessions. Avoid building,
screen recording, GPU capture or interacting with the editor during measurement.

### Sustained 120 fps investigation

The immediate Mac question is now sustained fullscreen 120 fps with denser grass.
The existing windowed 1440p result missed 120 under thermal pressure, but the
subsequent [15-minute fullscreen run](GRASS_FULLSCREEN_120_20260916.md) held
119.946 fps overall and 119.897 in the final five minutes. Pressure was temporarily
elevated, then recovered while the workload continued. Fans were not loud according
to the user; case heat was not checked. Preserve both outcomes rather than treating
the short windowed failure as an absolute hardware ceiling.

The current-density fullscreen baseline has completed as GP-015. This preset uses the normal
75% world scale and runs one continuous 15-minute measurement after a 60-second
warmup. Repeat it for promising optimization/density candidates:

```sh
python3 tools/grass_profile.py suite tools/profiles/grass-fullscreen-120.json
```

Equivalent: `python3 tools/grass_profile.py run --window fullscreen --size game
--fps 120 --warmup 60 --seconds 900`. This keeps the display's existing mode; it
does not switch to a lower display resolution or force a 120 Hz refresh mode.
The report records the monitor's advertised refresh rate as context, not measured
presentation. Resolution changes do not imply proportional savings: generation
and geometry work remain. Fullscreen also changes presentation conditions.
`low-walk`, the default view, repeats its route throughout the run. The existing
`grass-stream` route stops after its first outward/return trip, so do not treat
a long run of that view as continuous traversal.

Read the per-second application samples and power timeline in 30-second groups,
including the final five minutes, rather than accepting the whole-run average.
The report's `OK` is a screening result; it does not certify steady thermals or
low fan noise. Record subjective heat/noise separately. Keep candidate density
in a separately cooked `--world-db` and record near-detail reach alongside it;
`--density full` selects a different thinning policy, not more authored roots/m².

## Sudo is limited to one collector

Run **Python as your normal user**. The tool uses the local terminal for `sudo -v`
and then starts one finite `sudo -n /usr/bin/powermetrics` process for the entire
session, including multi-run suites. The game, snapshotting and report generation
are unprivileged. Password expiry cannot interrupt a collector already running.
No password is read by Python, saved, or passed through an argument or pipe. No
sudoers changes or background service are installed.

Ctrl+C stops the game and collector and preserves partial artifacts. The collector
also has a finite sample limit in case the runner disappears. If sudo cannot
authenticate from a noninteractive session, the runner stops before launching the
game with instructions to run it in a terminal. Sudo credentials can be scoped to
the terminal: authenticating in a different terminal is not a reliable workaround.

For tooling checks or timing-only runs, omit privileged collection explicitly:

```sh
python3 tools/grass_profile.py run --power off
```

This still captures frame cadence, Metal HUD, resolution/settings, residency and
macOS thermal state. It cannot measure GPU watts or frequency. Missing data stays
unavailable, never zero. There is no supported unprivileged substitute in this tool
for the same `powermetrics` CPU/GPU/frequency telemetry.

## Ready-made comparisons

```sh
python3 tools/grass_profile.py suite tools/profiles/grass-60-vs-120.json
python3 tools/grass_profile.py suite tools/profiles/grass-on-off.json
```

Each suite uses A–B–B–A ordering, 30 seconds of warmup and 120 seconds of measurement
per run, about eleven minutes overall excluding build/authentication. The camera
and wind use elapsed time, so a 60 fps run follows the same route at the same speed
as 120 fps. Warmup is excluded from summaries. A short inter-run idle gap does
**not** reset temperature; the timeline and repeated order expose drift. Repeat
with the reverse order if the comparison is close.

These two historical comparison presets explicitly retain **windowed 1440p**.
Use `grass-fullscreen-120.json` for the new fullscreen baseline. To repeat the old
single-run configuration, specify `--window windowed --size 2560x1440`.

Grass-off disables the grass rendering workload; it keeps the rest of the scene
and its ordinary streaming. The difference in system power is an A/B outcome, not
an exact isolated grass wattage: clocks, CPU work and uncovered terrain change.
`grass: full` means the normal grass pipeline is enabled. It does **not** mean Full
Reference density; that is a separate `density: full` option.

To compare an optimization, copy a suite and define `baseline` and `candidate`
variants. Each variant can override:

| Setting | Values / purpose |
| --- | --- |
| `window` | `fullscreen` (borderless, primary display; new default) / `windowed` (720 logical pixels high) |
| `size` | `game` (normal 75% surface dimensions; new default) / explicit world pixels such as `2560x1440` |
| `fps`, `msaa` | `60` / `120` / `0` (uncapped), `1` / `2` / `4` |
| `warmup`, `seconds` | Seconds; defaults 20 and 90 |
| `view` | `low-walk`, `grass-close`, `grass-away`, `grass-follow`, `grass-follow-far`, `grass-zoom`, `grass-overhead`, `grass-top-down`, `grass-stream` |
| `density`, `grass` | `balanced` / `full` / `authored`; `full` / `off` |
| `binary` | Previously built release binary, including the timed profile support |
| `world_db` | Separately cooked runtime SQLite database, for example another authored density |
| `shaders` | Directory of replacement `.wgsl` files to overlay on the current shader snapshot |
| `canopy` | Canopy look RON file |
| `vertex_reference`, `candidate_reference`, `placement_reference`, `terrain_reference` | Boolean switches for existing renderer reference paths |
| `terrain_lod` | Boolean; default true. False explicitly launches `--terrain-legacy` for comparison |
| `native_pacing`, `prepass` | Boolean; preserve gameplay pacing / enable the normal desktop prepass in a repro |
| `counters` | Boolean; default false, use only for separate workload diagnostics |

Input paths in JSON resolve relative to the repository root. Relative `--output`
paths resolve relative to the shell's working directory. Inputs are copied before
collection: release binary, shaders, canopy look, and a consistent SQLite backup
including committed WAL contents. Their SHA-256 hashes are recorded. Remaining
assets are linked to the workspace; leave them unchanged throughout the suite.
Archive a matching shader directory when comparing binaries from different commits.
The runner does not automatically rebuild or cook historical variants.

The hierarchy is now the default for new runs and for suite variants that omit
`terrain_lod`. Use `run --terrain-legacy` or an explicit `"terrain_lod": false` in
JSON for the previous renderer. The terrain soak suite already names both variants.
Existing reports retain their recorded settings; regenerating a report does not
apply current launch defaults. Renderer choice is checked against the game log, so
an older binary that ignores the new default cannot silently pass as a hierarchy run.
Cook the selected world with current material baking before a new hierarchy run.

Use a separate counter run to inspect submitted roots/topology, preparation
fallbacks and capacity drops:

```sh
python3 tools/grass_profile.py run --view grass-close --counters --power off
```

GPU counters change the workload, so these runs are labeled diagnostic. The existing
`tools/grass_game_benchmark.py` screenshot workflow and `--metal-capture` mechanism
remain available separately. Timed profiling rejects simultaneous screenshot,
frame-limited exit or Metal capture flags. Use an Xcode capture/Metal System Trace
for pass-level bottlenecks; this runner does not supply hardware counter attribution.

## Reading the output

Every session creates `tmp/grass-profiles/<timestamp>/` containing:

- `report.html`: standalone app-cadence/power/frequency/activity timeline with
  measured windows and thermal pressure shaded; no server or network dependency.
- `report.md`, `report.json`: summary and structured results, including per-second
  application cadence and retained power samples.
- `power.txt`, `power-stderr.txt`, `power-command.json`: original collector output
  and the exact command, when enabled.
- `session.json`, `inputs/`, `runs/*/run.json`, `runs/*/game.log`: order, settings,
  provenance, commands, exit status and raw game logs.

Regenerate a report without sudo or launching the game:

```sh
python3 tools/grass_profile.py report tmp/grass-profiles/<timestamp>
```

`INVALID` means the run cannot support the intended comparison: interruption,
rendering errors, lost focus, wrong resolution/settings, capacity drops or missing
required power data. `MISSED TARGET` means a completed run missed the requested
cadence; this is a performance finding, not discarded data. `CHECK` means a caveat
needs inspection, such as missing optional power, incomplete terrain preparation,
source residency changes, diagnostic counters, elevated thermal pressure or uneven
HUD presentation intervals. Pacing is flagged when HUD interval p95 exceeds 1.25
times the target interval, even if average FPS matches. This is a screening
heuristic on correlated samples, not an exact dropped-frame count.
Since GP-019 the analyzer also warns on any full app sampling window below 95%
of the target rate, and exposes the worst window's rate, duration and time.
Windows shorter than 0.9 seconds are excluded from this check to avoid short final
partial samples. Normally these are approximately one-second windows; this is
neither a frame-time 1% low nor an independently counted presentation rate.
The count/duration of affected sample windows is not the exact duration of stutter.
`OK` only means these checks passed; it does not establish steady-state thermals
or certify shipping performance. Raw artifacts are retained in every case.

Interpret the metrics together:

1. **Frame delivery:** application updates and Metal HUD presentation intervals are
   separate. Update p95/p99/max intervals include waiting and are not CPU execution
   times. Metal HUD packets overlap; their percentiles are not independent-frame
   statistics. Use a platform presentation trace for precise stutter analysis.
2. **Power and energy:** duration-weighted CPU and GPU subsystem estimates. These
   include other processes and exclude some system components. The idle window is
   context, not automatically subtracted. CPU+GPU mJ/application frame is a proxy,
   not energy attributed to one grass frame or a counted presentation.
3. **Clock/activity/thermal context:** similar milliseconds at lower power and lower
   clocks can still be an improvement. Rising power, pressure or declining cadence
   across repeated runs warrants a longer test. The tool never declares a winner
   from a tiny GPU-millisecond delta.

Power samples are selected only when wholly inside the measurement window.
`powermetrics` text timestamps have roughly one-second alignment precision. More
than 80% window coverage is required for each metric when power is requested. GPU power is printed
in more than one section; the parser takes the processor section once and does
not add the duplicate. Active-frequency averages are clock context, not a fixed
frequency measurement. The HTML includes warmup/idle samples so drift stays visible.

These repro scenes explicitly disable the depth prepass, following the existing
benchmark. Normal desktop gameplay enables it for participating geometry. A single
scene or cap is not a maximum-density guarantee: validate the final populated game
on the actual Windows GPU/backend at 1440p60. Start with the current production
density, test dense close views and traversal, then vary independently cooked
density/LOD/material candidates. See [the assessment](GRASS_PC_PERFORMANCE_ASSESSMENT.md)
for the implementation and Tsushima comparison.

## Tool verification

The fullscreen addition passed 19 Python and 15 application tests, a release build
and a native fullscreen check: 20-second warmup, 12-second measurement at 120 fps.
The surface was 3456×2168 and the world 2592×1626. Mode, scale, focus and cadence
checks passed; `CHECK` reflects deliberately disabled power collection. See the
[preserved control check](performance/20260916-200334-fullscreen-smoke/README.md).
The updated analyzer also reproduces both earlier suites' metrics and classifications
without altering their archived reports. The subsequent powered
[GP-015](GRASS_FULLSCREEN_120_20260916.md) measured 15 minutes at current density;
larger-density and populated-scene acceptance remain open.

The September 16 implementation was checked with native 1440p release runs at
60 fps, 120 fps, and 60 fps with grass disabled. After a 20-second warmup all three
12-second checks met their requested application and HUD cadence, with settings
and focus checks passing. These short runs verify the controls, not sustained
performance. Artifacts are in `tmp/grass-profile-tools-smoke-suite/` locally.
The five-second-warmup trial correctly reported source preparation and missed
cadence. The parser also read an existing 360-sample Mac power recording.

The user subsequently completed a full powered suite in
`tmp/grass-profiles/20260916-190422/`: all four runs have over 98% power coverage,
no collector stderr, consistent input hashes and clean exits. See
[the first powered results](GRASS_POWER_BASELINE_20260916.md). Those results also
exposed uneven presentation intervals hidden by the average-FPS check; the report
now flags that condition. The authentication failure path was separately verified
to stop before launching the game. Tests cover finite collector
arguments, no password piping, cleanup, missing metrics, warmup exclusion,
duplicate GPU sections, timezone alignment, partial reports and SQLite WAL backup.

```sh
python3 -m unittest discover -s tools -p 'test_grass_profile.py'
cargo test --offline --locked -p yarra-app-game
```

## Authored-density and preparation diagnostics (GP-017 / GP-018)

The actual September 16 runtime baseline is **66 roots/m²**, despite older
72-root RON/HUD labels. See [catalog correction and results](GRASS_DENSITY_DIAGNOSTICS_20260916.md).

To prepare numeric variants without touching the published world:

```sh
cargo build -p yarra-world-cook --offline --locked
python3 tools/grass_density_variants.py tmp/my-density-variants --densities 72 96 128
```

The tool exports the database catalog, validates an unchanged recook, and rejects
any changes outside vegetation catalog/runtime metadata. It requires a fresh
output directory. The current baseline is included automatically. Full Reference
mode is not a numeric density increase.

For diagnostic captures/screenshots, use `--profile-diagnostic` with the existing
`--profile-window`, `--profile-size`, `--profile-fps` and `--profile-msaa`
presentation controls plus `--metal-capture` or finite `--render-frames`.
Camera/wind remain frame-based for matching views. Diagnostic mode cannot be
combined with `--profile-seconds` and emits no timed measurement events.

On macOS 14+, a nonzero `--profile-fps` now uses the same display-link frame cap
as normal gameplay's `--fps`. ProMotion can stay enabled. For a comparison with
the original independent timer, add `--frame-pacing-timer` to a direct game
command. `--frame-pacing-display-only` keeps display callbacks but disables Metal's
minimum presentation interval to isolate update scheduling from presentation.
The startup `FRAME_PACING` logs identify the macOS display link and Metal interval.
`--profile-native-pacing` still preserves the
normal launch's pacing settings, including an explicit `--fps` if supplied.

Runner/suite option `prepared_blades` (CLI `--prepared-blades`) is an opt-in
preparation-arena experiment, integer 32768–524288. It passes
`--grass-prepared-blades` to the game. Default game allocation remains 131072.
Reports require the actual audited allocation to match the requested value,
so a stale binary cannot silently ignore this experiment.

The prepared long-test preset
`tools/profiles/grass-fullscreen-96-prepared-120.json` deliberately references
the immutable local inputs from GP-018. It is ready on this workspace; preserve
its `tmp/grass-draw-density-20260916` input directory. See the result document
for the experiment's +48 MiB memory tradeoff and exact interpretation.

The candidate completed as [GP-019](GRASS_DENSITY96_SUSTAINED_20260916.md), with
temporary thermal pressure and brief late stalls. The matching control preset is
`tools/profiles/grass-fullscreen-96-default-120.json`: the same frozen inputs and
15-minute protocol, changing only `prepared_blades` to 131072. This preserves
the same binary/density/quality for measuring the preparation change's effect.
The normal game default remains unchanged. Original reports are preserved;
GP-019's separate reanalysis demonstrates the new cadence warning.

The control completed as [GP-020](GRASS_PREPARATION_POWER_COMPARISON_20260916.md).
Both configurations show recurring late stalls; the larger arena reduces GPU
duration but shows no energy saving in this pair. The default is unchanged.
The proposed short native timing/presentation trace is deferred: reaching the
known late interval still takes roughly 12 minutes of game runtime. The user
prioritized early heat reduction and requested no long runs. Follow the
[GP-021 early-load review](GRASS_EARLY_HEAT_20260916.md) before preparing another
powered test. It also records that the first 60 seconds hold camera/wind still:
this is asset warmup, not thermal warmup at the later moving workload.
