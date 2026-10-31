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
import bevy_pbr::prepass::io::{VertexOutput, FragmentOutput};
@else
import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
};
@if(!PREPASS_PIPELINE) import package::shaders::lighting::pbr_lighting::apply_pbr_lighting;
@if(PREPASS_PIPELINE) import package::shaders::lighting::gbuffer;
@if(MOTION_VECTOR_PREPASS)
import bevy_pbr::render::pbr_prepass_functions::calculate_motion_vector;

@if(VISIBILITY_RANGE_DITHER)
import bevy_pbr::render::pbr_functions::visibility_range_dither;

@if(TREE_BARK_BLEND) import package::shaders::tree::bark::blend_upper_bark;
@if(TREE_SNOW) import package::shaders::tree::snow;
@if(CHARACTER_SKIN) import package::shaders::lighting::skin_layers;
@if(HAIR_CARDS) import package::shaders::frame::frame_count;

@if(MESHLET_MESH_MATERIAL_PASS)
import bevy_pbr::meshlet::visibility_buffer_resolve::resolve_vertex_output;

@if(MATERIAL_OIT_ENABLED)
import bevy_core_pipeline::oit::draw::oit_draw;

@if(FORWARD_DECAL)
import bevy_pbr::decal::forward::get_forward_decal_info;

// Hair cards cut out at a threshold that changes per pixel and per frame, over this range of alpha:
// the temporal upscaler averages it into the soft edge a card's hairs have, which one fixed threshold
// at this resolution turns to straw.
const HAIR_CUT_RANGE: vec2<f32> = vec2(0.12, 0.72);

// A card's normal map bends each painted strand up to 30 degrees, which breaks a highlight into white
// flecks that read as metal. This share of the card's own normal keeps the strands' shading and joins
// their highlights into a band.
const HAIR_SOFT_NORMAL: f32 = 0.5;

// Interleaved gradient noise (Jimenez 2014).
fn hair_threshold(pixel: vec2<f32>, frame: u32) -> f32 {
    let p = pixel + 5.588238 * f32(frame % 64u);
    let noise = fract(52.9829189 * fract(dot(p, vec2(0.06711056, 0.00583715))));
    return mix(HAIR_CUT_RANGE.x, HAIR_CUT_RANGE.y, noise);
}

@fragment
fn fragment(
@if(MESHLET_MESH_MATERIAL_PASS)
    @builtin(position) frag_coord: vec4<f32>,
@if(!MESHLET_MESH_MATERIAL_PASS)
    vertex_output: VertexOutput,
@if(!MESHLET_MESH_MATERIAL_PASS)
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
@if(MESHLET_MESH_MATERIAL_PASS)
    let vertex_output = resolve_vertex_output(frag_coord);
@if(MESHLET_MESH_MATERIAL_PASS)
    let is_front = true;

    var in = vertex_output;

    // If we're in the crossfade section of a visibility range, conditionally
    // discard the fragment according to the visibility pattern.
@if(VISIBILITY_RANGE_DITHER)
    visibility_range_dither(in.position, in.visibility_range_dither);

@if(FORWARD_DECAL)
    let forward_decal_info = get_forward_decal_info(in);
@if(FORWARD_DECAL)
    in.world_position = forward_decal_info.world_position;
@if(FORWARD_DECAL)
    in.uv = forward_decal_info.uv;

    // Trees drawn by the GPU trees (crates/engine/src/trees/gpu.rs) have no
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
    // Trees drawn by the GPU trees (crates/engine/src/trees/gpu.rs) have no
    // mesh uniform: like every tree entity they receive shadows and are not mirrored.
@if(TREE_INSTANCED)
    pbr_input.flags = mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT
        | mesh_types::MESH_FLAGS_SIGN_DETERMINANT_MODEL_3X3_BIT;

@if(TREE_CROWN_SHADING && VERTEX_COLORS) {
    // Crown-shaded tree foliage stores crown occlusion in vertex colour
    // (crates/engine/src/trees/material.rs). Apply it to indirect lighting;
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

    // Bark tagged `bark_sky_v1` (crates/engine/src/trees/material.rs) stores the share of the
    // sky its crown leaves it in vertex colour: lighting, not albedo (bark_sky_reach in
    // pbr_lighting). Clamped as crown occlusion above.
@if(TREE_BARK_SKY && VERTEX_COLORS) {
    let bark_sky = clamp(in.color.rgb, vec3(0.01), vec3(1.0));
    pbr_input.material.base_color = vec4(
        pbr_input.material.base_color.rgb / bark_sky,
        pbr_input.material.base_color.a,
    );
    pbr_input.diffuse_occlusion *= bark_sky;
}

    // Two-layer tree bark (crates/engine/src/trees/bark.rs): vertex colour alpha
    // blends the lower bark built above towards the upper bark.
    // Clamped for the same reason as crown occlusion: MSAA extrapolates vertex colour on
    // thin branches.
@if(TREE_BARK_BLEND && TREE_BARK_SKY && VERTEX_COLORS && VERTEX_UVS_A && VERTEX_TANGENTS)
    blend_upper_bark(&pbr_input, in.uv, vec3(1.0), in.color.a, in.world_normal, in.world_tangent);
@if(TREE_BARK_BLEND && !TREE_BARK_SKY && VERTEX_COLORS && VERTEX_UVS_A && VERTEX_TANGENTS)
    blend_upper_bark(&pbr_input, in.uv, clamp(in.color.rgb, vec3(0.0), vec3(1.0)), in.color.a,
        in.world_normal, in.world_tangent);

    // Snow over the surface built above (tree/snow.wesl). Foliage is cut out where its needles
    // or its snow are, as in its prepass (tree/wind_prepass.wesl); uv_b carries the card's snow.
@if(TREE_SNOW && TREE_CROWN_SHADING && VERTEX_UVS_A && VERTEX_UVS_B) {
    let cover = snow::needle_snow(in.uv, in.uv_b.x);
    let needles = pbr_input.material.base_color.a;
    let above = in.uv_b.y;
    snow::settle(&pbr_input, snow::needle_shown(cover, needles, pbr_input.material.alpha_cutoff, above),
        mix(-1.0, 1.0, above));
    pbr_input.material.base_color.a = max(needles, cover);
}
    // uv_b.x holds the share of the bark's snow characters have not shaken off.
@if(TREE_SNOW && TREE_BARK_SKY && VERTEX_COLORS && VERTEX_UVS_B)
    snow::settle(&pbr_input, snow::bark_cover(normalize(in.world_normal).y,
        clamp(in.color.r, 0.0, 1.0), in.world_position.xyz) * in.uv_b.x, 1.0);

    // What a character's skin paints over itself: brows and makeup (character_skin.rs).
@if(CHARACTER_SKIN)
    skin_layers::paint(&pbr_input, in.uv, in.world_normal);

@if(HAIR_CARDS) {
    let shown = pbr_input.material.base_color.a >= hair_threshold(in.position.xy, frame_count());
    pbr_input.material.base_color.a = f32(shown);
    pbr_input.N = normalize(mix(pbr_input.N, pbr_input.world_normal, HAIR_SOFT_NORMAL));
}

    // Skin reflects less than Bevy's default dielectric, 2.8% head-on.
@if(SKIN_LIGHTING)
    pbr_input.material.reflectance = vec3(0.42);

    // alpha discard
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color
    );

    // clustered decals
    apply_decals(&pbr_input);

    // Yarra: on deferred views, our G-buffer (lighting/gbuffer.wesl) and motion; the deferred
    // kernel lights it afterwards with the branches of the surface's model
    // (lighting/deferred.wesl).
@if(PREPASS_PIPELINE)
    var out: FragmentOutput;
@if(PREPASS_PIPELINE) {
    out.deferred = gbuffer::pack(gbuffer::Surface(pbr_input.material.base_color.rgb,
        pbr_input.material.perceptual_roughness, pbr_input.N, pbr_input.world_normal,
        pbr_input.diffuse_occlusion.g, pbr_input.specular_occlusion,
        pbr_input.material.reflectance.x, pbr_input.material.metallic, gbuffer::SHADED_MODEL,
        0.0));
    out.deferred_lighting_pass_id = 1u;
@if(MOTION_VECTOR_PREPASS)
    out.motion_vector = calculate_motion_vector(in.world_position, in.previous_world_position);
}
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

        return out;
}
