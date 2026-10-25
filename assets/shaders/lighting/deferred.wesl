// Deferred lighting (crates/atmosphere/src/environment/deferred.rs): every pixel the G-buffer pass
// wrote (lighting/gbuffer.wesl) is lit once, by the lighting the main pass's materials use
// (lighting/pbr_lighting.wesl). A full-screen triangle at the far plane: the depth test leaves the
// sky's pixels to the sky composite. Pixels of model NONE belong to surfaces a forward pass draws
// over afterwards.
import bevy_pbr::render::{
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
    mesh_view_bindings::{deferred_prepass_texture, view},
    pbr_functions::{calculate_view, main_pass_post_lighting_processing},
    pbr_types,
    view_transformations::position_ndc_to_world,
};
import bevy_pbr::prepass::utils::prepass_depth;
import package::shaders::lighting::gbuffer;
import package::shaders::lighting::pbr_lighting::apply_pbr_lighting;

struct Varyings {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> Varyings {
    let uv = vec2<f32>(f32(index >> 1u), f32(index & 1u)) * 2.0;
    // At the far plane (reverse Z), so only pixels holding a surface pass the depth test.
    return Varyings(vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0));
}

@fragment
fn fragment(in: Varyings) -> @location(0) vec4<f32> {
    let texel = textureLoad(deferred_prepass_texture, vec2<i32>(in.position.xy), 0);
    if gbuffer::model(texel) == gbuffer::MODEL_NONE {
        return vec4(0.0);
    }
    let s = gbuffer::unpack(texel);
    let depth = prepass_depth(in.position, 0u);
    // Within the main pass's region of the target: under MetalFX Temporal it covers part of it.
    let uv = (in.position.xy - view.main_pass_viewport.xy) / view.main_pass_viewport.zw;
    let ndc = vec3(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), depth);

    var pbr = pbr_types::pbr_input_new();
    pbr.frag_coord = vec4(in.position.xy, depth, 1.0);
    pbr.world_position = vec4(position_ndc_to_world(ndc), 1.0);
    pbr.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr.V = calculate_view(pbr.world_position, pbr.is_orthographic);
    pbr.N = s.normal;
    pbr.clearcoat_N = s.normal;
    pbr.world_normal = s.geometric_normal;
    pbr.material.base_color = vec4(s.albedo, 1.0);
    pbr.material.perceptual_roughness = s.perceptual_roughness;
    pbr.material.reflectance = vec3(s.reflectance);
    pbr.material.metallic = s.metallic;
    pbr.material.flags = pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    pbr.diffuse_occlusion = vec3(s.diffuse_occlusion);
    pbr.specular_occlusion = s.specular_occlusion;
    pbr.flags = MESH_FLAGS_SHADOW_RECEIVER_BIT;
    return main_pass_post_lighting_processing(pbr, apply_pbr_lighting(pbr));
}
