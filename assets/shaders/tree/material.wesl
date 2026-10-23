// Tree wind materials' vertex shader (crates/engine/src/trees/material.rs) for every pass:
// Bevy 0.20's mesh vertex path (bevy_pbr render/mesh.wesl, prepass/prepass.wesl) with the
// wind, characters' pushes (tree/touch.wesl), branch cards, snow (tree/snow.wesl) and the snow
// characters shake off (tree/shed.wesl) applied. Tree meshes are neither skinned, morphed nor
// compressed (VERTEX_*_COMPRESSED), so only Bevy's plain attributes are read.
import bevy_pbr::{render::mesh_functions, render::view_transformations::position_world_to_clip};
import bevy_pbr::render::mesh_view_bindings::view;
import package::shaders::tree::wind::{TreePose, TreeVertex, TreeWind, WindFrames, WindPose, tree_pose, tree_wind, turn_axis};
import package::shaders::tree::touch::{TouchWindow, touch_canonical, touch_inside, touch_moving, touch_uv, within};
import package::shaders::tree::instance::{Instance, InstanceFrame};
import package::shaders::tree::shed::{ChunkFall, CHUNK_CRUMBLE, LimbShed, ShedTrees, SHED_NONE, UNLOAD_LIFT, chunk_fall, limb_shed, shed_fallen, shed_slot, unload_rise};
@if(TREE_SNOW) import package::shaders::tree::snow::{card_amount, seen_from_above};

// Trees drawn by the GPU trees (crates/engine/src/trees/gpu.rs) rather than one mesh entity
// each: `instance_index` picks this draw's record (the instance and its dither level), and
// the instance's transform and wind come from its frame (shaders/tree/lod.wesl), not
// from the mesh uniforms.
@if(TREE_INSTANCED)
@group(4) @binding(0) var<storage, read> tree_records: array<u32>;
@if(TREE_INSTANCED)
@group(4) @binding(1) var<storage, read> tree_instances: array<Instance>;
@if(TREE_INSTANCED)
@group(4) @binding(3) var<storage, read> tree_frames: array<InstanceFrame>;
// Each snowy tree's limbs' states and what reads them (crates/engine/src/trees/gpu/shed.rs).
@if(TREE_INSTANCED)
@group(4) @binding(4) var<storage, read> shed_states: array<vec4<f32>>;
@if(TREE_INSTANCED)
@group(4) @binding(5) var<storage, read> shed_trees: ShedTrees;

fn world_from_local(instance_index: u32) -> mat4x4<f32> {
@if(TREE_INSTANCED) {
    let r = tree_frames[tree_records[instance_index] & 0xffffffu].transform;
    return transpose(mat4x4(r[0], r[1], r[2], vec4(0.0, 0.0, 0.0, 1.0)));
}
@else
    return mesh_functions::get_world_from_local(instance_index);
}

// The wind at this vertex's tree (tree/wind.wesl tree_wind) in `pose`, this frame's or the
// `previous` one: an instanced tree's from its frame, which all its vertices share.
fn instance_wind(instance_index: u32, model: mat4x4<f32>, pose: WindPose, previous: bool) -> TreeWind {
@if(TREE_INSTANCED)
    return tree_frames[tree_records[instance_index] & 0xffffffu].wind[u32(previous)];
@else
    return tree_wind(model[3].xz, pose);
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
// (crates/engine/src/trees/cards.rs) at locations 10-13, the wind hierarchy's at 14-15 and a snow
// load chunk's at 9.
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
    @if(TREE_SNOW_CHUNKS)
    @location(9) snow_chunk: vec4<f32>,
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
    @if(TREE_SNOW_CHUNKS)
    @location(9) snow_chunk: vec4<f32>,
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

// This vertex's limb (tree/shed.wesl `limb_shed`): snowy trees number each vertex's primary limb
// in uv_b.y (-1 the trunk). Only the GPU trees hold states; others hold their snow.
fn limb_state(tree: TreeVertex, instance_index: u32) -> LimbShed {
    let holding = LimbShed(-1.0, vec2(0.0), 0.0, 1.0);
@if(TREE_INSTANCED && VERTEX_UVS_B) {
    let pool = tree_records[instance_index] & 0xffffffu;
    let batch = shed_trees.batches[tree_instances[pool].origin];
    let limb = i32(round(tree.uv_b.y));
    if batch.x == SHED_NONE || limb < 0 || u32(limb) >= batch.z {
        return holding;
    }
    return limb_shed(shed_trees.now, shed_states[shed_slot(batch, pool, u32(limb))]);
}
@else
    return holding;
}

// The frame's time step, by which motion vectors see a shed limb a moment earlier.
fn shed_step() -> f32 {
@if(TREE_INSTANCED)
    return shed_trees.now.y;
@else
    return 0.0;
}

// The turn (axis times angle) with which a limb springs up to its unloaded pose once it shed,
// `back` seconds ago, about its root, across its axis, past a wobble; and sinks back as its snow
// settles back.
fn unload_turn(tree: TreeVertex, model: mat4x4<f32>, shed: LimbShed, back: f32) -> vec3<f32> {
@if(TREE_HIERARCHY) {
    let t = shed.since - back;
    if t < 0.0 || tree.wind_axis.w == 0.0 {
        return vec3(0.0);
    }
    let side = cross((model * vec4(tree.wind_axis.xyz, 0.0)).xyz, vec3(0.0, 1.0, 0.0));
    if dot(side, side) < 1e-8 {
        return vec3(0.0);
    }
    return normalize(side) * (UNLOAD_LIFT * shed.share * unload_rise(t) * (1.0 - shed.regrown));
}
@else
    return vec3(0.0);
}

// A snow load's chunk as its limb's snow falls: the chunk's middle where the bough holds it, and
// its fall from there. A chunk whose limb holds its snow does not fall.
struct Dropping {
    anchor: vec3<f32>,
    fall: ChunkFall,
}

// How the chunk `chunk` (middle xyz, seed w) of a limb drops, `back` seconds ago, its card posed
// as `posed` holds the vertex `tree`. It crumbles at once on reaching the ground beneath its tree;
// once crumbled, the snow settling back builds it up again where it lay.
@if(TREE_SNOW_CHUNKS)
fn dropping(tree: TreeVertex, chunk: vec4<f32>, model: mat4x4<f32>, posed: TreePose, shed: LimbShed,
            back: f32) -> Dropping {
    var out: Dropping;
    out.anchor = vec3(0.0);
    out.fall = ChunkFall(vec3(0.0), vec3(0.0, 0.0, 1.0), 0.0, 1.0);
    let t = shed.since - back;
    if t < 0.0 {
        return out;
    }
    // The chunk's middle is a point of the vertex's card.
    out.anchor = posed.position.xyz + posed.points * (model * vec4(chunk.xyz - tree.position, 0.0)).xyz;
    if t >= CHUNK_CRUMBLE.y {
        out.fall.scale = shed.regrown;
        return out;
    }
    out.fall = chunk_fall(chunk.w, shed, t);
    out.fall.scale *= smoothstep(0.0, 0.15, out.anchor.y + out.fall.offset.y - model[3].y);
    return out;
}

fn dropped(drop: Dropping, p: vec3<f32>) -> vec3<f32> {
    return drop.anchor + drop.fall.offset + turn_axis(p - drop.anchor, drop.fall.axis, drop.fall.angle) * drop.fall.scale;
}

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

// How much snow a snowy card holds and how much of it shows as from above (tree/snow.wesl
// `seen_from_above`), for its fragments; for bark, the share of its snow left. The card's own
// normal is the side it faces up with; cards that turn toward the camera show their snow from the
// side, and only their front. Snow characters shook off has gone (tree/shed.wesl).
@if(TREE_SNOW)
fn card_snow(tree: TreeVertex, model: mat4x4<f32>, sky: f32, shed: LimbShed) -> vec2<f32> {
    let held = max(1.0 - shed_fallen(shed.since), shed.regrown);
@if(TREE_BRANCH_CARDS) {
    if dot(tree.card_axis, tree.card_axis) >= 0.25 {
        return vec2(card_amount(sky, 1.0) * held, 1.0);
    }
    let normal = normalize((model * vec4(tree.card_normal, 0.0)).xyz);
    let to_camera = view.world_position - (model * vec4(tree.card_pivot, 1.0)).xyz;
    return vec2(card_amount(sky, normal.y) * held, seen_from_above(dot(normal, to_camera), length(to_camera)));
}
@else
    return vec2(held, 1.0);
}

// A direction of this vertex as its chunk, if a falling snow load's, tumbles.
fn snow_vector(v: vec3<f32>, vertex: Vertex, model: mat4x4<f32>, shed: LimbShed, turn: vec3<f32>) -> vec3<f32> {
@if(TREE_SNOW_CHUNKS) {
    let fall = chunk_fall(vertex.snow_chunk.w, shed, shed.since);
    let falling = shed.since >= 0.0 && shed.since < CHUNK_CRUMBLE.y;
    return select(v, turn_axis(v, fall.axis, fall.angle), falling);
}
@else
    return v;
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let model = world_from_local(vertex.instance_index);
    let tree = tree_vertex(vertex);
    // Snow shaken off (tree/shed.wesl): the limb, unloaded, springs up; its loads' chunks fall.
    let state = limb_state(tree, vertex.instance_index);
    let turn = touch_turn(tree, model, false) + unload_turn(tree, model, state, 0.0);
    let at_tree = instance_wind(vertex.instance_index, model, wind.current, false);
    let posed = tree_pose(tree, model, wind.current, at_tree, profile(), view.lod_view_world_position, turn);
    out.world_position = posed.position;
@if(TREE_SNOW_CHUNKS) {
    let drop = dropping(tree, vertex.snow_chunk, model, posed, state, 0.0);
    out.world_position = vec4(dropped(drop, out.world_position.xyz), 1.0);
}
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
    // Snowy foliage's vertex colour is its crown sky visibility (crown_sky_v2). Past the wind
    // nothing reads uv_b, so it carries the card's snow.
@if(TREE_SNOW && VERTEX_UVS_B && VERTEX_COLORS)
    out.uv_b = card_snow(tree, model, vertex.color.r, state);

// Depth, shadows and colour deliberately use the same wind pose. Damping the
// entire prepass (as the old renderer did for shadows) breaks temporal depth.
@if(PREPASS_PIPELINE) {
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS)
    out.world_normal = snow_vector(posed.turn * normal_to_world(vertex.normal, model, vertex.instance_index), vertex, model, state, turn);
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS) {
    let tangent = tangent_to_world(vertex.tangent, model, vertex.instance_index);
    out.world_tangent = vec4(snow_vector(posed.turn * tangent.xyz, vertex, model, state, turn), tangent.w);
}
@if(MOTION_VECTOR_PREPASS) {
@if(TREE_INSTANCED)
    let previous_model = model;
@else
    let previous_model = mesh_functions::get_previous_world_from_local(vertex.instance_index);
    let step = shed_step();
    let previous_turn = touch_turn(tree, previous_model, true) + unload_turn(tree, previous_model, state, step);
    let previous_at_tree = instance_wind(vertex.instance_index, previous_model, wind.previous, true);
    let previous_posed = tree_pose(tree, previous_model, wind.previous, previous_at_tree, profile(), view.lod_view_world_position, previous_turn);
    out.previous_world_position = previous_posed.position;
@if(TREE_SNOW_CHUNKS) {
    let previous_drop = dropping(tree, vertex.snow_chunk, previous_model, previous_posed, state, step);
    out.previous_world_position = vec4(dropped(previous_drop, out.previous_world_position.xyz), 1.0);
}
}
}
@else {
@if(VERTEX_NORMALS)
    out.world_normal = snow_vector(posed.turn * normal_to_world(vertex.normal, model, vertex.instance_index), vertex, model, state, turn);
@if(VERTEX_TANGENTS) {
    let tangent = tangent_to_world(vertex.tangent, model, vertex.instance_index);
    out.world_tangent = vec4(snow_vector(posed.turn * tangent.xyz, vertex, model, state, turn), tangent.w);
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
