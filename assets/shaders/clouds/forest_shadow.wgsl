// Distant forest shadows (crates/atmosphere/src/forest_shadow.rs). Past the shadow maps, a
// point marches towards the sun through a coarse top-down map of crowns: per texel the highest
// crown top, lowest crown base, foliage density and the local canopy top (the highest crown
// within about 100 m). Foliage met between them dims the sun, so distant crowns shade each other
// and the ground below as the shadow maps do near the camera. Points with no crown nearby
// above them skip the march. Only pipelines that bind the map define FOREST_SHADOW.
#import "shaders/clouds/surface.wgsl"::forest_shadow_parameters

@group(#{MATERIAL_BIND_GROUP}) @binding(124) var forest_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(125) var forest_sampler: sampler;

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
