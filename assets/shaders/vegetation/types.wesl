// GPU layouts and constants shared by the vegetation passes: scheduling, placement and drawing.
// Rust mirrors them in crates/vegetation_render/src/renderer/gpu_types.rs; the integer limits
// arrive from there as `constants::` definitions (`gpu_types::shader_defs`).

// One resident grass field: a page of one grass type's candidate lattice.
struct WorkItem {
    // xy: page origin x/z, z: page size, w: terrain contact blocked (1) or not (0)
    page: vec4<f32>,
    // xy: minimum world-lattice cell, zw: cell count
    domain: vec4<i32>,
    // x: candidate count, y: coverage offset, z: coverage resolution, w: grass type index
    field: vec4<u32>,
    // x: surface sample offset, y: surface resolution, zw: minimum/maximum height as f32 bits
    surface: vec4<u32>,
}

// One grass type (vegetation::GrassType).
struct GrassType {
    // xy: height range, zw: width range
    size: vec4<f32>,
    // xy: tilt range, zw: bend range
    shape: vec4<f32>,
    // x: clump size, y: pull, z: clump height variation, w: shared facing
    clumps: vec4<f32>,
    // x: splay, y: lattice spacing, z: type seed as bits, w: maximum horizontal reach
    placement: vec4<f32>,
    // xyz: root colour, w: dry clump share
    root: vec4<f32>,
    // xyz: tip colour, w: clump brightness variation
    tip: vec4<f32>,
    // xyz: dry tip colour, w: patches (coverage thins whole clumps)
    dry_tip: vec4<f32>,
    // x: gloss, y: roughness, z: translucency, w: stiffness
    material: vec4<f32>,
    // x: twin blades (1) or one (0); yzw reserved
    form: vec4<f32>,
}

struct Instance {
    // xyz: root, w: packed clump key and nested LOD rank (unorm16 each)
    root_clump: vec4<f32>,
    // x: packed facing angle and clump distance (unorm16 each),
    // y: grass type index + LOD morph + low-detail flag,
    // z: packed surface normal xz, w: low 24 bits seed + high 8 bits population-density target
    geometry: vec4<u32>,
}

// One camera layout for every pass; the light is read only by drawing.
struct Camera {
    clip_from_world: mat4x4<f32>,
    // xyz: world position, w: viewport height
    camera_position: vec4<f32>,
    // x: vertical focal length in pixels, y: viewport width, z: viewport height,
    // w: high-detail radius in metres
    projection: vec4<f32>,
    // xyz: direction from the surface toward the strongest directional light, w: active
    sun_direction: vec4<f32>,
    // xyz: strongest directional-light color and global ambient-light color
    sun_radiance: vec4<f32>,
    ambient_radiance: vec4<f32>,
    // x: diffuse, y: specular, z: transmission, w: received-shadow strength
    lighting: vec4<f32>,
    // xy: world-XZ direction, z: strength (tip displacement / height), w: phase seconds
    wind: vec4<f32>,
    // x: spatial frequency, y: speed, z: gustiness, w: blade flutter
    wind_shape: vec4<f32>,
    // xy: canonical XZ offset of render coordinates.
    render_origin: vec4<f32>,
    lod_focus: vec4<f32>,
    // xy: the trample map's canonical centre, z: its side in metres, w: 1 when it is stamped
    trample: vec4<f32>,
    // The weather's gusts over render XZ (weather/gust.wesl): the wind's direction xz, the
    // pattern's offsets along and across it; x of the second, how far they swell the wind.
    gusts: vec4<f32>,
    gust_swell: vec4<f32>,
}

// Last frame's camera, for motion vectors, and the clip transform the grass rasterizes with.
struct TemporalGrass { previous: Camera, raster_clip: mat4x4<f32> }

// One blade posed by trampling and the wind (pose.wesl), as drawing reads it: 18 halves in
// order, then two snorm16 pairs. The halves are this frame's control points p1, p2, p3 after the
// root (9), then their change since last frame (9): a frame's few millimetres keep their
// precision as a change, where two positions in halves would differ by their rounding. The
// pairs are the width axis xy, then its z and how far the blade is pressed down.
struct BladePose {
    curve: vec4<u32>,
    change: vec4<u32>,
    frame: vec4<u32>,
}

struct Config {
    // x: live work items, y: diagnostic counters enabled, z: candidate cache, w: reserved
    workload: vec4<u32>,
}

struct Telemetry {
    // 0: scheduled work items, 1: candidate evaluations, 2..3: eligible instances per bin,
    // 6..7: capacity-dropped instances per bin, 10: widest candidate dispatch
    values: array<atomic<u32>, 16>,
}

const PI: f32 = 3.141592653589793;
const TYPE_INDEX_MASK: u32 = constants::TYPE_INDEX_MASK;
const LOD_MORPH_MASK: u32 = 0x00007fffu;
const LOD_MORPH_SHIFT: u32 = 16u;
const MAX_PROCEDURAL_DISTANCE: f32 = f32(constants::PROCEDURAL_DISTANCE_METERS);
// Share of the stable rank range over which thinned blades narrow away instead of popping.
const DENSITY_FADE_BAND: f32 = 0.10;
