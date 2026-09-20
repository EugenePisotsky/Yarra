# Cloud prototype

The atmosphere owns one ground-view volumetric cloud layer. Open World → Tools →
Atmosphere → Clouds and choose Scattered or Overcast. Clear disables the layer.
Coverage, density, base altitude, thickness, size, edge detail, wind and seed are
saved with the world's atmosphere. Preview cloud motion is independent of day
playback: Animate clouds, Reset clouds and Reset preview do not edit source data.
Cloud speed is a separate preview multiplier (1–30×); the elapsed seconds show
that the transport is advancing even when distant motion is subtle. Day Play
continues until paused or the hour is manually scrubbed.
The standalone game advances wind in real time. A seed reproduces the same layout.
Save & Publish before running `cargo run -p yarra-app-game`. To start at the
previewed hour, use Startup and cycle → Use preview as startup before publishing;
the game currently keeps that time of day fixed while clouds move.

The generator creates a seamless 64³ RGBA noise volume once on startup: smooth
multi-scale noise, cellular shapes and edge detail. The shared density shader
combines this volume with a broad weather pattern, domain warping and a height
profile that rounds off scattered towers. Vertical noise spans the layer even
when it is thin, rather than repeating a shallow horizontal slice. No paint
assets or fluid simulation are required. A painted coverage map is a later feature.

A configurable HDR pass integrates 48 density samples and five light samples
per occupied step for each active celestial light. Balanced (the default) caches
the entire upper hemisphere in a fixed 512² texture, independent of window size.
Four strips refresh at no more than 32 strips/second (eight complete refreshes),
with no catch-up dispatches after slow frames. Camera turns sample the cache in
world directions every frame; exposure is applied at composite time. New textures,
shape/seed edits, teleports and resuming after a pause refresh the whole cache.
Ordinary camera translation, wind and lighting changes reach each strip on its
next refresh. High retains per-frame rendering at half width/height. The full-resolution
composite uses world-view depth to preserve foreground silhouettes, and blends
in-scattering/transmission directly into the existing HDR target without a full-frame
scene copy. Sun and moon disks are
attenuated by the integrated density; bloom runs after compositing. Cloud lighting
and foreground haze share an approximate clear-air extinction and smooth horizon
fade, so below-horizon sunlight cannot brighten twilight clouds. There is no
temporal accumulation or screen-space history reprojection.

A separate 256² RGBA texture stores sun and moon transmission at the cloud base.
It is built in cloud-field coordinates and reused while wind moves its lookup
coordinates. Density, seed, layer, active light/direction, GPU texture and shader
changes invalidate it; camera motion, origin rebases, wind and exposure do not.
Inactive lights skip their shadow integration. A moving sun still rebuilds the
map, while the current fixed-time game normally builds it only once.
Terrain, grass and lit StandardMaterial meshes project their surface positions
along the corresponding light direction into this texture. Clouds therefore
attenuate direct diffuse/specular illumination locally, independently of camera
orientation. Ambient/emissive/local lights retain their own lighting paths.
Overcast coverage modestly increases and desaturates the authored ambient fill.

The atmosphere crate supplies shared GPU data; it does not depend on terrain or
vegetation. Renderers consume that data. Material bindings 120–122 are reserved
for this input, avoiding the vegetation study's 100–106 extension. An explicit
zero-density buffer supports standalone terrain studies. Bevy's bindless light
textures are not used because that path is disabled on Metal in Bevy 0.19.1.

`assets/shaders/clouds/pbr_lighting.wgsl` keeps the upstream Bevy 0.19.1 forward
lighting function with cloud transmission added only to the directional light
contribution. Its license and upgrade obligation are documented alongside it.
Lit standard materials get an extension that retains their original source handle
and mirrors source material changes. Unlit editor overlays keep their original
material path. Cloud inputs are disabled while study workspaces own lighting.

Cloud coordinates use the canonical horizontal world origin and wrap in f64
before GPU conversion. Noise and shadows share the same wind offset. The current
weather pattern repeats every sixteen cloud sizes, with no terrain-cell residency
or streaming dependency.

Existing schema-24 source/schema-20 runtime atmosphere blobs remain readable and
retain the exact old binary encoding when cloud settings are default. Nondefault
settings append a bounded `CLD1` extension. Unknown, truncated or trailing data is
rejected. Older checkpoint binaries reject extended profiles; keep their data
backups when comparing builds. Save, undo, recovery and publication use the same
atmosphere transaction. Recovery journals advance to version 14 and accept 12–13.

Limits: one active outdoor world view, a flat cloud layer intended for cameras and
terrain below its base, approximate multiple scattering and broad ambient response.
Flying inside/above clouds, per-pixel partial integration against mountains within
the cloud layer, cloud reflections, atmospheric light shafts, multiple cloud decks,
painting and weather transitions remain outside this prototype. GPU cost and the
cloud silhouettes need evaluation on richer landscapes before choosing production
quality budgets. The cloud target currently assumes the normal
world render resolution, without an additional resolution-override scale.

Cloud quality is presentation-only: use Off / Balanced / High in the editor's
temporary preview controls, or `--cloud-quality off|balanced|high` when launching
the game. It does not change the saved weather or world-render resolution.
Balanced trades angular detail and cloud-update cadence for a bounded workload;
lighting samples, ground shadow resolution, terrain and grass stay the same.
At 120 FPS a normal full refresh takes about 0.13 seconds. Fast wind previews,
rapid lighting changes or cameras close to the layer can reveal differences
between strips until they refresh; use High for inspecting those cases. The cache
has no camera-rotation lag, but it approximates translation between refreshes.

## Validation

The implementation passed 438 workspace tests (22 existing manual GPU/performance
checks ignored), followed by 160 targeted editor/atmosphere/cooker tests after the
last transport and publication changes. All workspace targets compile. Native
Metal editor/game checks covered clear/scattered/overcast, daytime/nighttime,
opaque moon occlusion, standard mesh and grass rendering, and source save → cook
→ published runtime reload. The test world was copied into `tmp/cloud-prototype`;
the real project's atmosphere remains unchanged, with clouds initially disabled.
A sustained GPU benchmark, cloud-layer intersections and visual refinement are
still needed before treating this as a production cloud renderer.

The playback/shape follow-up passed 155 targeted editor, world and atmosphere
tests (one existing manual check ignored), workspace all-target compilation and
formatting. Native Metal checks verified sustained day/cloud playback, cloud
pause/reset and preview acceleration, 05.30/18.30 twilight, thin scattered clouds,
day/night overcast and moon occlusion, plus automatic wind in the standalone game.
These checks used the disposable databases above; existing project edits and
recovery data were preserved. The field still tiles at sixteen cloud sizes and
the 256² shadow map trades spatial detail for its larger coverage area.

The performance follow-up passed 154 editor/game/atmosphere tests (one existing
manual check ignored), workspace all-target compilation and formatting. Native
Metal captures checked the old and optimized overcast renderers, Balanced and
High, scattered clouds, and nighttime rendering with MSAA disabled. Full-depth
foreground silhouettes remain intact. See [Cloud performance](CLOUD_PERFORMANCE.md)
for the measured GPU costs, reproduction conditions and thermal limitations.

The subsequent cached-sky change passed 156 editor/game/atmosphere tests and
workspace all-target compilation. A native Metal overcast capture checked the
cache and foreground edges. Timed runs totaling 215 seconds confirmed much less
scheduled cloud work, but still recorded elevated thermal pressure; the fan/heat
complaint is not yet verified as resolved. The performance report records the
hot-start Off control and why it cannot isolate the remaining cloud cost.
