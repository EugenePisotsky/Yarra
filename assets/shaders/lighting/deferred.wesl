// Deferred lighting of the models the shared lighting serves
// (crates/atmosphere/src/environment/deferred.rs): every pixel of terrain, objects, tree crowns
// and bark in the G-buffer (lighting/gbuffer.wesl) is lit once, by the lighting the main pass's
// materials use (lighting/pbr_lighting.wesl), taking the branches of its model. Other models'
// pixels are left to their own kernels, and model NONE's to the forward passes drawn over them
// afterwards.
import bevy_pbr::render::{
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
    mesh_view_bindings::view,
    pbr_functions::{calculate_view, main_pass_post_lighting_processing},
    pbr_types,
};
import package::shaders::lighting::{deferred_pixel, gbuffer};
import package::shaders::lighting::pbr_lighting::{apply_pbr_lighting, light_model};

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return deferred_pixel::cover(index);
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let texel = deferred_pixel::texel(position);
    let model = gbuffer::model(texel);
    if !gbuffer::shared_lighting(model) {
        discard;
    }
    light_model(model);
    let s = gbuffer::unpack(texel);
    let pixel = deferred_pixel::pixel(position);

    var pbr = pbr_types::pbr_input_new();
    pbr.frag_coord = pixel.frag_coord;
    pbr.world_position = vec4(pixel.world_position, 1.0);
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
