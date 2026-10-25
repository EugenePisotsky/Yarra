// What every deferred lighting kernel shares (docs/ARCHITECTURE.md): the triangle that
// covers the view's surfaces, and a pixel's G-buffer texel and place.
import bevy_pbr::render::{
    mesh_view_bindings::{deferred_prepass_texture, view},
    view_transformations::position_ndc_to_world,
};
import bevy_pbr::prepass::utils::prepass_depth;

// A full-screen triangle at the far plane (reverse Z): drawn with depth compare Less, only
// pixels holding a surface pass, so no kernel runs on the sky.
fn cover(index: u32) -> vec4<f32> {
    let uv = vec2<f32>(f32(index >> 1u), f32(index & 1u)) * 2.0;
    return vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0);
}

fn texel(frag_coord: vec4<f32>) -> vec4<u32> {
    return textureLoad(deferred_prepass_texture, vec2<i32>(frag_coord.xy), 0);
}

struct Pixel {
    // The fragment coordinate with the surface's depth.
    frag_coord: vec4<f32>,
    world_position: vec3<f32>,
}

fn pixel(frag_coord: vec4<f32>) -> Pixel {
    let depth = prepass_depth(frag_coord, 0u);
    // Within the main pass's region of the target: under MetalFX Temporal it covers part of it.
    let uv = (frag_coord.xy - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let ndc = vec3(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), depth);
    return Pixel(vec4(frag_coord.xy, depth, 1.0), position_ndc_to_world(ndc));
}
