struct CloudParams {
    layer: vec4<f32>, // base, thickness, body period (weather/shadows span 4x), enabled
    shape: vec4<f32>, // coverage, extinction / metre, erosion, seed offset
    offset: vec4<f32>, // wrapped render origin XZ, wind offset XZ
    sun: vec4<f32>, // direction, lux at the cloud layer after the atmosphere
    moon: vec4<f32>,
    sun_color: vec4<f32>,
    moon_color: vec4<f32>,
    ambient: vec4<f32>, // linear RGB, strength
    haze: vec4<f32>, // linear RGB, visibility
    fog: vec4<f32>, // weather fog: unexposed in-scattered radiance, extra extinction / metre
    transition: vec4<f32>, // previous coverage, extinction / metre, erosion; linear progress
    weather: vec4<f32>, // x: surface wetness, y: precipitation intensity, z: overcast (0 open, 1 closed deck)
    shelter: vec4<f32>, // rain shelter map: origin xz, metres per texel, enabled
    forest_shadow: vec4<f32>, // forest shadow map: origin xz, metres per texel (0 off), tallest crown top
    forest_sky: vec4<f32>, // x: share of sky light the crowns above hold back (0 off, 1 full)
    low_haze: vec4<f32>, // ground haze: extinction / metre at its base, base height, e-folding height
    mist: vec4<f32>, // valley mist: extinction / metre in full mist (0 off), depth above a valley floor
    mist_map: vec4<f32>, // mist map: first corner xz in render coordinates, metres per texel, published
    mist_drift: vec4<f32>, // world-anchored mist noise offset xz, metres
    air_light: vec4<f32>, // unexposed light haze and mist scatter evenly (sky, moon)
    air_sun: vec4<f32>, // unexposed sunlight reaching haze and mist, scattered mostly forwards
    shafts: vec4<f32>, // light shafts: x extinction per metre of the air under crowns (0 off)
    near_sun: vec4<f32>, // unexposed sunlight at the camera; sun and sun_color hold it at the clouds
    ocean: vec4<f32>, // open sea: level, wind direction xz, wave clock seconds
    ocean_waves: vec4<f32>, // x: 1 with a sea to shade, y: wind strength scale of the waves
    shore_map: vec4<f32>, // shore map: first corner xz in render coordinates, metres per texel, swell period (0 off)
    lightning: vec4<f32>, // channel top at the cloud base (render space), flash light on clouds
    lightning_channel: vec4<f32>, // x: channel brightness 0..1
    moon_disc: vec4<f32>, // direction to the moon, angular radius (0: not drawn)
    moon_frame: vec4<f32>, // the moon's north, earthshine share of a fully lit face
    moon_sunward: vec4<f32>, // direction to the sun lighting the moon; the drawn face's light as a share of the moon light's
    moon_face: vec4<f32>, // unexposed light of white ground on the moon lit from straight above
    night_sky: vec4<f32>, // unexposed moonless night sky light just above the horizon
    lightning_segments: array<vec4<f32>, 32>, // channel segment ends in pairs: xyz, width
}

// Cached sky: azimuth across, square-root warped elevation down from the tracing cutoff (ray.y
// 0.01) to the zenith. Rows follow the horizon, so thin distant cloud slivers stay aligned with
// texels instead of crossing a square grid diagonally, and most rows sit near the horizon.
const SKY_MIN_ELEVATION: f32 = 0.0100002;
const HALF_PI: f32 = 1.5707963;
const TAU: f32 = 6.2831853;
fn sky_panorama_ray(uv: vec2<f32>) -> vec3<f32> {
    let azimuth = uv.x * TAU;
    let elevation = SKY_MIN_ELEVATION + uv.y * uv.y * (HALF_PI - SKY_MIN_ELEVATION);
    return vec3(cos(elevation) * cos(azimuth), sin(elevation), cos(elevation) * sin(azimuth));
}
fn sky_panorama_uv(ray: vec3<f32>) -> vec2<f32> {
    let elevation = asin(clamp(ray.y, -1.0, 1.0));
    let v = clamp((elevation - SKY_MIN_ELEVATION) / (HALF_PI - SKY_MIN_ELEVATION), 0.0, 1.0);
    return vec2(fract(atan2(ray.z, ray.x) / TAU), sqrt(v));
}
