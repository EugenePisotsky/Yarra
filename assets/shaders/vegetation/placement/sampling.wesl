// Candidates on the population lattice and the page data under them: coverage, terrain surface,
// grouping, competition between overlapping populations and species choice.
import package::shaders::vegetation::types::WorkItem;
import package::shaders::vegetation::random::{hash32, hash_cell, random01};
import package::shaders::vegetation::lod::floor_div_two;
import package::shaders::vegetation::placement::bindings::{
    SpeciesChoice, work_items, choices, coverage_values, surface_samples,
};

struct SurfaceResult {
    height: f32,
    normal: vec3<f32>,
    validity: f32,
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

const PI_2: f32 = 6.283185307179586;

fn normalize_or(value: vec2<f32>, fallback: vec2<f32>) -> vec2<f32> {
    let length_squared = dot(value, value);
    if (length_squared <= 1e-10) {
        return fallback;
    }
    return value * inverseSqrt(length_squared);
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
        let child_seed = hash32(cell_seed ^ (child_index * 0x9e3779b9u));
        let angle = random01(child_seed ^ 0xc2b2ae35u) * PI_2;
        let distance = sqrt(random01(child_seed ^ 0x27d4eb2fu)) * item.growth.y;
        let placement_radial = vec2<f32>(cos(angle), sin(angle));
        root = parent + placement_radial * distance;
    }

    let seed = hash32(cell_seed ^ (child_index * 0x85ebca6bu));
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
            0xa24baed5u ^ (group * 0x9e3779b9u),
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
        root - item.orientation.zw,
        group.center - item.orientation.zw,
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
