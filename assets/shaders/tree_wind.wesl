// Tree wind materials' vertex shader (crates/engine/src/tree_wind/material.rs) for every pass:
// Bevy 0.20's mesh vertex path (bevy_pbr render/mesh.wesl, prepass/prepass.wesl) with the
// wind and branch cards applied. Tree meshes are neither skinned, morphed nor compressed
// (VERTEX_*_COMPRESSED), so only Bevy's plain attributes are read.
import bevy_pbr::{render::mesh_functions, render::view_transformations::position_world_to_clip};
import bevy_pbr::render::mesh_view_bindings::view;
import package::shaders::tree::wind::{TreeVertex, WindFrames, animated_position, animated_vector};

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

@group(constants::MATERIAL_BIND_GROUP) @binding(100)
var<storage, read> wind: WindFrames;

@if(TREE_HIERARCHY)
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var<uniform> tree_profile: vec4<f32>;

// The tree's wind profile (tree_wind.rs); meshes without a wind hierarchy have none.
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
    out.world_position = animated_position(tree, model, wind.current, profile(), view.lod_view_world_position);
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
    out.world_normal = animated_vector(normal_to_world(vertex.normal, model, vertex.instance_index), tree, model, wind.current, profile());
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS) {
    let tangent = tangent_to_world(vertex.tangent, model, vertex.instance_index);
    out.world_tangent = vec4(animated_vector(tangent.xyz, tree, model, wind.current, profile()), tangent.w);
}
@if(MOTION_VECTOR_PREPASS) {
@if(TREE_INSTANCED)
    let previous_model = model;
@else
    let previous_model = mesh_functions::get_previous_world_from_local(vertex.instance_index);
    out.previous_world_position = animated_position(tree, previous_model, wind.previous, profile(), view.lod_view_world_position);
}
}
@else {
@if(VERTEX_NORMALS)
    out.world_normal = animated_vector(normal_to_world(vertex.normal, model, vertex.instance_index), tree, model, wind.current, profile());
@if(VERTEX_TANGENTS) {
    let tangent = tangent_to_world(vertex.tangent, model, vertex.instance_index);
    out.world_tangent = vec4(animated_vector(tangent.xyz, tree, model, wind.current, profile()), tangent.w);
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
