// The ground around the camera, seen from above (environment/maps.wesl `ground_texel`). What lies
// below a surface reflects light onto it in its own colour, the sun's only where the ground takes
// it; ground the map does not hold is open, sunlit grass and soil. Materials that light through
// lighting/pbr_lighting bind the map.
import package::shaders::environment::maps::ground_texel;
import package::shaders::lighting::surface::environment;

@group(constants::MATERIAL_BIND_GROUP) @binding(128) var ground_map: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(129) var ground_sampler: sampler;

// Grass and soil reflect a dim green-gold.
const GROUND_FALLBACK: vec3<f32> = vec3(0.09, 0.12, 0.05);

// The ground below render XZ `p`: its albedo, and the cosine of the sun's beam on it.
fn ground_below(p: vec2<f32>) -> vec4<f32> {
    let open = vec4(GROUND_FALLBACK, max(environment.sun.y, 0.0));
    return ground_texel(ground_map, ground_sampler, environment.ground_near,
        environment.ground_far, p, open);
}
