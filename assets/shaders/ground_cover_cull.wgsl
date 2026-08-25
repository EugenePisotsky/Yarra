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
}

struct VisibleInstance {
    position_yaw: vec4<f32>,
    bottom_height: vec4<f32>,
    top_half_width: vec4<f32>,
    motion: vec4<f32>,
    // xy: world-space tip displacement, zw: reserved
    interaction: vec4<f32>,
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

fn density_fraction(projected_height: f32) -> f32 {
    if (projected_height < camera.limits.w) {
        return 0.0;
    }
    if (projected_height < 3.0) {
        return 0.12 * smoothstep(camera.limits.w, 3.0, projected_height);
    }
    if (projected_height < camera.limits.y) {
        return mix(0.12, 0.45, smoothstep(3.0, camera.limits.y, projected_height));
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

    let retained_density = density_fraction(projected_visibility_extent);
    if (retained_density <= 0.0) {
        return;
    }
    let full_count = u32(max(round(cluster.center_density.w * cluster.half_extents_area.w), 0.0));
    if (full_count == 0u) {
        return;
    }

    // A broad mid tier should cover most of a third-person view. Stable per-clump dithering
    // below turns these bands into gradual transitions rather than visible distance rings.
    let overhead_zoom = clamp(camera.view_direction.w, 0.0, 1.0);
    let near_transition_start = mix(48.0, 36.0, overhead_zoom);
    let near_transition_end = mix(72.0, 54.0, overhead_zoom);
    var near_weight = smoothstep(
        near_transition_start,
        near_transition_end,
        projected_detail_size,
    );
    var far_weight = 1.0 - smoothstep(10.0, 20.0, projected_detail_size);
    let capacity = u32(camera.limits.z);
    let grid_x = max(1u, u32(ceil(sqrt(f32(full_count)))));
    let grid_z = max(1u, (full_count + grid_x - 1u) / grid_x);
    for (var tuft_index = 0u; tuft_index < full_count; tuft_index += 1u) {
        let seed = hash32(cluster.metadata.y ^ tuft_index * 0x9e3779b9u);
        if (random01(seed ^ 0xd1b54a35u) >= retained_density) {
            continue;
        }
        let grid_column = tuft_index % grid_x;
        let grid_row = tuft_index / grid_x;
        let grid_u = (
            f32(grid_column) + 0.5 + (random01(seed) - 0.5) * 0.72
        ) / f32(grid_x);
        let grid_v = (
            f32(grid_row) + 0.5 + (random01(seed ^ 0xa511e9b3u) - 0.5) * 0.72
        ) / f32(grid_z);
        let x = cluster.center_density.x
            + (grid_u * 2.0 - 1.0) * cluster.coverage_half_extents.x;
        let z = cluster.center_density.z
            + (grid_v * 2.0 - 1.0) * cluster.coverage_half_extents.y;
        let yaw = random01(seed ^ 0x63d83595u) * 6.28318530718;
        let height = mix(
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
        let width = mix(species_data.card.x, species_data.card.y, width_fraction);
        let flattened = random01(seed ^ 0x94d049bbu) < species_data.card.z;
        let visible = VisibleInstance(
            vec4<f32>(x, ground_y, z, yaw),
            vec4<f32>(species_data.bottom_min_height.xyz, height),
            vec4<f32>(species_data.top_max_height.xyz, width * 0.5),
            vec4<f32>(
                select(1.0, 0.0, flattened),
                species_data.card.w,
                random01(seed ^ 0x165667b1u),
                random01(seed ^ 0x85ebca77u),
            ),
            vec4<f32>(interaction_displacement(vec2<f32>(x, z)), 0.0, 0.0),
        );

        let lod_selector = random01(seed ^ 0xd6e8feb9u);
        if (lod_selector < near_weight) {
            if (camera.debug.x != 2u) {
                let output_index = atomicAdd(&near_args.instance_count, 1u);
                if (output_index < capacity) {
                    near_instances[output_index] = visible;
                }
            }
        } else if (lod_selector < near_weight + (1.0 - near_weight) * far_weight) {
            if (camera.debug.x != 3u) {
                let output_index = atomicAdd(&far_args.instance_count, 1u);
                if (output_index < capacity) {
                    far_instances[output_index] = visible;
                }
            }
        } else {
            if (camera.debug.x != 2u) {
                let output_index = atomicAdd(&mid_args.instance_count, 1u);
                if (output_index < capacity) {
                    mid_instances[output_index] = visible;
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
    atomicStore(&final_near_args.vertex_count, select(36u, 24u, reduced_card_layout));
    atomicStore(
        &final_near_args.instance_count,
        min(atomicLoad(&final_near_args.instance_count), 131072u),
    );
    atomicStore(&final_near_args.first_vertex, 0u);
    atomicStore(&final_near_args.first_instance, 0u);

    atomicStore(&final_mid_args.vertex_count, select(24u, 12u, reduced_card_layout));
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
