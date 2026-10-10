// Tree wind materials' vertex shader (crates/engine/src/tree_wind/material.rs) for every pass:
// Bevy 0.20's mesh vertex path (bevy_pbr render/mesh.wesl, prepass/prepass.wesl) with the
// wind and branch cards applied. Tree meshes are neither skinned, morphed nor compressed
// (VERTEX_*_COMPRESSED), so only Bevy's plain attributes are read.
import bevy_pbr::{render::mesh_functions, render::view_transformations::position_world_to_clip};
import bevy_pbr::render::mesh_view_bindings::view;

// Trees drawn from one instance buffer (crates/engine/src/tree_wind/instancing.rs) rather than
// one mesh entity each: their transform and fade come from here, not from the mesh uniforms.
@if(TREE_INSTANCED)
struct TreeInstance {
    // Rows of the world-from-local transform.
    rows: array<vec4<f32>, 3>,
    tag: u32,
    flags: u32,
    padding: vec2<u32>,
}
@if(TREE_INSTANCED)
@group(4) @binding(0) var<storage, read> tree_instances: array<TreeInstance>;

fn world_from_local(instance_index: u32) -> mat4x4<f32> {
@if(TREE_INSTANCED) {
    let r = tree_instances[instance_index].rows;
    return transpose(mat4x4(r[0], r[1], r[2], vec4(0.0, 0.0, 0.0, 1.0)));
}
@else
    return mesh_functions::get_world_from_local(instance_index);
}

// Instanced trees are only turned and scaled uniformly, so their model matrix turns normals as
// it turns directions.
fn normal_to_world(normal: vec3<f32>, model: mat4x4<f32>, instance_index: u32) -> vec3<f32> {
@if(TREE_INSTANCED) {
    if any(normal != vec3(0.0)) {
        return normalize(mat3x3(model[0].xyz, model[1].xyz, model[2].xyz) * normal);
    }
    return normal;
}
@else
    return mesh_functions::mesh_normal_local_to_world(normal, instance_index);
}

fn tangent_to_world(tangent: vec4<f32>, model: mat4x4<f32>, instance_index: u32) -> vec4<f32> {
@if(TREE_INSTANCED) {
    if any(tangent != vec4(0.0)) {
        return vec4(normalize(mat3x3(model[0].xyz, model[1].xyz, model[2].xyz) * tangent.xyz),
            tangent.w);
    }
    return tangent;
}
@else
    return mesh_functions::mesh_tangent_local_to_world(model, tangent, instance_index);
}

fn mesh_tag(instance_index: u32) -> u32 {
@if(TREE_INSTANCED)
    return tree_instances[instance_index].tag;
@else
    return mesh_functions::get_tag(instance_index);
}
// Match Bevy 0.20 vertex-output locations, but require invariant clip positions.
// Colour and depth compile separately; wind arithmetic must not be reassociated
// differently or opaque fragments can fail their own prepass depth test.
@if(PREPASS_PIPELINE)
struct VertexOutput {
    // This is `clip position` when the struct is used as a vertex stage output
    // and `frag coord` when used as a fragment stage input
    @builtin(position) @invariant position: vec4<f32>,

    @if(VERTEX_UVS_A)
    @location(0) uv: vec2<f32>,

    @if(VERTEX_UVS_B)
    @location(1) uv_b: vec2<f32>,

    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS)
    @location(2) world_normal: vec3<f32>,
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS)
    @location(3) world_tangent: vec4<f32>,

    @location(4) world_position: vec4<f32>,
    @if(MOTION_VECTOR_PREPASS)
    @location(5) previous_world_position: vec4<f32>,

    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION)
    @location(6) unclipped_depth: f32,
    @if(VERTEX_OUTPUT_INSTANCE_INDEX)
    @location(7) @interpolate(flat) instance_index: u32,

    @if(VERTEX_COLORS)
    @location(8) color: vec4<f32>,

    @if(VISIBILITY_RANGE_DITHER)
    @location(9) @interpolate(flat) visibility_range_dither: i32,
}
@else
struct VertexOutput {
    // This is `clip position` when the struct is used as a vertex stage output
    // and `frag coord` when used as a fragment stage input
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @if(VERTEX_UVS_A)
    @location(2) uv: vec2<f32>,
    @if(VERTEX_UVS_B)
    @location(3) uv_b: vec2<f32>,
    @if(VERTEX_TANGENTS)
    @location(4) world_tangent: vec4<f32>,
    @if(VERTEX_COLORS)
    @location(5) color: vec4<f32>,
    @if(VERTEX_OUTPUT_INSTANCE_INDEX)
    @location(6) @interpolate(flat) instance_index: u32,
    @if(VISIBILITY_RANGE_DITHER)
    @location(7) @interpolate(flat) visibility_range_dither: i32,
}

// Bevy's vertex inputs for this pass, plus the branch card attributes
// (crates/engine/src/tree_wind/cards.rs) at locations 10-13 and the wind hierarchy's at 14-15.
@if(PREPASS_PIPELINE)
struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @if(TREE_HIERARCHY)
    @location(14) wind_pivot: vec4<f32>,
    @if(TREE_HIERARCHY)
    @location(15) wind_axis: vec4<f32>,
    @if(VERTEX_UVS_A)
    @location(1) uv: vec2<f32>,
    @if(VERTEX_UVS_B)
    @location(2) uv_b: vec2<f32>,
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS)
    @location(3) normal: vec3<f32>,
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS)
    @location(4) tangent: vec4<f32>,
    @if(VERTEX_COLORS)
    @location(7) color: vec4<f32>,
    @if(TREE_BRANCH_CARDS)
    @location(10) card_pivot: vec3<f32>,
    @if(TREE_BRANCH_CARDS)
    @location(11) card_axis: vec3<f32>,
    @if(TREE_BRANCH_CARDS)
    @location(12) card_normal: vec3<f32>,
    @if(TREE_BRANCH_CARDS && TREE_CARD_FACING)
    @location(13) card_facing: vec2<f32>,
}
@else
struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @if(TREE_HIERARCHY)
    @location(14) wind_pivot: vec4<f32>,
    @if(TREE_HIERARCHY)
    @location(15) wind_axis: vec4<f32>,
    @if(VERTEX_NORMALS)
    @location(1) normal: vec3<f32>,
    @if(VERTEX_UVS_A)
    @location(2) uv: vec2<f32>,
    @if(VERTEX_UVS_B)
    @location(3) uv_b: vec2<f32>,
    @if(VERTEX_TANGENTS)
    @location(4) tangent: vec4<f32>,
    @if(VERTEX_COLORS)
    @location(5) color: vec4<f32>,
    @if(TREE_BRANCH_CARDS)
    @location(10) card_pivot: vec3<f32>,
    @if(TREE_BRANCH_CARDS)
    @location(11) card_axis: vec3<f32>,
    @if(TREE_BRANCH_CARDS)
    @location(12) card_normal: vec3<f32>,
    @if(TREE_BRANCH_CARDS && TREE_CARD_FACING)
    @location(13) card_facing: vec2<f32>,
}

struct WindPose {
    field: vec4<f32>,
    phases: vec4<f32>,
    response: vec4<f32>,
    camera: vec4<f32>,
    hierarchy: vec4<f32>,
    sway_phases: vec4<f32>,
}
struct WindFrames {
    current: WindPose,
    previous: WindPose,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(100)
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
@if(TREE_BRANCH_CARDS) {
    if dot(vertex.card_axis, vertex.card_axis) < 0.25 {
        return world;  // a card that does not turn
    }
    let pivot = (model * vec4(vertex.card_pivot, 1.0)).xyz;
    let axis = normalize((model * vec4(vertex.card_axis, 0.0)).xyz);
    let rest = (model * vec4(vertex.card_normal, 0.0)).xyz;
    let rest_facing = rest - axis * dot(rest, axis);
    let to_view = select(view.lod_view_world_position, camera.xyz, camera.w > 0.5) - pivot;
@if(TREE_CARD_FACING)
    if vertex.card_facing.x > 0.5 {
        return vec4(pivot + turn_camera(world.xyz - pivot, rest, to_view, vertex.card_facing.y), world.w);
    }
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
}
@else
    return world;
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

// Structural wind helpers. All positions, pivots and directions share this pose.
struct SwayFrame {
    source: vec3<f32>,
    deformed_anchor: vec3<f32>,
    trunk_axis: vec3<f32>,
    trunk_angle: f32,
    branch_axis: vec3<f32>,
    branch_angle: f32,
}

fn sway_vector(v: vec3<f32>, frame: SwayFrame) -> vec3<f32> {
    return turn_axis(turn_axis(v, frame.branch_axis, frame.branch_angle), frame.trunk_axis, frame.trunk_angle);
}
fn sway_point(p: vec3<f32>, frame: SwayFrame) -> vec3<f32> {
    return frame.deformed_anchor + sway_vector(p - frame.source, frame);
}

// Four coherent bands with the amplitude/phase response of a damped oscillator.
// This is an artistic approximation, not a fitted Kaimal spectrum. Species retain
// their own response period when wind strength changes; damping prevents ringing.
fn sway_signal(model: mat4x4<f32>, pose: WindPose, period: f32, detail: f32) -> f32 {
    let frequencies = vec4(0.55, 1.3, 2.7, 4.3);
    let spatial = vec4(0.035, 0.07, 0.14, 0.28);
    let phase = pose.sway_phases + dot(model[3].xz, pose.field.xy) * spatial + vec4(detail);
    let ratio = frequencies * max(0.5, period * pose.hierarchy.w) / 6.2831853;
    let spring = vec4(1.0) - ratio * ratio;
    let damping = 1.5 * ratio;
    let gain = inverseSqrt(spring * spring + damping * damping);
    let lag = atan2(damping, spring);
    // Phases travel backwards in time, so lag is added here.
    return dot(sin(phase + lag) * gain, vec4(0.65, 0.38, 0.19, 0.09));
}

fn structural_frame(local_anchor: vec3<f32>, limb_axis: vec4<f32>, height: f32,
                    model: mat4x4<f32>, pose: WindPose, profile: vec4<f32>) -> SwayFrame {
    let up = normalize(model[1].xyz);
    let direction = vec3(pose.field.x, 0.0, pose.field.y);
    var bend_axis = cross(up, direction);
    if dot(bend_axis, bend_axis) < 1e-8 { bend_axis = vec3(0.0, 0.0, 1.0); }
    bend_axis = normalize(bend_axis);
    let pressure = pose.field.w * pose.field.w;
    let wave = sway_signal(model, pose, profile.y, 0.0);
    let lean = clamp(0.11 * pressure * profile.x * profile.w * pose.hierarchy.x
                    * (1.0 + pose.response.x * wave), -0.15, 0.8);
    let tree_height = max(0.1, height * length(model[1].xyz));
    let y = max(0.0, local_anchor.y) * length(model[1].xyz);
    let angle = lean * clamp(y / tree_height, 0.0, 1.0);
    let curvature = lean / tree_height;
    var center = up * y;
    if abs(curvature) > 1e-6 {
        center = up * (sin(angle) / curvature)
                 + cross(bend_axis, up) * ((1.0 - cos(angle)) / curvature);
        // Sprays can extend beyond the authored stem height. Continue along the
        // tip tangent rather than collapsing those anchors onto the treetop.
        center += turn_axis(up, bend_axis, angle) * max(0.0, y - tree_height);
    }
    let source = (model * vec4(local_anchor, 1.0)).xyz;
    // Preserve buried feet and existing trunk lean/curvature in the rest shape.
    let lateral = source - model[3].xyz - up * y;
    let deformed_anchor = model[3].xyz + center + turn_axis(lateral, bend_axis, angle);
    var branch_axis = bend_axis;
    var branch_angle = 0.0;
    if limb_axis.w > 0.0 {
        let axis = normalize((model * vec4(limb_axis.xyz, 0.0)).xyz);
        let force_axis = cross(axis, direction);
        let lever = length(force_axis);
        if lever > 1e-5 { branch_axis = force_axis / lever; }
        let phase = dot(local_anchor, vec3(0.73, 0.41, -0.57));
        branch_angle = clamp(0.10 * pressure * profile.w * limb_axis.w * pose.hierarchy.y * lever
            * (0.6 + pose.response.x * sway_signal(model, pose, profile.y * 0.35, phase)), -0.5, 0.5);
    }
    return SwayFrame(source, deformed_anchor, bend_axis, angle, branch_axis, branch_angle);
}
// End structural helpers.

@if(TREE_HIERARCHY)
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var<uniform> tree_profile: vec4<f32>;

@if(TREE_HIERARCHY)
fn vertex_frame(vertex: Vertex, model: mat4x4<f32>, pose: WindPose) -> SwayFrame {
    var anchor = vertex.wind_pivot.xyz;
    if vertex.wind_axis.w == 0.0 {
        anchor = vertex.position;
@if(TREE_BRANCH_CARDS)
        anchor = vertex.card_pivot;
    }
    return structural_frame(anchor, vertex.wind_axis, vertex.wind_pivot.w, model, pose, tree_profile);
}

fn animated_position(vertex: Vertex, model: mat4x4<f32>, pose: WindPose) -> vec4<f32> {
    var p = model * vec4(vertex.position, 1.0);
@if(TREE_HIERARCHY) {
    let frame = vertex_frame(vertex, model, pose);
    p = vec4(sway_point(p.xyz, frame), 1.0);
@if(TREE_BRANCH_CARDS) {
    let pivot = sway_point((model * vec4(vertex.card_pivot, 1.0)).xyz, frame);
    let axis = sway_vector((model * vec4(vertex.card_axis, 0.0)).xyz, frame);
    let normal = sway_vector((model * vec4(vertex.card_normal, 0.0)).xyz, frame);
    if dot(axis, axis) > 0.25 {
        let to_view = select(view.lod_view_world_position, pose.camera.xyz, pose.camera.w > 0.5) - pivot;
        // Cards that face the camera (TREE_CARD_FACING) turn toward it; the rest about their axis.
@if(TREE_CARD_FACING)
        let turns_to_camera = vertex.card_facing.x > 0.5;
@else
        let turns_to_camera = false;
        if turns_to_camera {
@if(TREE_CARD_FACING)
            p = vec4(pivot + turn_camera(p.xyz - pivot, normal, to_view, vertex.card_facing.y), 1.0);
        } else {
            let a = normalize(axis);
            let rest = normal - a * dot(normal, a);
            let facing = to_view - a * dot(to_view, a);
            if dot(rest, rest) > 1e-8 && dot(facing, facing) > 1e-8 {
                let angle = atan2(dot(a, cross(normalize(rest), normalize(facing))), dot(normalize(rest), normalize(facing)));
                p = vec4(pivot + turn_axis(p.xyz - pivot, a, angle), 1.0);
            }
        }
    }
@if(VERTEX_UVS_B) {
    // Entire cards rock a little around their own attachment. A zero-length
    // root vector stays exactly zero, including V-card seams and bare twig cards.
    let leaf = select(0.35, 1.0, vertex.uv_b.x > 0.0);
    let flutter = sin(dot(vertex.card_pivot, vec3(1.91, 1.13, -1.37)) + pose.phases.w
                      + dot(model[3].xz, vec2(1.91, -1.37)))
                  * 0.035 * pose.field.w * pose.hierarchy.z * tree_profile.z * leaf;
    p = vec4(pivot + turn_axis(p.xyz - pivot, frame.trunk_axis, flutter), 1.0);
}
}
    return p;
}
@else {
    var weights = vec2(0.0);
@if(VERTEX_UVS_B)
    weights = vertex.uv_b;
    return displaced_position(facing_camera(p, model, vertex, pose.camera), model, weights, pose);
}
}

fn animated_vector(v: vec3<f32>, vertex: Vertex, model: mat4x4<f32>, pose: WindPose) -> vec3<f32> {
@if(TREE_HIERARCHY)
    return sway_vector(v, vertex_frame(vertex, model, pose));
@else
    return v;
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let model = world_from_local(vertex.instance_index);
    out.world_position = animated_position(vertex, model, wind.current);
    out.position = position_world_to_clip(out.world_position.xyz);

@if(UNCLIPPED_DEPTH_ORTHO_EMULATION) {
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
}
@if(VERTEX_UVS_A)
    out.uv = vertex.uv;
@if(VERTEX_UVS_B)
    out.uv_b = vertex.uv_b;
@if(VERTEX_COLORS)
    out.color = vertex.color;

// Depth, shadows and colour deliberately use the same wind pose. Damping the
// entire prepass (as the old renderer did for shadows) breaks temporal depth.
@if(PREPASS_PIPELINE) {
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS)
    out.world_normal = animated_vector(normal_to_world(vertex.normal, model, vertex.instance_index), vertex, model, wind.current);
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS) {
    let tangent = tangent_to_world(vertex.tangent, model, vertex.instance_index);
    out.world_tangent = vec4(animated_vector(tangent.xyz, vertex, model, wind.current), tangent.w);
}
@if(MOTION_VECTOR_PREPASS) {
@if(TREE_INSTANCED)
    let previous_model = model;
@else
    let previous_model = mesh_functions::get_previous_world_from_local(vertex.instance_index);
    out.previous_world_position = animated_position(vertex, previous_model, wind.previous);
}
}
@else {
@if(VERTEX_NORMALS)
    out.world_normal = animated_vector(normal_to_world(vertex.normal, model, vertex.instance_index), vertex, model, wind.current);
@if(VERTEX_TANGENTS) {
    let tangent = tangent_to_world(vertex.tangent, model, vertex.instance_index);
    out.world_tangent = vec4(animated_vector(tangent.xyz, vertex, model, wind.current), tangent.w);
}
}

@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex.instance_index;
@if(VISIBILITY_RANGE_DITHER) {
    // LODs faded over time (crates/engine/src/object_lod.rs) carry their dither level in
    // the mesh tag, as 64 + level, in every pass including the shadow cascades. Others keep
    // Bevy's crossfade by distance; passes Bevy gives no ranges (TREE_TAG_FADE) draw whole.
    let tag = mesh_tag(vertex.instance_index);
    if tag != 0u {
        var level = i32(tag) - 64;
@if(TREE_TAG_FADE)
        // Shadows overlap instead: the incoming LOD's is whole by mid-fade and the outgoing
        // one's stays whole until then, so a tree's shadow never thins while it dissolves.
        if level < 0 {
            level = min(0, 2 * level + 16);
        } else {
            level = max(0, 2 * level - 16);
        }
        out.visibility_range_dither = level;
    } else {
@if(TREE_TAG_FADE || TREE_INSTANCED)
        // Instanced trees have no visibility ranges of their own either.
        out.visibility_range_dither = 0;
@else
        out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex.instance_index, model[3]);
    }
}
    return out;
}
