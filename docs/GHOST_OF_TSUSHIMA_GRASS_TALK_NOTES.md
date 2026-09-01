# Ghost of Tsushima Procedural Grass Talk Notes

## Status and sources

This is a living companion to the local
[`gdc_2021_procedural_grass_in_got-2.pdf`](../content/gdc_2021_procedural_grass_in_got-2.pdf)
slide deck. It records Eugene's live retelling of the accompanying video so that the mostly visual
slides will still make sense when we return to them later. It is a technical paraphrase, not a
verbatim transcript and not an instruction to copy the design into Yarra.

Conventions:

- **Slide evidence** describes text or comparisons that are visible in the PDF.
- **Talk explanation** paraphrases the spoken explanation as heard by Eugene.
- **Uncertain wording** marks a phrase whose exact transcription still needs checking; the intended
  technical meaning is recorded when it is clear.
- Interpretations and Yarra-specific decisions belong in
  [`GRASS_IMPROVEMENT_PLAN.md`](GRASS_IMPROVEMENT_PLAN.md), not in this source notebook.

Notes currently cover slide 16, slides 22-38, slide 40, and slides 45-46. More can be appended as the
talk is reviewed.

## Slide 16: Voronoi clumps and structured variation

The team first tried varying grass parameters independently per blade. Although this removed obvious
repetition, the result did not resemble a natural field; it looked like unrelated random blades.
They therefore organized blades into procedural clumps and used clump membership as a shared source
of variation.

For any two-dimensional sample position, they evaluate the jittered feature point belonging to each
cell in the surrounding 3x3 grid. Each feature point is displaced within its cell by a deterministic
hash. The nearest of those nine candidates becomes the sample's Voronoi clump centre. This gives a
stable clump identity and the sample's distance/direction relative to that centre without storing
explicit clump objects.

Clump identity and relative position can then influence several grass properties coherently:

- vary blade height by clump or by distance from its centre;
- give the blades in a clump a related facing direction;
- pull blade roots toward the clump centre;
- orient blades radially away from the centre;
- combine attraction, radial facing, shared facing, and smaller per-blade variation.

The important result is not merely Voronoi-shaped placement. It is that variation happens at more
than one spatial scale: a clump supplies recognizable shared structure, while individual blades
retain limited variation inside it. This avoids both a uniform football-field pattern and
structureless per-blade noise.

## Slides 22-23: GPU work submission and bounded scratch memory

### Slide 22 - compute, indirect arguments, then rendering

The implementation uses two compute shaders before drawing a tile:

1. The first shader generates the visible per-blade instance data and accumulates the resulting
   blade count.
2. After that dispatch completes, a second very small shader copies the accumulated blade count into
   the indirect draw arguments for that tile. It runs only one wavefront and is described as taking
   almost no time.
3. Once the indirect arguments are ready, the vertex shader renders the generated instances.
4. The pixel shader performs the material shading.

The significant point is that the CPU does not need to read the visible count back. The GPU converts
its own generated count into a draw command.

### Slide 23 - overlap compute and graphics in small batches

They do not process every visible tile into one enormous instance buffer. Their scratch instance
memory holds eight tiles' grass data. Four tiles first run through compute; while those four tiles
run their vertex and pixel work, compute prepares the next four tiles in the other half of the
buffer.

This double-buffered, four-tile batching is intended to:

- keep compute and graphics work overlapping;
- keep the GPU busy;
- place a hard bound on transient instance memory instead of sizing it for all visible tiles.

## Slides 24-26: procedural topology and continuous LOD

### Slide 24 - vertex-ID-driven grass with no vertex streams

Grass is submitted as an instanced indexed draw with no conventional vertex streams. The vertex
shader derives its input entirely from the vertex/index identity and instance ID.

Each tile is one draw call and uses either the high or low geometry LOD. For a vertex in either LOD,
the shader can derive:

- a 0-to-1 location along the blade;
- whether the vertex is on the left or right edge of the blade.

An even distribution of vertices along the blade is not always a good distribution because a bent
blade may have concentrated curvature. An artist parameter remaps the longitudinal 0-to-1 values so
more of the available vertices can be placed where the blade shape needs them.

The 15 high-LOD vertices and seven low-LOD vertices drawn on this slide are **ribbon mesh vertices**,
not 15 or seven independently movable bend/control points. In the illustrated topology, paired
left/right vertices sample the blade centre curve at several longitudinal rows, followed by a shared
tip. More rows make the evaluated curve look smoother; they do not add new degrees of freedom to the
underlying curve. The longitudinal remap is therefore important: it spends the fixed samples where
the curve bends most instead of wasting them on nearly straight portions.

### Slides 25-26 - geometry and density transitions

The high LOD blends its shape toward the low LOD as it approaches the LOD boundary. This avoids a
visible pop when a highly curved blade loses vertices.

Low-LOD tiles cover twice the linear size of high-LOD tiles while retaining the same number of grass
blades, so their blades are spaced twice as far apart in each dimension. To make that density change
continuous, a high-LOD tile fades out three of every four blades before transitioning to the larger
low-LOD tile. The exact spoken phrase was initially unclear (it sounded like "load out"), but the
described operation is a stable three-of-four blade fade during the LOD transition.

Together, the two transitions address different discontinuities:

- geometry morphing removes the silhouette/curvature pop of 15 vertices becoming 7;
- stable density fading removes the population pop caused by the lower spatial density.

## Slides 27-30: short-grass topology and cubic Bezier construction

### Slide 27 - spend one strip on two short blades

Normally all vertices in an instance are spent on one blade. If the grass is short enough, the strip
is folded into two blades instead. This increases the apparent density and coverage of short grass
without increasing the submitted vertex count.

In this topology, vertex ID determines one more attribute in addition to longitudinal position and
left/right side: which of the two blades owns the vertex.

The available vertex count is odd, so the two blades cannot receive identical topology. One blade
has fewer vertices and a connecting/leftover triangle has to be dealt with. In the low LOD, the blade
with fewer vertices deliberately has no conventional tip because its very low-poly animated tip was
noticeable even at a distance. The slide illustrates both a 15-vertex high LOD and a 7-vertex low
LOD folded into two blade silhouettes.

### Slide 28 - each blade is a cubic Bezier curve

Each grass blade is represented as a cubic Bezier curve. The slide calls out the properties that make
this convenient:

- positions along the curve are easy to calculate;
- its derivative is easy to calculate and can be used to form a normal;
- moving the control points provides a useful basis for both animation and per-blade variation.

A cubic Bezier has four mathematical points: the root endpoint, two interior control points, and the
tip endpoint. Only the root and tip lie on the curve in general; the two interior points control the
root tangent, tip tangent, and distribution of curvature between them. This is distinct from the
15/7 ribbon vertices on slide 24, which are samples of the resulting curve.

This division is what makes the representation economical: a small fixed set of curve controls can
produce many smooth silhouettes, while high and low topology merely choose how accurately to sample
the selected silhouette. Evaluating the cubic and its derivative remains a fixed, inexpensive
vertex-shader operation.

### Slide 29 - construct the control points

The tip position is selected relative to the base using the blade's facing and tilt parameters. The
slide then labels a `midpoint` controlled by bend:

- with bend equal to zero, it lies on the line between base and tip;
- increasing bend pushes it upward and away from that line.

The deck identifies the representation as cubic and refers to movable control points in the plural,
but this slide visualizes only one derived `midpoint`. The slide and current live retelling do not
fully specify whether that midpoint is reused for both interior controls or how two interior controls
are derived from it. We should not infer that every ribbon row from slide 24 is an independent bend
point, nor should we invent an exact control-point formula that the talk did not provide.

What is explicit is the design intent: tilt chooses the tip relative to the base, bend moves the
interior control region away from the direct root-to-tip line, and moving the controls changes the
blade silhouette cheaply. A suitable implementation can preserve that intent while exposing enough
independent root- and tip-side control to produce upright blades, broad arches, late tip droop, and
other smooth curvature distributions without adding mesh vertices.

For the split short-grass form, the two blades keep the same general shape and facing but their
control points are pushed apart. This improves coverage without making the pair look unrelated.

### Slide 30 - generate position, width, and normal

The vertex shader builds a vertex as follows:

1. Feed its remapped 0-to-1 longitudinal value into the cubic Bezier function to obtain the center
   position in world space.
2. Construct a horizontal vector orthogonal to the facing direction by swapping the facing vector's
   X/Y components and negating one component.
3. Move the vertex left or right along that vector. The displacement uses the width calculated in
   compute and a longitudinal taper so the blade narrows toward its tip.
4. Evaluate the Bezier derivative at the same longitudinal position.
5. Cross the curve derivative with the width direction to obtain the geometric vertex normal.

### Yarra adoption: keep curve shape and vertex placement independent

Yarra keeps the same fixed-budget distinction exposed by these slides. A ribbon has one cubic
centreline with independent root- and tip-side tangent handles. Each of the two authored variants
owns a complete silhouette: normalized tip position plus both control handles. A blade interpolates
between those complete curves with one stable value instead of independently randomizing tilt and
the handles into combinations the artist never saw. The source angles and handle lengths are
converted to normalized-height handle vectors once per species, so the vertex shader does not
perform extra trigonometry for this authoring freedom.

The authoring view presents those endpoints as interactive minimum and maximum Bezier charts. The
root is fixed; the two interior controls and the tip are dragged directly. Markers on the curve show
the actual high-LOD row samples after the longitudinal remap, so silhouette shape and allocation of
the fixed vertex budget can be judged together. This is a Yarra tool decision, not evidence that the
talk used the same UI.

The longitudinal vertex-distribution control is separate. It remaps each fixed topology row before
the cubic evaluation: `1` is even spacing, values above `1` concentrate rows near the root, and
values below `1` concentrate them near the tip. High and low LOD continue to sample the same cubic
and use the same remap, so neither curve authoring nor non-uniform sampling adds vertices, instances,
per-blade storage, or draw calls.

## Slides 31-32: unified wind and constrained animation

### Slide 31 - a simple CPU/GPU-sampleable wind field

They chose a unified wind system that can be sampled on both CPU and GPU with relatively little
overhead. The base wind is effectively two-dimensional Perlin noise, shaped by artist parameters and
scrolled in the direction the wind is travelling.

Sampling the noise at a location produces one scalar push force. That scalar is combined with the
two-dimensional wind direction and supplied to systems that react to wind. Grass and some particle
systems sample an additional Perlin-noise layer to obtain more complex motion.

The goal is not physically complete air simulation. It is one coherent, inexpensive field that
different gameplay and rendering systems can agree on.

### Slide 32 - per-blade phase and bobbing

The grass adds a simple sine-wave bob. Its phase is offset by both a per-blade hash and the position
along the blade, which turns a uniform displacement into a swaying motion. The hash prevents every
blade from moving in exactly the same phase.

The talk also notes that cubic Bezier arc length is not easy to calculate or hold constant while the
control points animate. Blade length therefore varies somewhat during motion. They accept this
because the motion is constrained enough that the length change is not noticeable.

## Slides 33-35: normals and apparent fullness

### Slide 33 - rounded lighting without more geometry

The blade's normals are tilted outward slightly instead of remaining the normals of a perfectly flat
ribbon. The slide directly compares "Flat normals" with "Rounded normals." The rounded field makes
the blade look more naturally curved while costing much less than adding cross-section vertices.

### Slides 34-35 - thicken edge-on blades in view space

They also found that fields did not look full enough. Adding more blades worked but was too expensive.
Their alternative slightly shifts blade vertices in view space when a blade's normal is close to
orthogonal to the view vector - the orientation in which a thin ribbon would almost disappear.

The adjustment subtly widens an edge-on blade from the viewer's perspective. It makes the field look
fuller and avoids spending rasterization work on extremely thin triangles. Slides 34 and 35 show the
direct before/after field comparison.

Yarra adoption: the first attempt imposed a minimum projected pixel width. Although it preserved the
centreline and added no topology, close curved blades became uniformly thin screen-space filaments
and the field read as a spider web. That result is rejected.

The revised approach first transports the ribbon's width axis onto the plane perpendicular to the
local Bezier tangent at every existing row. It then rotates that unoriented width line toward the
camera-facing width line by no more than a small, species-bounded angle around the tangent. The
response is naturally zero when the ribbon already faces the camera and grows continuously as its
projected width approaches edge-on. The authored
centreline, world-space width, taper, vertex count, instance data, and draw structure remain intact;
broad leaves bypass the view-opening response. Lighting continues to use the physical ribbon normal,
with the separate authored outward normal tilt from slide 33, so highlights do not billboard with the
silhouette. Fixed-camera grazing captures must reject obvious camera-following motion or excess
fragment coverage.

## Slides 36-38: distant material stability and G-buffer output

### Slides 36-37 - suppress distant specular glitter

Glossy animated grass produced heavily aliased specular highlights at middle and far distances,
especially in rain. Neighboring grass normals can vary dramatically within a few screen pixels, so
the moving field turns into unstable glitter.

They apply two distance-based responses:

1. As camera distance increases, lerp each blade's output normal toward a common normal for its grass
   clump. The shared normal retains the large-scale shape of the field while removing unresolved
   blade-to-blade variation.
2. Reduce gloss in the pixel shader with distance. If gloss describes the unresolved distribution of
   surface normals, then growing subpixel normal variance should produce a rougher, less glossy
   result.

Slides 36 and 37 provide a direct comparison of the sparkling and stabilized fields.

### Slide 38 - material channels

Grass writes material data into the deferred renderer's G-buffers.

Gloss uses a one-dimensional texture stretched across the blade width and repeated down its length.

Diffuse color uses two textures:

- a similar one-dimensional texture supplies the central vein and variation across the blade width;
- a two-dimensional color texture supplies longitudinal and clump variation. Its V coordinate moves
  from base to tip, allowing (for example) a dark base and lighter upper blade. Its U coordinate is
  selected per clump rather than per blade, producing subtle, artist-controlled patches of color
  across the field instead of independent color noise on every blade.

Translucency and ambient occlusion are inexpensive profiles that vary over blade length. The base is
described as having little translucency because it is thick. The live retelling initially said the
value then "reduces toward the tip"; that direction is uncertain and should be checked against the
recording, because the physical explanation suggests the tip should become more translucent. The AO
intent is clear: darker/occluded near the crowded base and lighter toward the tip.

### Why not rely on temporally accumulated SSAO?

Their grass does not write a velocity buffer. Reconstructing a correct previous-frame vertex would
require previous wind state (including changing direction/speed), the previous interaction or
displacement field, and potentially processed per-blade data from compute. It is possible, but the
memory and performance cost was not practical for them.

More importantly, the content itself is hostile to temporally accumulated AO. Grass blades constantly
cross, occlude, and disocclude one another. Even correct velocity vectors would feed rapidly changing
coverage into the temporal effect and were expected to produce a splotchy, glittering result. They
therefore provide a stable authored AO profile on the blade instead.

## Slide 40: GPU-instanced authored field assets

The fields are not made only from procedural grass blades. They also contain fully artist-authored
assets such as tiny flowers, spider lilies, and - most commonly - pampas grass. These objects are not
created as normal heavyweight game objects. They are rendered through the same kind of GPU instance
draw system used by the game's growth system.

The reusable instance stream is deliberately minimal. It contains the information needed to place,
orient, cull, and draw an authored asset rather than a complete gameplay-object representation. When
a grass tile loads, a compute shader generates that same instance-data stream for the authored assets
that belong in the procedural field.

Placement follows the grass-blade strategy:

1. Generate candidate positions randomly and deterministically within the tile.
2. At each position, sample which grass/coverage type exists there.
3. If it matches the authored asset's required type, append a procedural transform and culling record
   to the instance stream.
4. Render the accepted instances later with GPU-instanced asset draws.

Only the nearest 3x3 tile neighborhood is retained. Tiles leaving that neighborhood discard their
generated asset instances so distant decoration does not consume unbounded memory. The slide itself
shows a field densely enriched with authored flowering plants rather than a data-flow diagram; the
spoken explanation supplies the system details above.

## Slides 45-46: grass shadows without rendering every blade into every light

They can run the full grass compute, vertex, and pixel pipeline for a shadow-casting light. Doing so
at least once per light is extremely expensive, so it is reserved for rare situations rather than
used as the normal grass-shadow path.

### Slide 45 - terrain-based shadow impostor

The primary approximation reuses the terrain beneath the grass as proxy geometry:

1. Raise the terrain proxy's vertices to approximately the grass height at that location.
2. Offset the depth written into the light's shadow map using a dithered pattern.
3. Let ordinary shadow filtering integrate the pattern into a result that approximates the field's
   average shadow density.

This represents the low-frequency mass of the grass field without submitting all of its individual
blades. It is not artifact-free: the proxy terrain is discrete, so its mesh resolution can leave hard
edges that are difficult to hide. The talk says the result is nevertheless good in the majority of
the game's scenes. Slide 45 shows the broad, soft terrain/proxy contribution with little individual
blade detail.

### Slide 46 - add short-range screen-space detail

Screen-space shadows restore the missing fine blade shadows near visible grass. Their usual
limitations happen to align reasonably well with this content:

- they cannot infer real object thickness, but grass blades are extremely thin;
- they operate only at short range, but grass usually occupies a small screen-space scale;
- they know nothing about off-screen geometry, but a small grass blade just outside the screen is
  unlikely to cast a large, important on-screen shadow.

Slide 46 shows the same boundary with much sharper short blade shadows added. The final result is a
two-scale solution: the terrain impostor supplies inexpensive broad density and screen-space shadows
supply local high-frequency contact detail. Together they achieve useful grass-shadow quality
without multiplying the complete procedural grass pipeline across every shadow-casting light.

## Cross-slide principles captured so far

These slides repeatedly use a small set of strategies:

- generate geometry and draw counts entirely on the GPU;
- bound transient memory, then overlap compute and graphics;
- derive topology procedurally from IDs rather than storing vertex streams;
- spend fewer samples on distant geometry, but explicitly morph both shape and density;
- reuse the same vertex budget more cleverly for short grass;
- prefer stable, filtered appearances over physically detailed signals that cannot be resolved at
  distance;
- put variation at the scale where it is visually meaningful: per blade for motion, per clump for
  color, and shared per clump for distant normals;
- avoid expensive temporal data when the underlying alpha-rich content is temporally unstable anyway;
- feed artist-authored field assets through compact GPU instance streams rather than heavyweight game
  objects;
- keep generated asset residency spatially bounded around the camera;
- split grass shadows by frequency: approximate the broad field density with cheap proxy geometry,
  then add only the visible short-range detail in screen space.

These are observations from the talk, not yet decisions for Yarra.
