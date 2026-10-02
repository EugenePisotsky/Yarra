#import bevy_pbr::{mesh_functions, view_transformations::position_world_to_clip}
#import bevy_pbr::mesh_view_bindings::view
#ifdef PREPASS_PIPELINE
#import bevy_pbr::prepass_io::VertexOutput
#else
#import bevy_pbr::forward_io::VertexOutput
#endif

// Bevy's vertex inputs for this pass, plus the branch card attributes
// (crates/engine/src/tree_wind/cards.rs) at locations 10-12.
#ifdef PREPASS_PIPELINE
struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
#ifdef VERTEX_UVS_A
    @location(1) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(2) uv_b: vec2<f32>,
#endif
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_NORMALS
    @location(3) normal: vec3<f32>,
#endif
#ifdef VERTEX_TANGENTS
    @location(4) tangent: vec4<f32>,
#endif
#endif
#ifdef VERTEX_COLORS
    @location(7) color: vec4<f32>,
#endif
#ifdef TREE_BRANCH_CARDS
    @location(10) card_pivot: vec3<f32>,
    @location(11) card_axis: vec3<f32>,
    @location(12) card_normal: vec3<f32>,
#ifdef TREE_CARD_FACING
    @location(13) card_facing: vec2<f32>,
#endif
#endif
}
#else
struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
#ifdef VERTEX_NORMALS
    @location(1) normal: vec3<f32>,
#endif
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(3) uv_b: vec2<f32>,
#endif
#ifdef VERTEX_TANGENTS
    @location(4) tangent: vec4<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
#ifdef TREE_BRANCH_CARDS
    @location(10) card_pivot: vec3<f32>,
    @location(11) card_axis: vec3<f32>,
    @location(12) card_normal: vec3<f32>,
#ifdef TREE_CARD_FACING
    @location(13) card_facing: vec2<f32>,
#endif
#endif
}
#endif

struct WindPose {
    field: vec4<f32>,
    phases: vec4<f32>,
    response: vec4<f32>,
    camera: vec4<f32>,
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

fn turn_axis(v: vec3<f32>, axis: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    return v * c + cross(axis, v) * sin(angle) + axis * dot(axis, v) * (1.0 - c);
}

// Yaw preserves the authored in-plane roll; elevation follow optionally tilts
// the surface to face elevated cameras. Full follow works at overhead angles too.
fn turn_camera(v: vec3<f32>, normal: vec3<f32>, to_view: vec3<f32>, follow: f32) -> vec3<f32> {
    if dot(to_view, to_view) < 1e-8 { return v; }
    let n = normalize(normal);
    let d = normalize(to_view);
    let up = vec3(0.0, 1.0, 0.0);
    var nh = vec3(n.x, 0.0, n.z);
    if dot(nh, nh) < 1e-12 { nh = vec3(0.0, 0.0, 1.0); }
    nh = normalize(nh);
    var dh = vec3(d.x, 0.0, d.z);
    if dot(dh, dh) < 1e-12 { dh = nh; }
    dh = normalize(dh);
    let yaw = atan2(dot(up, cross(nh, dh)), dot(nh, dh));
    let pitch = (atan2(n.y, length(n.xz)) - atan2(d.y, length(d.xz))) * clamp(follow, 0.0, 1.0);
    return turn_axis(turn_axis(v, up, yaw), normalize(cross(up, dh)), pitch);
}

// Use the main camera in shadows, and its previous pose for temporal motion.
fn facing_camera(world: vec4<f32>, model: mat4x4<f32>, vertex: Vertex, camera: vec4<f32>) -> vec4<f32> {
#ifdef TREE_BRANCH_CARDS
    if dot(vertex.card_axis, vertex.card_axis) < 0.25 {
        return world;  // a card that does not turn
    }
    let pivot = (model * vec4(vertex.card_pivot, 1.0)).xyz;
    let axis = normalize((model * vec4(vertex.card_axis, 0.0)).xyz);
    let rest = (model * vec4(vertex.card_normal, 0.0)).xyz;
    let rest_facing = rest - axis * dot(rest, axis);
    let to_view = select(view.lod_view_world_position, camera.xyz, camera.w > 0.5) - pivot;
#ifdef TREE_CARD_FACING
    if vertex.card_facing.x > 0.5 {
        return vec4(pivot + turn_camera(world.xyz - pivot, rest, to_view, vertex.card_facing.y), world.w);
    }
#endif
    let view_facing = to_view - axis * dot(to_view, axis);
    if dot(rest_facing, rest_facing) < 1e-8 || dot(view_facing, view_facing) < 1e-8 {
        return world;  // looking straight along the branch: any turn is as good
    }
    let a = normalize(rest_facing);
    let b = normalize(view_facing);
    let c = dot(a, b);
    let s = dot(axis, cross(a, b));
    let r = world.xyz - pivot;
    let turned = r * c + cross(axis, r) * s + axis * dot(axis, r) * (1.0 - c);
    return vec4(pivot + turned, world.w);
#else
    return world;
#endif
}

fn displaced_position(p: vec4<f32>, model: mat4x4<f32>, weights_in: vec2<f32>, pose: WindPose) -> vec4<f32> {
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
    let rest = mesh_functions::mesh_position_local_to_world(model, vec4(vertex.position, 1.0));
    out.world_position = displaced_position(facing_camera(rest, model, vertex, wind.current.camera), model, weights, wind.current);
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
    let previous_rest =
        mesh_functions::mesh_position_local_to_world(previous_model, vec4(vertex.position, 1.0));
    out.previous_world_position = displaced_position(
        facing_camera(previous_rest, previous_model, vertex, wind.previous.camera), previous_model, weights, wind.previous);
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
