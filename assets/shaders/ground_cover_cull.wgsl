// Resident cells provide compact clusters. This pass rejects clusters on the GPU and only
// expands surviving coverage into near, mid, and far draw lists consumed by ground_cover.wgsl.

struct Cluster {
    center_density: vec4<f32>,
    half_extents_area: vec4<f32>,
    coverage_half_extents: vec4<f32>,
    metadata: vec4<u32>,
}

struct Species {
    bottom_min_height: vec4<f32>,
    top_max_height: vec4<f32>,
    // x/y: card width range, z: flattened probability, w: maximum wind displacement
    card: vec4<f32>,
    // x: first artwork texture layer, y: variant count
    artwork: vec4<u32>,
}

struct VisibleInstance {
    // Cards: xyz root and w card yaw. Blades: xyz root and w resting facing.
    position_yaw: vec4<f32>,
    bottom_height: vec4<f32>,
    top_half_width: vec4<f32>,
    // Cards retain their legacy motion values. Blades store vertical wind displacement, clump
    // color, per-blade shape variation, and maximum wind response.
    motion: vec4<f32>,
    // Blades: xy actor interaction, zw horizontal wind displacement. Cards use xy only.
    interaction: vec4<f32>,
    // x: first artwork texture layer, y: variant count
    artwork: vec4<u32>,
}

struct Interaction {
    // x: stamp count
    metadata: vec4<u32>,
    // xy: capsule start, z: normalized recovery age (negative means live), w: radius
    centers: array<vec4<f32>, 16>,
    // xy: capsule end, z: normalized recovery age, w: maximum displacement
    ends: array<vec4<f32>, 16>,
}

struct DrawIndirectArgs {
    vertex_count: atomic<u32>,
    instance_count: atomic<u32>,
    first_vertex: atomic<u32>,
    first_instance: atomic<u32>,
}

struct RibbonCandidateCounts {
    near_instances: atomic<u32>,
    mid_instances: atomic<u32>,
    padding_0: atomic<u32>,
    padding_1: atomic<u32>,
}

struct Camera {
    clip_from_world: mat4x4<f32>,
    camera_position: vec4<f32>,
    // xyz: forward direction, w: continuous camera-pitch blend (third-person to overhead)
    view_direction: vec4<f32>,
    viewport: vec4<f32>,
    // x: near detail, y: mid detail, z: output capacity, w: geometry cutoff (pixels)
    limits: vec4<f32>,
    wind: vec4<f32>,
    wind_direction: vec4<f32>,
    // x: debug mode (0 normal, 1 LOD colors, 2 far only, 3 far disabled)
    debug: vec4<u32>,
}

@group(0) @binding(0) var<storage, read> clusters: array<Cluster>;
@group(0) @binding(1) var<storage, read> species: array<Species>;
@group(0) @binding(2) var<storage, read_write> near_instances: array<VisibleInstance>;
@group(0) @binding(3) var<storage, read_write> mid_instances: array<VisibleInstance>;
@group(0) @binding(4) var<storage, read_write> far_instances: array<VisibleInstance>;
@group(0) @binding(5) var<storage, read_write> near_args: DrawIndirectArgs;
@group(0) @binding(6) var<storage, read_write> mid_args: DrawIndirectArgs;
@group(0) @binding(7) var<storage, read_write> far_args: DrawIndirectArgs;
@group(0) @binding(8) var<uniform> camera: Camera;
@group(0) @binding(9) var<uniform> interaction: Interaction;
@group(0) @binding(10) var<storage, read_write> ribbon_candidates: RibbonCandidateCounts;

fn hash32(value: u32) -> u32 {
    var x = value;
    x = x ^ (x >> 16u);
    x = x * 0x7feb352du;
    x = x ^ (x >> 15u);
    x = x * 0x846ca68bu;
    return x ^ (x >> 16u);
}

fn random01(value: u32) -> f32 {
    return f32(hash32(value)) * (1.0 / 4294967295.0);
}

struct ClumpSample {
    facing: vec2<f32>,
    color: f32,
    phase: f32,
}

fn spatial_cell_seed(cell: vec2<i32>) -> u32 {
    return hash32(
        bitcast<u32>(cell.x) * 0x8da6b343u
            ^ bitcast<u32>(cell.y) * 0xd8163841u
            ^ 0xcb1ab31fu
    );
}

fn clump_lattice_value(cell: vec2<i32>) -> vec3<f32> {
    let seed = spatial_cell_seed(cell);
    let angle = f32(seed & 0xffffu) * (6.28318530718 / 65535.0);
    return vec3<f32>(
        cos(angle),
        sin(angle),
        f32(seed >> 16u) * (1.0 / 65535.0),
    );
}

// Clumps are a continuous metadata field, not placement groups. Interpolate unit directions,
// never numeric angles: an angle field incorrectly travels the long way around the zero-degree
// seam and produces conspicuously perfect circles. The vector field stays continuous while still
// allowing neighbouring groups to lean in genuinely different directions.
fn sample_clump_field(root: vec2<f32>) -> ClumpSample {
    let cell_size = 3.0;
    let lattice_position = root / cell_size;
    let base_cell = vec2<i32>(floor(lattice_position));
    let raw_fraction = fract(lattice_position);
    let fraction = raw_fraction * raw_fraction * (3.0 - 2.0 * raw_fraction);
    let bottom = mix(
        clump_lattice_value(base_cell),
        clump_lattice_value(base_cell + vec2<i32>(1, 0)),
        fraction.x,
    );
    let top = mix(
        clump_lattice_value(base_cell + vec2<i32>(0, 1)),
        clump_lattice_value(base_cell + vec2<i32>(1, 1)),
        fraction.x,
    );
    let field = mix(bottom, top, fraction.y);
    let facing_length_squared = dot(field.xy, field.xy);
    let facing = select(
        vec2<f32>(1.0, 0.0),
        field.xy * inverseSqrt(max(facing_length_squared, 0.000001)),
        facing_length_squared > 0.000001,
    );
    return ClumpSample(
        facing,
        field.z,
        fract((facing.x * 0.5 + 0.5) * 0.754877666 + field.z * 0.569840296),
    );
}

// Evaluate ribbon wind once per blade during expansion, not once per strip vertex. The shared
// world phases preserve broad travelling gusts while a bounded stable phase offset stops all
// blades in one group from moving as a rigid sheet. The result is a 3D tip displacement: forward
// remains dominant, with smaller signed cross-wind and vertical motion.
fn ribbon_wind_displacement(
    root: vec2<f32>,
    shape_random: f32,
    maximum_displacement: f32,
) -> vec3<f32> {
    let direction_length = max(length(camera.wind_direction.xy), 0.0001);
    let wind_direction = camera.wind_direction.xy / direction_length;
    let wind_perpendicular = vec2<f32>(-wind_direction.y, wind_direction.x);
    let primary_phase = dot(root, wind_direction) * camera.wind.w
        + camera.wind.x * camera.wind_direction.z;
    let crossing_phase = dot(root, wind_perpendicular) * camera.wind.w * 0.47
        - camera.wind.x * camera.wind_direction.z * 0.34;
    let blade_phase = (shape_random - 0.5) * 1.8;
    let group_wave = sin(primary_phase);
    let blade_wave = sin(primary_phase + blade_phase);
    let crossing_wave = sin(crossing_phase);
    let gust_envelope = smoothstep(0.18, 0.82, crossing_wave * 0.5 + 0.5);
    let gust_wave = sin(primary_phase * 0.73 + crossing_phase * 0.37 + blade_phase * 0.55);

    let coherent_wave = mix(group_wave, blade_wave, 0.42);
    let forward_drive = clamp(
        camera.wind.y * (0.58 + coherent_wave * 0.42)
            + camera.wind.z * gust_envelope * (gust_wave * 0.5 + 0.5),
        0.0,
        1.25,
    );
    let lateral_drive = (blade_wave - group_wave) * 0.34 + gust_wave * 0.12;
    let vertical_drive = blade_wave * 0.40 + gust_wave * 0.26 + crossing_wave * 0.10;
    let horizontal = wind_direction * forward_drive + wind_perpendicular * lateral_drive;
    var displacement = vec3<f32>(horizontal.x, vertical_drive, horizontal.y);
    let displacement_length = length(displacement);
    if (displacement_length > 1.0) {
        displacement /= displacement_length;
    }
    let response_variation = mix(0.82, 1.0, fract(shape_random * 1.61803398875));
    return displacement * maximum_displacement * response_variation;
}

fn density_fraction(projected_height: f32) -> f32 {
    if (projected_height < camera.limits.w) {
        return 0.0;
    }
    if (projected_height < 3.0) {
        return mix(0.10, 0.16, smoothstep(camera.limits.w, 3.0, projected_height));
    }
    if (projected_height < camera.limits.y) {
        return mix(0.16, 0.45, smoothstep(3.0, camera.limits.y, projected_height));
    }
    if (projected_height < camera.limits.x) {
        return mix(
            0.45,
            1.0,
            smoothstep(camera.limits.y, camera.limits.x, projected_height),
        );
    }
    return 1.0;
}

fn interaction_displacement(root: vec2<f32>) -> vec2<f32> {
    let stamp_count = min(interaction.metadata.x, 16u);
    var offset = vec2<f32>(0.0);
    var maximum_displacement = 0.0;
    for (var index = 0u; index < 16u; index += 1u) {
        if (index >= stamp_count) {
            break;
        }
        let center = interaction.centers[index];
        let end = interaction.ends[index];
        let capsule = end.xy - center.xy;
        let capsule_length_squared = dot(capsule, capsule);
        let capsule_fraction = clamp(
            dot(root - center.xy, capsule) / max(capsule_length_squared, 0.000001),
            0.0,
            1.0,
        );
        let closest = center.xy + capsule * capsule_fraction;
        let relative = root - closest;
        let distance_squared = dot(relative, relative);
        let radius = max(center.w, 0.01);
        let radius_squared = radius * radius;
        if (distance_squared >= radius_squared) {
            continue;
        }

        let falloff = 1.0 - smoothstep(0.04, 1.0, distance_squared / radius_squared);
        let radial = select(
            vec2<f32>(1.0, 0.0),
            relative * inverseSqrt(max(distance_squared, 0.000001)),
            distance_squared > 0.000001,
        );
        let movement = select(
            radial,
            capsule * inverseSqrt(max(capsule_length_squared, 0.000001)),
            capsule_length_squared > 0.000001,
        );
        let mixed_push = mix(radial, movement, 0.58);
        let mixed_length_squared = dot(mixed_push, mixed_push);
        let push = select(
            radial,
            mixed_push * inverseSqrt(max(mixed_length_squared, 0.000001)),
            mixed_length_squared > 0.000001,
        );
        let live = center.z < 0.0;
        let recovery_age = mix(center.z, end.z, capsule_fraction);
        let recovery = select(
            1.0 - smoothstep(0.0, 1.0, recovery_age),
            1.0,
            live,
        );
        let displacement = max(end.w, 0.0) * falloff * recovery;
        offset += push * displacement;
        maximum_displacement = max(maximum_displacement, displacement);
    }

    let offset_length_squared = dot(offset, offset);
    if (
        maximum_displacement > 0.0
        && offset_length_squared > maximum_displacement * maximum_displacement
    ) {
        offset *= maximum_displacement / sqrt(offset_length_squared);
    }
    return offset;
}

fn cluster_visible(cluster: Cluster) -> bool {
    let center = cluster.center_density.xyz;
    let extent = cluster.half_extents_area.xyz;
    var outside_left = true;
    var outside_right = true;
    var outside_bottom = true;
    var outside_top = true;
    var outside_near = true;
    var outside_far = true;

    for (var corner_index = 0u; corner_index < 8u; corner_index += 1u) {
        let signs = vec3<f32>(
            select(-1.0, 1.0, (corner_index & 1u) != 0u),
            select(-1.0, 1.0, (corner_index & 2u) != 0u),
            select(-1.0, 1.0, (corner_index & 4u) != 0u),
        );
        let clip = camera.clip_from_world * vec4<f32>(center + extent * signs, 1.0);
        outside_left = outside_left && clip.x < -clip.w;
        outside_right = outside_right && clip.x > clip.w;
        outside_bottom = outside_bottom && clip.y < -clip.w;
        outside_top = outside_top && clip.y > clip.w;
        outside_near = outside_near && clip.z > clip.w;
        outside_far = outside_far && clip.z < 0.0;
    }
    return !(outside_left || outside_right || outside_bottom || outside_top || outside_near || outside_far);
}

struct CarrierSelection {
    retained: u32,
    near: u32,
    far: u32,
}

fn carrier_root(
    cluster: Cluster,
    grid_x: u32,
    grid_z: u32,
    tuft_index: u32,
    seed: u32,
) -> vec2<f32> {
    let grid_column = tuft_index % grid_x;
    let grid_row = tuft_index / grid_x;
    let grid_u = (
        f32(grid_column) + 0.5 + (random01(seed) - 0.5) * 0.72
    ) / f32(grid_x);
    let grid_v = (
        f32(grid_row) + 0.5 + (random01(seed ^ 0xa511e9b3u) - 0.5) * 0.72
    ) / f32(grid_z);
    return vec2<f32>(
        cluster.center_density.x
            + (grid_u * 2.0 - 1.0) * cluster.coverage_half_extents.x,
        cluster.center_density.z
            + (grid_v * 2.0 - 1.0) * cluster.coverage_half_extents.y,
    );
}

fn select_carrier(
    cluster: Cluster,
    root: vec2<f32>,
    tuft_index: u32,
    seed: u32,
    bottom_clip_w: f32,
    projected_visibility_extent: f32,
    projected_detail_size: f32,
    coverage_sequence_offset: f32,
    near_transition_start: f32,
    near_transition_end: f32,
) -> CarrierSelection {
    let root_clip_w = max(
        bottom_clip_w
            + camera.clip_from_world[0].w * (root.x - cluster.center_density.x)
            + camera.clip_from_world[2].w * (root.y - cluster.center_density.z),
        0.0001,
    );
    let root_projection_scale = bottom_clip_w / root_clip_w;
    let retained_density = density_fraction(
        projected_visibility_extent * root_projection_scale,
    );
    let coverage_rank = fract(
        coverage_sequence_offset + (f32(tuft_index) + 0.5) * 0.61803398875,
    );
    if (coverage_rank >= retained_density) {
        return CarrierSelection(0u, 0u, 0u);
    }

    let root_detail_size = projected_detail_size * root_projection_scale;
    let near_weight = smoothstep(
        near_transition_start,
        near_transition_end,
        root_detail_size,
    );
    let far_weight = 1.0 - smoothstep(10.0, 20.0, root_detail_size);
    let lod_selector = random01(seed ^ 0xd6e8feb9u);
    let near_selected = lod_selector < near_weight;
    let far_selected = !near_selected
        && lod_selector < near_weight + (1.0 - near_weight) * far_weight;
    return CarrierSelection(
        1u,
        u32(near_selected),
        u32(far_selected),
    );
}

fn budgeted_blade_count(near: bool) -> u32 {
    let demand = select(
        atomicLoad(&ribbon_candidates.mid_instances),
        atomicLoad(&ribbon_candidates.near_instances),
        near,
    );
    if (demand <= 1u) {
        return 12u;
    }
    // Leave a small safety margin for platform-specific atomic scheduling and future debug draws.
    // Demand is counted in twelve-blade carriers, so flooring this ratio guarantees that the
    // uniformly reduced expansion remains inside the fixed draw buffer.
    let budget_target = camera.limits.z * 0.92;
    return u32(clamp(floor(12.0 * budget_target / f32(demand)), 1.0, 12.0));
}

// Count exact ribbon demand before expansion. The second pass uses these totals to reduce the
// number of low-discrepancy roots in every carrier uniformly, rather than filling the fixed output
// buffer with the first dispatched pages and dropping later pages as rectangular holes.
@compute @workgroup_size(64)
fn count_ribbon_candidates(@builtin(global_invocation_id) global_id: vec3<u32>) {
    if (camera.debug.y == 0u || camera.debug.x == 2u) {
        return;
    }
    let cluster_index = global_id.x;
    if (cluster_index >= arrayLength(&clusters)) {
        return;
    }
    let cluster = clusters[cluster_index];
    if (!cluster_visible(cluster)) {
        return;
    }

    let species_data = species[cluster.metadata.x];
    let ground_y = cluster.center_density.y - species_data.top_max_height.w * 0.5;
    let bottom_clip = camera.clip_from_world
        * vec4<f32>(cluster.center_density.x, ground_y, cluster.center_density.z, 1.0);
    let top_clip = camera.clip_from_world
        * vec4<f32>(
            cluster.center_density.x,
            ground_y + species_data.top_max_height.w,
            cluster.center_density.z,
            1.0,
        );
    let maximum_horizontal_reach = max(
        species_data.top_max_height.w * 0.5,
        species_data.card.y * 0.5,
    );
    let x_clip = camera.clip_from_world
        * vec4<f32>(
            cluster.center_density.x + maximum_horizontal_reach,
            ground_y,
            cluster.center_density.z,
            1.0,
        );
    let z_clip = camera.clip_from_world
        * vec4<f32>(
            cluster.center_density.x,
            ground_y,
            cluster.center_density.z + maximum_horizontal_reach,
            1.0,
        );
    if (bottom_clip.w <= 0.0 || top_clip.w <= 0.0 || x_clip.w <= 0.0 || z_clip.w <= 0.0) {
        return;
    }
    let viewport_scale = camera.viewport.xy * 0.5;
    let bottom_screen = bottom_clip.xy / bottom_clip.w * viewport_scale;
    let maximum_projected_height = distance(
        top_clip.xy / top_clip.w * viewport_scale,
        bottom_screen,
    );
    let maximum_projected_horizontal_reach = max(
        distance(x_clip.xy / x_clip.w * viewport_scale, bottom_screen),
        distance(z_clip.xy / z_clip.w * viewport_scale, bottom_screen),
    );
    let projected_visibility_extent = max(
        maximum_projected_height,
        maximum_projected_horizontal_reach,
    );
    let representative_height = mix(
        species_data.bottom_min_height.w,
        species_data.top_max_height.w,
        0.5,
    );
    let representative_width = mix(species_data.card.x, species_data.card.y, 0.5);
    let projected_representative_height = maximum_projected_height
        * representative_height / max(species_data.top_max_height.w, 0.0001);
    let projected_representative_width = maximum_projected_horizontal_reach
        * representative_width / max(maximum_horizontal_reach * 2.0, 0.0001);
    let overhead_weight = smoothstep(0.45, 0.75, abs(camera.view_direction.y));
    let projected_detail_size = mix(
        projected_representative_height,
        projected_representative_width,
        overhead_weight,
    );

    let full_count = u32(max(round(cluster.center_density.w * cluster.half_extents_area.w), 0.0));
    if (full_count == 0u) {
        return;
    }
    let overhead_zoom = clamp(camera.view_direction.w, 0.0, 1.0);
    let near_transition_start = mix(48.0, 16.0, overhead_zoom);
    let near_transition_end = mix(72.0, 30.0, overhead_zoom);
    let grid_x = max(1u, u32(ceil(sqrt(f32(full_count)))));
    let grid_z = max(1u, (full_count + grid_x - 1u) / grid_x);
    let coverage_sequence_offset = random01(cluster.metadata.y ^ 0x4f1bbcdcu);
    for (var tuft_index = 0u; tuft_index < full_count; tuft_index += 1u) {
        let seed = hash32(cluster.metadata.y ^ tuft_index * 0x9e3779b9u);
        let root = carrier_root(cluster, grid_x, grid_z, tuft_index, seed);
        let selection = select_carrier(
            cluster,
            root,
            tuft_index,
            seed,
            bottom_clip.w,
            projected_visibility_extent,
            projected_detail_size,
            coverage_sequence_offset,
            near_transition_start,
            near_transition_end,
        );
        if (selection.retained == 0u || selection.far != 0u) {
            continue;
        }
        if (selection.near != 0u) {
            atomicAdd(&ribbon_candidates.near_instances, 12u);
        } else {
            atomicAdd(&ribbon_candidates.mid_instances, 12u);
        }
    }
}

@compute @workgroup_size(64)
fn cull(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let cluster_index = global_id.x;
    if (cluster_index >= arrayLength(&clusters)) {
        return;
    }

    let cluster = clusters[cluster_index];
    if (!cluster_visible(cluster)) {
        return;
    }

    let species_data = species[cluster.metadata.x];
    let ground_y = cluster.center_density.y - species_data.top_max_height.w * 0.5;
    let bottom_clip = camera.clip_from_world
        * vec4<f32>(cluster.center_density.x, ground_y, cluster.center_density.z, 1.0);
    let top_clip = camera.clip_from_world
        * vec4<f32>(
            cluster.center_density.x,
            ground_y + species_data.top_max_height.w,
            cluster.center_density.z,
            1.0,
        );
    let maximum_horizontal_reach = max(
        species_data.top_max_height.w * 0.5,
        species_data.card.y * 0.5,
    );
    let x_clip = camera.clip_from_world
        * vec4<f32>(
            cluster.center_density.x + maximum_horizontal_reach,
            ground_y,
            cluster.center_density.z,
            1.0,
        );
    let z_clip = camera.clip_from_world
        * vec4<f32>(
            cluster.center_density.x,
            ground_y,
            cluster.center_density.z + maximum_horizontal_reach,
            1.0,
        );
    if (bottom_clip.w <= 0.0 || top_clip.w <= 0.0 || x_clip.w <= 0.0 || z_clip.w <= 0.0) {
        return;
    }
    let viewport_scale = camera.viewport.xy * 0.5;
    let bottom_screen = bottom_clip.xy / bottom_clip.w * viewport_scale;
    let maximum_projected_height = distance(
        top_clip.xy / top_clip.w * viewport_scale,
        bottom_screen,
    );
    let maximum_projected_horizontal_reach = max(
        distance(x_clip.xy / x_clip.w * viewport_scale, bottom_screen),
        distance(z_clip.xy / z_clip.w * viewport_scale, bottom_screen),
    );

    // Visibility and density use the largest possible card extent so that wide or flattened
    // cards cannot vanish prematurely. Geometry LOD deliberately uses a representative card.
    // Otherwise one unusually wide card keeps every clump in the expensive near tier.
    let projected_visibility_extent = max(
        maximum_projected_height,
        maximum_projected_horizontal_reach,
    );
    let representative_height = mix(
        species_data.bottom_min_height.w,
        species_data.top_max_height.w,
        0.5,
    );
    let representative_width = mix(species_data.card.x, species_data.card.y, 0.5);
    let projected_representative_height = maximum_projected_height
        * representative_height / max(species_data.top_max_height.w, 0.0001);
    let projected_representative_width = maximum_projected_horizontal_reach
        * representative_width / max(maximum_horizontal_reach * 2.0, 0.0001);
    let overhead_weight = smoothstep(0.45, 0.75, abs(camera.view_direction.y));
    let projected_detail_size = mix(
        projected_representative_height,
        projected_representative_width,
        overhead_weight,
    );

    // A cluster may span a meaningful depth range in perspective. Reject it only when even its
    // nearest authored edge is below the geometry cutoff; using the centre here made complete
    // rectangular mask samples disappear while samples behind them remained visible.
    let nearest_clip_w = max(
        bottom_clip.w
            - abs(camera.clip_from_world[0].w) * cluster.coverage_half_extents.x
            - abs(camera.clip_from_world[2].w) * cluster.coverage_half_extents.y,
        0.0001,
    );
    let maximum_projection_scale = bottom_clip.w / nearest_clip_w;
    if (density_fraction(projected_visibility_extent * maximum_projection_scale) <= 0.0) {
        return;
    }
    let full_count = u32(max(round(cluster.center_density.w * cluster.half_extents_area.w), 0.0));
    if (full_count == 0u) {
        return;
    }

    // A broad mid tier should cover most of a third-person view. Stable per-clump dithering
    // below turns these bands into gradual transitions rather than visible distance rings.
    let overhead_zoom = clamp(camera.view_direction.w, 0.0, 1.0);
    // A high camera exposes a much larger and more obvious LOD ring. Keep procedural
    // geometry farther out there; stable per-clump dithering still spreads the handoff.
    let near_transition_start = mix(48.0, 16.0, overhead_zoom);
    let near_transition_end = mix(72.0, 30.0, overhead_zoom);
    let capacity = u32(camera.limits.z);
    let grid_x = max(1u, u32(ceil(sqrt(f32(full_count)))));
    let grid_z = max(1u, (full_count + grid_x - 1u) / grid_x);
    let coverage_sequence_offset = random01(cluster.metadata.y ^ 0x4f1bbcdcu);
    for (var tuft_index = 0u; tuft_index < full_count; tuft_index += 1u) {
        let seed = hash32(cluster.metadata.y ^ tuft_index * 0x9e3779b9u);
        let grid_column = tuft_index % grid_x;
        let grid_row = tuft_index / grid_x;
        let card_root = carrier_root(cluster, grid_x, grid_z, tuft_index, seed);
        let selection = select_carrier(
            cluster,
            card_root,
            tuft_index,
            seed,
            bottom_clip.w,
            projected_visibility_extent,
            projected_detail_size,
            coverage_sequence_offset,
            near_transition_start,
            near_transition_end,
        );
        if (selection.retained == 0u) {
            continue;
        }
        let card_x = card_root.x;
        let card_z = card_root.y;
        let card_yaw = random01(seed ^ 0x63d83595u) * 6.28318530718;
        let card_height = mix(
            species_data.bottom_min_height.w,
            species_data.top_max_height.w,
            random01(seed ^ 0xc2b2ae35u),
        );
        let width_bucket = floor(random01(seed ^ 0x85ebca77u) * 3.0) * 0.5;
        let width_fraction = clamp(
            width_bucket + (random01(seed ^ 0x27d4eb2fu) - 0.5) * 0.12,
            0.0,
            1.0,
        );
        let card_width = mix(species_data.card.x, species_data.card.y, width_fraction);
        let flattened = random01(seed ^ 0x94d049bbu) < species_data.card.z;
        let card_visible = VisibleInstance(
            vec4<f32>(card_x, ground_y, card_z, card_yaw),
            vec4<f32>(species_data.bottom_min_height.xyz, card_height),
            vec4<f32>(species_data.top_max_height.xyz, card_width * 0.5),
            vec4<f32>(
                select(1.0, 0.0, flattened),
                species_data.card.w,
                random01(seed ^ 0x165667b1u),
                random01(seed ^ 0x85ebca77u),
            ),
            vec4<f32>(interaction_displacement(vec2<f32>(card_x, card_z)), 0.0, 0.0),
            species_data.artwork,
        );

        let near_selected = selection.near != 0u;
        let far_selected = selection.far != 0u;
        let blade_selected = camera.debug.y != 0u && !far_selected;

        if (blade_selected) {
            if (camera.debug.x != 2u) {
                // The card-density sample is only a deterministic generation tile here. Its
                // complete footprint is filled by independent low-discrepancy blade roots; it
                // is never rendered or treated as a visual clump in the ribbon tiers.
                // Geometry LOD normally changes strip resolution, not root coverage. If a tier's
                // measured demand exceeds its fixed output budget, every carrier instead keeps the
                // same smaller low-discrepancy subset. This prevents page-order buffer overflow
                // from dropping complete rectangular areas.
                let blade_count = budgeted_blade_count(near_selected);
                let blade_width_scale = min(sqrt(12.0 / f32(blade_count)), 1.75);
                let tile_width = cluster.coverage_half_extents.x * 2.0 / f32(grid_x);
                let tile_depth = cluster.coverage_half_extents.y * 2.0 / f32(grid_z);
                let tile_center = vec2<f32>(
                    cluster.center_density.x
                        + ((f32(grid_column) + 0.5) / f32(grid_x) * 2.0 - 1.0)
                            * cluster.coverage_half_extents.x,
                    cluster.center_density.z
                        + ((f32(grid_row) + 0.5) / f32(grid_z) * 2.0 - 1.0)
                            * cluster.coverage_half_extents.y,
                );
                let sequence_offset = vec2<f32>(
                    random01(seed ^ 0x7f4a7c15u),
                    random01(seed ^ 0x94d049bbu),
                );
                // Interaction varies on a larger scale than this sub-metre generation tile.
                // Evaluate it once and reserve the complete output range with one atomic operation.
                let tile_interaction = interaction_displacement(tile_center);
                var output_base = 0u;
                if (near_selected) {
                    output_base = atomicAdd(&near_args.instance_count, blade_count);
                } else {
                    output_base = atomicAdd(&mid_args.instance_count, blade_count);
                }
                for (var blade_index = 0u; blade_index < 12u; blade_index += 1u) {
                    if (blade_index >= blade_count) {
                        break;
                    }
                    let blade_seed = hash32(seed ^ blade_index * 0x9e3779b9u ^ 0x68bc21ebu);
                    let sample = fract(
                        sequence_offset
                            + vec2<f32>(0.754877666, 0.569840296)
                                * (f32(blade_index) + 0.5)
                    );
                    let root_xz = tile_center
                        + (sample - 0.5) * vec2<f32>(tile_width, tile_depth) * 0.96;
                    let clump = sample_clump_field(root_xz);
                    let clump_angle = atan2(clump.facing.y, clump.facing.x);
                    let individual_fan = (random01(blade_seed ^ 0x165667b1u) - 0.5) * 0.42;
                    let stray = random01(blade_seed ^ 0x85ebca77u) < 0.10;
                    let stray_rotation = select(
                        0.0,
                        (random01(blade_seed ^ 0x27d4eb2fu) - 0.5) * 1.35,
                        stray,
                    );
                    let blade_facing = clump_angle + individual_fan + stray_rotation;
                    let blade_height = mix(
                        species_data.bottom_min_height.w,
                        species_data.top_max_height.w,
                        mix(0.28, 0.96, random01(blade_seed ^ 0xc2b2ae35u)),
                    );
                    let blade_shape = random01(blade_seed ^ 0x63d83595u);
                    let maximum_wind_response = species_data.card.w
                        * mix(0.82, 1.0, clump.phase);
                    let blade_wind = ribbon_wind_displacement(
                        root_xz,
                        blade_shape,
                        maximum_wind_response,
                    );
                    let blade_width_source = mix(
                        species_data.card.x,
                        species_data.card.y,
                        random01(blade_seed ^ 0xd1b54a35u),
                    );
                    let blade_half_width = clamp(
                        blade_width_source
                            * mix(0.010, 0.018, random01(blade_seed ^ 0xa511e9b3u))
                            * blade_width_scale,
                        0.004,
                        0.022,
                    );
                    let blade_visible = VisibleInstance(
                        vec4<f32>(root_xz.x, ground_y, root_xz.y, blade_facing),
                        vec4<f32>(species_data.bottom_min_height.xyz, blade_height),
                        vec4<f32>(species_data.top_max_height.xyz, blade_half_width),
                        vec4<f32>(
                            blade_wind.y,
                            clump.color,
                            blade_shape,
                            maximum_wind_response,
                        ),
                        vec4<f32>(tile_interaction, blade_wind.x, blade_wind.z),
                        species_data.artwork,
                    );
                    let output_index = output_base + blade_index;
                    if (near_selected) {
                        if (output_index < capacity) {
                            near_instances[output_index] = blade_visible;
                        }
                    } else {
                        if (output_index < capacity) {
                            mid_instances[output_index] = blade_visible;
                        }
                    }
                }
            }
        } else if (far_selected) {
            if (camera.debug.x != 3u) {
                let output_index = atomicAdd(&far_args.instance_count, 1u);
                if (output_index < capacity) {
                    far_instances[output_index] = card_visible;
                }
            }
        } else if (near_selected) {
            if (camera.debug.x != 2u) {
                let output_index = atomicAdd(&near_args.instance_count, 1u);
                if (output_index < capacity) {
                    near_instances[output_index] = card_visible;
                }
            }
        } else {
            if (camera.debug.x != 2u) {
                let output_index = atomicAdd(&mid_args.instance_count, 1u);
                if (output_index < capacity) {
                    mid_instances[output_index] = card_visible;
                }
            }
        }
    }
}

@group(0) @binding(0) var<storage, read_write> final_near_args: DrawIndirectArgs;
@group(0) @binding(1) var<storage, read_write> final_mid_args: DrawIndirectArgs;
@group(0) @binding(2) var<storage, read_write> final_far_args: DrawIndirectArgs;
@group(0) @binding(3) var<uniform> final_camera: Camera;

@compute @workgroup_size(1)
fn finalize() {
    let reduced_card_layout = final_camera.view_direction.w <= 0.001;
    let near_vertex_count = select(
        select(36u, 24u, reduced_card_layout),
        15u,
        final_camera.debug.y != 0u,
    );
    atomicStore(&final_near_args.vertex_count, near_vertex_count);
    atomicStore(
        &final_near_args.instance_count,
        min(atomicLoad(&final_near_args.instance_count), 131072u),
    );
    atomicStore(&final_near_args.first_vertex, 0u);
    atomicStore(&final_near_args.first_instance, 0u);

    let mid_vertex_count = select(
        select(24u, 12u, reduced_card_layout),
        7u,
        final_camera.debug.y != 0u,
    );
    atomicStore(&final_mid_args.vertex_count, mid_vertex_count);
    atomicStore(
        &final_mid_args.instance_count,
        min(atomicLoad(&final_mid_args.instance_count), 131072u),
    );
    atomicStore(&final_mid_args.first_vertex, 0u);
    atomicStore(&final_mid_args.first_instance, 0u);

    atomicStore(&final_far_args.vertex_count, 6u);
    atomicStore(
        &final_far_args.instance_count,
        min(atomicLoad(&final_far_args.instance_count), 131072u),
    );
    atomicStore(&final_far_args.first_vertex, 0u);
    atomicStore(&final_far_args.first_instance, 0u);
}
