# Hill and valley test landscape

This editable fixture exercises looking down across a populated landscape and
descending into it. It complements the small meadow performance baseline and the
unpopulated synthetic mountain cooking fixture. It is not a performance fix or a
claim that the distant-world renderer meets its budget.

## Launch

From the repository root:

```sh
python3 tools/hill_landscape.py
```

The launcher prepares `tmp/hill-landscape/project.sqlite` and `runtime.sqlite` on
first use, then starts the release game with the default terrain hierarchy. The
character spawns at the hilltop lookout, looking southwest over the valley. Movement
and camera controls work normally. Subsequent launches reuse the databases. Regular
launch commands continue to open the default authoring world.

Other entry points:

```sh
python3 tools/hill_landscape.py editor
python3 tools/hill_landscape.py game --view slope
python3 tools/hill_landscape.py game --view valley
python3 tools/hill_landscape.py descent
```

`descent` waits five seconds, then moves the character/residency focus and camera
along the road at 6 m/s and stops at the end. It is a scripted camera/streaming
inspection, not a navigation or character-animation test. It keeps the same
southwest-facing camera for comparisons. Close the application normally afterward.

Use `--recook` after saving source edits without publishing, or `prepare` to cook
without launching. Editor Save & Publish uses these separate databases. To make a
fresh independent scene, pass `--directory tmp/hill-landscape-2`; generation refuses
to overwrite an existing project or bookmark directory. Extra application flags
can follow `--`. The launcher uses the existing release rendering settings; it
does not impose a new FPS cap or a smaller rendering resolution.

Normal game and editor launches now use this terrain path without `--terrain-lod`.
For a diagnostic comparison, append `-- --terrain-legacy` to the launcher command.
This changes rendering only; the published landscape and its materials are shared.

## Scene and viewpoints

- 1,536 × 1,536 m, 2,304 source cells, 32 m cells with 33 × 33 height samples
  (1 m spacing). Ground/vegetation masks retain 0.5 m detail. The hierarchy is built
  by the normal production cooker.
- A rounded 192 m summit, roughly 180 m above the valley floor, rolling ground and
  a second ridge across the valley. The spawn is at the 187.6 m hilltop lookout,
  just in front of the crown so the crest does not hide the valley. The initial
  camera uses the normal maximum zoom distance and a 35-degree downward angle.
- Dry and green meadow coverage, two clearings and a curved cart road with broad
  worn tracks, retained center grass and shallow whole-road relief. Wheel wear is
  in the surface/grass masks; narrow geometric ruts need finer geometry than this
  distance fixture. No distant forest proxies or trees.
- Summit, slope and valley launch bookmarks in `project.views/*.ron`. Each stores
  world-space feet position, camera yaw/pitch/distance and fog visibility. The
  summit bookmark also holds a sampled route along the actual road curves.
- Viewpoint fog visibility and camera far distance are 2.5 km, so the valley and
  opposing ridge can be inspected. This override applies only with `--start-view`.

The shared `--start-view FILE` option works in both game and editor. It initializes
the actor (game), streaming viewpoint, camera and distance fog together; it does
not move the camera alone while loading grass around a different position.

## What to inspect

First check summit visibility, terrain silhouettes, road/material continuity and
grass contact near the character. Zoom/orbit, then descend and watch for terrain
or material transitions and loading stalls. Keep the summit bookmark as a fixed
view for later measurements.

For performance comparisons use the same database, bookmark, resolution, FPS cap
and thermal starting conditions. A normal launch without LOD intentionally draws
less terrain; it cannot validate the full summit view. Compare renderer changes
against the same hill view, and retain the small meadow as a separate regression
case. This fixture changes elevation, world area, source cell size and visible
coverage, so its timings are not directly interchangeable with the earlier meadow.

The release cook produces 3,064 ground-composite tiles. Its largest input
batch holds 9,801 height samples and 114,075 coverage bytes, with at most three road
spans; these are cooker counters, not frame-time or total-memory measurements.

Implementation: `crates/world_cook/src/hill_fixture.rs` generates the editable
project; `tools/hill_landscape.py` creates, publishes and opens it. No schema change
is required: viewpoints are explicit launch files, separate from authored content.

The initial 65 × 65 geometry trial hit the existing one-million-triangle cover
budget and blocked the character/grass contact checks even at rest. The final
fixture uses the existing 33 × 33 patch topology, with broader shallow road relief.
Increasing sample density multiplies the cost of every hierarchy patch; it must
be evaluated separately from increasing world area. The subsequent contact-allocation
correction is recorded in `DISTANT_WORLD_RENDERING.md`: the current hill's actor
remains certified in CPU planning checks with one quarter of the normal triangle
budget, while grass detail yields. The original 65 × 65 generation has not been
recooked/retested, and dense grass demand is not guaranteed to fit that budget.

Validation on 2026-09-19: release game/editor builds; fixture compilation and shared
height-edge checks; elevated actor/camera initialization; route interpolation and
validation. Short native Metal captures checked the hilltop and the beginning of
the descent at 1920 × 1080 internal resolution, capped at 60 for visual inspection.
The settled lookout used 342 patches / 700,416 terrain triangles, with no geometry
budget limit, blocked actors, blocked grass pages or mismatched grass pages. The
early descent also reported no contact blocks. The fine composite-material cache
reached its detail limit and used its coarser fallback. These checks do not establish
fullscreen 120 FPS or sustained thermal performance; those measurements remain open.

## Appearance investigation — 2026-09-19

The first hill screenshots look soft and blotchy across the opposing mountain.
Three short native captures used the same published terrain, camera and mesh:
the production shader, mesh normals substituted for the distant composite normals,
and terrain albedo without PBR lighting. The camera was at the summit bookmark's
feet position, yaw 45°, pitch 25.3°, distance 13.28 m. These were visual checks at
1920 × 1080 internal resolution / 2560 × 1440 output, capped at 60 FPS, not GPU
cost measurements. Diagnostic assets and captures are in `tmp/terrain-appearance/`;
the production shaders and world were not changed.

Mesh normals made little visible difference in this view. Removing lighting removed
the pronounced dark patches, while the underlying green surface remained soft.
The fixture uses smooth Gaussian hills plus generic fractal noise, meadow materials
across the slopes, and an approximately 11.7° sun elevation. It lacks large ridge,
gully and rock structures; increasing the sample count cannot supply those missing
landforms. These checks do not rule out material-resolution or silhouette problems
at other viewpoints.

Follow-up discussion: retain the current relief and shading for now. If appearance
work resumes, first compare reduced small-scale height noise at the same camera,
lighting, materials and sample spacing. Adding mountain structure or rock materials
is not an established fix for the soft-looking meadow. Permanent smoothing adds no
runtime filtering pass; distance-dependent smoothing would need consistent geometry,
normal and contact transitions. Global mesh subdivision, larger caches and additional
runtime terrain passes are not justified by these captures alone.

Fog and future distant vegetation/scenery may improve the composed landscape. Keep
this exposed hill view as a regression case even after those representations arrive.
It should continue to reveal terrain boundaries and cost without relying on scenery
to hide them. Appearance changes are deferred; the foundation priorities are recorded
in `DISTANT_WORLD_RENDERING.md`.

## Live editor terrain checks

Open `python3 tools/hill_landscape.py editor`. Local environment paint and road
edits now feed the distant hierarchy after a background, GPU-ready handoff. Move
away and back to check that the terrain edit survives source eviction; undo should
restore both ground appearance and relief. Save preserves the source, while
Save & Publish makes the result available to the game.

Keep this check short. The current live region admits 256 source/affected cells;
a shared preset/style change across this 2,304-cell landscape may require
Save & Publish. The smaller current road-authoring world is useful for shared
style relief and save/undo checks. The previous accepted image stays visible when
the live budget is exceeded. Fine ground detail may briefly fall back to the new
baked ground while its cache refills; no new sustained FPS claim is attached to
this integration.
