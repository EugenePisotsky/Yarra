// Foliage lighting: rounded blade normals, the sun's diffuse, specular and transmitted light after
// clouds, shadows and the atmosphere, and bounded sky fill.
@if(YARRA_CLOUDS)
import package::shaders::clouds::surface::{cloud_light, surface_wetness};
@if(YARRA_CLOUDS)
import package::shaders::clouds::forest_shadow::{forest_sky_light, forest_sky_visibility};
@if(ATMOSPHERE)
import bevy_pbr::atmosphere::functions::{clamp_to_surface, calculate_visible_sun_ratio};

import package::shaders::grass_canopy::{canopy_visibility_at};
import package::shaders::vegetation::types::{PI, LIGHTING_MODE_UNLIT_DIAGNOSTIC};
import package::shaders::vegetation::blade::normalize3_or;
import package::shaders::vegetation::draw::bindings::{VertexOutput, camera, config};

import bevy_pbr::render::{
    pbr_lighting as pbr_lighting,
    mesh_view_bindings as view_bindings,
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    shadows,
};

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
        // Terrain-facing receiver bias is stable for thin two-sided ribbons. Using the rounded
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

fn radiance_tint(radiance: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let peak = max(max(radiance.r, radiance.g), radiance.b);
    if (peak <= 1e-6) {
        return fallback;
    }
    return radiance / peak;
}

fn stable_clump_normal(surface_normal: vec3<f32>, clump_variant: f32) -> vec3<f32> {
    let tangent = normalize3_or(
        cross(vec3<f32>(0.0, 1.0, 0.0), surface_normal),
        vec3<f32>(1.0, 0.0, 0.0),
    );
    let bitangent = normalize3_or(
        cross(surface_normal, tangent),
        vec3<f32>(0.0, 0.0, 1.0),
    );
    let angle = clump_variant * 2.0 * PI;
    let tilt_direction = tangent * cos(angle) + bitangent * sin(angle);
    // A shallow, group-stable tilt makes neighboring blades enter and leave the broad highlight
    // together. The surface normal keeps the response attached to terrain relief.
    return normalize3_or(surface_normal + tilt_direction * 0.11, surface_normal);
}

fn analytic_rounded_normal(
    flat_normal: vec3<f32>,
    ribbon_side: vec3<f32>,
    blade_u: f32,
    authored_rounding: f32,
) -> vec3<f32> {
    let orthogonal_side = normalize3_or(
        ribbon_side - flat_normal * dot(ribbon_side, flat_normal),
        vec3<f32>(1.0, 0.0, 0.0),
    );
    // A projected cylinder has x = sin(theta). Reconstructing cos(theta) here gives every covered
    // pixel a genuine curved cross-section instead of merely interpolating two slightly tilted edge
    // normals. Most authored values predate this shader and occupy the lower 0..0.4 range, so that
    // range deliberately spans flat to nearly cylindrical while retaining zero as exactly flat.
    let signed_width = clamp(blade_u * 2.0 - 1.0, -0.995, 0.995);
    let cylinder_height = sqrt(max(1.0 - signed_width * signed_width, 0.0));
    let cylinder_normal = normalize3_or(
        flat_normal * cylinder_height + orthogonal_side * signed_width,
        flat_normal,
    );
    let rounding = smoothstep(0.0, 0.42, authored_rounding);
    return normalize3_or(mix(flat_normal, cylinder_normal, rounding), flat_normal);
}

fn ggx_foliage_specular(
    normal: vec3<f32>,
    view_direction: vec3<f32>,
    light_direction: vec3<f32>,
    half_direction: vec3<f32>,
    alpha_roughness: f32,
) -> f32 {
    let n_dot_l = max(dot(normal, light_direction), 0.0);
    let n_dot_v = max(dot(normal, view_direction), 0.04);
    let n_dot_h = max(dot(normal, half_direction), 0.0);
    let v_dot_h = clamp(dot(view_direction, half_direction), 0.0, 1.0);
    let one_minus_v_dot_h = 1.0 - v_dot_h;
    let one_minus_v_dot_h_2 = one_minus_v_dot_h * one_minus_v_dot_h;
    let fresnel = 0.04
        + 0.96 * one_minus_v_dot_h_2 * one_minus_v_dot_h_2 * one_minus_v_dot_h;
    let distribution = pbr_lighting::D_GGX(alpha_roughness, n_dot_h);
    let visibility = pbr_lighting::V_SmithGGXCorrelated(
        alpha_roughness,
        n_dot_v,
        max(n_dot_l, 0.04),
    );
    return min(fresnel * distribution * visibility * n_dot_l, 1.5);
}

// The sun at a blade fragment, after clouds, the atmosphere and exposure.
struct Sun {
    direction: vec3<f32>,
    // Exposed radiance as a tint (peak 1), and its peak.
    tint: vec3<f32>,
    peak: f32,
    // What shades the direct beam and the light the clouds pass on diffusely.
    shadow: f32,
    on: f32,
    // 1 near the horizon, 0 high in the sky.
    low: f32,
}

// Rounded local normals move the highlight from one side of a nearby ribbon to the other. Once
// that variation becomes unresolved, the broad response converges on the stable clump normal.
fn sun_specular(
    blade_normal: vec3<f32>, shading_normal: vec3<f32>, view_direction: vec3<f32>,
    half_direction: vec3<f32>, roughness: f32, distance_stability: f32, camera_distance: f32,
    upper_ribbon: f32, far_highlight_weight: f32, sun: Sun,
) -> vec3<f32> {
    let perceptual_roughness = clamp(roughness, 0.08, 1.0);
    let base_alpha_roughness = perceptual_roughness * perceptual_roughness;
    let normal_width = fwidth(blade_normal);
    let normal_variance = clamp(dot(normal_width, normal_width), 0.0, 0.5);
    // Treat screen-space normal variance as unresolved microgeometry. Distance roughening remains
    // necessary for narrow far ribbons whose derivatives can be deceptively uniform per triangle.
    let filtered_alpha_roughness = clamp(
        base_alpha_roughness
            + normal_variance * 0.72
            + distance_stability * 0.34,
        0.035,
        0.95,
    );
    let filtered_broad_specular = ggx_foliage_specular(
        shading_normal,
        view_direction,
        sun.direction,
        half_direction,
        filtered_alpha_roughness,
    );
    // Leaves have a narrow waxy sheen sitting over the broad material response. Keeping this lobe
    // distinct is what makes the half-vector select one side of the analytic cylinder; a single
    // high-roughness lobe only reads as a field-wide brightness gradient.
    let local_sheen_weight = 1.0 - smoothstep(22.0, 48.0, camera_distance);
    var local_sheen_specular = 0.0;
    // Once the narrow lobe has faded out, avoid evaluating a second GGX response.
    // Normal derivatives remain above this branch so filtering stays quad-consistent.
    if local_sheen_weight > 0.0 {
        let sheen_alpha_roughness = clamp(
            mix(0.10, 0.22, perceptual_roughness) + normal_variance * 0.24,
            0.08,
            0.48,
        );
        local_sheen_specular = ggx_foliage_specular(
            blade_normal,
            view_direction,
            sun.direction,
            half_direction,
            sheen_alpha_roughness,
        );
    }
    // The narrow waxy lobe is useful while a blade has a resolvable width, but becomes glitter once
    // it is a subpixel line. A broad clump lobe survives so sunrise still sweeps coherently across
    // the field instead of flattening to diffuse-only shading.
    let broad_specular_weight = mix(0.16, 0.26, distance_stability);
    let specular_lobe = filtered_broad_specular * broad_specular_weight
        + local_sheen_specular * local_sheen_weight;

    let specular_elevation = mix(0.22, 1.0, sun.low);
    let specular_energy = min(sun.peak, 8.0);
    return sun.tint
        * specular_lobe
        * upper_ribbon
        * specular_elevation
        * far_highlight_weight
        * specular_energy
        * camera.lighting.y
        * sun.shadow
        * sun.on;
}

// A leaf's body is a thin sheet. The strongly rounded normal shapes its waxy gloss; using that same
// cylinder for diffuse rolls exposed margins out of the sun and paints dark longitudinal stripes
// onto a lit face. Retain only a shallow fold for body illumination.
fn sun_diffuse(
    body_color: vec3<f32>, flat_blade_normal: vec3<f32>, blade_normal: vec3<f32>,
    clump_normal: vec3<f32>, distance_stability: f32, sun: Sun,
) -> vec3<f32> {
    let diffuse_normal = normalize3_or(
        mix(flat_blade_normal, blade_normal, 0.18 * (1.0 - distance_stability)),
        flat_blade_normal,
    );
    let leaf_n_dot_l = dot(diffuse_normal, sun.direction);
    let directional_diffuse = clamp((leaf_n_dot_l + 0.18) / 1.18, 0.0, 1.0);
    let canopy_diffuse = clamp((dot(clump_normal, sun.direction) + 0.18) / 1.18, 0.0, 1.0);
    // Average part of the unresolved illumination, retaining 35% of the blade response.
    // Leaving its full contrast on subpixel ribbons produces a stippled horizon; replacing it
    // completely with a common normal removes the directional variation we need to preserve.
    let diffuse_filter = distance_stability * 0.65;
    let wrapped_diffuse = mix(directional_diffuse, canopy_diffuse, diffuse_filter);
    let diffuse_energy = min(sun.peak * camera.lighting.x, 1.20);
    return body_color
        * sun.tint
        * wrapped_diffuse
        * diffuse_energy
        * sun.shadow
        * sun.on;
}

// Low sun seen through the upper blade, toward the light.
fn sun_transmission(
    body_color: vec3<f32>, view_direction: vec3<f32>, amount: f32, upper_ribbon: f32,
    far_highlight_weight: f32, sun: Sun,
) -> vec3<f32> {
    let backscatter_alignment = clamp(dot(-view_direction, sun.direction), 0.0, 1.0);
    let backscatter = smoothstep(0.14, 0.86, backscatter_alignment);
    let transmission_energy = min(sun.peak, 3.0);
    return mix(
        body_color * sun.tint,
        sun.tint,
        0.22,
    ) * backscatter
        * sun.low
        * upper_ribbon
        * amount
        * far_highlight_weight
        * transmission_energy
        * camera.lighting.z
        * sun.shadow
        * sun.on;
}

fn shade(input: VertexOutput) -> vec4<f32> {
    if (input.material.w < 0.5) {
        return vec4<f32>(input.color, 1.0);
    }
    if (config.values.z == LIGHTING_MODE_UNLIT_DIAGNOSTIC) {
        return vec4<f32>(input.color, 1.0);
    }

    let view_direction = normalize3_or(
        camera.camera_position.xyz - input.world_position,
        vec3<f32>(0.0, 1.0, 0.0),
    );
    // Vegetation ribbons are two-sided. Face the physical blade plane toward the viewer instead of
    // deriving its sign from triangle winding: adjacent transported width axes can twist a highly
    // curved strip without meaning that its lighting side should invert.
    let face_sign = select(-1.0, 1.0, dot(input.world_normal, view_direction) >= 0.0);
    let flat_blade_normal = normalize3_or(
        input.world_normal * face_sign,
        vec3<f32>(0.0, 1.0, 0.0),
    );
    let blade_normal = analytic_rounded_normal(
        flat_blade_normal,
        input.ribbon_side_rounding.xyz,
        input.blade_u,
        input.ribbon_side_rounding.w,
    );
    let light_direction = normalize3_or(camera.sun_direction.xyz, vec3<f32>(0.0, 1.0, 0.0));
    let camera_distance = distance(camera.camera_position.xyz, input.world_position);
    // Blend unresolved middle/far blades toward a stable up-dominated field normal. This preserves
    // broad lighting direction while preventing animated or densely alternating ribbon normals from
    // turning into specular glitter.
    let field_normal = normalize3_or(
        vec3<f32>(blade_normal.x * 0.16, 1.0, blade_normal.z * 0.16),
        vec3<f32>(0.0, 1.0, 0.0),
    );
    let distance_stability = smoothstep(20.0, 72.0, camera_distance);
    let normal = normalize3_or(
        mix(blade_normal, field_normal, distance_stability),
        field_normal,
    );
    let half_direction = normalize3_or(light_direction + view_direction, normal);
    let ambient_occlusion = clamp(input.material.z, 0.0, 1.0);
    let canopy_visibility = canopy_visibility_at(input.world_position.xz,
        input.canopy_coordinates.x, input.canopy_coordinates.y,
        camera.canopy_appearance, camera.canopy_shape, camera.canopy_distance,
        camera.canopy_origin, camera.canopy_appearance.z, input.canopy_coordinates.z, input.canopy_coordinates.w);
    let shadow_visibility = directional_shadow_visibility(input);
    // Sunlight passing the cloud layer at this point, and the share of it in the direct beam
    // (the rest the clouds pass on diffusely, casting no shadows).
    var cloud_sun = 1.0;
    var cloud_beam = 1.0;
    @if(YARRA_CLOUDS)
    let cloud = cloud_light(input.world_position, camera.sun_direction.xyz);
    @if(YARRA_CLOUDS) {
        cloud_sun = cloud.x + cloud.y;
        cloud_beam = cloud.x;
    }
    // Shadowing of the sun terms: the shadow maps shade the direct beam, while what the clouds
    // pass on diffusely casts no shadows and crowns hold it back as they do the sky.
    var sun_shadow = shadow_visibility;
    @if(YARRA_CLOUDS)
    sun_shadow = (cloud.x * shadow_visibility + cloud.y * forest_sky_visibility(input.world_position))
        / max(cloud_sun, 1e-4);
    // The old receiver cache could only darken direct light and therefore became almost invisible
    // under the stable authored body color. Let dense/AO-heavy blade regions lose part of that body
    // as well, while retaining enough ambient fill to avoid black cutout silhouettes. A sun shadow
    // cannot darken anything once clouds block the sun itself, as on PBR surfaces.
    let shadow_floor = mix(0.16, 0.42, ambient_occlusion);
    let received_shadow = mix(
        1.0,
        mix(shadow_floor, 1.0, shadow_visibility),
        camera.lighting.w * cloud_beam,
    );
    let sun_tint = radiance_tint(camera.sun_radiance.xyz, vec3<f32>(1.0));
    let sun_active = camera.sun_direction.w;

    // Wet blades darken like wet PBR surfaces, a little less since grass is already dark.
    var body_color = input.color;
    @if(YARRA_CLOUDS)
    body_color *= mix(1.0, 0.7, surface_wetness(input.world_position, vec3<f32>(0.0, 1.0, 0.0)));

    // Direct-light energy must use the same camera exposure as Bevy's PBR path. Normalizing the
    // directional radiance to a tint made a 100,000-lux sun indistinguishable from a dim light.
    var atmospheric_sun = camera.sun_radiance.xyz * cloud_sun;
    @if(ATMOSPHERE) {
        let atmosphere = view_bindings::atmosphere;
        let p_as = (atmosphere.world_to_atmosphere * vec4(input.world_position, 1.0)).xyz;
        let p_clamped = clamp_to_surface(atmosphere, p_as);
        let r = length(p_clamped);
        let mu = dot(light_direction, normalize(p_clamped));
        atmospheric_sun *= pbr_lighting::sample_transmittance_lut(r, mu)
            * calculate_visible_sun_ratio(atmosphere, r, mu, view_bindings::lights.directional_lights[vegetation_light_index()].sun_disk_angular_size);
    }
    let exposed_sun = atmospheric_sun * view_bindings::view.exposure;
    let exposed_sun_peak = max(max(exposed_sun.r, exposed_sun.g), exposed_sun.b);
    let exposed_sun_tint = radiance_tint(exposed_sun, sun_tint);
    let sun_elevation = clamp(light_direction.y, 0.0, 1.0);
    let low_sun = 1.0 - smoothstep(0.28, 0.72, sun_elevation);
    let sun = Sun(light_direction, exposed_sun_tint, exposed_sun_peak, sun_shadow, sun_active, low_sun);

    // Rounded local normals move the highlight from one side of a nearby ribbon to the other.
    // Once that variation becomes unresolved, converge the broad specular response on a stable
    // clump normal. Diffuse lighting below retains each blade's direction: sharing this normal
    // with diffuse made the field uniformly lit when viewed away from the sun.
    let surface_normal = normalize3_or(
        input.surface_normal_clump.xyz,
        vec3<f32>(0.0, 1.0, 0.0),
    );
    let clump_normal = stable_clump_normal(surface_normal, input.surface_normal_clump.w);
    let shading_normal = normalize3_or(
        mix(blade_normal, clump_normal, distance_stability),
        clump_normal,
    );
    let upper_ribbon = smoothstep(0.14, 0.78, input.blade_t);
    let far_highlight_weight = mix(
        1.0,
        0.08,
        smoothstep(38.0, 84.0, camera_distance),
    );
    let highlight = sun_specular(
        blade_normal, shading_normal, view_direction, half_direction, input.material.x,
        distance_stability, camera_distance, upper_ribbon, far_highlight_weight, sun,
    );
    let diffuse = sun_diffuse(
        body_color, flat_blade_normal, blade_normal, clump_normal, distance_stability, sun,
    );
    let transmission = sun_transmission(
        body_color, view_direction, input.material.y, upper_ribbon, far_highlight_weight, sun,
    );

    // A broad upper-hemisphere fill keeps sky-facing surfaces readable between sun highlights.
    // Use scene intensity and exposure rather than a normalized tint plus fixed body brightness.
    // This is directional ambient illumination, not visibility of neighboring blades.
    let exposed_ambient = camera.ambient_radiance.xyz * view_bindings::view.exposure;
    let ambient_peak = max(max(exposed_ambient.r, exposed_ambient.g), exposed_ambient.b);
    // The bound keeps clear-day fill stylized; weather's exposure adaptation raises it.
    let ambient_bound = 0.60 * select(1.0, camera.ambient_radiance.w, camera.ambient_radiance.w > 0.0);
    let bounded_ambient = exposed_ambient / max(1.0, ambient_peak / ambient_bound);
    // Both sides of a thin leaf receive sky fill; viewer-facing normal flips must not blacken
    // the underside of an otherwise exposed leaf. Directional contrast comes from the sun term.
    let diffuse_filter = distance_stability * 0.65;
    let sky_facing = mix(abs(flat_blade_normal.y), clump_normal.y, diffuse_filter);
    var sky_fill = vec3(mix(0.75, 1.0, clamp(sky_facing, 0.0, 1.0)));
    // Under tree crowns most of the sky is hidden, as for the ground below.
    @if(YARRA_CLOUDS)
    sky_fill *= forest_sky_light(input.world_position);
    let foliage_ambient = body_color
        * bounded_ambient
        * mix(0.40, 0.85, ambient_occlusion)
        * sky_fill
        * received_shadow;

    return vec4<f32>((foliage_ambient + diffuse + transmission + highlight) * canopy_visibility, 1.0);
}
