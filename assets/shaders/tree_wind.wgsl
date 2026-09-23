#import bevy_pbr::{mesh_functions, view_transformations::position_world_to_clip}
#ifdef PREPASS_PIPELINE
#import bevy_pbr::prepass_io::{Vertex, VertexOutput}
#else
#import bevy_pbr::forward_io::{Vertex, VertexOutput}
#endif

struct WindPose {
    field: vec4<f32>,
    phases: vec4<f32>,
    response: vec4<f32>,
}
struct WindFrames {
    current: WindPose,
    previous: WindPose,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<storage, read> wind: WindFrames;

// Matches VegetationWind's coherent travelling field. All independent phase
// offsets include the floating origin and time, reduced on the CPU in f64.
fn field_force(anchor: vec2<f32>, pose: WindPose) -> f32 {
    let direction = pose.field.xy;
    let perpendicular = vec2(-direction.y, direction.x);
    let broad_local = dot(anchor, direction) * pose.field.z;
    let cross_local = dot(anchor, perpendicular) * pose.field.z * 0.71;
    let broad = broad_local + pose.phases.x;
    let cross = cross_local + pose.phases.y;
    let gust = broad_local * 0.43 - cross_local * 0.61 + pose.phases.z;
    let broad_amount = sin(broad + sin(cross) * 0.85) * 0.5 + 0.5;
    let gust_coordinate = clamp(sin(gust) * 0.5 + 0.5, 0.0, 1.0);
    let rise = clamp((gust_coordinate - 0.28) / 0.72, 0.0, 1.0);
    let pulse = rise * rise * (3.0 - 2.0 * rise);
    return pose.field.w * clamp(0.25 + broad_amount * 0.18 + pulse * pose.response.x * 0.90, 0.12, 1.30);
}

fn displaced_position(local: vec3<f32>, model: mat4x4<f32>, weights_in: vec2<f32>, pose: WindPose) -> vec4<f32> {
    let p = mesh_functions::mesh_position_local_to_world(model, vec4(local, 1.0));
    let weights = clamp(weights_in, vec2(0.0), vec2(1.0));
    let direction = pose.field.xy;
    let perpendicular = vec2(-direction.y, direction.x);
    let flutter_wave = sin(dot(p.xz, vec2(1.91, -1.37)) + p.y * 1.13 + pose.phases.w);
    let branch = pose.response.y * weights.y * field_force(model[3].xz, pose);
    let flutter = pose.response.z * weights.x * pose.field.w * flutter_wave;
    let horizontal = direction * branch + perpendicular * flutter;
    return p + vec4(horizontal.x, flutter * 0.16, horizontal.y, 0.0);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let model = mesh_functions::get_world_from_local(vertex.instance_index);
    var weights = vec2(0.0);
#ifdef VERTEX_UVS_B
    weights = vertex.uv_b;
#endif
    out.world_position = displaced_position(vertex.position, model, weights, wind.current);
    out.position = position_world_to_clip(out.world_position.xyz);

#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

// Depth, shadows and colour deliberately use the same wind pose. Damping the
// entire prepass (as the old renderer did for shadows) breaks temporal depth.
#ifdef PREPASS_PIPELINE
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(model, vertex.tangent, vertex.instance_index);
#endif
#endif
#ifdef MOTION_VECTOR_PREPASS
    let previous_model = mesh_functions::get_previous_world_from_local(vertex.instance_index);
    out.previous_world_position = displaced_position(vertex.position, previous_model, weights, wind.previous);
#endif
#else
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(model, vertex.tangent, vertex.instance_index);
#endif
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex.instance_index, model[3]);
#endif
    return out;
}
