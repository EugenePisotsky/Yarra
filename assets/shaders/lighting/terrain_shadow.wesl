// Distant terrain shadows and sky occlusion (crates/atmosphere/src/terrain_shadow.rs). Past the
// shadow maps, a point is lit by the sun (or the moon) only above the terrain's horizon towards
// it: a coarse map holds per texel that horizon's tangent and distance for the light's compass
// direction, the ground height and the share of the sky the terrain around leaves open. The
// light's elevation is exact here, so shadows lengthen and shorten smoothly between rebuilds.
// Materials that light through lighting/pbr_lighting bind the map.
import package::shaders::lighting::surface::{terrain_shadow_parameters, terrain_light_parameters};

@group(constants::MATERIAL_BIND_GROUP) @binding(126) var terrain_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(127) var terrain_sampler: sampler;

// The map's texel at `p` (horizon tangent, ground height, sky share, horizon distance); open to
// light and sky where there is no map.
fn terrain_texel(p: vec3<f32>) -> vec4<f32> {
    let parameters = terrain_shadow_parameters();
    if parameters.z <= 0.0 {
        return vec4(-4.0, p.y, 1.0, 1.0e4);
    }
    let uv = (p.xz - parameters.xy) / (parameters.z * vec2<f32>(textureDimensions(terrain_map)));
    if any(uv <= vec2(0.0)) || any(uv >= vec2(1.0)) {
        return vec4(-4.0, p.y, 1.0, 1.0e4);
    }
    return textureSampleLevel(terrain_map, terrain_sampler, uv, 0.0);
}

// Share of the light towards `to_light` reaching `p` past the terrain: 1 above the horizon,
// 0 below it. 1 for a light whose compass direction the map does not hold.
fn terrain_sunlight(p: vec3<f32>, to_light: vec3<f32>) -> f32 {
    let parameters = terrain_shadow_parameters();
    let level = length(to_light.xz);
    if parameters.z <= 0.0 || to_light.y <= 0.0 || level < 1.0e-4 {
        return 1.0;
    }
    if dot(to_light.xz / level, terrain_light_parameters().xy) < 0.995 {
        return 1.0;
    }
    let t = terrain_texel(p);
    // A point above the ground looks over a horizon `t.w` metres off by its height.
    let above = max(p.y - t.y, 0.0);
    let distance = max(t.w, parameters.z);
    let horizon = t.x - above / distance;
    // Soft over the angle a texel spans at the horizon's distance: coarse ground blurs a
    // shadow's edge rather than stepping it.
    let soft = 0.01 + 0.5 * parameters.z / distance;
    return smoothstep(horizon - soft, horizon + soft, to_light.y / level);
}

// Share of the sky's light the terrain around `p` leaves open: valley floors and gullies see
// less of the sky than ridges. A point above the ground sees more of it.
fn terrain_sky_visibility(p: vec3<f32>) -> f32 {
    let t = terrain_texel(p);
    let lift = smoothstep(0.0, 40.0, p.y - t.y);
    return mix(clamp(t.z, 0.0, 1.0), 1.0, lift);
}
