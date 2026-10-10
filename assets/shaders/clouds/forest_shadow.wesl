// Distant forest shadows (crates/atmosphere/src/forest_shadow.rs). Past the shadow maps, a
// point marches towards the sun through a coarse top-down map of crowns: per texel the highest
// crown top, lowest crown base, foliage density and the local canopy top (the highest crown
// within about 100 m). Foliage met between them dims the sun, so distant crowns shade each other
// and the ground below as the shadow maps do near the camera. Points with no crown nearby
// above them skip the march. At every distance the same map holds back sky light under the
// crowns (forest_sky_visibility). Materials that light through pbr_lighting bind the map.
import package::shaders::clouds::surface::{forest_shadow_parameters, forest_sky_parameters};
import package::shaders::clouds::maps::forest_sky_share;

@group(constants::MATERIAL_BIND_GROUP) @binding(124) var forest_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(125) var forest_sampler: sampler;

// Sunlight lost per metre of fully dense crown; calibrated against the shadow maps.
const FOREST_EXTINCTION: f32 = 0.35;
const FOREST_STEPS: u32 = 8u;
// Longer shadows (a low sun) fade out rather than march further.
const FOREST_REACH: f32 = 160.0;

// Sunlight reaching `p` through the forest, starting `start` metres along the ray (a crown
// skips its own front foliage). 1 without a map or a sun below the horizon.
fn forest_transmittance(p: vec3<f32>, to_light: vec3<f32>, start: f32) -> f32 {
    let parameters = forest_shadow_parameters();
    if parameters.z <= 0.0 || to_light.y <= 0.02 {
        return 1.0;
    }
    let extent = parameters.z * vec2<f32>(textureDimensions(forest_map));
    let here = (p.xz - parameters.xy) / extent;
    if any(here <= vec2(0.0)) || any(here >= vec2(1.0)) {
        return 1.0;
    }
    // The march ends where the ray rises above every crown near its start.
    let canopy = textureSampleLevel(forest_map, forest_sampler, here, 0.0).a;
    let end = min((canopy - p.y) / to_light.y, FOREST_REACH);
    if end <= start {
        return 1.0;
    }
    let step = (end - start) / f32(FOREST_STEPS);
    var depth = 0.0;
    for (var i = 0u; i < FOREST_STEPS; i += 1u) {
        let q = p + to_light * (start + (f32(i) + 0.5) * step);
        let uv = (q.xz - parameters.xy) / extent;
        if any(uv <= vec2(0.0)) || any(uv >= vec2(1.0)) {
            continue;
        }
        let crown = textureSampleLevel(forest_map, forest_sampler, uv, 0.0);
        // Inside the crown layer, softened by a metre at its base and top.
        let inside = smoothstep(crown.g - 1.0, crown.g + 1.0, q.y)
            * (1.0 - smoothstep(crown.r - 1.0, crown.r + 1.0, q.y));
        depth += crown.b * inside * step;
    }
    return exp(-FOREST_EXTINCTION * depth);
}

// Share of the sky's light reaching `p` past the crowns around and above it, scaled by the
// forest_sky strength. 1 without a map, or with no crown above nearby.
fn forest_sky_visibility(p: vec3<f32>) -> f32 {
    return forest_sky_share(forest_map, forest_sampler, forest_shadow_parameters(),
        forest_sky_parameters().x, p);
}

// Light that replaces the sky the crowns hide, as a share of the sky's light: daylight passed
// through and reflected between leaves, so shade under crowns turns green-gold instead of the sky's
// blue. Dim against the open sky; a closed stand's floor stays dark.
const CANOPY_FILL: vec3<f32> = vec3(0.08, 0.13, 0.03);

// The sky's light reaching `p` under crowns per channel: the open share of the sky plus the
// leaf-filtered light standing in for the rest. 1 where no crowns are near.
fn forest_sky_light(p: vec3<f32>) -> vec3<f32> {
    let open = forest_sky_visibility(p);
    return open + (1.0 - open) * CANOPY_FILL;
}
