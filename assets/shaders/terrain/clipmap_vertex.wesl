// Terrain v2 patches (crates/terrain_render/src/clipmap) for every pass. Every node draws the
// same grid of its (intervals + 1)² samples; its mesh tag carries the node's key. A vertex
// reads its height from its level's clipmap and, over the outer part of the level's range,
// morphs towards the next coarser level (CDLOD, Strugar 2009): odd samples slide onto their
// lower even neighbour, so at the range the patch is the coarser grid and meets coarser
// neighbours without cracks. Distances are from the main camera (the state texture's eye), so
// shadow cascades draw the same surface. Detail patches, parts of a leaf on soft ground, place
// their vertices on the leaves' triangles and morph the same way. The heights are the soft
// layer's (snow's) top; on the leaves, where feet pushed the layer down (terrain/prints.wesl),
// vertices stand on it as they left it.
import bevy_pbr::render::{mesh_functions, view_transformations::position_world_to_clip};
import package::shaders::terrain::clipmap_common::{
    DETAIL_LEVELS, eye, heights, intervals, leaf_spacing, leaf_surface_height, level_normal, levels,
    normals, origin_samples, render_xz, settings, soft_depth, texel, unpack_normal,
};
import package::shaders::terrain::prints::{pressed_layer_at, print_view};
@if(PREPASS_PIPELINE) import bevy_pbr::prepass::io::VertexOutput;
@else import bevy_pbr::render::forward_io::VertexOutput;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    // The vertex's sample within its node; y is unused.
    @location(0) position: vec3<f32>,
}

// Mesh tag: level counted from the finest detail level in bits 28-31, the patch's x and z among
// patches of its size as 14-bit signed values in bits 14-27 and 0-13
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

// Morphing towards the next coarser grid at `position` for a level drawn out to `range`.
fn morph(position: vec3<f32>, range: f32) -> f32 {
    let start = range * settings.ranges.y;
    let end = range * settings.ranges.z;
    return clamp((distance(eye(), position) - start) / (end - start), 0.0, 1.0);
}

// A detail level's sample is a leaf sample plus a fraction of 2^-detail steps.
fn detail_leaf(sample: vec2<i32>, detail: u32) -> vec2<i32> {
    return sample >> vec2(detail);
}

fn detail_fraction(sample: vec2<i32>, detail: u32) -> vec2<f32> {
    return vec2<f32>(sample - (detail_leaf(sample, detail) << vec2(detail))) / f32(1u << detail);
}

// On the leaves' triangles.
fn detail_point(sample: vec2<i32>, detail: u32) -> vec3<f32> {
    let leaf = detail_leaf(sample, detail);
    let fraction = detail_fraction(sample, detail);
    let xz = render_xz(leaf) + fraction * leaf_spacing();
    return vec3(xz.x, leaf_surface_height(leaf, fraction), xz.y);
}

fn detail_normal(sample: vec2<i32>, detail: u32) -> vec3<f32> {
    return level_normal(0u, detail_leaf(sample, detail), detail_fraction(sample, detail));
}

// The ground at render xz `p` as feet left the soft layer over it: `height` lowered by however
// far below its top they pushed it.
fn pressed_height(p: vec2<f32>, height: f32) -> f32 {
    let r = p / leaf_spacing();
    let cell = floor(r);
    let depth = soft_depth(origin_samples() + vec2<i32>(cell), r - cell);
    if depth <= 0.0 {
        return height;
    }
    return height + pressed_layer_at(print_view(p), p, depth) - depth;
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let tag = mesh_functions::get_tag(vertex.instance_index);
    let sample = tag_node(tag) * intervals() + vec2<i32>(vertex.position.xz);
    let even = sample & vec2(-2);
    var position: vec3<f32>;
    var normal: vec3<f32>;
    var k = 0.0;
    let detailed = i32(tag_level(tag)) < DETAIL_LEVELS;
    let detail = u32(max(DETAIL_LEVELS - i32(tag_level(tag)), 1));
    let level = u32(max(i32(tag_level(tag)) - DETAIL_LEVELS, 0));
    if detailed {
        position = detail_point(sample, detail);
        normal = detail_normal(sample, detail);
        k = morph(position, settings.detail.x / f32(1u << (detail - 1u)));
        if k > 0.0 && any(even != sample) {
            let coarse = detail_point(even, detail);
            position = select(mix(position, coarse, k), coarse, k >= 1.0);
            normal = normalize(mix(normal, detail_normal(even, detail), k));
        }
    } else {
        let xz = render_xz(sample << vec2(level));
        position = vec3(xz.x, sample_height(sample, level), xz.y);
        normal = sample_normal(sample, level);
        // The top level has nothing coarser to morph to.
        if level + 1u < levels() {
            k = morph(position, settings.ranges.x * f32(1u << level));
        }
        if k > 0.0 && any(even != sample) {
            let coarse_xz = render_xz(even << vec2(level));
            let coarse = vec3(coarse_xz.x, sample_height(even, level), coarse_xz.y);
            position = select(mix(position, coarse, k), coarse, k >= 1.0);
            normal = normalize(mix(normal, sample_normal(even, level), k));
        }
    }
    // Only the leaves hold their soft layer's depth.
    if detailed || level == 0u {
        position.y = pressed_height(position.xz, position.y);
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
