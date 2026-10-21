// The ground around the camera, seen from above (crates/atmosphere/src/ground_albedo.rs, filled by
// terrain/ground_albedo.wesl): its albedo, and the cosine at which the sun's beam meets it past the
// terrain's horizon. What lies below a surface reflects light onto it in its own colour, the sun's
// only where the ground takes it. The near layer lies over the far one and fades into it at its
// edge; ground neither holds is open, sunlit grass and soil. Materials that light through
// lighting/pbr_lighting bind the map.
import package::shaders::lighting::surface::{environment, ground_map_parameters};

@group(constants::MATERIAL_BIND_GROUP) @binding(128) var ground_map: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(129) var ground_sampler: sampler;

// Grass and soil reflect a dim green-gold.
const GROUND_FALLBACK: vec3<f32> = vec3(0.09, 0.12, 0.05);
// The near layer fades into the far one over this many of its texels.
const NEAR_FADE: f32 = 8.0;

// The ground below render XZ `p`: its albedo, and the cosine of the sun's beam on it.
fn ground_below(p: vec2<f32>) -> vec4<f32> {
    let open = vec4(GROUND_FALLBACK, max(environment.sun.y, 0.0));
    let near = ground_map_parameters(0u);
    if near.z <= 0.0 {
        return open;
    }
    let size = f32(textureDimensions(ground_map).x);
    let near_weight = layer_weight(p, near, size, NEAR_FADE);
    let near_texel = layer_texel(p, near, size, 0u, near_weight);
    if near_weight >= 1.0 {
        return near_texel;
    }
    let far = ground_map_parameters(1u);
    let far_weight = layer_weight(p, far, size, 1.0);
    let below = mix(open, layer_texel(p, far, size, 1u, far_weight), far_weight);
    return mix(below, near_texel, near_weight);
}

// How much of a layer placed at `placement` (`size` texels a side) holds at `p`: none past its
// edge, all from `fade` texels inside it, measured from the outer texels' centres so filtering
// never reaches past them.
fn layer_weight(p: vec2<f32>, placement: vec4<f32>, size: f32, fade: f32) -> f32 {
    let texels = (p - placement.xy) / placement.z;
    let inside = min(min(texels.x, texels.y), size - max(texels.x, texels.y)) - 0.5;
    return clamp(inside / fade, 0.0, 1.0);
}

fn layer_texel(p: vec2<f32>, placement: vec4<f32>, size: f32, layer: u32, weight: f32) -> vec4<f32> {
    if weight <= 0.0 {
        return vec4(0.0);
    }
    let uv = (p - placement.xy) / (placement.z * size);
    return textureSampleLevel(ground_map, ground_sampler, uv, layer, 0.0);
}
