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

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) texture_layer: u32,
    @location(3) @interpolate(flat) card_visibility: f32,
}

@group(0) @binding(0) var<storage, read> instances: array<VisibleInstance>;
@group(0) @binding(1) var<uniform> camera: Camera;
@group(0) @binding(2) var<uniform> config: DrawConfig;
@group(0) @binding(3) var clump_texture: texture_2d_array<f32>;
@group(0) @binding(4) var clump_sampler: sampler;

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

@vertex
fn vertex(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let instance = instances[instance_index];
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
    return output;
}

fn interleaved_gradient_noise(pixel: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(pixel, vec2<f32>(0.06711056, 0.00583715))));
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
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
    let edge_coverage = smoothstep(0.32, 0.68, coverage);
    let shaded_color = input.color * mix(0.82, 1.06, coverage);
    return vec4<f32>(shaded_color, edge_coverage);
}
