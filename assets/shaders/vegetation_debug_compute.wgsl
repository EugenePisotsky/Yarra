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
}

struct ProceduralInstance {
    // xyz: root, w: clump variant
    root_clump: vec4<f32>,
    // x: packed rest direction, y: species index, z: packed surface normal xz,
    // w: low 24 bits seed + high 8 bits population-density target
    geometry: vec4<u32>,
}

struct DebugInstance {
    root_direction: vec4<f32>,
    direction_species: vec4<f32>,
    parent_status: vec4<f32>,
    diagnostics: vec4<f32>,
}

struct DebugConfig {
    // x: 0 geometry, 1 accepted species, 2 parent links, 3 outcomes, 4 group structure
    // y: 0 authored density, 1 balanced production density, 2 full-density reference
    // z: 0 rounded/clump gloss, 1 legacy empirical lighting
    values: vec4<u32>,
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
    population_density: f32,
    eligible: u32,
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

const PI_2: f32 = 6.283185307179586;
const SINGLE_HIGH_CAPACITY: u32 = 16384u;
const SINGLE_LOW_CAPACITY: u32 = 32768u;
const SPLIT_HIGH_CAPACITY: u32 = 32768u;
const SPLIT_LOW_CAPACITY: u32 = 262144u;
const MAX_DIAGNOSTIC_INSTANCES: u32 = 65536u;
const MAX_PROCEDURAL_DISTANCE: f32 = 96.0;
const QUARTER_LOD_FLAG: u32 = 0x80000000u;
const WORK_ITEM_INDEX_MASK: u32 = 0x7fffffffu;
const DIAGNOSTIC_INDEX_COUNT: u32 = 6u;
const SINGLE_HIGH_INDEX_COUNT: u32 = 48u;
const SINGLE_LOW_INDEX_COUNT: u32 = 18u;
const SPLIT_HIGH_INDEX_COUNT: u32 = 42u;
const SPLIT_LOW_INDEX_COUNT: u32 = 6u;
const DIAGNOSTIC_FIRST_INDEX: u32 = 0u;
const SINGLE_HIGH_FIRST_INDEX: u32 = 6u;
const SINGLE_LOW_FIRST_INDEX: u32 = 54u;
const SPLIT_HIGH_FIRST_INDEX: u32 = 72u;
const SPLIT_LOW_FIRST_INDEX: u32 = 114u;
const DENSITY_MODE_BALANCED: u32 = 1u;
const DENSITY_MODE_FULL_REFERENCE: u32 = 2u;
const BALANCED_DENSITY_FULL_SPACING_PIXELS: f32 = 6.0;
const BALANCED_DENSITY_MIDDLE_SPACING_PIXELS: f32 = 2.0;
const BALANCED_DENSITY_FAR_SPACING_PIXELS: f32 = 0.75;
const BALANCED_DENSITY_MIDDLE_FRACTION: f32 = 0.55;
const BALANCED_DENSITY_FAR_FRACTION: f32 = 0.30;
const BALANCED_DENSITY_FADE_BAND: f32 = 0.10;

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
    let weights = array<f32, 4>(
        (1.0 - blend.x) * (1.0 - blend.y),
        blend.x * (1.0 - blend.y),
        (1.0 - blend.x) * blend.y,
        blend.x * blend.y,
    );
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
    let maximum_reach = bitcast<f32>(choice.metadata.z);
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
    let maximum_reach = bitcast<f32>(choice.metadata.z);
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

fn budgeted_projected_blade_extent_pixels(
    candidate: Candidate,
    surface: SurfaceResult,
    choice: SpeciesChoice,
) -> f32 {
    let projected_extent = projected_blade_extent_pixels(candidate, surface, choice);
    let high_threshold = choice.threshold.y;
    let high_radius = max(choice.density.w, 1e-3);
    // A single camera-centred radius made the whole field cross the topology boundary as a ring.
    // Reuse the stable nested LOD rank to spread that boundary without increasing its expected
    // area (E[r^2] remains below the authored budget radius). Classification and draw
    // reconstruction use this identical radius, so the transition remains deterministic.
    let staggered_high_radius = high_radius * mix(0.84, 1.12, candidate.lod_rank);
    let distance = length(candidate.root - camera.camera_position.xz);
    let high_weight = 1.0 - smoothstep(
        staggered_high_radius * 0.68,
        staggered_high_radius,
        distance,
    );
    // High topology is admitted through a stable world-space disk sized from the authored root
    // density and the device-profile bin capacity. The annulus uses the existing high-to-low
    // geometry morph. It never relies on atomic append order to decide which roots survive.
    let budget_extent = high_threshold * mix(0.999, 1.45, high_weight);
    return min(projected_extent, budget_extent);
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

fn population_lod_retention_limit(population_density: f32) -> f32 {
    // Balanced mode keeps a narrow, stable rank band alive so the draw shader can contract retiring
    // blades laterally. Centering that band on the target approximately preserves integrated width.
    if (
        debug_config.values.y == DENSITY_MODE_BALANCED
        && population_density < 0.999
    ) {
        return min(population_density + BALANCED_DENSITY_FADE_BAND * 0.5, 1.0);
    }
    return population_density;
}

fn evaluate_candidate(item: WorkItem, candidate_index: u32) -> CandidateEvaluation {
    let candidate = sample_candidate(item, candidate_index);
    let surface = sample_surface(item, candidate.root);
    let occupancy = local_occupancy(item, candidate.root) * candidate.group_density;
    var outcome = 0u;
    if (candidate.stable_rank >= item.growth.w) {
        outcome = 1u;
    } else if (surface.validity < 0.5) {
        outcome = 2u;
    } else if (random01(candidate.seed ^ 0x4cf5ad43u) >= occupancy) {
        outcome = 3u;
    }

    let choice = choose_species_choice(item, random01(candidate.seed ^ 0xd1b54a35u));
    let projected_extent = budgeted_projected_blade_extent_pixels(candidate, surface, choice);
    let projected_spacing = projected_population_spacing_pixels(item, candidate, surface);
    let authored_population_density = population_lod_density(choice, projected_spacing);
    var population_density = authored_population_density;
    if (debug_config.values.y == DENSITY_MODE_BALANCED) {
        population_density = balanced_population_lod_density(projected_spacing);
    } else if (debug_config.values.y == DENSITY_MODE_FULL_REFERENCE) {
        population_density = 1.0;
    }
    var lod = 0u;
    if (projected_extent < choice.threshold.y) {
        lod = 1u;
    }
    let bin = select(0u, min(choice.metadata.y, 1u) * 2u + lod, debug_config.values.x == 0u);
    var eligible = 1u;
    if (!owns(item, candidate.root)) {
        eligible = 0u;
    } else if (!candidate_is_visible(candidate, surface, choice)) {
        eligible = 0u;
    } else if (
        debug_config.values.x == 0u
        && (projected_extent < choice.threshold.w
            || (lod == 1u
                && candidate.lod_rank >= population_lod_retention_limit(population_density)))
    ) {
        eligible = 0u;
    } else if (outcome != 0u && debug_config.values.x != 3u) {
        eligible = 0u;
    } else if (debug_config.values.x == 2u && item.peers.w != 1u) {
        eligible = 0u;
    } else if (debug_config.values.x == 4u && item.peers.w == 0u) {
        eligible = 0u;
    }
    return CandidateEvaluation(
        candidate,
        surface,
        occupancy,
        outcome,
        choice.metadata.x,
        bin,
        population_density,
        eligible,
    );
}

fn active_capacity(bin: u32) -> u32 {
    if (debug_config.values.x != 0u) {
        return select(0u, MAX_DIAGNOSTIC_INSTANCES, bin == 0u);
    }
    switch bin {
        case 0u: { return SINGLE_HIGH_CAPACITY; }
        case 1u: { return SINGLE_LOW_CAPACITY; }
        case 2u: { return SPLIT_HIGH_CAPACITY; }
        default: { return SPLIT_LOW_CAPACITY; }
    }
}

fn instance_offset(bin: u32) -> u32 {
    switch bin {
        case 0u: { return 0u; }
        case 1u: { return SINGLE_HIGH_CAPACITY; }
        case 2u: { return SINGLE_HIGH_CAPACITY + SINGLE_LOW_CAPACITY; }
        default: {
            return SINGLE_HIGH_CAPACITY + SINGLE_LOW_CAPACITY + SPLIT_HIGH_CAPACITY;
        }
    }
}

@compute @workgroup_size(64, 1, 1)
fn generate(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let encoded_work_item = visible_work_items[invocation.y];
    let quarter_lod = (encoded_work_item & QUARTER_LOD_FLAG) != 0u;
    let item = work_items[encoded_work_item & WORK_ITEM_INDEX_MASK];
    let candidate_count = select(
        item.candidate_layout.y,
        quarter_candidate_count(item),
        quarter_lod,
    );
    if (invocation.x >= candidate_count) {
        return;
    }
    let candidate_index = select(
        invocation.x,
        remap_quarter_candidate(item, invocation.x),
        quarter_lod,
    );
    if (candidate_index == 0xffffffffu) {
        return;
    }
    atomicAdd(&telemetry.values[1], 1u);
    let evaluation = evaluate_candidate(item, candidate_index);
    if (evaluation.eligible == 0u) {
        return;
    }
    atomicAdd(&telemetry.values[2u + evaluation.bin], 1u);

    let local_slot = atomicAdd(&draw_args[evaluation.bin].instance_count, 1u);
    if (local_slot >= active_capacity(evaluation.bin)) {
        atomicAdd(&telemetry.values[6u + evaluation.bin], 1u);
        return;
    }
    if (debug_config.values.x == 0u) {
        let slot = instance_offset(evaluation.bin) + local_slot;
        procedural_instances[slot].root_clump = vec4<f32>(
            evaluation.candidate.root.x,
            evaluation.surface.height,
            evaluation.candidate.root.y,
            bitcast<f32>(pack2x16unorm(vec2<f32>(
                evaluation.candidate.clump_variant,
                evaluation.candidate.lod_rank,
            ))),
        );
        procedural_instances[slot].geometry = vec4<u32>(
            pack2x16snorm(evaluation.candidate.direction),
            evaluation.species_index | ((evaluation.bin & 1u) << 31u),
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
        SINGLE_HIGH_CAPACITY,
    );

    atomicStore(&draw_args[2].index_count, SPLIT_HIGH_INDEX_COUNT);
    atomicStore(&draw_args[2].first_index, SPLIT_HIGH_FIRST_INDEX);
    atomicStore(
        &draw_args[2].first_instance,
        SINGLE_HIGH_CAPACITY + SINGLE_LOW_CAPACITY,
    );

    atomicStore(&draw_args[3].index_count, SPLIT_LOW_INDEX_COUNT);
    atomicStore(&draw_args[3].first_index, SPLIT_LOW_FIRST_INDEX);
    atomicStore(
        &draw_args[3].first_instance,
        SINGLE_HIGH_CAPACITY + SINGLE_LOW_CAPACITY + SPLIT_HIGH_CAPACITY,
    );
}
