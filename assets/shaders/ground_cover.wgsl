#import bevy_pbr::{
    mesh_view_bindings as view_bindings,
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    shadows,
}

struct VisibleInstance {
    position_yaw: vec4<f32>,
    bottom_height: vec4<f32>,
    top_half_width: vec4<f32>,
    motion: vec4<f32>,
    // xy: world-space tip displacement, zw: reserved
    interaction: vec4<f32>,
    // x: first artwork texture layer, y: variant count
    artwork: vec4<u32>,
}

struct Camera {
    clip_from_world: mat4x4<f32>,
    camera_position: vec4<f32>,
    // xyz: forward direction, w: continuous camera-pitch blend (third-person to overhead)
    view_direction: vec4<f32>,
    viewport: vec4<f32>,
    limits: vec4<f32>,
    wind: vec4<f32>,
    wind_direction: vec4<f32>,
    // x: debug mode (0 normal, 1 LOD colors, 2 far only, 3 far disabled)
    debug: vec4<u32>,
}

struct DrawConfig {
    // x: crossed ribbons, y: vertical segments, z: LOD index
    geometry: vec4<u32>,
}

struct GroundShadowVolume {
    // xy: minimum world x/z, z: square extent, w: first slice world height
    origin_extent: vec4<f32>,
    // x: slice spacing, y: inverse spacing, z: last slice index, w: edge blend width in UV
    height: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) texture_layer: u32,
    @location(3) @interpolate(flat) card_visibility: f32,
    @location(4) world_position: vec3<f32>,
    @location(5) @interpolate(flat) procedural_blade: u32,
    @location(6) world_normal: vec3<f32>,
}

// Group zero is Bevy's mesh-view bind group, including the directional shadow map.
@group(1) @binding(0) var<storage, read> instances: array<VisibleInstance>;
@group(1) @binding(1) var<uniform> camera: Camera;
@group(1) @binding(2) var<uniform> config: DrawConfig;
@group(1) @binding(3) var clump_texture: texture_2d_array<f32>;
@group(1) @binding(4) var clump_sampler: sampler;
@group(1) @binding(5) var ground_shadow_volume: texture_2d_array<f32>;
@group(1) @binding(6) var ground_shadow_sampler: sampler;
@group(1) @binding(7) var<uniform> ground_shadow: GroundShadowVolume;

fn quad_vertex(vertex_in_quad: u32) -> vec2<f32> {
    switch vertex_in_quad {
        case 0u: { return vec2<f32>(-1.0, 0.0); }
        case 1u: { return vec2<f32>(1.0, 0.0); }
        case 2u: { return vec2<f32>(1.0, 1.0); }
        case 3u: { return vec2<f32>(-1.0, 0.0); }
        case 4u: { return vec2<f32>(1.0, 1.0); }
        default: { return vec2<f32>(-1.0, 1.0); }
    }
}

// A high blade is a native 15-vertex strip with seven cross-sections and a shared tip. The low
// blade keeps three cross-sections and the same tip in a 7-vertex strip.
fn blade_strip_vertex(strip_index: u32, section_count: u32) -> vec2<f32> {
    let tip_index = section_count * 2u;
    if (strip_index >= tip_index) {
        return vec2<f32>(0.0, 1.0);
    }
    let section = strip_index / 2u;
    let side = select(-1.0, 1.0, (strip_index & 1u) != 0u);
    return vec2<f32>(side, f32(section) / f32(section_count));
}

fn cubic_bezier(
    p0: vec3<f32>,
    p1: vec3<f32>,
    p2: vec3<f32>,
    p3: vec3<f32>,
    t: f32,
) -> vec3<f32> {
    let inverse_t = 1.0 - t;
    return p0 * inverse_t * inverse_t * inverse_t
        + p1 * 3.0 * inverse_t * inverse_t * t
        + p2 * 3.0 * inverse_t * t * t
        + p3 * t * t * t;
}

fn cubic_bezier_derivative(
    p0: vec3<f32>,
    p1: vec3<f32>,
    p2: vec3<f32>,
    p3: vec3<f32>,
    t: f32,
) -> vec3<f32> {
    let inverse_t = 1.0 - t;
    return (p1 - p0) * 3.0 * inverse_t * inverse_t
        + (p2 - p1) * 6.0 * inverse_t * t
        + (p3 - p2) * 3.0 * t * t;
}

fn procedural_blade(
    instance: VisibleInstance,
    vertex_index: u32,
    lod: u32,
) -> VertexOutput {
    let low_detail = lod == 1u;
    let section_count = select(7u, 3u, low_detail);
    let blade_vertex_data = blade_strip_vertex(vertex_index, section_count);
    let side = blade_vertex_data.x;
    let height_fraction = blade_vertex_data.y;

    // Each visible instance is now one blade. Clump membership only supplied the coherent
    // resting facing and color during GPU expansion; it never moves or bunches blade roots.
    let root = instance.position_yaw.xyz;
    let blade_height = instance.bottom_height.w;
    let shape_random = instance.motion.z;
    let rest_direction = vec2<f32>(
        cos(instance.position_yaw.w),
        sin(instance.position_yaw.w),
    );

    // Wind was evaluated once per blade in the expansion pass. Reusing that 3D tip displacement
    // here avoids repeating several trigonometric functions for all 7/15 strip vertices.
    let wind_displacement = instance.interaction.zw;
    let vertical_wind_displacement = instance.motion.x;
    let width_axis = vec2<f32>(-rest_direction.y, rest_direction.x);

    // Tilt controls the endpoint, while bend controls the two inner Bézier points. All blades
    // are long smooth arcs; none of the ordinary shape range degenerates into upright sticks.
    let tilt = mix(1.02, 1.28, shape_random);
    let resting_reach = blade_height * sin(tilt);
    let resting_tip_height = max(blade_height * cos(tilt), blade_height * 0.20);
    let resting_tip = rest_direction * resting_reach;
    var horizontal_tip = resting_tip + wind_displacement + instance.interaction.xy;
    let maximum_tip_offset = blade_height * 0.98;
    let horizontal_tip_length = length(horizontal_tip);
    if (horizontal_tip_length > maximum_tip_offset) {
        horizontal_tip *= maximum_tip_offset / horizontal_tip_length;
    }

    let p0 = root;
    let p1 = root + vec3<f32>(0.0, blade_height * mix(0.24, 0.34, shape_random), 0.0);
    let side_curve = width_axis * blade_height * (shape_random - 0.5) * 0.035;
    let middle_displacement = wind_displacement * 0.42 + instance.interaction.xy * 0.58;
    let p2_horizontal = resting_tip * 0.58 + middle_displacement;
    let p2 = root + vec3<f32>(
        p2_horizontal.x + side_curve.x,
        resting_tip_height
            + blade_height * mix(0.10, 0.18, 1.0 - shape_random)
            + vertical_wind_displacement * 0.44,
        p2_horizontal.y + side_curve.y,
    );
    let animated_tip_height = clamp(
        resting_tip_height + vertical_wind_displacement,
        blade_height * 0.05,
        blade_height * 0.82,
    );
    let p3 = root + vec3<f32>(
        horizontal_tip.x,
        animated_tip_height,
        horizontal_tip.y,
    );
    let center = cubic_bezier(p0, p1, p2, p3, height_fraction);
    let tangent = normalize(cubic_bezier_derivative(p0, p1, p2, p3, height_fraction));

    let to_camera = camera.camera_position.xz - root.xz;
    let to_camera_length = length(to_camera);
    var face_alignment = 1.0;
    if (to_camera_length > 0.0001) {
        face_alignment = abs(dot(rest_direction, to_camera / to_camera_length));
    }
    let edge_on = 1.0 - smoothstep(0.05, 0.32, face_alignment);
    let half_width = instance.top_half_width.w * mix(1.0, 1.55, edge_on);
    let taper = 1.0 - smoothstep(0.72, 1.0, height_fraction);
    let world_position = center
        + vec3<f32>(width_axis.x, 0.0, width_axis.y) * side * half_width * taper;

    // Rotate the flat surface normal across the blade width. This rounded normal provides
    // readable volume without adding geometry and remains deterministic at every view angle.
    let width_axis_3d = vec3<f32>(width_axis.x, 0.0, width_axis.y);
    let flat_normal = normalize(cross(width_axis_3d, tangent));
    let round_amount = side * 0.55;
    let world_normal = normalize(
        flat_normal * sqrt(max(1.0 - round_amount * round_amount, 0.0))
            + width_axis_3d * round_amount
    );

    var output: VertexOutput;
    output.clip_position = camera.clip_from_world * vec4<f32>(world_position, 1.0);
    output.world_position = world_position;
    let clump_color = mix(0.84, 1.12, instance.motion.y);
    let blade_color = mix(0.94, 1.06, shape_random);
    output.color = mix(instance.bottom_height.xyz, instance.top_half_width.xyz, height_fraction)
        * clump_color * blade_color;
    if (camera.debug.x == 1u) {
        output.color = select(
            vec3<f32>(0.95, 0.12, 0.08),
            vec3<f32>(1.0, 0.68, 0.06),
            low_detail,
        );
    }
    output.uv = vec2<f32>(side * 0.5 + 0.5, 1.0 - height_fraction);
    output.texture_layer = instance.artwork.x;
    output.card_visibility = 1.0;
    output.world_normal = world_normal;
    // Two means high-detail/direct CSM; one means low-detail/filtered shadow volume.
    output.procedural_blade = select(2u, 1u, low_detail);
    return output;
}

@vertex
fn vertex(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let instance = instances[instance_index];
    if (config.geometry.z <= 1u && camera.debug.y != 0u) {
        return procedural_blade(instance, vertex_index, config.geometry.z);
    }
    let vertices_per_ribbon = config.geometry.y * 6u;
    let ribbon_index = vertex_index / vertices_per_ribbon;
    let within_ribbon = vertex_index % vertices_per_ribbon;
    let segment_index = within_ribbon / 6u;
    let vertex_in_quad = within_ribbon % 6u;
    let quad = quad_vertex(vertex_in_quad);
    let segment_count = f32(config.geometry.y);
    let lower = f32(segment_index) / segment_count;
    let upper = f32(segment_index + 1u) / segment_count;
    let height_fraction = mix(lower, upper, quad.y);

    let lod = config.geometry.z;
    let half_width = instance.top_half_width.w;
    let local_x = quad.x * half_width;
    let bend_strength = 0.04 + instance.motion.z * 0.08;
    let bend = height_fraction * height_fraction * instance.bottom_height.w * bend_strength;
    // LOD tiers are nested: every tier keeps ribbon zero, mid adds ribbon one, and near
    // adds ribbon two. A fixed angular step keeps surviving ribbons in exactly the same
    // place when a clump changes tier.
    let ribbon_angle = instance.position_yaw.w
        + f32(ribbon_index) * 3.14159265359 / 3.0;
    var direction = vec2<f32>(cos(ribbon_angle), sin(ribbon_angle));
    let top_down_blend = clamp(camera.view_direction.w, 0.0, 1.0);
    let to_camera = camera.camera_position.xz - instance.position_yaw.xz;
    let squared_distance = dot(to_camera, to_camera);
    if (ribbon_index == 0u && squared_distance > 0.0001) {
        let view_direction = to_camera * inverseSqrt(squared_distance);
        let point_facing_direction = vec2<f32>(view_direction.y, -view_direction.x);
        let horizontal_forward = camera.view_direction.xz;
        let horizontal_forward_length = length(horizontal_forward);
        if (horizontal_forward_length > 0.0001) {
            // Point-facing cards produce obvious concentric arcs in a high camera. Blend
            // toward a distant virtual camera in top-down mode so the field is mostly
            // parallel while retaining enough convergence to avoid a rigid screen pattern.
            let forward = horizontal_forward / horizontal_forward_length;
            let parallel_facing_direction = vec2<f32>(forward.y, -forward.x);
            let aligned_parallel_direction = select(
                -parallel_facing_direction,
                parallel_facing_direction,
                dot(point_facing_direction, parallel_facing_direction) >= 0.0,
            );
            direction = normalize(mix(
                point_facing_direction,
                aligned_parallel_direction,
                top_down_blend * 0.82,
            ));
        } else {
            direction = point_facing_direction;
        }
    }
    let bend_direction = vec2<f32>(
        cos(instance.position_yaw.w + 1.1),
        sin(instance.position_yaw.w + 1.1),
    );
    let tilt_variation = fract(instance.motion.z + f32(ribbon_index) * 0.38196601125);
    // Pitch affects the whole field uniformly. A viewport-relative mask created a visible
    // horizontal deformation band that followed the camera across stationary grass.
    let forward_lean = top_down_blend;
    var tilt = mix(0.10, 0.30, tilt_variation) + forward_lean * 0.42;
    if (ribbon_index == 2u && instance.motion.x < 0.5) {
        // A small stable subset replaces its third upright ribbon with a card that
        // almost lies across the ground, breaking up the vertical-card pattern.
        tilt = mix(1.08, 1.31, instance.motion.z);
    }
    var tilt_direction = vec2<f32>(
        cos(instance.position_yaw.w + 1.27),
        sin(instance.position_yaw.w + 1.27),
    );
    let camera_forward = camera.view_direction.xz;
    let camera_forward_length = length(camera_forward);
    if (camera_forward_length > 0.0001) {
        // Keep one stable lean direction throughout the zoom path. Interpolating from a
        // random direction could cross a zero-length vector and make individual cards
        // appear to snap or bend backward halfway through the transition.
        tilt_direction = camera_forward / camera_forward_length;
    }
    let card_distance = height_fraction * instance.bottom_height.w;
    let wind_direction_length = max(length(camera.wind_direction.xy), 0.0001);
    let wind_direction = camera.wind_direction.xy / wind_direction_length;
    let wind_perpendicular = vec2<f32>(-wind_direction.y, wind_direction.x);
    let wind_phase = dot(instance.position_yaw.xz, wind_direction) * camera.wind.w
        + camera.wind.x * camera.wind_direction.z
        + instance.motion.z * 6.28318530718;
    let gust_phase = dot(instance.position_yaw.xz, wind_perpendicular) * camera.wind.w * 0.43
        + camera.wind.x * camera.wind_direction.z * 0.37
        + instance.motion.w * 4.31;
    let gust_envelope = smoothstep(0.28, 0.92, sin(gust_phase) * 0.5 + 0.5);
    let wind_signal = camera.wind.y * sin(wind_phase)
        + camera.wind.z * gust_envelope * sin(wind_phase * 0.71 + 1.13);
    let local_wind_direction = normalize(
        wind_direction + wind_perpendicular * sin(wind_phase * 0.29) * 0.18
    );
    let wind_bend = local_wind_direction
        * instance.motion.y
        * wind_signal
        * height_fraction
        * height_fraction;
    // Interaction is evaluated once per clump by the compute pass. Every card and LOD tier
    // receives the same world-space response, while the quadratic height weight pins roots.
    let interaction_bend = instance.interaction.xy
        * height_fraction
        * height_fraction;
    let interaction_length_squared = dot(interaction_bend, interaction_bend);
    let remaining_vertical = sqrt(max(
        card_distance * card_distance - interaction_length_squared,
        0.0,
    ));
    let interaction_vertical_drop = card_distance - remaining_vertical;
    let horizontal = direction * local_x
        + tilt_direction * card_distance * sin(tilt)
        + bend_direction * bend
        + wind_bend
        + interaction_bend;
    let world_position = instance.position_yaw.xyz
        + vec3<f32>(
            horizontal.x,
            max(card_distance * cos(tilt) - interaction_vertical_drop, 0.0),
            horizontal.y,
        );
    var output: VertexOutput;
    output.clip_position = camera.clip_from_world * vec4<f32>(world_position, 1.0);
    output.world_position = world_position;
    output.color = mix(instance.bottom_height.xyz, instance.top_half_width.xyz, height_fraction);
    if (camera.debug.x == 1u) {
        if (lod == 0u) {
            output.color = vec3<f32>(0.95, 0.12, 0.08);
        } else if (lod == 1u) {
            output.color = vec3<f32>(1.0, 0.68, 0.06);
        } else {
            output.color = vec3<f32>(0.08, 0.35, 1.0);
        }
    }
    let base_u = quad.x * 0.5 + 0.5;
    let mirrored = fract(instance.motion.z + f32(ribbon_index) * 0.61803398875) >= 0.5;
    output.uv = vec2<f32>(select(base_u, 1.0 - base_u, mirrored), 1.0 - height_fraction);
    let variant_count = max(instance.artwork.y, 1u);
    output.texture_layer = instance.artwork.x + min(
        u32(floor(fract(instance.motion.w + f32(ribbon_index) * 0.277) * f32(variant_count))),
        variant_count - 1u,
    );
    output.card_visibility = 1.0;
    if ((lod == 0u && ribbon_index == 2u) || (lod == 1u && ribbon_index == 1u)) {
        output.card_visibility = top_down_blend;
    }
    output.procedural_blade = 0u;
    output.world_normal = vec3<f32>(direction.y, 0.0, -direction.x);
    return output;
}

fn interleaved_gradient_noise(pixel: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(pixel, vec2<f32>(0.06711056, 0.00583715))));
}

fn visible_card_coverage(input: VertexOutput) -> f32 {
    if (input.procedural_blade != 0u) {
        return 1.0;
    }
    let coverage = textureSample(
        clump_texture,
        clump_sampler,
        input.uv,
        i32(input.texture_layer),
    ).r;
    if (input.card_visibility <= 0.0) {
        discard;
    }
    if (
        input.card_visibility < 0.999
        && interleaved_gradient_noise(floor(input.clip_position.xy)) > input.card_visibility
    ) {
        discard;
    }
    if (coverage < 0.32) {
        discard;
    }
    return coverage;
}

fn exact_directional_shadow_visibility(input: VertexOutput) -> f32 {
    let light = &view_bindings::lights.directional_lights[0u];
    if (((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u) {
        return 1.0;
    }

    let world_position = vec4<f32>(input.world_position, 1.0);
    let view_z = dot(vec4<f32>(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z
    ), world_position);
    return shadows::fetch_directional_shadow(
        0u,
        world_position,
        // Receiver bias must stay terrain-facing. An animated ribbon normal can point almost
        // horizontally and offset the sample into the terrain, producing black blades that blink
        // as wind changes the normal even though ground cover is not a shadow caster.
        vec3<f32>(0.0, 1.0, 0.0),
        view_z,
        input.clip_position.xy,
    );
}

fn directional_shadow_visibility(input: VertexOutput) -> f32 {
    if (camera.debug.x != 0u) {
        return 1.0;
    }
    let uv = (input.world_position.xz - ground_shadow.origin_extent.xy)
        / ground_shadow.origin_extent.z;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return exact_directional_shadow_visibility(input);
    }

    let slice_coordinate = clamp(
        (input.world_position.y - ground_shadow.origin_extent.w) * ground_shadow.height.y,
        0.0,
        ground_shadow.height.z,
    );
    let lower_slice = u32(floor(slice_coordinate));
    let upper_slice = min(lower_slice + 1u, u32(ground_shadow.height.z));
    let height_blend = fract(slice_coordinate);
    let lower_visibility = textureSampleLevel(
        ground_shadow_volume,
        ground_shadow_sampler,
        uv,
        i32(lower_slice),
        0.0,
    ).r;
    let upper_visibility = textureSampleLevel(
        ground_shadow_volume,
        ground_shadow_sampler,
        uv,
        i32(upper_slice),
        0.0,
    ).r;
    let volume_visibility = mix(lower_visibility, upper_visibility, height_blend);

    // Only the narrow field boundary pays for both paths. This hides a hard transition to
    // the existing direct CSM receiver without restoring per-card shadow work in the near field.
    let edge_distance = min(min(uv.x, uv.y), min(1.0 - uv.x, 1.0 - uv.y));
    if (edge_distance < ground_shadow.height.w) {
        let direct_visibility = exact_directional_shadow_visibility(input);
        return mix(
            direct_visibility,
            volume_visibility,
            smoothstep(0.0, ground_shadow.height.w, edge_distance),
        );
    }
    return volume_visibility;
}

fn blade_surface_response(input: VertexOutput, coverage: f32) -> f32 {
    if (input.procedural_blade == 0u) {
        return mix(0.82, 1.06, coverage);
    }
    // Keep the first ribbon baseline temporally stable. Animated rounded-normal lighting made
    // individual blades pulse between dark and bright; proper leaf BRDF/transmission will be
    // reintroduced only after it has its own stable material test.
    return 0.96;
}

// This fragment entry point performs only the shared alpha test and writes depth. The color pass
// uses depth equality, so overlapping cards that lost here never execute a shadow-map lookup.
@fragment
fn prepass_fragment(input: VertexOutput) {
    visible_card_coverage(input);
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let coverage = visible_card_coverage(input);
    let edge_coverage = smoothstep(0.32, 0.68, coverage);
    let shadow_visibility = directional_shadow_visibility(input);
    let shadow_attenuation = mix(0.48, 1.0, shadow_visibility);
    let surface_response = blade_surface_response(input, coverage);
    let shaded_color = input.color * surface_response * shadow_attenuation;
    return vec4<f32>(shaded_color, edge_coverage);
}
