// Tree wind materials' vertex shader (crates/engine/src/trees/material.rs) for every pass:
// Bevy 0.20's mesh vertex path (bevy_pbr render/mesh.wesl, prepass/prepass.wesl) with the
// wind, characters' pushes (tree/touch.wesl) and branch cards applied. Tree meshes are neither skinned, morphed nor compressed
// (VERTEX_*_COMPRESSED), so only Bevy's plain attributes are read.
import bevy_pbr::{render::mesh_functions, render::view_transformations::position_world_to_clip};
import bevy_pbr::render::mesh_view_bindings::view;
import package::shaders::tree::wind::{TreeVertex, WindFrames, animated_position, animated_vector};
import package::shaders::tree::touch::{TouchWindow, touch_canonical, touch_inside, touch_moving, touch_uv, within};

// Trees drawn by the GPU trees (crates/engine/src/trees/gpu.rs) rather than one mesh entity
// each: `instance_index` picks this draw's record (the instance and its dither level), and
// the instance's transform comes from the frame's transforms (shaders/tree/lod.wesl), not
// from the mesh uniforms.
@if(TREE_INSTANCED)
@group(4) @binding(0) var<storage, read> tree_records: array<u32>;
@if(TREE_INSTANCED)
@group(4) @binding(3) var<storage, read> tree_transforms: array<array<vec4<f32>, 3>>;

fn world_from_local(instance_index: u32) -> mat4x4<f32> {
@if(TREE_INSTANCED) {
    let r = tree_transforms[tree_records[instance_index] & 0xffffffu];
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
    // The record's dither level + 16 (bits 24-29), as the mesh tag's 64 + level.
    return 48u + ((tree_records[instance_index] >> 24u) & 63u);
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
// (crates/engine/src/trees/cards.rs) at locations 10-13 and the wind hierarchy's at 14-15.
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

@group(constants::MATERIAL_BIND_GROUP) @binding(100)
var<storage, read> wind: WindFrames;

@if(TREE_HIERARCHY)
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var<uniform> tree_profile: vec4<f32>;

// The touch field around the player (crates/engine/src/trees/touch.rs) and where it lies.
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var touch_field: texture_3d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(107) var touch_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(108)
var<storage, read> touch_window: TouchWindow;

// How characters push foliage resting at render-space `p` aside, in world metres; last frame's
// push for `previous`, from the spring's velocity.
fn touch_push(window: TouchWindow, p: vec3<f32>, previous: bool) -> vec3<f32> {
    let q = touch_canonical(window, p);
    let inside = touch_inside(window, q);
    if inside <= 0.0 {
        return vec3(0.0);
    }
    let spring = textureSampleLevel(touch_field, touch_sampler, touch_uv(window, q), 0.0);
    var push = spring.xy;
    if previous {
        push -= spring.zw * window.sides.z;
    }
    return vec3(push.x, 0.0, push.y) * inside;
}

// The turn (axis times angle, radians) about its root with which characters push this vertex's
// limb aside: the largest that points of the limb between its root and the vertex need to clear
// them, so a limb pushed anywhere swings whole beyond the push, and foliage beside it follows.
fn touch_turn(tree: TreeVertex, model: mat4x4<f32>, previous: bool) -> vec3<f32> {
@if(TREE_HIERARCHY) {
    let window = touch_window;
    if tree.wind_axis.w == 0.0 || window.centre.w <= 0.0 {
        return vec3(0.0);
    }
    // Only trees within a limb's length of the moving voxels move.
    let moving = touch_moving(window, 0.0);
    if !within(touch_moving(window, TOUCH_LIMB), touch_canonical(window, model[3].xyz).xz) {
        return vec3(0.0);
    }
    let root = (model * vec4(tree.wind_pivot.xyz, 1.0)).xyz;
    let along = normalize((model * vec4(tree.wind_axis.xyz, 0.0)).xyz);
    let point = (model * vec4(tree.position, 1.0)).xyz;
    let span = max(dot(point - root, along), 0.0);
    var turn = vec3(0.0);
    for (var k = 1u; k <= TOUCH_SAMPLES; k += 1u) {
        var at = root + along * (span * f32(k) / f32(TOUCH_SAMPLES));
        if k == TOUCH_SAMPLES {
            at = point;
        }
        let lever = at - root;
        let length2 = dot(lever, lever);
        if length2 < 1e-4 || !within(moving, touch_canonical(window, at).xz) {
            continue;
        }
        let push = touch_push(window, at, previous);
        let across = push - lever * (dot(push, lever) / length2);
        // |cross| is |lever| |across|: the angle that moves `at` by `across`.
        let candidate = cross(lever, across) / length2;
        if dot(candidate, candidate) > dot(turn, turn) {
            turn = candidate;
        }
    }
    return turn;
}
@else
    return vec3(0.0);
}

// Points along a limb, the vertex last, at which its push is read.
const TOUCH_SAMPLES: u32 = 4u;
// Longest a limb reaches from its tree's trunk, metres.
const TOUCH_LIMB: f32 = 6.0;

// The tree's wind profile (trees/wind.rs); meshes without a wind hierarchy have none.
fn profile() -> vec4<f32> {
@if(TREE_HIERARCHY)
    return tree_profile;
@else
    return vec4(0.0);
}

// What the wind reads of this pass's vertex (tree/wind.wesl).
fn tree_vertex(vertex: Vertex) -> TreeVertex {
    var tree: TreeVertex;
    tree.position = vertex.position;
@if(TREE_HIERARCHY) {
    tree.wind_pivot = vertex.wind_pivot;
    tree.wind_axis = vertex.wind_axis;
}
@if(VERTEX_UVS_B)
    tree.uv_b = vertex.uv_b;
@if(TREE_BRANCH_CARDS) {
    tree.card_pivot = vertex.card_pivot;
    tree.card_axis = vertex.card_axis;
    tree.card_normal = vertex.card_normal;
}
@if(TREE_BRANCH_CARDS && TREE_CARD_FACING)
    tree.card_facing = vertex.card_facing;
    return tree;
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let model = world_from_local(vertex.instance_index);
    let tree = tree_vertex(vertex);
    let turn = touch_turn(tree, model, false);
    out.world_position = animated_position(tree, model, wind.current, profile(), view.lod_view_world_position, turn);
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
    out.world_normal = animated_vector(normal_to_world(vertex.normal, model, vertex.instance_index), tree, model, wind.current, profile(), turn);
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS) {
    let tangent = tangent_to_world(vertex.tangent, model, vertex.instance_index);
    out.world_tangent = vec4(animated_vector(tangent.xyz, tree, model, wind.current, profile(), turn), tangent.w);
}
@if(MOTION_VECTOR_PREPASS) {
@if(TREE_INSTANCED)
    let previous_model = model;
@else
    let previous_model = mesh_functions::get_previous_world_from_local(vertex.instance_index);
    let previous_turn = touch_turn(tree, previous_model, true);
    out.previous_world_position = animated_position(tree, previous_model, wind.previous, profile(), view.lod_view_world_position, previous_turn);
}
}
@else {
@if(VERTEX_NORMALS)
    out.world_normal = animated_vector(normal_to_world(vertex.normal, model, vertex.instance_index), tree, model, wind.current, profile(), turn);
@if(VERTEX_TANGENTS) {
    let tangent = tangent_to_world(vertex.tangent, model, vertex.instance_index);
    out.world_tangent = vec4(animated_vector(tangent.xyz, tree, model, wind.current, profile(), turn), tangent.w);
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
