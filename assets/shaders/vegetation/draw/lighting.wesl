// Grass lighting: rounded blade normals, the sun's diffuse, sheen and transmitted light after
// clouds, shadows and the atmosphere, and bounded sky fill. Distant blades take their sheen from
// their clump's stable normal, so the field glitters as broad sweeps instead of noise.
@if(ENVIRONMENT_SURFACE)
import package::shaders::lighting::surface::{cloud_light, surface_wetness};
@if(ENVIRONMENT_SURFACE)
import package::shaders::lighting::forest_shadow::{forest_sky_light, forest_sky_visibility};
@if(ATMOSPHERE)
import bevy_pbr::atmosphere::functions::{clamp_to_surface, calculate_visible_sun_ratio};

import package::shaders::vegetation::types::PI;
import package::shaders::vegetation::blade::{normalize3_or, value_noise};
import package::shaders::vegetation::draw::bindings::{VertexOutput, camera};

import bevy_pbr::render::{
    pbr_lighting as pbr_lighting,
    mesh_view_bindings as view_bindings,
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    shadows,
};

// The cross-section of a blade is curved: its normal turns by up to this many radians toward
// either edge (the talk's rounded normals).
const ROUNDING: f32 = 0.8;
// Distance over which unresolved blades hand their sheen and part of their diffuse direction to
// their clump.
const STABILITY_DISTANCE: vec2<f32> = vec2(16.0, 64.0);
// How much wider the distant field's sheen lobe is than one blade's (GGX alpha): blades lean
// every way, so together they reflect over a broad cone.
const FIELD_SHEEN_SPREAD: f32 = 0.4;

fn directional_shadow_visibility(input: VertexOutput) -> f32 {
    if (camera.sun_direction.w <= 0.0) {
        return 1.0;
    }
    let light_index = vegetation_light_index();
    let light = &view_bindings::lights.directional_lights[light_index];
    if (((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u) {
        return 1.0;
    }
    let world_position = vec4<f32>(input.world_position, 1.0);
    let view_z = dot(vec4<f32>(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z,
    ), world_position);
    return shadows::fetch_directional_shadow(
        light_index,
        world_position,
        // Terrain-facing receiver bias is stable for thin two-sided blades. Using the rounded
        // blade normal here can offset samples below terrain and make shadows blink by facing.
        vec3<f32>(0.0, 1.0, 0.0),
        view_z,
        input.clip_position.xy,
    );
}

// Bevy sorts lights by shadow/volume policy, not by illuminance. Locate the
// selected surface light so moonlight samples the moon's cascades and disk size.
fn vegetation_light_index() -> u32 {
    var selected = 0u;
    var closest = -2.0;
    for (var i = 0u; i < view_bindings::lights.n_directional_lights; i += 1u) {
        let alignment = dot(view_bindings::lights.directional_lights[i].direction_to_light,
            camera.sun_direction.xyz);
        if alignment > closest {
            closest = alignment;
            selected = i;
        }
    }
    return selected;
}

// Sunlight left after the blades above a point deep in the canopy. Not their real shadows: the
// beam is broken by a pattern of gaps where it enters the canopy top, fewer the deeper the
// point, so lower blades carry shadow lines that move with the sun (Ghost of Tsushima dithers a
// raised terrain into its shadow maps for the same look). The pattern fades to its mean, the
// beam's transmittance through the canopy, where it would alias.
const CANOPY_EXTINCTION: f32 = 2.3;
const CANOPY_GAP_SCALE: f32 = 0.13;
const CANOPY_PATTERN_DISTANCE: vec2<f32> = vec2(10.0, 24.0);
fn canopy_light(position: vec3<f32>, canopy: vec2<f32>, light: vec3<f32>, camera_distance: f32) -> f32 {
    let depth = canopy.x;
    if (depth <= 0.0) {
        return 1.0;
    }
    let mean = exp(-CANOPY_EXTINCTION * depth);
    let pattern = 1.0 - smoothstep(CANOPY_PATTERN_DISTANCE.x, CANOPY_PATTERN_DISTANCE.y, camera_distance);
    if (pattern <= 0.0) {
        return mean;
    }
    // Where the beam to this point entered the canopy, in canonical metres.
    let entry = position.xz + camera.render_origin.xy
        + light.xz / max(light.y, 0.2) * depth * canopy.y;
    let p = entry / CANOPY_GAP_SCALE;
    let n = value_noise(p) * 0.6 + value_noise(p * 2.3 + vec2(7.1, 3.7)) * 0.4;
    let lit = smoothstep(0.92 - mean - 0.16, 0.92 - mean + 0.16, n);
    return mix(mean, lit, pattern * 0.75);
}

fn ggx_specular(
    normal: vec3<f32>, view_direction: vec3<f32>, light_direction: vec3<f32>, alpha: f32,
) -> f32 {
    let half_direction = normalize3_or(light_direction + view_direction, normal);
    let n_dot_l = max(dot(normal, light_direction), 0.0);
    let n_dot_v = max(dot(normal, view_direction), 0.04);
    let n_dot_h = max(dot(normal, half_direction), 0.0);
    let v_dot_h = clamp(dot(view_direction, half_direction), 0.0, 1.0);
    let f = 1.0 - v_dot_h;
    let fresnel = 0.04 + 0.96 * f * f * f * f * f;
    let distribution = pbr_lighting::D_GGX(alpha, n_dot_h);
    let visibility = pbr_lighting::V_SmithGGXCorrelated(alpha, n_dot_v, max(n_dot_l, 0.04));
    return min(fresnel * distribution * visibility * n_dot_l, 2.0);
}

fn shade(input: VertexOutput) -> vec4<f32> {
    let view_direction = normalize3_or(
        camera.camera_position.xyz - input.world_position,
        vec3<f32>(0.0, 1.0, 0.0),
    );
    // Blades are two-sided: the lit face is the one toward the viewer.
    let face = select(-1.0, 1.0, dot(input.world_normal, view_direction) >= 0.0);
    let flat_normal = normalize3_or(input.world_normal * face, vec3<f32>(0.0, 1.0, 0.0));
    let across = normalize3_or(
        input.side - flat_normal * dot(input.side, flat_normal),
        vec3<f32>(1.0, 0.0, 0.0),
    );
    let angle = clamp(input.blade_u, -1.0, 1.0) * ROUNDING;
    let rounded = normalize3_or(flat_normal * cos(angle) + across * sin(angle), flat_normal);
    let camera_distance = distance(camera.camera_position.xyz, input.world_position);
    let stability = smoothstep(STABILITY_DISTANCE.x, STABILITY_DISTANCE.y, camera_distance);
    let clump = normalize3_or(input.clump_normal, vec3<f32>(0.0, 1.0, 0.0));
    let light_direction = normalize3_or(camera.sun_direction.xyz, vec3<f32>(0.0, 1.0, 0.0));
    let t = input.blade_t;
    let occlusion = clamp(input.material.w, 0.0, 1.0);

    // The sun at this point. `beam` is the share arriving straight from its direction past the
    // clouds and the shadow maps; `scattered` the share the clouds pass on diffusely, which lights
    // blades as on the ground but makes no sheen and shines through none. Crowns hold the
    // scattered share back as they do the sky.
    let shadow_visibility = directional_shadow_visibility(input);
    let shadow = mix(1.0, shadow_visibility, camera.lighting.w);
    var beam = shadow;
    var scattered = 0.0;
    var cloud_beam = 1.0;
    @if(ENVIRONMENT_SURFACE) {
        let cloud = cloud_light(input.world_position, camera.sun_direction.xyz);
        cloud_beam = cloud.x;
        beam = cloud.x * shadow;
        scattered = cloud.y * mix(1.0, forest_sky_visibility(input.world_position), camera.lighting.w);
    }
    var sun_radiance = camera.sun_radiance.xyz;
    @if(ATMOSPHERE) {
        let atmosphere = view_bindings::atmosphere;
        let p_as = (atmosphere.world_to_atmosphere * vec4(input.world_position, 1.0)).xyz;
        let p_clamped = clamp_to_surface(atmosphere, p_as);
        let r = length(p_clamped);
        let mu = dot(light_direction, normalize(p_clamped));
        sun_radiance *= pbr_lighting::sample_transmittance_lut(r, mu)
            * calculate_visible_sun_ratio(atmosphere, r, mu,
                view_bindings::lights.directional_lights[vegetation_light_index()].sun_disk_angular_size);
    }
    let exposed_sun = sun_radiance * view_bindings::view.exposure * camera.sun_direction.w;
    beam *= canopy_light(input.world_position, input.canopy, light_direction, camera_distance);
    // Lower blades sit in the shade of their neighbours.
    let self_shadow = mix(0.6, 1.0, occlusion);

    // Wet blades darken like wet PBR surfaces, a little less since grass is already dark.
    var albedo = input.color;
    @if(ENVIRONMENT_SURFACE)
    albedo *= mix(1.0, 0.7, surface_wetness(input.world_position, vec3<f32>(0.0, 1.0, 0.0)));

    // Diffuse: a thin sheet, its normal halfway to the clump's ground so a field lit from above
    // reads as lit like the ground beneath it, wrapping the light a little; unresolved blades
    // turn fully to the clump so the far field is not stippled. Energy as on PBR surfaces.
    let diffuse_normal = normalize3_or(
        mix(mix(flat_normal, rounded, 0.25), clump, mix(0.55, 1.0, stability)),
        clump,
    );
    let wrapped = clamp((dot(diffuse_normal, light_direction) + 0.25) / 1.25, 0.0, 1.0);
    let diffuse = albedo * exposed_sun * wrapped / PI * (beam + scattered) * self_shadow
        * camera.lighting.x;

    // Transmission: the sun's beam through blades lit from behind, strongest looking toward it.
    let behind = clamp(dot(-flat_normal, light_direction), 0.0, 1.0);
    let toward_sun = smoothstep(-0.2, 0.9, dot(-view_direction, light_direction));
    let transmission = albedo * exposed_sun / PI * beam
        * behind * mix(0.35, 1.0, toward_sun) * input.material.y
        * camera.lighting.z * smoothstep(0.1, 0.6, t);

    // Sheen: the beam's highlight sliding across each rounded blade nearby. Far away, where
    // blades are unresolved, a field of blades leaning every way reflects as a broad lobe about
    // its clump's normal, not as a polished ground: a soft sheen toward the light, no glare.
    let roughness = clamp(input.material.x, 0.08, 1.0);
    let alpha_near = roughness * roughness;
    let normal_width = fwidth(rounded);
    let variance = clamp(dot(normal_width, normal_width), 0.0, 0.5);
    let near_sheen = ggx_specular(rounded, view_direction, light_direction,
        clamp(alpha_near + variance * 0.5, 0.02, 0.9));
    let far_sheen = ggx_specular(clump, view_direction, light_direction,
        clamp(alpha_near + FIELD_SHEEN_SPREAD, 0.3, 0.95));
    let along = smoothstep(0.05, 0.45, t) * (1.0 - 0.35 * t);
    let sheen = mix(near_sheen, far_sheen * 0.3, stability) * along * input.material.z;
    let highlight = exposed_sun * sheen * beam * camera.lighting.y;

    let sun = diffuse + transmission + highlight;

    // Sky fill from above, for both faces of the thin blade; bounded in exposed units for the
    // clear-day look, raised with exposure adaptation under weather.
    let exposed_ambient = camera.ambient_radiance.xyz * view_bindings::view.exposure;
    let ambient_peak = max(max(exposed_ambient.r, exposed_ambient.g), exposed_ambient.b);
    let ambient_bound = 0.60 * select(1.0, camera.ambient_radiance.w, camera.ambient_radiance.w > 0.0);
    let bounded_ambient = exposed_ambient / max(1.0, ambient_peak / ambient_bound);
    let sky_facing = mix(abs(flat_normal.y), clump.y, stability * 0.6);
    var sky_fill = vec3(mix(0.7, 1.0, clamp(sky_facing, 0.0, 1.0)));
    @if(ENVIRONMENT_SURFACE)
    sky_fill *= forest_sky_light(input.world_position);
    // A sun shadow takes part of the fill too, so shaded grass is not flat: as on the ground.
    let shade_dimming = mix(1.0, mix(0.55, 1.0, shadow_visibility), camera.lighting.w * cloud_beam);
    let ambient = albedo * bounded_ambient * mix(0.2, 1.0, occlusion) * sky_fill * shade_dimming;

    return vec4<f32>(ambient + sun, 1.0);
}
