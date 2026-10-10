// The air as the sky composite and light shafts both see it: the phase function of haze and mist
// droplets, and the shape of valley mist (atmosphere/src/valley_mist.rs). Maps are parameters, so
// each pass reads them through its own bindings.

// Phase function of haze and mist droplets: an even share, a forward lobe and the narrow
// aureole of large droplets, so both glow towards the sun and brightest right around it.
const FORWARD_SHARE: f32 = 0.4;
const FORWARD_G: f32 = 0.6;
const AUREOLE_SHARE: f32 = 0.05;
const AUREOLE_G: f32 = 0.92;
const INV_FOUR_PI: f32 = 0.07957747;
fn lobe(g: f32, cos_angle: f32) -> f32 {
    return (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * cos_angle, 1.5);
}
fn scattering(cos_angle: f32) -> f32 {
    return INV_FOUR_PI * ((1.0 - FORWARD_SHARE - AUREOLE_SHARE)
        + FORWARD_SHARE * lobe(FORWARD_G, cos_angle) + AUREOLE_SHARE * lobe(AUREOLE_G, cos_angle));
}

// Open ground holds these shares of a valley's mist depth and density.
const OPEN_DEPTH: f32 = 0.35;
const OPEN_DENSITY: f32 = 0.2;
// Noise lifts and lowers the mist top by up to this share of its depth.
const WISP_LIFT: f32 = 0.35;
// Horizontal size of the noise tile, metres (`atmosphere::clouds::MIST_NOISE_PERIOD`).
const MIST_NOISE_PERIOD: f32 = f32(constants::MIST_NOISE_PERIOD);

// The mist map at `xz` (`placement`: first corner xz, metres per texel): floor height, valley
// share, ground height, land share.
fn mist_texel(map: texture_2d<f32>, map_sampler: sampler, placement: vec4<f32>, xz: vec2<f32>)
    -> vec4<f32> {
    let extent = placement.z * vec2<f32>(textureDimensions(map));
    return textureSampleLevel(map, map_sampler, (xz - placement.xy) / extent, 0.0);
}

// Slowly drifting, world-anchored variation of mist density and height, 0..1; `drift` is the
// noise offset (`mist_drift`).
fn mist_wisps(noise: texture_3d<f32>, noise_sampler: sampler, drift: vec2<f32>, p: vec3<f32>)
    -> f32 {
    let q = (p.xz + drift) / MIST_NOISE_PERIOD;
    let n = textureSampleLevel(noise, noise_sampler, vec3(q.x, p.y / 512.0, q.y), 0.0);
    return smoothstep(0.2, 0.8, n.r * 0.7 + n.b * 0.3);
}
