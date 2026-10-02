#import "shaders/clouds/pbr_lighting.wgsl"::apply_pbr_lighting
#import bevy_pbr::{
    pbr_types,
    pbr_functions::alpha_discard,
    pbr_fragment::pbr_input_from_standard_material,
    decal::clustered::apply_decals,
}

#ifdef PREPASS_PIPELINE
#import bevy_pbr::{
    prepass_io::{VertexOutput, FragmentOutput},
    pbr_deferred_functions::deferred_output,
}
#else
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}
#endif

#ifdef VISIBILITY_RANGE_DITHER
#import bevy_pbr::pbr_functions::visibility_range_dither;
#endif

#ifdef TREE_BARK_BLEND
#import "shaders/tree_bark.wgsl"::blend_upper_bark
#endif

#ifdef MESHLET_MESH_MATERIAL_PASS
#import bevy_pbr::meshlet_visibility_buffer_resolve::resolve_vertex_output
#endif

#ifdef OIT_ENABLED
#import bevy_core_pipeline::oit::oit_draw
#endif // OIT_ENABLED

#ifdef FORWARD_DECAL
#import bevy_pbr::decal::forward::get_forward_decal_info
#endif

@fragment
fn fragment(
#ifdef MESHLET_MESH_MATERIAL_PASS
    @builtin(position) frag_coord: vec4<f32>,
#else
    vertex_output: VertexOutput,
    @builtin(front_facing) is_front: bool,
#endif
) -> FragmentOutput {
#ifdef MESHLET_MESH_MATERIAL_PASS
    let vertex_output = resolve_vertex_output(frag_coord);
    let is_front = true;
#endif

    var in = vertex_output;

    // If we're in the crossfade section of a visibility range, conditionally
    // discard the fragment according to the visibility pattern.
#ifdef VISIBILITY_RANGE_DITHER
    visibility_range_dither(in.position, in.visibility_range_dither);
#endif

#ifdef FORWARD_DECAL
    let forward_decal_info = get_forward_decal_info(in);
    in.world_position = forward_decal_info.world_position;
    in.uv = forward_decal_info.uv;
#endif

    // generate a PbrInput struct from the StandardMaterial bindings
    var pbr_input = pbr_input_from_standard_material(in, is_front);

#ifdef TREE_CROWN_SHADING
#ifdef VERTEX_COLORS
    // Crown-shaded tree foliage stores crown occlusion in vertex colour
    // (crates/engine/src/tree_wind/material.rs). It dims sky and ambient light only;
    // shadow maps already darken the sun, so leaves in sunlight keep their colour.
    // MSAA evaluates edge pixels at the pixel centre, which can lie outside a small distant
    // card; vertex colour extrapolated there can leave [0, 1] by a lot. Clamped, it can
    // neither cancel the base colour nor brighten the lighting below.
    let crown_occlusion = clamp(in.color.rgb, vec3(0.05), vec3(1.0));
    pbr_input.material.base_color = vec4(
        pbr_input.material.base_color.rgb / max(crown_occlusion, vec3(0.01)),
        pbr_input.material.base_color.a,
    );
    pbr_input.diffuse_occlusion *= crown_occlusion;
    pbr_input.specular_occlusion *= crown_occlusion.g;
#endif
#endif

#ifdef TREE_BARK_BLEND
#ifndef PREPASS_PIPELINE
#ifdef VERTEX_COLORS
#ifdef VERTEX_UVS_A
#ifdef VERTEX_TANGENTS
    // Two-layer tree bark (crates/engine/src/tree_wind/bark.rs): vertex colour alpha
    // blends the lower bark built above towards the upper bark.
    // Clamped for the same reason as crown occlusion: MSAA extrapolates vertex colour on
    // thin branches.
    blend_upper_bark(&pbr_input, in.uv, clamp(in.color.rgb, vec3(0.0), vec3(1.0)), in.color.a,
        in.world_normal, in.world_tangent);
#endif
#endif
#endif
#endif
#endif

    // alpha discard
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    // clustered decals
    apply_decals(&pbr_input);

#ifdef PREPASS_PIPELINE
    // write the gbuffer, lighting pass id, and optionally normal and motion_vector textures
    let out = deferred_output(in, pbr_input);
#else
    // in forward mode, we calculate the lit color immediately, and then apply some post-lighting effects here.
    // in deferred mode the lit color and these effects will be calculated in the deferred lighting shader
    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }

    // apply in-shader post processing (fog, alpha-premultiply, and also tonemapping, debanding if the camera is non-hdr)
    // note this does not include fullscreen postprocessing effects like bloom.
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
#endif

#ifdef OIT_ENABLED
    let alpha_mode = pbr_input.material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_RESERVED_BITS;
    if alpha_mode != pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_OPAQUE {
        // The fragments will only be drawn during the oit resolve pass.
        oit_draw(in.position, out.color);
        discard;
    }
#endif // OIT_ENABLED

#ifdef FORWARD_DECAL
        out.color.a = min(forward_decal_info.alpha, out.color.a);
#endif

        return out;
}
