// The sun as the passes that draw its light share it (atmosphere/src/sun_glare.rs,
// light_shafts.rs, ambient_particles.rs): its glare kernels, sunlight past one of its shadow-map
// cascades, and the noise that offsets marches.

// Glare of the eye and lens around the sun: a core about a degree wide and a veil over tens of
// degrees, each holding a share of the sun's light. The core is scaled by the share of the sun
// seen, the veil by the share of it reaching the pixel past what lies between, so beams fan out
// from silhouettes in front of the sun.
// Kernels are normalised over the image plane: core (1 + u)^-2 / (pi w^2), veil
// (1 + u)^-1.25 / (4 pi w^2) and air glow (1 + u)^-1.5 / (2 pi w^2), u = (angle / w)^2; the
// veil's slow fall leaves no visible rim.
const GLARE_CORE_SHARE: f32 = 0.02;
const GLARE_CORE_WIDTH: f32 = 0.021;
const GLARE_VEIL_SHARE: f32 = 0.012;
const GLARE_VEIL_WIDTH: f32 = 0.12;
// Sunlit air in front of the scene scattering towards the eye around the sun, shaded like the
// veil by what lies between, so sunset beams fan out from silhouettes. Drawn with sun rays only,
// fading out as the sun leaves the image: off screen nothing shows what shades it.
const AIR_GLOW_SHARE: f32 = 0.05;
const AIR_GLOW_WIDTH: f32 = 0.15;

// Sunlight past the casters of the shadow-map layer `layer` at the cascade's coordinates `ndc`;
// 1 outside the cascade.
fn cascade_visibility(maps: texture_depth_2d_array, compare: sampler_comparison, layer: i32,
    ndc: vec3<f32>) -> f32 {
    if any(abs(ndc.xy) > vec2(1.0)) || ndc.z < 0.0 || ndc.z > 1.0 {
        return 1.0;
    }
    return textureSampleCompareLevel(maps, compare, ndc.xy * vec2(0.5, -0.5) + 0.5, layer, ndc.z);
}

// Interleaved gradient noise (Jimenez 2014): offsets each texel's steps so banding becomes fine
// noise.
fn gradient_noise(texel: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(texel, vec2(0.06711056, 0.00583715))));
}
