# Upscaling prototype

For the latest release-build investigation, see the
[2026-09-20 MetalFX report](performance/20260920-metalfx-investigation/README.md).
It reproduces the Temporal cost without grass, distinguishes Spatial from
Temporal, and confirms that the panel's independent GPU marker spans can miss
substantial work. Use the report's Metal HUD comparisons rather than treating
those marker spans as whole-frame timings.
The [follow-up](performance/20260920-metalfx-investigation/FOLLOWUP.md) tests
actual game-input replay and an approximately 33% input size. The latter improved
the short grass-off comparison, but the user's subsequent release gameplay test
still found severe heating/performance degradation and unacceptable image quality.
Temporal is not an accepted performance option at either tested scale.
The focused [empty-scene Temporal check](performance/20260920-metalfx-investigation/TEMPORAL_EMPTY.md)
reproduces a substantial reconstruction cost with no geometry and in native MetalFX
alone, and lists the work that remains in the surrounding engine path.
The subsequent [grass-scene MSAA comparison](performance/20260920-metalfx-investigation/MSAA_GRASS.md)
measures the antialiasing cost Temporal replaces: native 4× MSAA added about 1.55 ms,
but the complete Temporal frame was still about 1.17 ms slower than native + MSAA.
The [configuration review](performance/20260920-metalfx-investigation/CONFIG_OPTIONS.md)
documents available controls and a packed-HDR native experiment; its lower measured
duration came with higher GPU clock/power and is not an accepted game optimization.

The game starts at **50% of physical display width and height**, with 4× MSAA.
Scene rendering therefore uses one quarter of the native pixel count. UI is composed
after upscaling at the full display resolution. Editor rendering is unchanged.

**F1 → Quality → Resolution** cycles through **100%, 75%, 50%, and 33%**.
The 33% option uses one third of each output dimension, rounded up to whole pixels.
Select **MetalFX Temporal** separately to try the setting from the investigation;
the startup resolution remains 50%.

Open **F1 → Quality → Upscaler** to cycle Auto, Linear, MetalFX Spatial and MetalFX Temporal. Auto
selects MetalFX Spatial when the build, device and image configuration support it;
otherwise it uses Linear. The panel summary reports requested and active methods,
plus any fallback reason. A/B reports include that status. Launch overrides:

```sh
cargo run --release -p yarra-app-game -- --upscaler linear
cargo run --release -p yarra-app-game -- --upscaler metalfx-spatial
cargo run --release -p yarra-app-game -- --upscaler metalfx-temporal
```

`--upscaler auto` is the default. At 100% or when the requested output is smaller
than the input, spatial reconstruction is bypassed in favour of the linear copy.
Explicit **Linear** filters the scene image directly during native UI composition;
it skips the separate upscale pass and display-sized intermediate image. Auto's
fallback still uses the plugin's Linear backend.
The [matched comparison](performance/20260920-metalfx-investigation/LINEAR_COMPOSITION.md)
found no meaningful GPU-time improvement from this change.
Advanced → Direct renders straight to the window and bypasses this plugin entirely.
Reset launch settings restores the launch method and scale. These are temporary
settings; they do not modify world assets.

## Ownership and extension points

`yarra-upscaling` exposes `UpscalingPlugin`, `UpscaleView`, `UpscaleMethod`,
`UpscaleStatus` and `UpscalingCapabilities`. The public API contains no Metal types.
The game chooses input/output dimensions, manages the image handles and presents
the result. Explicit Linear composition samples the scene handle directly without
an `UpscaleView`; other methods use the plugin's backend selection, GPU pipelines
and per-view instances.
Resource keys include GPU texture/view identities and the requested method: changing
resolution or method recreates the affected backend. Device recovery rebuilds GPU
state; disabled/despawned views release their cached instances.

Current render order:

```text
Low-resolution scene + MSAA → postprocessing / tone mapping → resolved LDR image
  → spatial upscaler → display-sized image → native UI → window
```

Auto and MetalFX Spatial use this path, with the portable Linear backend available
as a fallback. Explicit Linear combines enlargement with the final UI composition:

```text
Low-resolution scene + MSAA → postprocessing / tone mapping → resolved LDR image
  → linear sampling + native UI → window
```

Auto still selects the spatial path; Temporal is an explicit prototype choice.

Temporal rendering has a separate contract in `upscaling::temporal`:

```text
Jittered low-resolution opaque scene + mesh motion/depth prepass
  → grass colour + motion + final scene depth (one grass raster pass)
  → sky/clouds → MetalFX Temporal → full-size bloom and tone mapping → native UI
```

`TemporalView` describes input dimensions, a caller-controlled reset epoch and debug mode.
`TemporalFrame` exposes current/previous unjittered matrices plus the jittered raster
matrix. `TemporalMotionTarget` is shared by the scene and custom grass renderer.
Motion is current UV minus previous UV, excluding jitter. Backends convert that to their
own convention (MetalFX uses negative render-pixel scale). These APIs contain no Metal
types and can also feed a future FSR/DLSS backend; neither backend is implemented here.
Colour inputs already include camera exposure. MetalFX receives an explicit unit exposure
texture instead of estimating a second exposure. See the [grass motion investigation](performance/20260921-temporal-grass/README.md)
for the calibration check and the still-unresolved loss of grass detail during strafing.

Bevy's `MainPassResolutionOverride` renders into a low-resolution rectangle inside native
HDR targets. The temporal pass reconstructs the alternate full-size HDR target before
post-processing. Terrain LOD and contact-error projection use the input rectangle,
so selecting Temporal does not silently double the terrain pixel budget. It reads **final scene depth**, including grass, rather than the mesh-only
prepass depth. Grass is excluded from the ordinary opaque phase in this mode. It writes
colour, depth and RG16Float motion in one geometry pass. The previous camera and wind pose
use each current blade's stable root and seed, independent of compacted instance order.
Bounded previous-pose preparation has the same capacity/overflow behaviour as current
preparation; overflow evaluates the previous pose in the vertex shader.

Temporal automatically disables MSAA and requires mesh depth/motion prepasses. Switching
back restores the selected MSAA setting. UI stays native. History resets on enabling,
resize, camera cuts/large jumps, explicit reset epochs, grass source changes, origin
rebases and wind-time discontinuities. Debug views invalidate history before resuming.
A native creation/encode failure reports its reason and uses a linear HDR fallback;
jitter stops on the following frame. Unsupported platforms retain the spatial choices.

**F1 → Quality → Temporal view** cycles Image, Motion vectors, Depth and Reconstruction bypass. Neutral motion
is grey; direction/speed changes its red/green channels. The depth view includes grass.
Debug views bypass reconstruction and cannot be used for a normal A/B timing capture.
Reconstruction bypass retains jitter, depth/motion, grass and the full-resolution
post-processing path, but substitutes a linear HDR enlargement for MetalFX. It is
an isolation tool, not an alternative anti-aliasing mode.

This is an initial quality prototype; a performance win over native MSAA is not guaranteed. Additional work includes mesh
prepasses, motion preparation, previous grass deformation, reconstruction and native-size
post-processing. Previous grass preparation adds about 19 MiB at the default arena size;
temporal render targets and MetalFX's internal history add more. When placement and
pose inputs match, the renderer swaps the two existing pose buffers, retaining last frame’s prepared
pose as history and writing the new pose into the other buffer. This avoids both a
geometry copy and a second deformation dispatch; bindings are cached for the two buffers.
Changed placement, camera/pose inputs, shader reloads and resets retain the recalculation
path. Lighting is excluded from placement and pose cache keys: cloud shadows and the
day cycle still update shading, without triggering unrelated geometry generation.
Pass timing labels provide diagnostic spans with probe overhead; use the normal GPU
figure or A/B captures for comparisons. Native runs should stay below the requested
3–4-minute limit.

### Performance check: M2 Max, 2026-09-20

Short checks used the same `tmp/cloud-performance/static.sqlite` world and
`close-view.ron` camera, full grass/wind, a 2560×1440 window, 120-FPS pacing,
five seconds of warmup and twelve seconds of measurement. Native used 100% +
4× MSAA; Temporal used 1280×720 input. The panel/audit logging was on in both.
No extended thermal-stability claim is made from these runs.

The last normal-timing runs reported medians of about **4.8 ms native** and
**7.5 ms temporal** from the once-per-second GPU samples. Earlier temporal runs
varied around 6–7 ms. GPU clocks were not locked; these observations do **not**
establish an overall speedup from the pose-buffer reuse. Temporal remains a
quality prototype, with a measurable cost over native in this view.

An isolated native MetalFX test measured approximately **1.4 ms** for 720p →
1440p reconstruction; compact input allocations changed that by less than
0.05 ms. Enlarging the prepared-blade cache also did not yield a reliable win,
so neither change was adopted. Low-resolution rendering still evaluates grass
geometry and its previous position, in addition to the reconstruction cost.

Detailed GPU probes substantially changed the timings in this scene. They now
run less frequently, are labelled separately, and are paused for A/B captures.
Use those captures for the next scene comparison; a pass span is not an isolated
feature cost. Buffer reuse is guarded by placement generation, source, pose,
configuration and shader identity, and falls back to recomputation when needed.

Remaining quality work: exact previous LOD/morph correspondence (the first prototype uses
the current topology/morph for both poses), per-pixel reactive handling of transparent
particles and cloud animation, and broader camera/disocclusion tuning. Streaming resets
are intentionally conservative and can temporarily reduce accumulation. Thin foliage at
50% still needs visual judgement in motion; support is not a guarantee of artifact-free AA.

### Isolating reconstruction at the physical output resolution

Follow-up checks at **1728×971 → 3456×1942**, with grass disabled in every variant,
reproduced the reported regression. In 17-second runs (5 s warmup, 12 s measurement),
medians of the once-per-second frame timestamp samples were **3.54 ms Spatial**,
**7.24 ms Temporal** and **3.64 ms temporal-path reconstruction bypass**. This rules
out grass as the main explanation for this comparison. It does not establish a
speedup over native MSAA.

An important measurement caveat: turning bloom off produced much smaller frame
timestamp spans, while native MetalFX command-buffer elapsed time stayed around
**4.6 ms** in a later paired check. Native command-buffer elapsed can include
dependency stalls; these values must not be added to other pass times. The two
measurement paths are not yet consistent enough to claim the bloom toggle saves
the difference, or to treat the stage-marker span as an exact whole-frame cost in
all temporal configurations. GPU clocks were not locked. The current integration
still has a substantial cost independent of grass; it has not been optimized away.

Controlled launch options (only active with timed/diagnostic profiling):

* `--profile-size game --profile-surface 3456x1942`: keep the normal 50% scene
  scale while specifying the **physical** window size, independent of Retina scaling.
* `--profile-bloom on|off`: isolate post-processing; defaults to on.
* `--profile-temporal-bypass`: retain temporal inputs/post-processing but skip MetalFX.
* `--metalfx-timing-log`: opt-in native command-buffer elapsed logging, once every
  120 reconstruction calls. Completion is polled without waiting, with at most four
  retained command buffers. Logs include history-reset state. This flag also works
  in a normal launch.

Keep pass timing probes off for the primary comparison, and use the same physical
surface, camera, features and frame cap. A captured runtime frame verified the
1728×971 input, 3456×1942 output and `reset=false`. Xcode's replay encoder table did
not expose the internal MetalFX work; its replay total is not a replacement for
runtime timing. Apple recommends temporal reconstruction before post-processing:
moving bloom/tone mapping ahead of reconstruction is not a neutral optimization.
See [Apple's MetalFX integration guidance](https://developer.apple.com/videos/play/wwdc2022/10103/).

The game enables the crate's `metalfx` Cargo feature. Native dependencies are limited
to Apple targets; a build without the feature retains Linear and capability reporting.
The linked MetalFX framework requires **macOS 13+ / iOS 16+** for Apple builds with
this feature. Device capability and resource failures fall back at runtime; that
fallback does not remove the deployment OS requirement. Other platforms do not link
the Apple framework.

### Direct tone-mapped output

Temporal gameplay now opts into `DirectTonemapOutput`. It runs Bevy's existing
tone-mapping shader directly into the native-sized sRGB output, after HDR bloom.
This replaces tone mapping to an RGBA16Float intermediate followed by a separate
output blit. Reconstruction, scene resolution, grass/wind, bloom, colour grading
and dithering are unchanged. The UI still renders at native resolution afterward.

At 3456×1942 this removes one HDR write and read: about **102 MiB of logical
texture traffic per frame**, before hardware caching/compression. It does not
remove Bevy's HDR ping-pong allocations or MetalFX's internal reconstruction cost.
Launch with `--temporal-standard-output` to retain the original two-pass output
for comparison. Detailed pass timings label the fused pass **Tone map + output**.

Two short matched comparisons, in opposite order, used 1728×971 input,
3456×1942 output, full grass/wind and bloom, 120-FPS pacing, five seconds of warmup
and ten seconds of measurement. No compilation or detailed pass probes ran during
these comparisons:

| Path | Run A GPU span median | Run B GPU span median |
| --- | ---: | ---: |
| Standard output | 7.81 ms | 7.84 ms |
| Direct output | 7.68 ms | 7.63 ms |

These are medians of the once-per-second sampled GPU log values. GPU clocks were
not locked; the previously noted timestamp limitations still apply. The observed
gain is modest, approximately 0.2 ms, and is **not a fix for the full temporal
performance regression**. Native MetalFX command-buffer elapsed medians remained
approximately 2.9–3.1 ms. A separate compact-input packing experiment added a pass
without a meaningful saving and was removed.

`DirectTonemapOutput` is an opt-in camera contract: use it only with temporal
views that own a full output target and have no effects after tone mapping.
HDR effects such as bloom remain supported. Remove the component before adding
LDR post-processing. Viewports, output blending, non-linear compositing, disabled
tone mapping and pending shader compilation use Bevy's original output systems.
Their conditions are attached before schedule initialization, preserving existing
ordering edges and timing instrumentation. The adapter targets Bevy 0.19.

A bounded GPU readback comparison covered HDR and night colours, bloom, LUT tone
mapping, colour grading, dithering, resize and removing the optimisation. Maximum
per-channel difference from the original path was **1/255**, due to removing the
intermediate half-float rounding; restoring the standard path matched exactly.

## Native interop and colour

MetalFX runs on the existing wgpu Metal device, with its command buffers queued by Bevy. It never creates
a second queue, submits independently or copies pixels through the CPU. Its commands
are included in the game's GPU render timings; detailed pass timings expose the
upscaling system separately. Instances are reused between frames.

For the spatial path, the input is the game's resolved sRGB image. Output uses storage-capable RGBA8 memory
with an sRGB view for MetalFX and UI, preserving colour through the native and Bevy
paths. Wgpu initialization and resource transitions surround native encoding so later
sampling does not clear native-written output. Both paths use a full-resolution output
texture; even Linear has an output pass and memory cost. Compare **GPU milliseconds**,
not capped FPS, to judge that tradeoff.

[Apple's spatial upscaler](https://developer.apple.com/documentation/metalfx/mtlfxspatialscaler)
uses the current colour frame. It can improve detail relative to linear enlargement,
but has no frame history to stabilize moving foliage. This prototype does not promise
to eliminate leaf shimmer or improve CPU/streaming costs. MSAA remains available.

## Verification

```sh
cargo test -p yarra-upscaling --features metalfx
cargo test -p yarra-app-game
cargo check -p yarra-upscaling --no-default-features
```

The ignored GPU readback test requires a native adapter. With `metalfx` enabled on
Apple it requires MetalFX to run successfully, rather than accepting a fallback.
It checks colour/orientation after subsequent Bevy-style sampling, changing frames,
switching between MetalFX and Linear, and odd resized dimensions:

```sh
cargo test -p yarra-upscaling --features metalfx spatial_backends_preserve_colour_orientation_and_updates -- --ignored --nocapture
```

Verified on an M2 Max: the native readback probe preserved all sampled colour values
exactly. A 70-second scene check confirmed Auto → MetalFX, 1280×720 scene / 2560×1440
display, live switching to Linear, and bypass at 100%. This was a functional check,
not a sustained performance comparison.

Additional bounded probes:

```sh
cargo test -p yarra-upscaling --features metalfx temporal_preserves_hdr_and_resets_history -- --ignored --nocapture
cargo test -p yarra-upscaling --features metalfx temporal_motion_sampling_probe -- --ignored --nocapture
cargo test -p yarra-upscaling --features metalfx direct_output_matches_standard_postprocessing -- --ignored --nocapture
cargo test -p yarra-vegetation-render --features upscaling/metalfx temporal_grass_motion_tracks_wind_camera_and_fallback -- --ignored --nocapture
cargo test -p yarra-vegetation-render --features upscaling/metalfx temporal_strafe_motion_matches_reprojection -- --ignored --nocapture
```

These cover HDR preservation and resets, reconstruction of a known moving pattern,
tone-mapping output equivalence, grass wind/prepared/fallback motion, and numerical
camera-motion agreement with final depth. The camera reprojection test validates both
Bevy meshes and custom grass. See the [sampling and motion results](performance/20260921-camera-temporal-motion.md)
for measured errors and the limits of these checks.

For a scene comparison, keep camera, weather and resolution fixed, and capture Spatial
versus Temporal in the Performance panel. Temporal intentionally replaces MSAA; compare the
total frame cost, including all prerequisite work. Keep runs short; these captures establish
render cost and visual behaviour, not sustained thermal performance.
