// GPU layouts and constants shared by the vegetation passes: scheduling, placement, blade
// preparation and drawing. Rust mirrors them in crates/vegetation_render/src/renderer/gpu_types.rs;
// the integer limits arrive from there as `constants::` definitions (`gpu_types::shader_defs`).

// One resident population field: a page of the candidate lattice and its placement parameters.
struct WorkItem {
    page: vec4<f32>,
    domain: vec4<i32>,
    candidate_layout: vec4<u32>,
    population: vec4<u32>,
    growth: vec4<f32>,
    direction_weights: vec4<f32>,
    flow_density: vec4<f32>,
    grouping: vec4<f32>,
    group_density: vec4<f32>,
    orientation: vec4<f32>,
    peers: vec4<u32>,
    // xy: surface offset/resolution, zw: minimum/maximum page height as f32 bits
    surface: vec4<u32>,
    // x: maximum height, y: maximum horizontal reach, z: minimum high-LOD threshold,
    // w: maximum low-LOD density
    bounds: vec4<f32>,
}

struct ProceduralInstance {
    // xyz: root, w: packed clump variant and nested LOD rank
    root_clump: vec4<f32>,
    // x: packed rest direction, y: species index + split-topology flag + LOD morph + low flag,
    // z: packed surface normal xz,
    // w: low 24 bits seed + high 8 bits population-density target
    geometry: vec4<u32>,
}

// One accepted (or, for outcomes, rejected) candidate in a placement diagnostic mode.
struct DiagnosticInstance {
    // xyz: root, w: rest direction x
    root_direction: vec4<f32>,
    // x: rest direction z, y: clump variant, z: species index as f32, w: packed surface normal xz
    direction_species: vec4<f32>,
    // xyz: parent position, w: candidate outcome code
    parent_status: vec4<f32>,
    // x: local occupancy, y: stable rank, z: group distance, w: group influence
    diagnostics: vec4<f32>,
}

struct Species {
    // xyz: root color, w: topology family code
    root_color: vec4<f32>,
    // xyz: tip color, w: maximum height
    tip_color_height: vec4<f32>,
    // xy: height range, zw: half-width range
    bounds: vec4<f32>,
    // x: high sections, y: low sections, z: reserved, w: longitudinal power
    topology: vec4<f32>,
    // xy: tilt range; zw: broad-leaf droop range
    shape: vec4<f32>,
    // x: lateral curve/camber, y: pair spread,
    // z: tangent of maximum ribbon view-opening angle / broad crown radius,
    // w: maximum horizontal reach
    shape_secondary: vec4<f32>,
    // xy: normalized-height root-handle forward/normal vector,
    // zw: normalized-height tip-handle forward/normal vector
    curve_variant_a: vec4<f32>,
    curve_variant_b: vec4<f32>,
    // x: clump color variation, y: roughness, z: transmission, w: normal rounding
    material: vec4<f32>,
    // x: root AO, y: tip AO, z: high-LOD threshold, w: reserved
    shading: vec4<f32>,
    // xyz: group coherence for height, complete silhouette, and lateral curve
    group_response: vec4<f32>,
    // x: short/tall height bias, y: reserved, zw: single/split high-topology radii
    height_packing: vec4<f32>,
}

// One camera layout for every pass; the environment values are read only by drawing.
struct Camera {
    clip_from_world: mat4x4<f32>,
    // xyz: world position, w: viewport height
    camera_position: vec4<f32>,
    // x: vertical focal length in pixels, y: viewport width, z: viewport height,
    // w: far-ribbon screen-space width compensation enabled
    projection: vec4<f32>,
    // xyz: direction from the surface toward the strongest directional light, w: active
    sun_direction: vec4<f32>,
    // xyz: strongest directional-light color and global ambient-light color
    sun_radiance: vec4<f32>,
    ambient_radiance: vec4<f32>,
    // x: diffuse, y: specular, z: transmission, w: received-shadow strength
    lighting: vec4<f32>,
    // xy: world-XZ direction, z: maximum tip displacement / height, w: phase seconds
    wind: vec4<f32>,
    // x: spatial frequency, y: speed, z: gustiness, w: hashed blade flutter
    wind_shape: vec4<f32>,
    // xy: canonical XZ offset of render coordinates.
    render_origin: vec4<f32>,
    lod_focus: vec4<f32>,
    canopy_appearance: vec4<f32>,
    canopy_shape: vec4<f32>,
    canopy_distance: vec4<f32>,
    canopy_origin: vec4<f32>,
}

struct Config {
    // x: 0 geometry, 1 accepted species, 2 parent links, 3 outcomes, 4 group structure
    // y: 0 authored density, 1 balanced production density, 2 full-density reference
    // z: 0 rounded/clump gloss, 2 unlit diagnostic, 3 vertex-only diagnostic
    // w: scene-adaptive single-low arena capacity (draw uses it only through indirect offsets)
    values: vec4<u32>,
    // x: live work items, y: diagnostic counters enabled, z: prepared blade data available,
    // w: bit 0 early rejection, bit 1 candidate cache
    workload: vec4<u32>,
}

struct Telemetry {
    // 0: scheduled work items, 1: classify-once candidate evaluations
    // 2..5: eligible instances per bin, 6..9: capacity-dropped instances per bin,
    // 10: widest candidate dispatch, 11..12: prepared and fallback blades
    values: array<atomic<u32>, 16>,
}

const PI: f32 = 3.141592653589793;
const DENSITY_MODE_BALANCED: u32 = 1u;
const DENSITY_MODE_FULL_REFERENCE: u32 = 2u;
const LIGHTING_MODE_UNLIT_DIAGNOSTIC: u32 = 2u;
const LIGHTING_MODE_VERTEX_ONLY_DIAGNOSTIC: u32 = 3u;
const SPECIES_INDEX_MASK: u32 = constants::SPECIES_INDEX_MASK;
// Placement binned the root as a split render unit: two blades, a folded pair or a broad leaf.
const SPLIT_TOPOLOGY_BIT: u32 = constants::SPLIT_TOPOLOGY_BIT;
const LOD_MORPH_MASK: u32 = 0x00007fffu;
const LOD_MORPH_SHIFT: u32 = 16u;
// A visible work-item entry with this bit schedules the quarter lattice.
const QUARTER_LOD_FLAG: u32 = 0x80000000u;
const MAX_PROCEDURAL_DISTANCE: f32 = f32(constants::PROCEDURAL_DISTANCE_METERS);
const BALANCED_DENSITY_FADE_BAND: f32 = 0.10;
// Retain the established distant population with the original three-triangle low pair.
const SPLIT_LOW_DENSITY_BUDGET_SCALE: f32 = 0.65;
