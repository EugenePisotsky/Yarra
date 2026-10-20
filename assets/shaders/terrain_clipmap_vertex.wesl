// Terrain v2 patches (crates/terrain_render/src/clipmap) for every pass. Every node draws the
// same grid of its (intervals + 1)² samples; its mesh tag carries the node's key. A vertex
// reads its height from its level's clipmap and, over the outer part of the level's range,
// morphs towards the next coarser level (CDLOD, Strugar 2009): odd samples slide onto their
// lower even neighbour, so at the range the patch is the coarser grid and meets coarser
// neighbours without cracks. Distances are from the main camera (the state texture's eye), so
// shadow cascades draw the same surface.
import bevy_pbr::render::{mesh_functions, view_transformations::position_world_to_clip};
import package::shaders::terrain_clipmap_common::{
    eye, heights, intervals, levels, normals, render_xz, settings, texel, unpack_normal,
};
@if(PREPASS_PIPELINE) import bevy_pbr::prepass::io::VertexOutput;
@else import bevy_pbr::render::forward_io::VertexOutput;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    // The vertex's sample within its node; y is unused.
    @location(0) position: vec3<f32>,
}

// Mesh tag: level in bits 28-31, node x and z as 14-bit signed values in bits 14-27 and 0-13
// (crates/terrain_render/src/clipmap/gpu.rs `patch_tag`).
fn tag_level(tag: u32) -> u32 { return tag >> 28u; }
fn tag_node(tag: u32) -> vec2<i32> {
    return vec2(bitcast<i32>(tag << 4u) >> 18u, bitcast<i32>(tag << 18u) >> 18u);
}

fn sample_height(sample: vec2<i32>, level: u32) -> f32 {
    return textureLoad(heights, texel(sample), level, 0).r;
}
fn sample_normal(sample: vec2<i32>, level: u32) -> vec3<f32> {
    return unpack_normal(textureLoad(normals, texel(sample), level, 0).xy);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let tag = mesh_functions::get_tag(vertex.instance_index);
    let level = tag_level(tag);
    let sample = tag_node(tag) * intervals() + vec2<i32>(vertex.position.xz);
    let height = sample_height(sample, level);
    let xz = render_xz(sample << vec2(level));
    // The top level has nothing coarser to morph to.
    var k = 0.0;
    if level + 1u < levels() {
        let range = settings.ranges.x * f32(1u << level);
        let start = range * settings.ranges.y;
        let end = range * settings.ranges.z;
        k = clamp((distance(eye(), vec3(xz.x, height, xz.y)) - start) / (end - start), 0.0, 1.0);
    }
    let even = sample & vec2(-2);
    var position = vec3(xz.x, height, xz.y);
    var normal = sample_normal(sample, level);
    if k > 0.0 && any(even != sample) {
        let coarse_xz = render_xz(even << vec2(level));
        let coarse = vec3(coarse_xz.x, sample_height(even, level), coarse_xz.y);
        position = select(mix(position, coarse, k), coarse, k >= 1.0);
        normal = normalize(mix(normal, sample_normal(even, level), k));
    }
    out.world_position = vec4(position, 1.0);
    out.position = position_world_to_clip(position);
@if(UNCLIPPED_DEPTH_ORTHO_EMULATION) {
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
}
@if(PREPASS_PIPELINE) {
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS)
    out.world_normal = normal;
@if(MOTION_VECTOR_PREPASS)
    // Terrain is static; its morph is not motion.
    out.previous_world_position = out.world_position;
}
@else
    out.world_normal = normal;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = 0;
    return out;
}
