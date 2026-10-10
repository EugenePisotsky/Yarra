// StandardMaterial fragment shader adapted from Bevy 0.20.0 `pbr.wesl`, MIT OR Apache-2.0.
// Copyright Bevy contributors. See THIRD_PARTY.md in this directory.
import bevy_pbr::{
    render::{
        pbr_types,
        pbr_functions::alpha_discard,
        pbr_fragment::pbr_input_from_standard_material,
    },
    decal::clustered::apply_decals,
};
@if(TREE_INSTANCED) import bevy_pbr::render::{mesh_types, mesh_bindings::mesh};

@if(PREPASS_PIPELINE)
import bevy_pbr::{
    prepass::io::{VertexOutput, FragmentOutput},
    deferred::functions::deferred_output,
};
@else
import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
};
@if(!PREPASS_PIPELINE) import package::shaders::lighting::pbr_lighting::apply_pbr_lighting;

@if(VISIBILITY_RANGE_DITHER)
import bevy_pbr::render::pbr_functions::visibility_range_dither;
@if(VISIBILITY_RANGE_DITHER && !PREPASS_PIPELINE && CROSSFADE_SAMPLE_MASK)
import package::shaders::crossfade::crossfade_sample_mask;

@if(TREE_BARK_BLEND) import package::shaders::tree_bark::blend_upper_bark;

@if(MESHLET_MESH_MATERIAL_PASS)
import bevy_pbr::meshlet::visibility_buffer_resolve::resolve_vertex_output;

@if(MATERIAL_OIT_ENABLED)
import bevy_core_pipeline::oit::draw::oit_draw;

@if(FORWARD_DECAL)
import bevy_pbr::decal::forward::get_forward_decal_info;

// The forward output with the crossfade's covered samples.
@if(VISIBILITY_RANGE_DITHER && !PREPASS_PIPELINE && CROSSFADE_SAMPLE_MASK)
struct CrossfadeOutput {
    @location(0) color: vec4<f32>,
    @builtin(sample_mask) sample_mask: u32,
}

@if(VISIBILITY_RANGE_DITHER && !PREPASS_PIPELINE && CROSSFADE_SAMPLE_MASK)
alias MaterialOutput = CrossfadeOutput;
@else
alias MaterialOutput = FragmentOutput;

@fragment
fn fragment(
@if(MESHLET_MESH_MATERIAL_PASS)
    @builtin(position) frag_coord: vec4<f32>,
@if(!MESHLET_MESH_MATERIAL_PASS)
    vertex_output: VertexOutput,
@if(!MESHLET_MESH_MATERIAL_PASS)
    @builtin(front_facing) is_front: bool,
) -> MaterialOutput {
@if(MESHLET_MESH_MATERIAL_PASS)
    let vertex_output = resolve_vertex_output(frag_coord);
@if(MESHLET_MESH_MATERIAL_PASS)
    let is_front = true;

    var in = vertex_output;

    // If we're in the crossfade section of a visibility range, conditionally
    // discard the fragment according to the visibility pattern.
@if(VISIBILITY_RANGE_DITHER && !PREPASS_PIPELINE && CROSSFADE_SAMPLE_MASK)
    let crossfade_mask = crossfade_sample_mask(in.position, in.visibility_range_dither);
@if(VISIBILITY_RANGE_DITHER && !PREPASS_PIPELINE && CROSSFADE_SAMPLE_MASK)
    if crossfade_mask == 0u {
        discard;
    }
@elif(VISIBILITY_RANGE_DITHER)
    visibility_range_dither(in.position, in.visibility_range_dither);

@if(FORWARD_DECAL)
    let forward_decal_info = get_forward_decal_info(in);
@if(FORWARD_DECAL)
    in.world_position = forward_decal_info.world_position;
@if(FORWARD_DECAL)
    in.uv = forward_decal_info.uv;

    // Trees drawn from the instance buffer (crates/engine/src/tree_wind/instancing.rs) have no
    // mesh uniform: `in.instance_index` indexes the instance buffer, so the mesh flags Bevy reads
    // with it belong to some other mesh. Bevy corrects `is_front` by their winding sign
    // (`winding_corrected_front_facing`); undo that here, so double-sided cards keep facing as
    // the rasterizer says.
@if(TREE_INSTANCED)
    let unrelated_positive = (mesh[in.instance_index].flags
        & mesh_types::MESH_FLAGS_SIGN_DETERMINANT_MODEL_3X3_BIT) != 0u;
@if(TREE_INSTANCED)
    let front = is_front == unrelated_positive;
@else
    let front = is_front;

    // generate a PbrInput struct from the StandardMaterial bindings
    var pbr_input = pbr_input_from_standard_material(in, front);
    // Trees drawn from the instance buffer (crates/engine/src/tree_wind/instancing.rs) have no
    // mesh uniform: like every tree entity they receive shadows and are not mirrored.
@if(TREE_INSTANCED)
    pbr_input.flags = mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT
        | mesh_types::MESH_FLAGS_SIGN_DETERMINANT_MODEL_3X3_BIT;

@if(TREE_CROWN_SHADING && VERTEX_COLORS) {
    // Crown-shaded tree foliage stores crown occlusion in vertex colour
    // (crates/engine/src/tree_wind/material.rs). Apply it to indirect lighting;
    // pbr_lighting handles the separate crown self-shadow / cascade handoff.
    // Remove Bevy's vertex tint so occlusion never changes leaf albedo itself.
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
}

    // Two-layer tree bark (crates/engine/src/tree_wind/bark.rs): vertex colour alpha
    // blends the lower bark built above towards the upper bark.
    // Clamped for the same reason as crown occlusion: MSAA extrapolates vertex colour on
    // thin branches.
@if(TREE_BARK_BLEND && !PREPASS_PIPELINE && VERTEX_COLORS && VERTEX_UVS_A && VERTEX_TANGENTS)
    blend_upper_bark(&pbr_input, in.uv, clamp(in.color.rgb, vec3(0.0), vec3(1.0)), in.color.a,
        in.world_normal, in.world_tangent);

    // alpha discard
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color
    );

    // clustered decals
    apply_decals(&pbr_input);

    // write the gbuffer, lighting pass id, and optionally normal and motion_vector textures
@if(PREPASS_PIPELINE)
    let out = deferred_output(in, pbr_input);
    // in forward mode, we calculate the lit color immediately, and then apply some post-lighting effects here.
    // in deferred mode the lit color and these effects will be calculated in the deferred lighting shader
@if(!PREPASS_PIPELINE)
    var out: FragmentOutput;
@if(!PREPASS_PIPELINE)
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }

    // apply in-shader post processing (fog, alpha-premultiply, and also tonemapping, debanding if the camera is non-hdr)
    // note this does not include fullscreen postprocessing effects like bloom.
@if(!PREPASS_PIPELINE)
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);

@if(MATERIAL_OIT_ENABLED) {
    let alpha_mode = pbr_input.material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_RESERVED_BITS;
    if alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_BLEND {
        // The fragments will only be drawn during the oit resolve pass.
        oit_draw(in.position, vec4(out.color.rgb * out.color.a, out.color.a));
        discard;
    }
    // Both `Premultiplied` and `Add` colors are premultiplied in `premultiply_alpha()`
    if alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_PREMULTIPLIED
        || alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_ADD {
        // The fragments will only be drawn during the oit resolve pass.
        oit_draw(in.position, out.color);
        discard;
    }
}

@if(FORWARD_DECAL)
        out.color.a = min(forward_decal_info.alpha, out.color.a);

@if(VISIBILITY_RANGE_DITHER && !PREPASS_PIPELINE && CROSSFADE_SAMPLE_MASK)
        return CrossfadeOutput(out.color, crossfade_mask);
@else
        return out;
}
