// Vegetation V2 placement, projected-size LOD, and bounded classify-once emission.
//
// Every candidate performs expensive placement, terrain, competition, species, visibility, and LOD
// work exactly once. Accepted instances append directly to one of four bounded topology arenas. The
// capacities are emergency guards, not population controls: a nonzero capacity-drop counter rejects
// the content/device budget and must be fixed rather than accepted as normal rendering behavior.

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
    surface: vec4<u32>,
    bounds: vec4<f32>,
}

struct SpeciesChoice {
    metadata: vec4<u32>,
    threshold: vec4<f32>,
    density: vec4<f32>,
    // xy: minimum/maximum height, z: short/tall bias, w: group height coherence
    height: vec4<f32>,
    // x: pair-below height (zero disables), yz: single/split high-topology radii
    packing: vec4<f32>,
}

struct ProceduralInstance {
    // xyz: root, w: clump variant
    root_clump: vec4<f32>,
    // x: packed rest direction, y: species index + LOD morph + low flag,
    // z: packed surface normal xz,
    // w: low 24 bits seed + high 8 bits population-density target
    geometry: vec4<u32>,
}

struct DebugInstance {
    root_direction: vec4<f32>,
    direction_species: vec4<f32>,
    parent_status: vec4<f32>,
    diagnostics: vec4<f32>,
}

// Equal-area ellipse: spend the same high-topology budget along the view direction.
// The gameplay centre follows the subject; clients without a focus retain the camera disk.
fn detail_distance(root: vec2<f32>) -> f32 {
    let delta = root - camera.lod_focus.xy;
    let forward = camera.lod_focus.zw;
    if (dot(forward, forward) < 0.5) { return length(root - camera.camera_position.xz); }
    let along = dot(delta, forward) / 1.5;
    let across = dot(delta, vec2(-forward.y, forward.x)) * 1.5;
    return length(vec2(along, across));
}

fn near_field_coverage(root: vec2<f32>) -> f32 {
    if (dot(camera.lod_focus.zw, camera.lod_focus.zw) < 0.5) { return 0.0; }
    return 1.0 - smoothstep(12.0, 26.0, detail_distance(root));
}

struct DebugConfig {
    // x: 0 geometry, 1 accepted species, 2 parent links, 3 outcomes, 4 group structure
    // y: 0 authored density, 1 balanced production density, 2 full-density reference
    // z: 0 rounded/clump gloss, 1 legacy empirical lighting
    // w: scene-adaptive single-low arena capacity
    values: vec4<u32>,
    // x: live work items, y: diagnostic counters enabled
    workload: vec4<u32>,
}

struct Camera {
    clip_from_world: mat4x4<f32>,
    camera_position: vec4<f32>,
    // x: vertical focal length in pixels, y: viewport width, z: viewport height
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
    lod_focus: vec4<f32>,
    canopy_appearance: vec4<f32>,
    canopy_shape: vec4<f32>,
    canopy_distance: vec4<f32>,
    canopy_origin: vec4<f32>,
}

struct SurfaceSample {
    height_validity: vec4<f32>,
    normal: vec4<f32>,
}

struct SurfaceResult {
    height: f32,
    normal: vec3<f32>,
    validity: f32,
}

struct DrawIndexedIndirectArgs {
    index_count: atomic<u32>,
    instance_count: atomic<u32>,
    first_index: atomic<u32>,
    base_vertex: atomic<u32>,
    first_instance: atomic<u32>,
}

struct Candidate {
    root: vec2<f32>,
    group_center: vec2<f32>,
    direction: vec2<f32>,
    stable_rank: f32,
    lod_rank: f32,
    clump_variant: f32,
    group_distance: f32,
    group_influence: f32,
    group_density: f32,
    seed: u32,
}

struct GroupSample {
    center: vec2<f32>,
    radial: vec2<f32>,
    normalized_distance: f32,
    boundary_influence: f32,
    density_retention: f32,
    key: u32,
}

struct CandidateEvaluation {
    candidate: Candidate,
    surface: SurfaceResult,
    occupancy: f32,
    outcome: u32,
    species_index: u32,
    bin: u32,
    lod_morph: f32,
    population_density: f32,
    eligible: u32,
    // Projected and budget extents, expressed as multiples of the high threshold.
    shape_extents: vec2<f32>,
}

struct Telemetry {
    // 0: scheduled work items, 1: classify-once candidate evaluations
    // 2..5: eligible instances per bin, 6..9: capacity-dropped instances per bin
    values: array<atomic<u32>, 16>,
}

@group(0) @binding(0) var<storage, read> work_items: array<WorkItem>;
@group(0) @binding(1) var<storage, read> choices: array<SpeciesChoice>;
@group(0) @binding(2) var<storage, read> coverage_values: array<f32>;
@group(0) @binding(3) var<storage, read> surface_samples: array<SurfaceSample>;
@group(0) @binding(4) var<storage, read_write> procedural_instances: array<ProceduralInstance>;
@group(0) @binding(5) var<storage, read_write> diagnostic_instances: array<DebugInstance>;
@group(0) @binding(6) var<storage, read_write> draw_args: array<DrawIndexedIndirectArgs, 4>;
@group(0) @binding(7) var<storage, read> visible_work_items: array<u32>;
@group(0) @binding(8) var<uniform> debug_config: DebugConfig;
@group(0) @binding(9) var<uniform> camera: Camera;
@group(0) @binding(10) var<storage, read_write> telemetry: Telemetry;


// One source-stable acceptance bit per original candidate, preserving lattice dispatch order.
// Ready is published only after every word is written. Zero capacity means reference fallback.
struct CandidateCacheEntry {
    base: u32,
    capacity: u32,
    reserved: u32,
    ready: atomic<u32>,
}

@group(0) @binding(11) var<storage, read_write> candidate_cache_entries: array<CandidateCacheEntry>;
@group(0) @binding(12) var<storage, read_write> candidate_acceptance_bits: array<u32>;
@group(0) @binding(13) var<storage, read> cache_build_items: array<u32>;

fn use_candidate_cache(index: u32, quarter_lod: bool) -> bool {
    return !quarter_lod && debug_config.values.x == 0u && (debug_config.workload.w & 2u) != 0u
        && atomicLoad(&candidate_cache_entries[index].ready) != 0u;
}

// Deliberately excludes all camera, wind, projected size and population LOD inputs.
fn stable_candidate_is_present(item: WorkItem, index: u32) -> bool {
    let candidate = sample_candidate(item, index);
    if (!owns(item, candidate.root) || candidate.stable_rank >= item.growth.w) { return false; }
    let surface = sample_surface(item, candidate.root);
    if (surface.validity < 0.5) { return false; }
    let occupancy = local_occupancy(item, candidate.root) * candidate.group_density;
    return random01(candidate.seed ^ 0x4cf5ad43u) < occupancy;
}

var<workgroup> acceptance: array<u32, 64>;

@compute @workgroup_size(64)
fn build_candidate_cache(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_index) lane: u32,
) {
    let index = cache_build_items[id.y];
    var present = false;
    if (id.x < candidate_cache_entries[index].capacity) {
        present = stable_candidate_is_present(work_items[index], id.x);
    }
    acceptance[lane] = select(0u, 1u, present);
    workgroupBarrier();
    if ((lane & 31u) == 0u && id.x < candidate_cache_entries[index].capacity) {
        var mask = 0u;
        for (var bit = 0u; bit < 32u; bit += 1u) {
            mask |= acceptance[lane + bit] << bit;
        }
        candidate_acceptance_bits[candidate_cache_entries[index].base + id.x / 32u] = mask;
    }
}

@compute @workgroup_size(4)
fn finish_candidate_cache(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = cache_build_items[id.x];
    if (index != 0xffffffffu) { atomicStore(&candidate_cache_entries[index].ready, 1u); }
}

const PI_2: f32 = 6.283185307179586;
const SINGLE_HIGH_CAPACITY: u32 = 32768u;
const SPLIT_HIGH_CAPACITY: u32 = 32768u;
const LOW_DETAIL_CAPACITY: u32 = 786432u;
const LOW_DETAIL_MINIMUM_PARTITION: u32 = 32768u;
const SPLIT_HIGH_OFFSET: u32 = SINGLE_HIGH_CAPACITY;
const LOW_DETAIL_OFFSET: u32 = SINGLE_HIGH_CAPACITY + SPLIT_HIGH_CAPACITY;
const MAX_DIAGNOSTIC_INSTANCES: u32 = 65536u;
const MAX_PROCEDURAL_DISTANCE: f32 = 96.0;
// Geometry quality control, separate from the near-field density footprint.
// 1.0 restores the previous range; 0.65 switches to the low mesh 35% sooner.
const HIGH_TOPOLOGY_DISTANCE_SCALE: f32 = 0.65;
const QUARTER_LOD_FLAG: u32 = 0x80000000u;
const WORK_ITEM_INDEX_MASK: u32 = 0x7fffffffu;
const DIAGNOSTIC_INDEX_COUNT: u32 = 6u;
const SINGLE_HIGH_INDEX_COUNT: u32 = 48u;
const SINGLE_LOW_INDEX_COUNT: u32 = 18u;
const SPLIT_HIGH_INDEX_COUNT: u32 = 39u;
const SPLIT_LOW_INDEX_COUNT: u32 = 9u;
const DIAGNOSTIC_FIRST_INDEX: u32 = 0u;
const SINGLE_HIGH_FIRST_INDEX: u32 = 6u;
const SINGLE_LOW_FIRST_INDEX: u32 = 54u;
const SPLIT_HIGH_FIRST_INDEX: u32 = 72u;
const SPLIT_LOW_FIRST_INDEX: u32 = 111u;
// Packed above the lighting mode in DebugConfig.values.z. This preserves the accepted population
// while measuring the existing low topology across the complete field.
const FORCE_LOW_TOPOLOGY_BIT: u32 = 0x00000100u;
const MOBILE_POPULATION_LOD_BIT: u32 = 0x00000200u;
const DENSITY_MODE_BALANCED: u32 = 1u;
const DENSITY_MODE_FULL_REFERENCE: u32 = 2u;
const SPECIES_INDEX_MASK: u32 = 0x0000ffffu;
const LOD_MORPH_MASK: u32 = 0x00007fffu;
const LOD_MORPH_SHIFT: u32 = 16u;
const BALANCED_DENSITY_FULL_SPACING_PIXELS: f32 = 6.0;
const BALANCED_DENSITY_MIDDLE_SPACING_PIXELS: f32 = 2.0;
const BALANCED_DENSITY_FAR_SPACING_PIXELS: f32 = 0.75;
const BALANCED_DENSITY_MIDDLE_FRACTION: f32 = 0.55;
const BALANCED_DENSITY_FAR_FRACTION: f32 = 0.30;
const BALANCED_DENSITY_FADE_BAND: f32 = 0.10;
// Retain the established distant population with the original three-triangle low pair.
const SPLIT_LOW_DENSITY_BUDGET_SCALE: f32 = 0.65;
// Deliberately aggressive calibration point for mobile. If one quarter of the roots on the
// cheapest topology cannot materially change frame rate, a gentler ribbon LOD cannot reach the
// target and the far field needs a different representation.
const MOBILE_DIAGNOSTIC_DENSITY_FRACTION: f32 = 0.25;

fn hash32(value: u32) -> u32 {
    var x = value;
    x = x ^ (x >> 16u);
    x = x * 0x7feb352du;
    x = x ^ (x >> 15u);
    x = x * 0x846ca68bu;
    return x ^ (x >> 16u);
}

fn hash_cell(seed: u32, cell: vec2<i32>, salt: u32) -> u32 {
    return hash32(
        seed
        ^ bitcast<u32>(cell.x) * 0x8da6b343u
        ^ bitcast<u32>(cell.y) * 0xd8163841u
        ^ salt,
    );
}

fn random01(value: u32) -> f32 {
    return f32(hash32(value)) * (1.0 / 4294967295.0);
}

fn normalize_or(value: vec2<f32>, fallback: vec2<f32>) -> vec2<f32> {
    let length_squared = dot(value, value);
    if (length_squared <= 1e-10) {
        return fallback;
    }
    return value * inverseSqrt(length_squared);
}

fn floor_div_two(value: i32) -> i32 {
    if (value < 0 && (value & 1) != 0) {
        return (value - 1) / 2;
    }
    return value / 2;
}

fn quarter_candidate_count(item: WorkItem) -> u32 {
    if (item.peers.z != 0u) {
        return item.candidate_layout.y / 4u;
    }
    let minimum = item.domain.xy;
    let maximum = minimum + item.domain.zw;
    let block_minimum = vec2<i32>(floor_div_two(minimum.x), floor_div_two(minimum.y));
    let block_maximum = vec2<i32>(
        floor_div_two(maximum.x - 1) + 1,
        floor_div_two(maximum.y - 1) + 1,
    );
    let block_count = vec2<u32>(block_maximum - block_minimum);
    return block_count.x * block_count.y;
}

fn remap_quarter_candidate(item: WorkItem, reduced_index: u32) -> u32 {
    if (item.peers.z != 0u) {
        let groups_per_cell = item.candidate_layout.x / 4u;
        let cell_index = reduced_index / groups_per_cell;
        let group = reduced_index % groups_per_cell;
        let cell = vec2<i32>(
            item.domain.x + i32(cell_index % u32(item.domain.z)),
            item.domain.y + i32(cell_index / u32(item.domain.z)),
        );
        let rotation = hash_cell(
            item.population.z,
            cell,
            0xa24baed5u ^ group * 0x9e3779b9u,
        ) & 3u;
        let selected_child = group * 4u + ((4u - rotation) & 3u);
        return cell_index * item.candidate_layout.x + selected_child;
    }

    let minimum = item.domain.xy;
    let maximum = minimum + item.domain.zw;
    let block_minimum = vec2<i32>(floor_div_two(minimum.x), floor_div_two(minimum.y));
    let block_maximum = vec2<i32>(
        floor_div_two(maximum.x - 1) + 1,
        floor_div_two(maximum.y - 1) + 1,
    );
    let block_count_x = u32(block_maximum.x - block_minimum.x);
    let block = block_minimum + vec2<i32>(
        i32(reduced_index % block_count_x),
        i32(reduced_index / block_count_x),
    );
    let rotation = hash_cell(item.population.z, block, 0xa24baed5u) & 3u;
    let selected_quadrant = (4u - rotation) & 3u;
    let cell = block * 2 + vec2<i32>(
        i32(selected_quadrant & 1u),
        i32(selected_quadrant >> 1u),
    );
    if (any(cell < minimum) || any(cell >= maximum)) {
        return 0xffffffffu;
    }
    let local = cell - minimum;
    return (u32(local.y) * u32(item.domain.z) + u32(local.x)) * item.candidate_layout.x;
}

fn owns(item: WorkItem, world_xz: vec2<f32>) -> bool {
    return all(world_xz >= item.page.xy) && all(world_xz < item.page.xy + vec2<f32>(item.page.z));
}

fn sample_coverage(item: WorkItem, world_xz: vec2<f32>) -> f32 {
    if (!owns(item, world_xz)) {
        return 0.0;
    }
    let resolution = item.candidate_layout.w;
    let local = clamp((world_xz - item.page.xy) / item.page.z, vec2<f32>(0.0), vec2<f32>(0.999999));
    let texel = vec2<u32>(local * f32(resolution));
    return coverage_values[item.candidate_layout.z + texel.y * resolution + texel.x];
}

fn sample_surface(item: WorkItem, world_xz: vec2<f32>) -> SurfaceResult {
    let resolution = item.surface.y;
    let local = clamp((world_xz - item.page.xy) / item.page.z, vec2<f32>(0.0), vec2<f32>(1.0));
    let grid = local * f32(resolution - 1u);
    let minimum = vec2<u32>(floor(grid));
    let maximum = min(minimum + vec2<u32>(1u), vec2<u32>(resolution - 1u));
    let blend = fract(grid);
    let indices = array<u32, 4>(
        item.surface.x + minimum.y * resolution + minimum.x,
        item.surface.x + minimum.y * resolution + maximum.x,
        item.surface.x + maximum.y * resolution + minimum.x,
        item.surface.x + maximum.y * resolution + maximum.x,
    );
    // Same 00--11 triangle diagonal as the ground mesh and CPU surface_triangle_weights.
    var weights = array<f32, 4>(1.0 - blend.x, blend.x - blend.y, 0.0, blend.y);
    if (blend.y >= blend.x) {
        weights = array<f32, 4>(1.0 - blend.y, 0.0, blend.y - blend.x, blend.x);
    }
    var height = 0.0;
    var normal = vec3<f32>(0.0);
    var validity = 0.0;
    for (var corner = 0u; corner < 4u; corner += 1u) {
        let sample = surface_samples[indices[corner]];
        height += sample.height_validity.x * weights[corner];
        validity += sample.height_validity.y * weights[corner];
        normal += sample.normal.xyz * weights[corner];
    }
    if (dot(normal, normal) <= 1e-10) {
        normal = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        normal = normalize(normal);
    }
    return SurfaceResult(height, normal, validity);
}

fn sample_voronoi_group(item: WorkItem, root: vec2<f32>) -> GroupSample {
    let spacing = item.grouping.x;
    let base_cell = vec2<i32>(floor(root / spacing));
    var nearest_distance_squared = 1e30;
    var second_distance_squared = 1e30;
    var nearest_center = root;
    var nearest_key = item.population.z;
    for (var dz = -1; dz <= 1; dz += 1) {
        for (var dx = -1; dx <= 1; dx += 1) {
            let cell = base_cell + vec2<i32>(dx, dz);
            let key = hash_cell(item.population.z, cell, 0x4f1bcdc9u);
            let offset = vec2<f32>(
                0.5 + (random01(key ^ 0x9e3779b9u) - 0.5) * item.grouping.y,
                0.5 + (random01(key ^ 0x85ebca6bu) - 0.5) * item.grouping.y,
            );
            let center = (vec2<f32>(cell) + offset) * spacing;
            let delta = root - center;
            let distance_squared = dot(delta, delta);
            if (distance_squared < nearest_distance_squared) {
                second_distance_squared = nearest_distance_squared;
                nearest_distance_squared = distance_squared;
                nearest_center = center;
                nearest_key = key;
            } else if (distance_squared < second_distance_squared) {
                second_distance_squared = distance_squared;
            }
        }
    }

    let nearest_distance = sqrt(nearest_distance_squared);
    let second_distance = sqrt(second_distance_squared);
    let softness_width = spacing * item.grouping.z;
    var boundary_influence = 1.0;
    if (softness_width > 1e-7) {
        boundary_influence = smoothstep(
            0.0,
            softness_width,
            second_distance - nearest_distance,
        );
    }
    let normalized_distance = clamp(nearest_distance / (spacing * 1.41421356237), 0.0, 1.0);
    let distance_profile = pow(normalized_distance, item.group_density.z);
    let spatial_retention = mix(item.group_density.x, item.group_density.y, distance_profile);
    let group_retention = 1.0
        - item.group_density.w * random01(nearest_key ^ 0xd1b54a35u);
    return GroupSample(
        nearest_center,
        normalize_or(root - nearest_center, vec2<f32>(1.0, 0.0)),
        normalized_distance,
        boundary_influence,
        clamp(spatial_retention * group_retention, 0.0, 1.0),
        nearest_key,
    );
}

fn sample_candidate(item: WorkItem, candidate_index: u32) -> Candidate {
    let cell_index = candidate_index / item.candidate_layout.x;
    let child_index = candidate_index % item.candidate_layout.x;
    let cell_count_x = u32(item.domain.z);
    let cell = vec2<i32>(
        item.domain.x + i32(cell_index % cell_count_x),
        item.domain.y + i32(cell_index / cell_count_x),
    );
    let cell_seed = hash_cell(item.population.z, cell, 0x6d2b79f5u);

    var root: vec2<f32>;
    var parent: vec2<f32>;
    if (item.peers.z == 0u) {
        let offset = vec2<f32>(
            0.5 + (random01(cell_seed ^ 0xa511e9b3u) - 0.5) * item.growth.z,
            0.5 + (random01(cell_seed ^ 0x63d83595u) - 0.5) * item.growth.z,
        );
        root = (vec2<f32>(cell) + offset) * item.growth.x;
        parent = root;
    } else {
        let parent_offset = vec2<f32>(
            0.5 + (random01(cell_seed ^ 0xa511e9b3u) - 0.5) * item.growth.z,
            0.5 + (random01(cell_seed ^ 0x63d83595u) - 0.5) * item.growth.z,
        );
        parent = (vec2<f32>(cell) + parent_offset) * item.growth.x;
        let child_seed = hash32(cell_seed ^ child_index * 0x9e3779b9u);
        let angle = random01(child_seed ^ 0xc2b2ae35u) * PI_2;
        let distance = sqrt(random01(child_seed ^ 0x27d4eb2fu)) * item.growth.y;
        let placement_radial = vec2<f32>(cos(angle), sin(angle));
        root = parent + placement_radial * distance;
    }

    let seed = hash32(cell_seed ^ child_index * 0x85ebca6bu);
    var group = GroupSample(
        root,
        vec2<f32>(0.0),
        0.0,
        0.0,
        1.0,
        seed,
    );
    if (item.peers.w == 1u) {
        let delta = root - parent;
        let distance = length(delta);
        var normalized_distance = 0.0;
        if (item.growth.y > 1e-7) {
            normalized_distance = clamp(distance / item.growth.y, 0.0, 1.0);
        }
        group = GroupSample(
            parent,
            normalize_or(delta, vec2<f32>(1.0, 0.0)),
            normalized_distance,
            1.0,
            1.0,
            cell_seed,
        );
    } else if (item.peers.w == 2u) {
        group = sample_voronoi_group(item, root);
        root = mix(root, group.center, item.grouping.w * group.boundary_influence);
    }
    var lod_lane = 0u;
    if (item.peers.z == 0u) {
        let block = vec2<i32>(floor_div_two(cell.x), floor_div_two(cell.y));
        let quadrant = u32(cell.x - block.x * 2) + 2u * u32(cell.y - block.y * 2);
        let rotation = hash_cell(item.population.z, block, 0xa24baed5u) & 3u;
        lod_lane = (quadrant + rotation) & 3u;
    } else {
        let group = child_index / 4u;
        let rotation = hash_cell(
            item.population.z,
            cell,
            0xa24baed5u ^ group * 0x9e3779b9u,
        ) & 3u;
        lod_lane = (child_index % 4u + rotation) & 3u;
    }
    let random_angle = random01(seed ^ 0x165667b1u) * PI_2;
    let random_direction = vec2<f32>(cos(random_angle), sin(random_angle));
    let shared_angle = random01(group.key ^ 0x68e31da4u) * PI_2;
    let shared_direction = vec2<f32>(cos(shared_angle), sin(shared_angle));
    let tangent = vec2<f32>(-group.radial.y, group.radial.x);
    let flow = normalize_or(item.flow_density.xy, vec2<f32>(1.0, 0.0));
    let mixed = shared_direction * item.orientation.x * group.boundary_influence
        + group.radial * item.direction_weights.x * group.boundary_influence
        + tangent * item.direction_weights.y
            * group.boundary_influence
        + random_direction * item.direction_weights.z
        + flow * item.direction_weights.w;
    let base_direction = normalize_or(mixed, random_direction);
    let angular_jitter = (random01(seed ^ 0x7f4a7c15u) * 2.0 - 1.0) * item.orientation.y;
    let sine = sin(angular_jitter);
    let cosine = cos(angular_jitter);
    let direction = vec2<f32>(
        base_direction.x * cosine - base_direction.y * sine,
        base_direction.x * sine + base_direction.y * cosine,
    );
    return Candidate(
        root,
        group.center,
        direction,
        random01(seed ^ 0x94d049bbu),
        (f32(lod_lane) + random01(seed ^ 0x91e10da5u)) * 0.25,
        random01(group.key ^ 0x3c6ef372u),
        group.normalized_distance,
        group.boundary_influence,
        group.density_retention,
        seed,
    );
}

fn local_occupancy(item: WorkItem, root: vec2<f32>) -> f32 {
    let own_coverage = sample_coverage(item, root);
    if (item.population.w == 0u) {
        return own_coverage;
    }

    var total_requested_density = 0.0;
    var strongest_requested_density = 0.0;
    for (var peer_offset = 0u; peer_offset < item.peers.y; peer_offset += 1u) {
        let peer = work_items[item.peers.x + peer_offset];
        if (peer.population.w == item.population.w) {
            let requested_density = peer.flow_density.z * sample_coverage(peer, root);
            total_requested_density += requested_density;
            strongest_requested_density = max(strongest_requested_density, requested_density);
        }
    }
    if (total_requested_density <= 1e-7) {
        return 0.0;
    }
    return clamp(strongest_requested_density * own_coverage / total_requested_density, 0.0, 1.0);
}

fn choose_species_choice(item: WorkItem, random_value: f32) -> SpeciesChoice {
    for (var choice_offset = 0u; choice_offset < item.population.y; choice_offset += 1u) {
        let choice = choices[item.population.x + choice_offset];
        if (random_value <= choice.threshold.x) {
            return choice;
        }
    }
    return choices[item.population.x + item.population.y - 1u];
}

fn candidate_is_visible(
    candidate: Candidate,
    surface: SurfaceResult,
    choice: SpeciesChoice,
) -> bool {
    let maximum_reach = bitcast<f32>(choice.metadata.z)
        + bitcast<f32>(choice.metadata.w) * camera.wind.z * 1.65;
    let maximum_height = bitcast<f32>(choice.metadata.w);
    let camera_delta = candidate.root - camera.camera_position.xz;
    let distance = length(camera_delta);
    if (distance > MAX_PROCEDURAL_DISTANCE + maximum_reach) {
        return false;
    }

    let center = vec3<f32>(
        candidate.root.x,
        surface.height + maximum_height * 0.45,
        candidate.root.y,
    );
    let clip = camera.clip_from_world * vec4<f32>(center, 1.0);
    if (clip.w <= 1e-5) {
        return false;
    }
    let ndc = clip.xy / clip.w;
    let extent = max(maximum_reach, maximum_height * 0.55);
    let margin = min(0.4, 0.035 + extent / max(distance, 1.0) * 1.8);
    return abs(ndc.x) <= 1.0 + margin && abs(ndc.y) <= 1.0 + margin;
}

fn projected_distance_pixels(origin: vec3<f32>, endpoint: vec3<f32>) -> f32 {
    let origin_clip = camera.clip_from_world * vec4<f32>(origin, 1.0);
    let endpoint_clip = camera.clip_from_world * vec4<f32>(endpoint, 1.0);
    if (origin_clip.w <= 1e-5 || endpoint_clip.w <= 1e-5) {
        // Near-plane intersections are not safe to reduce or cull from projected size.
        return 1e30;
    }
    return length(
        endpoint_clip.xy / endpoint_clip.w - origin_clip.xy / origin_clip.w,
    ) * camera.camera_position.w * 0.5;
}

fn projected_blade_extent_pixels(
    candidate: Candidate,
    surface: SurfaceResult,
    choice: SpeciesChoice,
) -> f32 {
    let maximum_height = bitcast<f32>(choice.metadata.w);
    let maximum_reach = bitcast<f32>(choice.metadata.z)
        + maximum_height * camera.wind.z * 1.65;
    let root = vec3<f32>(candidate.root.x, surface.height, candidate.root.y);
    let tip = root + surface.normal * maximum_height;
    var maximum_pixels = projected_distance_pixels(root, tip);
    // A blade seen from overhead can project its surface-normal height to nearly zero while its
    // authored bend/tilt still covers a large part of the screen. Bound both horizontal axes so
    // classification is conservative regardless of the candidate's random facing.
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root + vec3<f32>(maximum_reach, 0.0, 0.0)),
    );
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root - vec3<f32>(maximum_reach, 0.0, 0.0)),
    );
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root + vec3<f32>(0.0, 0.0, maximum_reach)),
    );
    maximum_pixels = max(
        maximum_pixels,
        projected_distance_pixels(root, root - vec3<f32>(0.0, 0.0, maximum_reach)),
    );
    return maximum_pixels;
}

fn blade_extent_limits_pixels(
    candidate: Candidate,
    surface: SurfaceResult,
    choice: SpeciesChoice,
    high_radius: f32,
) -> vec2<f32> {
    let projected_extent = projected_blade_extent_pixels(candidate, surface, choice);
    let high_threshold = choice.threshold.y;
    let bounded_high_radius = max(high_radius * HIGH_TOPOLOGY_DISTANCE_SCALE, 1e-3);
    // Stable per-root staggering softens the topology transition. The focus ellipse
    // preserves the disk area used by the CPU budget (0.5 * 1.3² = 84.5% maximum).
    let staggered_high_radius = bounded_high_radius * mix(
        1.10, 1.30, random01(candidate.seed ^ 0x6a09e667u),
    );
    let distance = detail_distance(candidate.root);
    let high_weight = 1.0 - smoothstep(
        staggered_high_radius * 0.90,
        staggered_high_radius,
        distance,
    );
    // High topology is admitted through a stable world-space footprint sized from the authored root
    // density and the device-profile bin capacity. The annulus uses the existing high-to-low
    // geometry morph. It never relies on atomic append order to decide which roots survive.
    let budget_extent = high_threshold * mix(0.999, 1.45, high_weight);
    return vec2(projected_extent, budget_extent);
}

fn generated_candidate_height(choice: SpeciesChoice, candidate: Candidate) -> f32 {
    let group_key = u32(round(candidate.clump_variant * 65535.0));
    // Procedural instances retain the low 24 seed bits; classify from that exact persisted value so
    // the draw shader reconstructs the same height and therefore the same topology class.
    let unit_seed = hash32(candidate.seed & 0x00ffffffu);
    let source_coordinate = mix(
        random01(unit_seed ^ 0xa511e9b3u),
        random01(group_key ^ 0x52dce729u),
        choice.height.w,
    );
    let height_exponent = exp2(-2.0 * choice.height.z);
    let height_coordinate = pow(clamp(source_coordinate, 0.0, 1.0), height_exponent);
    return mix(choice.height.x, choice.height.y, height_coordinate);
}

fn candidate_topology_class(
    choice: SpeciesChoice,
    generated_height: f32,
) -> u32 {
    if (choice.packing.x > 0.0) {
        return select(0u, 1u, generated_height <= choice.packing.x);
    }
    return min(choice.metadata.y, 1u);
}

fn projected_population_spacing_pixels(
    item: WorkItem,
    candidate: Candidate,
    surface: SurfaceResult,
) -> f32 {
    let spacing = inverseSqrt(max(item.flow_density.z, 1e-6));
    let root = vec3<f32>(candidate.root.x, surface.height, candidate.root.y);
    let to_camera = camera.camera_position.xyz - root;
    let distance = max(length(to_camera), 1e-3);
    let foreshortening = sqrt(clamp(abs(dot(surface.normal, to_camera / distance)), 0.0, 1.0));
    // This is the linearized projected area of one density cell. Unlike blade height it remains
    // large when the camera looks down on visibly separated roots.
    return spacing * camera.projection.x * foreshortening / distance;
}

fn population_lod_density(choice: SpeciesChoice, projected_spacing: f32) -> f32 {
    // Geometry complexity follows the blade envelope, while population follows projected root-cell
    // spacing. Stable rank turns this continuous target into a nested spatial fade.
    let low_boundary = choice.threshold.z;
    let far_boundary = choice.threshold.w;
    let low_density = choice.density.y;
    let far_density = choice.density.z;
    let full_boundary = max(low_boundary * 2.0, low_boundary + 1e-4);
    if (projected_spacing >= full_boundary) {
        return 1.0;
    }
    if (projected_spacing >= low_boundary) {
        return mix(
            low_density,
            1.0,
            smoothstep(low_boundary, full_boundary, projected_spacing),
        );
    }
    if (projected_spacing <= far_boundary) {
        return far_density;
    }
    return mix(
        far_density,
        low_density,
        smoothstep(far_boundary, low_boundary, projected_spacing),
    );
}

fn balanced_population_lod_density(projected_spacing: f32) -> f32 {
    // Preserve separately visible roots, then spend density only as their projected cells become
    // difficult to resolve. The final 30% plateau is an interim ribbon-only horizon policy; a
    // future coverage representation can eventually replace it below this range.
    if (projected_spacing >= BALANCED_DENSITY_FULL_SPACING_PIXELS) {
        return 1.0;
    }
    if (projected_spacing >= BALANCED_DENSITY_MIDDLE_SPACING_PIXELS) {
        return mix(
            BALANCED_DENSITY_MIDDLE_FRACTION,
            1.0,
            smoothstep(
                BALANCED_DENSITY_MIDDLE_SPACING_PIXELS,
                BALANCED_DENSITY_FULL_SPACING_PIXELS,
                projected_spacing,
            ),
        );
    }
    if (projected_spacing <= BALANCED_DENSITY_FAR_SPACING_PIXELS) {
        return BALANCED_DENSITY_FAR_FRACTION;
    }
    return mix(
        BALANCED_DENSITY_FAR_FRACTION,
        BALANCED_DENSITY_MIDDLE_FRACTION,
        smoothstep(
            BALANCED_DENSITY_FAR_SPACING_PIXELS,
            BALANCED_DENSITY_MIDDLE_SPACING_PIXELS,
            projected_spacing,
        ),
    );
}

fn mobile_population_lod_density() -> f32 {
    return MOBILE_DIAGNOSTIC_DENSITY_FRACTION;
}

fn population_lod_retention_limit(population_density: f32, topology_class: u32) -> f32 {
    // Balanced mode keeps a narrow, stable rank band alive so the draw shader can contract retiring
    // blades laterally. Centering that band on the target approximately preserves integrated width.
    if ((debug_config.values.z & MOBILE_POPULATION_LOD_BIT) != 0u) {
        return population_density;
    }
    if (
        debug_config.values.y == DENSITY_MODE_BALANCED
        && population_density < 0.999
    ) {
        let budget_scale = 1.0;
        return min(population_density + BALANCED_DENSITY_FADE_BAND * budget_scale * 0.5, 1.0);
    }
    return population_density;
}

fn evaluate_candidate(item: WorkItem, candidate_index: u32, early_rejection: bool) -> CandidateEvaluation {
    // Rejected production candidates never reach emission. Avoid height, projection, and LOD
    // calculations for them; retain the full evaluation for visual diagnostic modes.
    var rejected: CandidateEvaluation;
    let reject_early = early_rejection && debug_config.values.x == 0u;
    let candidate = sample_candidate(item, candidate_index);
    if (reject_early && (!owns(item, candidate.root) || candidate.stable_rank >= item.growth.w)) {
        return rejected;
    }
    let surface = sample_surface(item, candidate.root);
    if (reject_early && surface.validity < 0.5) {
        return rejected;
    }
    let occupancy = local_occupancy(item, candidate.root) * candidate.group_density;
    var outcome = 0u;
    if (candidate.stable_rank >= item.growth.w) {
        outcome = 1u;
    } else if (surface.validity < 0.5) {
        outcome = 2u;
    } else if (random01(candidate.seed ^ 0x4cf5ad43u) >= occupancy) {
        outcome = 3u;
    }
    if (reject_early && outcome != 0u) {
        return rejected;
    }

    let choice = choose_species_choice(item, random01(candidate.seed ^ 0xd1b54a35u));
    if (reject_early && !candidate_is_visible(candidate, surface, choice)) {
        return rejected;
    }
    let generated_height = generated_candidate_height(choice, candidate);
    let topology_class = candidate_topology_class(choice, generated_height);
    let high_radius = select(choice.packing.y, choice.packing.z, topology_class != 0u);
    let extent_limits = blade_extent_limits_pixels(
        candidate,
        surface,
        choice,
        high_radius,
    );
    let projected_extent = min(extent_limits.x, extent_limits.y);
    let projected_spacing = projected_population_spacing_pixels(item, candidate, surface);
    let authored_population_density = population_lod_density(choice, projected_spacing);
    var population_density = authored_population_density;
    if (debug_config.values.y == DENSITY_MODE_BALANCED) {
        population_density = select(
            balanced_population_lod_density(projected_spacing),
            mobile_population_lod_density(),
            (debug_config.values.z & MOBILE_POPULATION_LOD_BIT) != 0u,
        );
    } else if (debug_config.values.y == DENSITY_MODE_FULL_REFERENCE) {
        population_density = 1.0;
    }
    let near_coverage = near_field_coverage(candidate.root);
    if (debug_config.values.y == DENSITY_MODE_BALANCED) {
        population_density = max(population_density, near_coverage);
    }
    // Remove the paired-root reduction gradually inside the gameplay focus region.
    // Far grass still uses the existing low-density, three-triangle representation.
    if (topology_class != 0u && debug_config.values.y != DENSITY_MODE_FULL_REFERENCE) {
        let near_scale = select(0.0, near_coverage, debug_config.values.y == DENSITY_MODE_BALANCED);
        population_density *= mix(SPLIT_LOW_DENSITY_BUDGET_SCALE, 1.0, near_scale);
    }
    var lod = 0u;
    if (
        projected_extent < choice.threshold.y
        || (debug_config.values.z & FORCE_LOW_TOPOLOGY_BIT) != 0u
    ) {
        lod = 1u;
    }
    let lod_morph = select(
        smoothstep(choice.threshold.y, choice.threshold.y * 1.45, projected_extent),
        0.0,
        lod != 0u,
    );
    let bin = select(0u, topology_class * 2u + lod, debug_config.values.x == 0u);
    var eligible = 1u;
    if (!owns(item, candidate.root)) {
        eligible = 0u;
    } else if (!candidate_is_visible(candidate, surface, choice)) {
        eligible = 0u;
    } else if (
        debug_config.values.x == 0u
        && (projected_extent < choice.threshold.w
            || (lod == 1u
                && candidate.lod_rank >= population_lod_retention_limit(population_density, topology_class)))
    ) {
        eligible = 0u;
    } else if (outcome != 0u && debug_config.values.x != 3u) {
        eligible = 0u;
    } else if (debug_config.values.x == 2u && item.peers.w != 1u) {
        eligible = 0u;
    } else if (debug_config.values.x == 4u && item.peers.w == 0u) {
        eligible = 0u;
    }
    var inspection_extents = vec2<f32>(0.0);
    if (debug_config.values.x == 0u && ((debug_config.workload.w >> 4u) & 15u) != 0u) {
        inspection_extents = extent_limits / max(choice.threshold.y, 1e-5);
    }
    return CandidateEvaluation(
        candidate,
        surface,
        occupancy,
        outcome,
        choice.metadata.x,
        bin,
        lod_morph,
        population_density,
        eligible,
        inspection_extents,
    );
}

fn active_capacity(bin: u32) -> u32 {
    if (debug_config.values.x != 0u) {
        return select(0u, MAX_DIAGNOSTIC_INSTANCES, bin == 0u);
    }
    switch bin {
        case 0u: { return SINGLE_HIGH_CAPACITY; }
        case 1u: {
            return clamp(
                debug_config.values.w,
                LOW_DETAIL_MINIMUM_PARTITION,
                LOW_DETAIL_CAPACITY - LOW_DETAIL_MINIMUM_PARTITION,
            );
        }
        case 2u: { return SPLIT_HIGH_CAPACITY; }
        default: {
            return LOW_DETAIL_CAPACITY - clamp(
                debug_config.values.w,
                LOW_DETAIL_MINIMUM_PARTITION,
                LOW_DETAIL_CAPACITY - LOW_DETAIL_MINIMUM_PARTITION,
            );
        }
    }
}

fn instance_offset(bin: u32) -> u32 {
    switch bin {
        case 0u: { return 0u; }
        case 1u: { return LOW_DETAIL_OFFSET; }
        case 2u: { return SPLIT_HIGH_OFFSET; }
        default: {
            return LOW_DETAIL_OFFSET + active_capacity(1u);
        }
    }
}

@compute @workgroup_size(64, 1, 1)
fn generate(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let encoded_work_item = visible_work_items[invocation.y];
    let quarter_lod = (encoded_work_item & QUARTER_LOD_FLAG) != 0u;
    let work_item_index = encoded_work_item & WORK_ITEM_INDEX_MASK;
    let item = work_items[work_item_index];
    let cached = use_candidate_cache(work_item_index, quarter_lod);
    var candidate_count = item.candidate_layout.y;
    if (quarter_lod) {
        candidate_count = quarter_candidate_count(item);
    }
    if (invocation.x >= candidate_count) { return; }
    var candidate_index = invocation.x;
    if (quarter_lod) {
        candidate_index = remap_quarter_candidate(item, invocation.x);
    }
    if (candidate_index == 0xffffffffu) {
        return;
    }
    if (cached) {
        let mask = candidate_acceptance_bits[candidate_cache_entries[work_item_index].base + candidate_index / 32u];
        if ((mask & (1u << (candidate_index & 31u))) == 0u) { return; }
    }
    if (debug_config.workload.y != 0u) {
        atomicAdd(&telemetry.values[1], 1u);
    }
    let evaluation = evaluate_candidate(item, candidate_index, (debug_config.workload.w & 1u) != 0u);
    if (evaluation.eligible == 0u) {
        return;
    }
    // Promote topology only after production culling/retention. All inspection modes retain the
    // exact same root identities and original packed morph/density. Limited high arenas still apply.
    let inspection = debug_config.values.x == 0u && ((debug_config.workload.w >> 4u) & 15u) != 0u;
    let draw_bin = select(evaluation.bin, evaluation.bin & ~1u, inspection);
    if (debug_config.workload.y != 0u) {
        atomicAdd(&telemetry.values[2u + draw_bin], 1u);
    }

    let local_slot = atomicAdd(&draw_args[draw_bin].instance_count, 1u);
    if (local_slot >= active_capacity(draw_bin)) {
        if (debug_config.workload.y != 0u) {
            atomicAdd(&telemetry.values[6u + draw_bin], 1u);
        }
        return;
    }
    if (debug_config.values.x == 0u) {
        let slot = instance_offset(draw_bin) + local_slot;
        procedural_instances[slot].root_clump = vec4<f32>(
            evaluation.candidate.root.x,
            evaluation.surface.height,
            evaluation.candidate.root.y,
            bitcast<f32>(pack2x16unorm(vec2<f32>(
                evaluation.candidate.clump_variant,
                evaluation.candidate.lod_rank,
            ))),
        );
        if (inspection) {
            // High bins together fit the existing 65,536-record diagnostic buffer.
            // No extra allocation or production writes; retain the full seed for native inspection.
            diagnostic_instances[slot].root_direction = procedural_instances[slot].root_clump;
            diagnostic_instances[slot].direction_species = vec4<f32>(bitcast<f32>(evaluation.candidate.seed), f32(evaluation.species_index), f32(evaluation.bin), evaluation.lod_morph);
            diagnostic_instances[slot].diagnostics = vec4<f32>(evaluation.shape_extents, evaluation.population_density, evaluation.candidate.lod_rank);
        }
        procedural_instances[slot].geometry = vec4<u32>(
            pack2x16snorm(evaluation.candidate.direction),
            (evaluation.species_index & SPECIES_INDEX_MASK)
                | (u32(round(clamp(evaluation.lod_morph, 0.0, 1.0) * f32(LOD_MORPH_MASK)))
                    << LOD_MORPH_SHIFT)
                | ((evaluation.bin & 1u) << 31u),
            pack2x16snorm(evaluation.surface.normal.xz),
            (evaluation.candidate.seed & 0x00ffffffu)
                | (u32(round(clamp(evaluation.population_density, 0.0, 1.0) * 255.0)) << 24u),
        );
        return;
    }

    let parent_surface = sample_surface(item, evaluation.candidate.group_center);
    let parent_height = select(
        evaluation.surface.height,
        parent_surface.height,
        parent_surface.validity >= 0.5,
    );
    diagnostic_instances[local_slot].root_direction = vec4<f32>(
        evaluation.candidate.root.x,
        evaluation.surface.height,
        evaluation.candidate.root.y,
        evaluation.candidate.direction.x,
    );
    diagnostic_instances[local_slot].direction_species = vec4<f32>(
        evaluation.candidate.direction.y,
        evaluation.candidate.clump_variant,
        f32(evaluation.species_index),
        bitcast<f32>(pack2x16snorm(evaluation.surface.normal.xz)),
    );
    diagnostic_instances[local_slot].parent_status = vec4<f32>(
        evaluation.candidate.group_center.x,
        parent_height,
        evaluation.candidate.group_center.y,
        f32(evaluation.outcome),
    );
    diagnostic_instances[local_slot].diagnostics = vec4<f32>(
        evaluation.occupancy,
        evaluation.candidate.stable_rank,
        evaluation.candidate.group_distance,
        evaluation.candidate.group_influence,
    );
}

@compute @workgroup_size(1, 1, 1)
fn finalize() {
    let diagnostic_mode = debug_config.values.x != 0u;
    for (var bin = 0u; bin < 4u; bin += 1u) {
        atomicStore(
            &draw_args[bin].instance_count,
            min(atomicLoad(&draw_args[bin].instance_count), active_capacity(bin)),
        );
        atomicStore(&draw_args[bin].base_vertex, 0u);
    }
    atomicStore(
        &draw_args[0].index_count,
        select(SINGLE_HIGH_INDEX_COUNT, DIAGNOSTIC_INDEX_COUNT, diagnostic_mode),
    );
    atomicStore(
        &draw_args[0].first_index,
        select(SINGLE_HIGH_FIRST_INDEX, DIAGNOSTIC_FIRST_INDEX, diagnostic_mode),
    );
    atomicStore(&draw_args[0].first_instance, 0u);

    atomicStore(&draw_args[1].index_count, SINGLE_LOW_INDEX_COUNT);
    atomicStore(&draw_args[1].first_index, SINGLE_LOW_FIRST_INDEX);
    atomicStore(
        &draw_args[1].first_instance,
        LOW_DETAIL_OFFSET,
    );

    atomicStore(&draw_args[2].index_count, SPLIT_HIGH_INDEX_COUNT);
    atomicStore(&draw_args[2].first_index, SPLIT_HIGH_FIRST_INDEX);
    atomicStore(
        &draw_args[2].first_instance,
        SPLIT_HIGH_OFFSET,
    );

    atomicStore(&draw_args[3].index_count, SPLIT_LOW_INDEX_COUNT);
    atomicStore(&draw_args[3].first_index, SPLIT_LOW_FIRST_INDEX);
    atomicStore(
        &draw_args[3].first_instance,
        LOW_DETAIL_OFFSET + active_capacity(1u),
    );
}
