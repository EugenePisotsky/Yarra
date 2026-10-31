// Forward lighting adapted from Bevy 0.20.0, MIT OR Apache-2.0.
// Copyright Bevy contributors. See THIRD_PARTY.md in this directory.
// Kept separate from Bevy assets: no global shader replacement or engine fork.
import bevy_pbr::{
    render::{
        pbr_types,
        pbr_bindings,
        mesh_view_bindings as view_bindings,
        mesh_view_types,
        pbr_lighting as lighting,
        pbr_lighting::{LAYER_BASE, LAYER_CLEARCOAT},
        clustered_forward as clustering,
        shadows,
        shadow_sampling,
        pbr_ambient as ambient,
        view_transformations,
        utils,
        mesh_types::{
            MESH_FLAGS_SHADOW_RECEIVER_BIT,
            MESH_FLAGS_SIGN_DETERMINANT_MODEL_3X3_BIT,
            MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT,
        },
    },
    ssr::raymarch,
};
import bevy_pbr::render::mesh_view_bindings::globals;
import bevy_pbr::render::view_transformations::{position_world_to_ndc};
import bevy_render::maths::{E, PI, powsafe};
@if(ATMOSPHERE) import bevy_pbr::atmosphere::functions::{calculate_visible_sun_ratio, clamp_to_surface};

@if(STANDARD_MATERIAL_SPECULAR_TRANSMISSION) import bevy_pbr::transmission;

@if(IRRADIANCE_VOLUME) import bevy_pbr::light_probe::irradiance_volume;

@if(MESHLET_MESH_MATERIAL_PASS) import bevy_pbr::meshlet::visibility_buffer_resolve::VertexOutput;
@elif(PREPASS_PIPELINE) import bevy_pbr::prepass::io::VertexOutput;
@else import bevy_pbr::render::forward_io::VertexOutput;

@if(ENVIRONMENT_MAP) import bevy_pbr::light_probe::environment_map;

@if(TONEMAP_IN_SHADER) import bevy_core_pipeline::tonemapping::{tone_mapping, screen_space_dither};

import bevy_pbr::render::pbr_functions::{calculate_diffuse_color, calculate_F0, calculate_F0_dielectric};
@if(STANDARD_MATERIAL_ANISOTROPY) import bevy_pbr::render::pbr_functions::bend_normal_for_anisotropy;
@if(CONTACT_SHADOWS && DEPTH_PREPASS) import bevy_pbr::render::pbr_functions::calculate_contact_shadow;
import package::shaders::lighting::surface::{cloud_light, environment, ground_celestial_irradiance, surface_wetness};
import package::shaders::lighting::forest_shadow::{forest_transmittance, forest_sky_light, forest_sky_visibility, forest_crown_sky};
import package::shaders::lighting::terrain_shadow::{terrain_sunlight, terrain_sky_visibility};
import package::shaders::lighting::ground::ground_below;
import package::shaders::lighting::gbuffer::{
    MODEL_BARK, MODEL_CROWN, MODEL_CROWN_SKY, MODEL_IMPOSTOR, MODEL_SKIN, MODEL_TERRAIN, SHADED_MODEL,
};

// Yarra: the lighting model of the surface being lit (lighting/gbuffer.wesl), which picks the
// branches below for terrain, tree crowns and bark. A material's own pipeline fixes it by its
// shader definitions, so its branches fold away; the deferred kernel (lighting/deferred.wesl)
// reads it per pixel and sets it first.
@if(DEFERRED_LIGHTING_PIPELINE) var<private> lit_model: u32;

@if(DEFERRED_LIGHTING_PIPELINE)
fn light_model(model: u32) {
    lit_model = model;
}

fn lit_model_is(model: u32) -> bool {
    @if(DEFERRED_LIGHTING_PIPELINE) {
        return lit_model == model;
    }
    @else {
        return SHADED_MODEL == model;
    }
}

// Yarra: light scatters under skin, red furthest. A pixel's direct diffuse light takes these shares of
// the mesh's own smooth normal over the bumpy one, per channel, so pores show less in red, and wraps
// past the terminator by these amounts of N.L. In shade its shadow's edge reddens by these powers, and
// the sky's light meets an albedo and occlusion deepened by these powers, the colour light takes after
// bouncing around under the skin.
const SKIN_SOFT_NORMAL: vec3<f32> = vec3(0.8, 0.4, 0.15);
const SKIN_WRAP: vec3<f32> = vec3(0.24, 0.1, 0.05);
const SKIN_SHADOW_EDGE: vec3<f32> = vec3(0.6, 1.0, 1.3);
const SKIN_AMBIENT_ALBEDO: vec3<f32> = vec3(0.92, 1.0, 1.05);
const SKIN_AMBIENT_OCCLUSION: vec3<f32> = vec3(0.6, 1.0, 1.25);

fn lit_crown() -> bool {
    return lit_model_is(MODEL_CROWN) || lit_crown_sky();
}

// Foliage whose crown occlusion is its linear sky visibility: `crown_v2` meshes and impostors.
fn lit_crown_sky() -> bool {
    return lit_model_is(MODEL_CROWN_SKY) || lit_model_is(MODEL_IMPOSTOR);
}

// Yarra: light reflected towards a surface by the ground around it, as environment radiance
// (Bevy's ambient colour is used the same way): the albedo of the ground below it times the sun,
// moon and sky light reaching that ground, the sun's as far as `sunlit` of the ground takes it
// and the sky's as far as the terrain around leaves it `sky`.
fn ground_light(P: vec3<f32>, sunlit: f32, sky: f32) -> vec3<f32> {
    let ground = ground_below(P.xz);
    let irradiance = view_bindings::lights.ambient_color.rgb * PI * sky
        + ground_celestial_irradiance(P, ground.w * sunlit);
    return ground.rgb * irradiance / PI;
}

// Yarra: how much of the ground around `P` lies in light `light_id`'s beam, by its shadow maps a
// step towards the light, past the surface's own body or hollow: a figure's shaded back and a
// trench's shaded floor still face sunlit snow, while both stand on shaded ground in a ridge's or
// a house's shadow. Where that point is shaded, four more around it across the beam tell a
// shadow as narrow as a figure's, which leaves most of the ground around lit. Past the cascades
// `past` stands in.
fn ground_in_light(light_id: u32, P: vec3<f32>, view_z: f32, past: f32) -> f32 {
    let light = &view_bindings::lights.directional_lights[light_id];
    let cascade = shadows::get_cascade_index(light_id, view_z);
    if cascade >= (*light).num_cascades {
        return past;
    }
    let to_light = (*light).direction_to_light;
    let centre = P + to_light * GROUND_SHADOW_STEP;
    var lit = cascade_light(light_id, cascade, centre);
    if lit < 1.0 {
        let side = normalize(cross(to_light, vec3(0.0, 1.0, 0.0))) * GROUND_SHADOW_REACH;
        let across = cross(side, to_light);
        lit = (lit + cascade_light(light_id, cascade, centre + side)
            + cascade_light(light_id, cascade, centre - side)
            + cascade_light(light_id, cascade, centre + across)
            + cascade_light(light_id, cascade, centre - across)) / 5.0;
    }
    let end = (*light).cascades[(*light).num_cascades - 1u].far_bound;
    return mix(lit, past, smoothstep(end * 0.7, end, -view_z));
}

// Yarra: whether light `light_id`'s cascade `cascade` lets its beam reach `point`; 1 outside it.
// One hardware sample.
fn cascade_light(light_id: u32, cascade: u32, point: vec3<f32>) -> f32 {
    let local = shadows::world_to_directional_light_local(light_id, cascade, vec4(point, 1.0));
    if local.w == 0.0 {
        return 1.0;
    }
    let light = &view_bindings::lights.directional_lights[light_id];
    let layer = i32((*light).depth_texture_base_index + cascade);
    return shadow_sampling::sample_shadow_map_hardware(local.xy, local.z, layer);
}

// Metres towards the light a point's ground is tested from (past a body, a trunk or a print's
// walls), and across the beam around it.
const GROUND_SHADOW_STEP: f32 = 1.0;
const GROUND_SHADOW_REACH: f32 = 1.0;
// The terrain tests the ground around it as its normal's height falls from the second of these
// to the first: level ground takes almost no light from below.
const GROUND_TILT: vec2<f32> = vec2(0.95, 0.99);

// Yarra: sky light still reaching foliage the rest of its crown hides from the sky, passed
// through and between the leaves (TREE_CROWN_SKY).
const CROWN_FILL: f32 = 0.12;

// Yarra: the share of the sky's light reaching foliage at `p` whose crown leaves it `sky`
// (TREE_CROWN_SKY): past its own crown and the crowns around, plus what leaves pass on.
fn crown_sky_reach(sky: f32, p: vec3<f32>) -> f32 {
    let open = clamp(sky, 0.0, 1.0) * forest_crown_sky(p);
    return open + (1.0 - open) * CROWN_FILL;
}

// Yarra: the share of the stand's sky light reaching bark that its own crown leaves `sky`
// (TREE_BARK_SKY), plus what leaves pass on. The forest map's 8-16 m average already holds part
// of the tree's own crown, so bark keeps the darker of the two shares rather than their product:
// trunks inside a stand stay as dark as before, lone and edge trees darken under their crowns.
fn bark_sky_reach(sky: f32, p: vec3<f32>) -> f32 {
    let own = clamp(sky, 0.0, 1.0);
    let stand = forest_sky_visibility(p);
    return min((own + (1.0 - own) * CROWN_FILL) / max(stand, 1e-3), 1.0);
}

// Yarra: a crown's linear sky visibility (TREE_CROWN_SKY) as the eased occlusion the distant
// self-shadow was tuned on: deep foliage keeps about half the sun.
fn crown_sky_shade(sky: f32) -> f32 {
    return 1.0 - 0.52 * (1.0 - pow(sky, 0.65));
}

// Yarra: the sunlight left at `P` after the atmosphere and the horizon, as Bevy applies it inside
// `lighting::directional_light`, for direct light added outside that function. Without it the
// crowns' wrap-around light stayed white at sunset and lit foliage after the sun had set.
fn through_atmosphere(P: vec3<f32>, light_id: u32) -> vec3<f32> {
    @if(ATMOSPHERE) {
        let light = &view_bindings::lights.directional_lights[light_id];
        let atmosphere = view_bindings::atmosphere;
        let P_as = (atmosphere.world_to_atmosphere * vec4(P, 1.0)).xyz;
        let P_clamped = clamp_to_surface(atmosphere, P_as);
        let r = length(P_clamped);
        let mu = dot((*light).direction_to_light, normalize(P_clamped));
        return lighting::sample_transmittance_lut(r, mu)
            * calculate_visible_sun_ratio(atmosphere, r, mu, (*light).sun_disk_angular_size);
    }
    @else {
        return vec3(1.0);
    }
}

fn apply_pbr_lighting(
    input: pbr_types::PbrInput,
) -> vec4<f32> {
    // Yarra: wet surfaces absorb more (darker diffuse albedo) and reflect more coherently.
    var in = input;
    let wet = surface_wetness(in.world_position.xyz, in.N);
    in.material.base_color = vec4(in.material.base_color.rgb * mix(1.0, 0.55, wet), in.material.base_color.a);
    in.material.perceptual_roughness = mix(
        in.material.perceptual_roughness,
        max(0.2, in.material.perceptual_roughness * 0.45),
        wet,
    );
    var output_color: vec4<f32> = in.material.base_color;

    let emissive = in.material.emissive;

    // calculate non-linear roughness from linear perceptualRoughness
    let metallic = in.material.metallic;
    let perceptual_roughness = in.material.perceptual_roughness;
    let roughness = lighting::perceptualRoughnessToRoughness(perceptual_roughness);
    let ior = in.material.ior;
    let thickness = in.material.thickness;
    let reflectance = in.material.reflectance;
    let diffuse_transmission = in.material.diffuse_transmission;
    let specular_transmission = in.material.specular_transmission;

    let specular_transmissive_color = specular_transmission * in.material.base_color.rgb;

    var diffuse_occlusion = in.diffuse_occlusion;
    if lit_model_is(MODEL_BARK) {
        diffuse_occlusion = vec3(bark_sky_reach(in.diffuse_occlusion.g, in.world_position.xyz));
    }
    let specular_occlusion = in.specular_occlusion;

    // Neubelt and Pettineo 2013, "Crafting a Next-gen Material Pipeline for The Order: 1886"
    // Yarra: crown normals are shared by both sides of a leaf card, so a card seen from behind
    // is not at a grazing angle; treating it as one lit it with grey sky reflection.
    let NdotV = max(select(dot(in.N, in.V), abs(dot(in.N, in.V)), lit_crown()), 0.0001);
    let R = reflect(-in.V, in.N);

    // Do the above calculations again for the clearcoat layer. Remember that
    // the clearcoat can have its own roughness and its own normal.
    @if(STANDARD_MATERIAL_CLEARCOAT)
    let clearcoat = in.material.clearcoat;
    @if(STANDARD_MATERIAL_CLEARCOAT)
    let clearcoat_perceptual_roughness = in.material.clearcoat_perceptual_roughness;
    @if(STANDARD_MATERIAL_CLEARCOAT)
    let clearcoat_roughness = lighting::perceptualRoughnessToRoughness(clearcoat_perceptual_roughness);
    @if(STANDARD_MATERIAL_CLEARCOAT)
    let clearcoat_N = in.clearcoat_N;
    @if(STANDARD_MATERIAL_CLEARCOAT)
    let clearcoat_NdotV = max(dot(clearcoat_N, in.V), 0.0001);
    @if(STANDARD_MATERIAL_CLEARCOAT)
    let clearcoat_R = reflect(-in.V, clearcoat_N);

    let diffuse_color = calculate_diffuse_color(
        output_color.rgb,
        metallic,
        specular_transmission,
        diffuse_transmission
    );

    // Diffuse transmissive strength is inversely related to metallicity and specular transmission, but directly related to diffuse transmission
    let diffuse_transmissive_color = output_color.rgb * (1.0 - metallic) * (1.0 - specular_transmission) * diffuse_transmission;

    // Calculate the world position of the second Lambertian lobe used for diffuse transmission, by subtracting material thickness
    let diffuse_transmissive_lobe_world_position = in.world_position - vec4<f32>(in.world_normal, 0.0) * thickness;

    let F0 = calculate_F0(output_color.rgb, metallic, reflectance);
    let F_ab = lighting::F_AB(perceptual_roughness, NdotV);

    var direct_light: vec3<f32> = vec3<f32>(0.0);

    // Transmitted Light (Specular and Diffuse)
    var transmitted_light: vec3<f32> = vec3<f32>(0.0);

    // Pack all the values into a structure.
    var lighting_input: lighting::LightingInput;
    lighting_input.layers[LAYER_BASE].NdotV = NdotV;
    lighting_input.layers[LAYER_BASE].N = in.N;
    lighting_input.layers[LAYER_BASE].R = R;
    lighting_input.layers[LAYER_BASE].perceptual_roughness = perceptual_roughness;
    lighting_input.layers[LAYER_BASE].roughness = roughness;
    lighting_input.P = in.world_position.xyz;
    lighting_input.V = in.V;
    lighting_input.diffuse_color = diffuse_color;
    lighting_input.metallic = metallic;
    lighting_input.F0_dielectric = calculate_F0_dielectric(reflectance);
    lighting_input.F0_metallic = output_color.rgb;
    lighting_input.F_ab = F_ab;
@if(STANDARD_MATERIAL_CLEARCOAT) {
    lighting_input.layers[LAYER_CLEARCOAT].NdotV = clearcoat_NdotV;
    lighting_input.layers[LAYER_CLEARCOAT].N = clearcoat_N;
    lighting_input.layers[LAYER_CLEARCOAT].R = clearcoat_R;
    lighting_input.layers[LAYER_CLEARCOAT].perceptual_roughness = clearcoat_perceptual_roughness;
    lighting_input.layers[LAYER_CLEARCOAT].roughness = clearcoat_roughness;
    lighting_input.clearcoat_strength = clearcoat;
}
@if(STANDARD_MATERIAL_ANISOTROPY) {
    lighting_input.anisotropy = in.anisotropy_strength;
    lighting_input.Ta = in.anisotropy_T;
    lighting_input.Ba = in.anisotropy_B;
}

    // And do the same for transmissive if we need to.
    @if(STANDARD_MATERIAL_DIFFUSE_TRANSMISSION)
    var transmissive_lighting_input: lighting::LightingInput;
@if(STANDARD_MATERIAL_DIFFUSE_TRANSMISSION) {
    transmissive_lighting_input.layers[LAYER_BASE].NdotV = 1.0;
    transmissive_lighting_input.layers[LAYER_BASE].N = -in.N;
    transmissive_lighting_input.layers[LAYER_BASE].R = vec3(0.0);
    transmissive_lighting_input.layers[LAYER_BASE].perceptual_roughness = 1.0;
    transmissive_lighting_input.layers[LAYER_BASE].roughness = 1.0;
    transmissive_lighting_input.P = diffuse_transmissive_lobe_world_position.xyz;
    transmissive_lighting_input.V = -in.V;
    transmissive_lighting_input.diffuse_color = diffuse_transmissive_color;
    transmissive_lighting_input.metallic = 0.0;
    transmissive_lighting_input.F0_dielectric = vec3(0.0);
    transmissive_lighting_input.F0_metallic = vec3(0.0);
    transmissive_lighting_input.F_ab = vec2(0.1);
@if(STANDARD_MATERIAL_CLEARCOAT) {
    transmissive_lighting_input.layers[LAYER_CLEARCOAT].NdotV = 0.0;
    transmissive_lighting_input.layers[LAYER_CLEARCOAT].N = vec3(0.0);
    transmissive_lighting_input.layers[LAYER_CLEARCOAT].R = vec3(0.0);
    transmissive_lighting_input.layers[LAYER_CLEARCOAT].perceptual_roughness = 0.0;
    transmissive_lighting_input.layers[LAYER_CLEARCOAT].roughness = 0.0;
    transmissive_lighting_input.clearcoat_strength = 0.0;
}
@if(STANDARD_MATERIAL_ANISOTROPY) {
    transmissive_lighting_input.anisotropy = in.anisotropy_strength;
    transmissive_lighting_input.Ta = in.anisotropy_T;
    transmissive_lighting_input.Ba = in.anisotropy_B;
}
}

    let view_z = dot(vec4<f32>(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z
    ), in.world_position);
    let cluster_index = clustering::view_fragment_cluster_index(in.frag_coord.xy, view_z, in.is_orthographic);
    var clusterable_object_index_ranges =
        clustering::unpack_clusterable_object_index_ranges(cluster_index);

    @if(CONTACT_SHADOWS)
    let contact_shadow_steps = view_bindings::contact_shadows_settings.linear_steps;
    @if(CONTACT_SHADOWS)
    let contact_shadow_enabled = contact_shadow_steps > 0u;

    // Point lights (direct)
    for (var i: u32 = clusterable_object_index_ranges.first_point_light_index_offset;
            i < clusterable_object_index_ranges.first_spot_light_index_offset;
            i = i + 1u) {
        let light_id = clustering::get_clusterable_object_id(i);

        // If we're lightmapped, disable diffuse contribution from the light if
        // requested, to avoid double-counting light.
        @if(LIGHTMAP)
        let enable_diffuse =
            (view_bindings::clustered_lights.data[light_id].flags &
                mesh_view_types::POINT_LIGHT_FLAGS_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE_BIT) != 0u;
        @else   // LIGHTMAP
        let enable_diffuse = true;

        var shadow: f32 = 1.0;
        if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
                && (view_bindings::clustered_lights.data[light_id].flags & mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            shadow = shadows::fetch_point_shadow(light_id, in.world_position, in.world_normal, in.frag_coord.xy);
        }

        @if(CONTACT_SHADOWS && DEPTH_PREPASS)
        if contact_shadow_enabled && (in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u && shadow > 0.0 &&
                (view_bindings::clustered_lights.data[light_id].flags &
                    mesh_view_types::POINT_LIGHT_FLAGS_CONTACT_SHADOWS_ENABLED_BIT) != 0u {
            let L = normalize(view_bindings::clustered_lights.data[light_id].position_radius.xyz - in.world_position.xyz);
            shadow *= calculate_contact_shadow(in.world_position.xyz, in.frag_coord.xy, L, contact_shadow_steps);
        }

        let light_contrib = lighting::point_light(light_id, &lighting_input, enable_diffuse, true);
        direct_light += light_contrib * shadow;

@if(STANDARD_MATERIAL_DIFFUSE_TRANSMISSION) {
        // NOTE: We use the diffuse transmissive color, the second Lambertian lobe's calculated
        // world position, inverted normal and view vectors, and the following simplified
        // values for a fully diffuse transmitted light contribution approximation:
        //
        // roughness = 1.0;
        // NdotV = 1.0;
        // R = vec3<f32>(0.0) // doesn't really matter
        // F_ab = vec2<f32>(0.1)
        // F0 = vec3<f32>(0.0)
        var transmitted_shadow: f32 = 1.0;
        if ((in.flags & (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)) == (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)
                && (view_bindings::clustered_lights.data[light_id].flags & mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            transmitted_shadow = shadows::fetch_point_shadow(light_id, diffuse_transmissive_lobe_world_position, -in.world_normal, in.frag_coord.xy);
        }

        let transmitted_light_contrib =
            lighting::point_light(light_id, &transmissive_lighting_input, enable_diffuse, true);
        transmitted_light += transmitted_light_contrib * transmitted_shadow;
}
    }

    // Spot lights (direct)
    for (var i: u32 = clusterable_object_index_ranges.first_spot_light_index_offset;
            i < clusterable_object_index_ranges.first_rect_light_index_offset;
            i = i + 1u) {
        let light_id = clustering::get_clusterable_object_id(i);

        // If we're lightmapped, disable diffuse contribution from the light if
        // requested, to avoid double-counting light.
        @if(LIGHTMAP)
        let enable_diffuse =
            (view_bindings::clustered_lights.data[light_id].flags &
                mesh_view_types::POINT_LIGHT_FLAGS_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE_BIT) != 0u;
        @else   // LIGHTMAP
        let enable_diffuse = true;

        var shadow: f32 = 1.0;
        if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
                && (view_bindings::clustered_lights.data[light_id].flags &
                    mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            shadow = shadows::fetch_spot_shadow(
                light_id,
                in.world_position,
                in.world_normal,
                view_bindings::clustered_lights.data[light_id].shadow_map_near_z,
                in.frag_coord.xy,
            );
        }

        @if(CONTACT_SHADOWS && DEPTH_PREPASS)
        if contact_shadow_enabled && (in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u && shadow > 0.0 &&
                (view_bindings::clustered_lights.data[light_id].flags &
                    mesh_view_types::POINT_LIGHT_FLAGS_CONTACT_SHADOWS_ENABLED_BIT) != 0u {
            let L = normalize(view_bindings::clustered_lights.data[light_id].position_radius.xyz - in.world_position.xyz);
            shadow *= calculate_contact_shadow(in.world_position.xyz, in.frag_coord.xy, L, contact_shadow_steps);
        }

        let light_contrib = lighting::spot_light(light_id, &lighting_input, enable_diffuse);
        direct_light += light_contrib * shadow;

@if(STANDARD_MATERIAL_DIFFUSE_TRANSMISSION) {
        // NOTE: We use the diffuse transmissive color, the second Lambertian lobe's calculated
        // world position, inverted normal and view vectors, and the following simplified
        // values for a fully diffuse transmitted light contribution approximation:
        //
        // roughness = 1.0;
        // NdotV = 1.0;
        // R = vec3<f32>(0.0) // doesn't really matter
        // F_ab = vec2<f32>(0.1)
        // F0 = vec3<f32>(0.0)
        var transmitted_shadow: f32 = 1.0;
        if ((in.flags & (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)) == (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)
                && (view_bindings::clustered_lights.data[light_id].flags & mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            transmitted_shadow = shadows::fetch_spot_shadow(
                light_id,
                diffuse_transmissive_lobe_world_position,
                -in.world_normal,
                view_bindings::clustered_lights.data[light_id].shadow_map_near_z,
                in.frag_coord.xy,
            );
        }

        let transmitted_light_contrib =
            lighting::spot_light(light_id, &transmissive_lighting_input, enable_diffuse);
        transmitted_light += transmitted_light_contrib * transmitted_shadow;
}
    }

    // directional lights (direct)
    let n_directional_lights = view_bindings::lights.n_directional_lights;
    // Yarra: how much of the ground around takes the sun's beam (ground_light).
    var ground_sunlit = 1.0;
    for (var i: u32 = 0u; i < n_directional_lights; i = i + 1u) {
        // check if this light should be skipped, which occurs if this light does not intersect with the view
        // note point and spot lights aren't skippable, as the relevant lights are filtered in `assign_lights_to_clusters`
        let light = &view_bindings::lights.directional_lights[i];
        // Yarra: a light giving no light (the moon by day) costs nothing.
        if all((*light).color.rgb == vec3(0.0)) {
            continue;
        }

        // If we're lightmapped, disable diffuse contribution from the light if
        // requested, to avoid double-counting light.
        @if(LIGHTMAP)
        let enable_diffuse =
            ((*light).flags &
                mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE_BIT) !=
                0u;
        @else   // LIGHTMAP
        let enable_diffuse = true;

        var shadow: f32 = 1.0;
        if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
                && (view_bindings::lights.directional_lights[i].flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            shadow = shadows::fetch_directional_shadow(i, in.world_position, in.world_normal, view_z, in.frag_coord.xy);
            shadow *= in.directional_shadow_factor;
        }

        @if(CONTACT_SHADOWS && DEPTH_PREPASS)
        if contact_shadow_enabled && (in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u && shadow > 0.0 &&
                (view_bindings::lights.directional_lights[i].flags &
                    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_CONTACT_SHADOWS_ENABLED_BIT) != 0u {
            let L = view_bindings::lights.directional_lights[i].direction_to_light;
            shadow *= calculate_contact_shadow(in.world_position.xyz, in.frag_coord.xy, L, contact_shadow_steps);
        }

        // Yarra: only direct celestial lighting is attenuated. Ambient, emissive,
        // local lights and material response retain the upstream behavior.
        let cloud = cloud_light(in.world_position.xyz, (*light).direction_to_light);
        if (in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u {
            // Past the last shadow cascade nothing casts shadows, and distant forests went
            // flat and pale. There the forest shadow map stands in for the shadow maps,
            // fading in over the outer 30% of the shadow range: the sun is dimmed by the
            // crowns around a point, and a crown also by its own foliage towards the sun.
            let cascades = (*light).num_cascades;
            let shadows_on = cascades > 0u
                && ((*light).flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u;
            let shadow_end = (*light).cascades[max(cascades, 1u) - 1u].far_bound;
            let beyond = select(1.0, smoothstep(shadow_end * 0.7, shadow_end, -view_z), shadows_on);
            var weight = beyond;
            // Impostors cast nothing into the shadow maps, so near the camera nothing would
            // shade them and they washed out. Their crown and forest term applies at every
            // distance; the shadow maps only add what is darker (neighbouring meshes).
            if lit_model_is(MODEL_IMPOSTOR) {
                weight = 1.0;
            }
            if weight > 0.0 {
                let to_light = (*light).direction_to_light;
                // The authored crown normal faces the sun on exposed sides; the crown's
                // occlusion deepens sheltered ones. The march skips the crown's own front.
                var far: f32;
                if lit_crown() {
                    var crown = clamp(in.diffuse_occlusion.g, 0.0, 1.0);
                    // Linear sky visibility, eased into the range the self-shadow was tuned on.
                    if lit_crown_sky() {
                        crown = crown_sky_shade(crown);
                    }
                    let sun_facing = smoothstep(-0.25, 0.65,
                        dot(normalize(in.world_normal), to_light));
                    let own = mix(crown * crown * crown * crown, sqrt(crown), sun_facing);
                    far = own * forest_transmittance(in.world_position.xyz, to_light, 2.5);
                } else {
                    far = forest_transmittance(in.world_position.xyz, to_light, 0.25);
                }
                // Terrain casts nothing past the cascades either: ridges shade the ground
                // below their horizon (terrain_shadow).
                let distant = far * terrain_sunlight(in.world_position.xyz, to_light);
                if lit_model_is(MODEL_IMPOSTOR) {
                    shadow = mix(min(shadow, distant), distant, beyond);
                } else {
                    shadow = mix(shadow, distant, beyond);
                }
            }
        }
        // Yarra: the ground below a point inside a crown lies in the point's own shadow, and so
        // does the terrain's around a sunlit or level point of it. A shaded tilted point of the
        // terrain may lie in a print, and anything else stands on ground the shadow maps test a
        // step towards the sun; past the cascades the terrain's own shadow holds the horizon, and
        // the ground map's does for the rest.
        if dot((*light).direction_to_light, environment.sun.xyz) > 0.999 {
            let shadows_on = ((*light).flags
                & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u;
            if lit_crown_sky() {
                ground_sunlit = shadow;
            } else if lit_model_is(MODEL_TERRAIN) {
                ground_sunlit = shadow;
                let tilted = 1.0 - smoothstep(GROUND_TILT.x, GROUND_TILT.y, in.N.y);
                if shadows_on && shadow < 1.0 && tilted > 0.0 {
                    let around = ground_in_light(i, in.world_position.xyz, view_z, shadow);
                    ground_sunlit = mix(shadow, around, tilted);
                }
            } else if shadows_on {
                ground_sunlit = ground_in_light(i, in.world_position.xyz, view_z, 1.0);
            }
        }
        // The clouds' direct beam is shaded like any sunlight; what they pass on diffusely casts
        // no shadows, and crowns hold it back as they do the sky.
        var diffuse = cloud.y * forest_sky_visibility(in.world_position.xyz);
        // Bark under its own crown holds it back as it does the sky (bark_sky_reach).
        if lit_model_is(MODEL_BARK) {
            diffuse *= diffuse_occlusion.g;
        }
        // Under clouds this is sky light: foliage the crown hides from the sky sees less of it.
        if lit_crown_sky() {
            diffuse = cloud.y * crown_sky_reach(in.diffuse_occlusion.g, in.world_position.xyz);
        }
        shadow = shadow * cloud.x + diffuse;
        var light_contrib = lighting::directional_light(i, &lighting_input, enable_diffuse);
        if lit_crown() {
            // Light wraps around a crown clump past its terminator and shines through the
            // needles from behind, so neighbouring cards fade into each other instead of
            // switching between lit and black.
            let n_dot_l = dot(in.N, (*light).direction_to_light);
            let wrap = max((n_dot_l + 0.5) / 1.5, 0.0) - max(n_dot_l, 0.0);
            let through = max(-n_dot_l, 0.0) * 0.35;
            light_contrib += diffuse_color * (1.0 / PI) * (*light).color.rgb * (wrap + through)
                * through_atmosphere(in.world_position.xyz, i);
        }

        if lit_model_is(MODEL_SKIN) {
            let to_light = (*light).direction_to_light;
            let beam = diffuse_color * (1.0 / PI) * (*light).color.rgb
                * through_atmosphere(in.world_position.xyz, i);
            let bumpy = dot(in.N, to_light);
            let soft = mix(vec3(bumpy), vec3(dot(in.world_normal, to_light)), SKIN_SOFT_NORMAL);
            let wrapped = saturate((soft + SKIN_WRAP) / (1.0 + SKIN_WRAP));
            light_contrib += beam * (wrapped - max(bumpy, 0.0));
            direct_light += beam * wrapped * (pow(vec3(saturate(shadow)), SKIN_SHADOW_EDGE) - saturate(shadow));
        }

        @if(DIRECTIONAL_LIGHT_SHADOW_MAP_DEBUG_CASCADES)
        light_contrib = shadows::cascade_debug_visualization(light_contrib, i, view_z);
        direct_light += light_contrib * shadow;

@if(STANDARD_MATERIAL_DIFFUSE_TRANSMISSION) {
        // NOTE: We use the diffuse transmissive color, the second Lambertian lobe's calculated
        // world position, inverted normal and view vectors, and the following simplified
        // values for a fully diffuse transmitted light contribution approximation:
        //
        // roughness = 1.0;
        // NdotV = 1.0;
        // R = vec3<f32>(0.0) // doesn't really matter
        // F_ab = vec2<f32>(0.1)
        // F0 = vec3<f32>(0.0)
        var transmitted_shadow: f32 = 1.0;
        if ((in.flags & (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)) == (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)
                && (view_bindings::lights.directional_lights[i].flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            transmitted_shadow = shadows::fetch_directional_shadow(i, diffuse_transmissive_lobe_world_position, -in.world_normal, view_z, in.frag_coord.xy);
        }

        let transmitted_light_contrib =
            lighting::directional_light(i, &transmissive_lighting_input, enable_diffuse);
        transmitted_light += transmitted_light_contrib * (transmitted_shadow * cloud.x + diffuse);
}
    }

@if(AREA_LIGHT_LUTS) {
    // Rect lights
    // This combines a clustered path if AVAILABLE_STORAGE_BUFFER_BINDINGS >= 3
    // and a non-clustered fallback for WebGL2.
@if(AVAILABLE_STORAGE_BUFFER_BINDINGS__GE_3)
    let rect_light_start = clusterable_object_index_ranges.first_rect_light_index_offset;
@if(AVAILABLE_STORAGE_BUFFER_BINDINGS__GE_3)
    let rect_light_end = clusterable_object_index_ranges.first_reflection_probe_index_offset;
@if(!AVAILABLE_STORAGE_BUFFER_BINDINGS__GE_3)
    let rect_light_start = 0u;
@if(!AVAILABLE_STORAGE_BUFFER_BINDINGS__GE_3)
    let rect_light_end = view_bindings::lights.n_rect_lights;
    for (var i: u32 = rect_light_start; i < rect_light_end; i = i + 1u) {
@if(AVAILABLE_STORAGE_BUFFER_BINDINGS__GE_3)
        var light = lighting::unpack_clustered_rect_light(clustering::get_clusterable_object_id(i));
@if(!AVAILABLE_STORAGE_BUFFER_BINDINGS__GE_3)
        var light = view_bindings::lights.rect_lights[i];
        let enable_diffuse = true;
        let light_contrib = lighting::rect_light(&light, &lighting_input, enable_diffuse);
        direct_light += light_contrib;

    @if(STANDARD_MATERIAL_DIFFUSE_TRANSMISSION) {
        let transmitted_light_contrib =
            lighting::rect_light(&light, &transmissive_lighting_input, enable_diffuse);
        transmitted_light += transmitted_light_contrib;
    }
    }
}

    @if(STANDARD_MATERIAL_DIFFUSE_TRANSMISSION)
    // NOTE: We use the diffuse transmissive color, the second Lambertian lobe's calculated
    // world position, inverted normal and view vectors, and the following simplified
    // values for a fully diffuse transmitted light contribution approximation:
    //
    // perceptual_roughness = 1.0;
    // NdotV = 1.0;
    // F0 = vec3<f32>(0.0)
    // diffuse_occlusion = vec3<f32>(1.0)
    transmitted_light += ambient::ambient_light(diffuse_transmissive_lobe_world_position, -in.N, -in.V, 1.0, diffuse_transmissive_color, vec3<f32>(0.0), 1.0, vec3<f32>(1.0));

    // Diffuse indirect lighting can come from a variety of sources. The
    // priority goes like this:
    //
    // 1. Lightmap (highest)
    // 2. Irradiance volume
    // 3. Environment map (lowest)
    //
    // When we find a source of diffuse indirect lighting, we stop accumulating
    // any more diffuse indirect light. This avoids double-counting if, for
    // example, both lightmaps and irradiance volumes are present.

    var indirect_light = vec3(0.0f);
    var found_diffuse_indirect = false;

@if(LIGHTMAP) {
    indirect_light += in.lightmap_light * diffuse_color;
    found_diffuse_indirect = true;
}

    @if(IRRADIANCE_VOLUME)
    // Irradiance volume light (indirect)
    if (!found_diffuse_indirect) {
        let irradiance_volume_light = irradiance_volume::irradiance_volume_light(
            in.world_position.xyz,
            in.N,
            &clusterable_object_index_ranges,
        );
        indirect_light += irradiance_volume_light * diffuse_color * diffuse_occlusion;
        found_diffuse_indirect = true;
    }

    // Environment map light (indirect)
@if(ENVIRONMENT_MAP) {
    // If screen space reflections are going to be used for this material, only
    // accumulate the diffuse part of the environment map light. The SSR shader
    // will accumulate the specular part (including the environment map fallback
    // if SSR misses).
    @if(SCREEN_SPACE_REFLECTIONS)
    let use_ssr = perceptual_roughness <= view_bindings::ssr_settings.max_perceptual_roughness
        && perceptual_roughness >= view_bindings::ssr_settings.min_perceptual_roughness;
    @else   // SCREEN_SPACE_REFLECTIONS
    let use_ssr = false;

    @if(STANDARD_MATERIAL_ANISOTROPY)
    var bent_normal_lighting_input = lighting_input;
    @if(STANDARD_MATERIAL_ANISOTROPY)
    bend_normal_for_anisotropy(&bent_normal_lighting_input);
    @if(STANDARD_MATERIAL_ANISOTROPY)
    let environment_map_lighting_input = &bent_normal_lighting_input;
    @else   // STANDARD_MATERIAL_ANISOTROPY
    let environment_map_lighting_input = &lighting_input;

    let environment_light = environment_map::environment_map_light(
        environment_map_lighting_input,
        &clusterable_object_index_ranges,
        found_diffuse_indirect,
    );

    indirect_light += environment_light.diffuse * diffuse_occlusion;
    if (!use_ssr) {
        indirect_light += environment_light.specular * specular_occlusion;
    }
}

    // Ambient light (indirect)
    // If we are lightmapped, disable the ambient contribution if requested.
    // This is to avoid double-counting ambient light. (It might be part of the lightmap)
    @if(LIGHTMAP)
    let enable_ambient = (view_bindings::lights.ambient_light_flags & mesh_view_types::AMBIENT_LIGHT_FLAGS_AFFECTS_LIGHTMAPPED_MESHES_BIT) != 0u;
    @else   // LIGHTMAP
    let enable_ambient = true;
    if (enable_ambient) {
        // Yarra: ambient comes from the sky above. Up-facing surfaces keep it all (flat
        // ground is unchanged); vertical ones get 75% and downward ones 50%, so shapes
        // still read when clouds hide the sun.
        var sky_facing = vec3(0.75 + 0.25 * in.N.y);
        // Valley floors and gullies see less of the sky than ridges (terrain_shadow).
        let terrain_sky = terrain_sky_visibility(in.world_position.xyz);
        if lit_crown_sky() {
            // Foliage sees the sky past the rest of its crown (baked) and past the crowns
            // around it, which shade the lower crowns of a stand. Light passed through and
            // between leaves keeps the deepest foliage from black. Below the crown's
            // horizon lies the ground, as for every other surface.
            let reach = crown_sky_reach(diffuse_occlusion.g, in.world_position.xyz);
            let below = 0.5 - 0.5 * in.N.y;
            let ground = ground_light(in.world_position.xyz, ground_sunlit, terrain_sky) * forest_sky_light(in.world_position.xyz);
            indirect_light += ambient::ambient_light(in.world_position, in.N, in.V, NdotV, diffuse_color, F0, perceptual_roughness, vec3(reach * (1.0 - below) * terrain_sky));
            indirect_light += diffuse_color * ground * below * reach;
        }
        else if lit_model_is(MODEL_CROWN) {
            indirect_light += ambient::ambient_light(in.world_position, in.N, in.V, NdotV, diffuse_color, F0, perceptual_roughness, diffuse_occlusion * sky_facing * terrain_sky);
        }
        else {
            // Below the horizon of a surface lies the ground, not sky: its lower half sees
            // the light the ground reflects, so trunks, cliffs and characters are not lit blue
            // from below. Flat ground is unchanged.
            let below = 0.5 - 0.5 * in.N.y;
            sky_facing = vec3(1.0 - below);
            var ground = ground_light(in.world_position.xyz, ground_sunlit, terrain_sky);
            // Under the crowns most of the sky is hidden and the ground lies in their shade;
            // leaf-filtered light stands in for part of it.
            let canopy = forest_sky_light(in.world_position.xyz);
            sky_facing *= canopy * terrain_sky;
            ground *= canopy;
            var ambient_albedo = diffuse_color;
            var ambient_occlusion = diffuse_occlusion;
            if lit_model_is(MODEL_SKIN) {
                ambient_albedo = pow(diffuse_color, SKIN_AMBIENT_ALBEDO);
                ambient_occlusion = pow(diffuse_occlusion, SKIN_AMBIENT_OCCLUSION);
            }
            indirect_light += ambient::ambient_light(in.world_position, in.N, in.V, NdotV, ambient_albedo, F0, perceptual_roughness, ambient_occlusion * sky_facing);
            indirect_light += ambient_albedo * ground * below * ambient_occlusion;
        }
    }

    // we'll use the specular component of the transmitted environment
    // light in the call to `specular_transmissive_light()` below
    var specular_transmitted_environment_light = vec3<f32>(0.0);

@if(ENVIRONMENT_MAP) {

@if(STANDARD_MATERIAL_DIFFUSE_OR_SPECULAR_TRANSMISSION) {
    // NOTE: We use the diffuse transmissive color, inverted normal and view vectors,
    // and the following simplified values for the transmitted environment light contribution
    // approximation:
    //
    // diffuse_color = vec3<f32>(1.0) // later we use `diffuse_transmissive_color` and `specular_transmissive_color`
    // NdotV = 1.0;
    // R = T // see definition below
    // F0 = vec3<f32>(1.0) (using F0_dielectric = 1, F0_metallic = 0 and metallic = 0)
    // diffuse_occlusion = 1.0
    //
    // (This one is slightly different from the other light types above, because the environment
    // map light returns both diffuse and specular components separately, and we want to use both)

    let T = -normalize(
        in.V + // start with view vector at entry point
        refract(in.V, -in.N, 1.0 / ior) * thickness // add refracted vector scaled by thickness, towards exit point
    ); // normalize to find exit point view vector

    var transmissive_environment_light_input: lighting::LightingInput;
    transmissive_environment_light_input.diffuse_color = vec3(1.0);
    transmissive_environment_light_input.layers[LAYER_BASE].NdotV = 1.0;
    transmissive_environment_light_input.P = in.world_position.xyz;
    transmissive_environment_light_input.layers[LAYER_BASE].N = -in.N;
    transmissive_environment_light_input.V = in.V;
    transmissive_environment_light_input.layers[LAYER_BASE].R = T;
    transmissive_environment_light_input.layers[LAYER_BASE].perceptual_roughness = perceptual_roughness;
    transmissive_environment_light_input.layers[LAYER_BASE].roughness = roughness;
    transmissive_environment_light_input.metallic = 0.0;
    transmissive_environment_light_input.F0_dielectric = vec3<f32>(1.0);
    transmissive_environment_light_input.F0_metallic = vec3<f32>(0.0);
    transmissive_environment_light_input.F_ab = vec2(0.1);
@if(STANDARD_MATERIAL_CLEARCOAT) {
    // No clearcoat.
    transmissive_environment_light_input.clearcoat_strength = 0.0;
    transmissive_environment_light_input.layers[LAYER_CLEARCOAT].NdotV = 0.0;
    transmissive_environment_light_input.layers[LAYER_CLEARCOAT].N = in.N;
    transmissive_environment_light_input.layers[LAYER_CLEARCOAT].R = vec3(0.0);
    transmissive_environment_light_input.layers[LAYER_CLEARCOAT].perceptual_roughness = 0.0;
    transmissive_environment_light_input.layers[LAYER_CLEARCOAT].roughness = 0.0;
}

    let transmitted_environment_light = environment_map::environment_map_light(
        &transmissive_environment_light_input,
        &clusterable_object_index_ranges,
        false,
    );

    @if(STANDARD_MATERIAL_DIFFUSE_TRANSMISSION)
    transmitted_light += transmitted_environment_light.diffuse * diffuse_transmissive_color;
    @if(STANDARD_MATERIAL_SPECULAR_TRANSMISSION)
    specular_transmitted_environment_light = transmitted_environment_light.specular * specular_transmissive_color;

}

}

    var emissive_light = emissive.rgb * output_color.a;

    // "The clearcoat layer is on top of emission in the layering stack.
    // Consequently, the emission is darkened by the Fresnel term."
    //
    // <https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_materials_clearcoat/README.md#emission>
    @if(STANDARD_MATERIAL_CLEARCOAT) {
    let clearcoat_fresnel = lighting::F_Schlick(0.04, 1.0, clearcoat_NdotV);
    emissive_light = emissive_light * (1.0 - clearcoat * clearcoat_fresnel);
    }

    emissive_light = emissive_light * mix(1.0, view_bindings::view.exposure, emissive.a);

@if(STANDARD_MATERIAL_SPECULAR_TRANSMISSION) {
    transmitted_light += transmission::specular_transmissive_light(in.world_position, in.frag_coord.xyz, view_z, in.N, in.V, F0, ior, thickness, perceptual_roughness, specular_transmissive_color, specular_transmitted_environment_light).rgb;

    if (in.material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_ATTENUATION_ENABLED_BIT) != 0u
        && in.material.attenuation_distance != 0.0 {
        // Compute light attenuation using Beer's law.
        let transmittance = pow(
            in.material.attenuation_color.rgb,
            vec3<f32>(thickness / in.material.attenuation_distance),
        );
        transmitted_light *= transmittance;
    }
}

    // Total light
    output_color = vec4<f32>(
        (view_bindings::view.exposure * (transmitted_light + direct_light + indirect_light)) + emissive_light,
        output_color.a
    );

    output_color = clustering::cluster_debug_visualization(
        output_color,
        view_z,
        in.is_orthographic,
        clusterable_object_index_ranges,
        cluster_index,
    );

    return output_color;
}
