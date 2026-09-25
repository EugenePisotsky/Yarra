struct CloudParams {
    layer: vec4<f32>, // base, thickness, body period (weather/shadows span 4x), enabled
    shape: vec4<f32>, // coverage, extinction / metre, erosion, seed offset
    offset: vec4<f32>, // wrapped render origin XZ, wind offset XZ
    sun: vec4<f32>, // direction, cloud lux after clear-air/horizon attenuation
    moon: vec4<f32>,
    sun_color: vec4<f32>,
    moon_color: vec4<f32>,
    ambient: vec4<f32>, // linear RGB, strength
    haze: vec4<f32>, // linear RGB, visibility
    fog: vec4<f32>, // weather fog: unexposed in-scattered radiance, extra extinction / metre
    transition: vec4<f32>, // previous coverage, extinction / metre, erosion; linear progress
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
