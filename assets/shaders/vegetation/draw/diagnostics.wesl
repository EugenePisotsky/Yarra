// Placement diagnostics: one flat strip per accepted (or, for outcomes, every) candidate, colored by
// species, parent link, outcome or group.
import package::shaders::vegetation::types::DiagnosticInstance;
import package::shaders::vegetation::random::random01;
import package::shaders::vegetation::blade::normalize3_or;
import package::shaders::vegetation::draw::bindings::{
    VertexOutput, diagnostic_instances, species, camera, config,
};

fn surface_normal_from_diagnostic_instance(instance: DiagnosticInstance) -> vec3<f32> {
    let normal_xz = unpack2x16snorm(bitcast<u32>(instance.direction_species.w));
    return normalize3_or(
        vec3<f32>(
            normal_xz.x,
            sqrt(max(0.0, 1.0 - dot(normal_xz, normal_xz))),
            normal_xz.y,
        ),
        vec3<f32>(0.0, 1.0, 0.0),
    );
}

fn diagnostic_quad_vertex(vertex_index: u32) -> vec2<f32> {
    switch vertex_index {
        case 0u: { return vec2<f32>(0.0, -1.0); }
        case 1u: { return vec2<f32>(0.0, 1.0); }
        case 2u: { return vec2<f32>(1.0, 1.0); }
        default: { return vec2<f32>(1.0, -1.0); }
    }
}

fn diagnostic_vertex(vertex_index: u32, instance_index: u32) -> VertexOutput {
    let instance = diagnostic_instances[instance_index];
    let profile = species[u32(instance.direction_species.z)];
    let root = instance.root_direction.xyz;
    let surface_normal = surface_normal_from_diagnostic_instance(instance);
    let rest_direction = normalize(
        vec2<f32>(instance.root_direction.w, instance.direction_species.x),
    );

    var surface_direction = vec3<f32>(rest_direction.x, 0.0, rest_direction.y);
    surface_direction -= surface_normal * dot(surface_direction, surface_normal);
    surface_direction = normalize3_or(surface_direction, vec3<f32>(1.0, 0.0, 0.0));

    var start = root;
    var end = root + surface_direction * profile.tip_color_height.w * 0.46
        + surface_normal * profile.tip_color_height.w * 0.52;
    var start_color = profile.root_color.xyz;
    var end_color = profile.tip_color_height.xyz;
    var width_scale = select(1.0, 2.4, profile.root_color.w >= 1.5);

    if (config.values.x == 2u) {
        start = instance.parent_status.xyz + vec3<f32>(0.0, 0.035, 0.0);
        end = root + surface_normal * 0.04;
        start_color = vec3<f32>(1.0, 0.92, 0.15);
        end_color = vec3<f32>(0.15, 0.85, 1.0);
        width_scale = 0.7;
    } else if (config.values.x == 4u) {
        start = instance.parent_status.xyz + vec3<f32>(0.0, 0.035, 0.0);
        end = root + surface_normal * 0.04;
        let group_key = u32(round(instance.direction_species.y * 65535.0));
        let group_color = vec3<f32>(
            mix(0.18, 1.0, random01(group_key ^ 0xa511e9b3u)),
            mix(0.18, 1.0, random01(group_key ^ 0x63d83595u)),
            mix(0.18, 1.0, random01(group_key ^ 0xc2b2ae35u)),
        );
        start_color = min(group_color * 1.28, vec3<f32>(1.0));
        end_color = group_color;
        width_scale = mix(0.45, 0.9, instance.diagnostics.w);
    } else if (config.values.x == 3u) {
        start = root + surface_normal * 0.025;
        end = start + surface_normal * 0.18;
        let outcome = u32(instance.parent_status.w);
        if (outcome == 0u) {
            start_color = vec3<f32>(0.15, 1.0, 0.25);
        } else if (outcome == 1u) {
            start_color = vec3<f32>(0.15, 0.5, 1.0);
        } else if (outcome == 2u) {
            start_color = vec3<f32>(1.0, 0.1, 1.0);
        } else {
            start_color = vec3<f32>(1.0, 0.22, 0.05);
        }
        end_color = start_color;
        width_scale = select(0.65, 1.25, outcome == 0u);
    }

    var axis_delta = end - start;
    if (dot(axis_delta, axis_delta) < 1e-8) {
        axis_delta = surface_normal * 0.05;
        end = start + axis_delta;
    }
    let axis = normalize(axis_delta);
    let to_camera = normalize3_or(camera.camera_position.xyz - mix(start, end, 0.5), surface_normal);
    var side = cross(axis, to_camera);
    if (dot(side, side) < 1e-6) {
        side = vec3<f32>(-rest_direction.y, 0.0, rest_direction.x);
    } else {
        side = normalize(side);
    }
    let strip = diagnostic_quad_vertex(vertex_index);
    var width = 0.014 * width_scale;
    if (config.values.x == 1u) {
        width = mix(0.018, 0.006, strip.x) * width_scale;
    }
    let world_position = mix(start, end, strip.x) + side * strip.y * width;
    let clump_tint = mix(0.82, 1.14, instance.direction_species.y);

    var output: VertexOutput;
    output.clip_position = camera.clip_from_world * vec4<f32>(world_position, 1.0);
    output.color = mix(start_color, end_color, strip.x);
    if (config.values.x == 1u) {
        output.color *= clump_tint;
    }
    output.world_normal = surface_normal;
    output.world_position = world_position;
    output.blade_t = strip.x;
    output.material = vec4<f32>(1.0, 0.0, 1.0, 0.0);
    output.blade_u = strip.y * 0.5 + 0.5;
    output.surface_normal_clump = vec4<f32>(surface_normal, instance.direction_species.y);
    output.ribbon_side_rounding = vec4<f32>(side, 0.0);
    return output;
}
