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
    weather: vec4<f32>, // x: surface wetness, y: precipitation intensity
    shelter: vec4<f32>, // rain shelter map: origin xz, metres per texel, enabled
    forest_shadow: vec4<f32>, // forest shadow map: origin xz, metres per texel (0 off), tallest crown top
    forest_sky: vec4<f32>, // x: share of sky light the crowns above hold back (0 off, 1 full)
    low_haze: vec4<f32>, // ground haze: extinction / metre at its base, base height, e-folding height
    mist: vec4<f32>, // valley mist: extinction / metre in full mist (0 off), depth above a valley floor
    mist_map: vec4<f32>, // mist map: first corner xz in render coordinates, metres per texel, published
    mist_drift: vec4<f32>, // world-anchored mist noise offset xz, metres
    air_light: vec4<f32>, // unexposed light haze and mist scatter evenly (sky, moon)
    air_sun: vec4<f32>, // unexposed sunlight reaching haze and mist, scattered mostly forwards
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

// Rain shelter lookup shared by materials and rain: 1 where rain reaches `p`, 0 under full
// cover. Bilinear coverage of texels whose object top lies above `p`, as on the CPU.
fn shelter_exposure(map: texture_2d<f32>, shelter: vec4<f32>, p: vec3<f32>) -> f32 {
    if shelter.w < 0.5 { return 1.0; }
    let size = vec2<i32>(textureDimensions(map));
    let texel = (p.xz - shelter.xy) / shelter.z - 0.5;
    let base = floor(texel);
    let f = texel - base;
    var cover = 0.0;
    for (var i = 0; i < 4; i += 1) {
        let offset = vec2(i & 1, i >> 1u);
        let at = vec2<i32>(base) + offset;
        if any(at < vec2(0)) || any(at >= size) { continue; }
        let value = textureLoad(map, at, 0).rg;
        let w = select(1.0 - f.x, f.x, offset.x == 1) * select(1.0 - f.y, f.y, offset.y == 1);
        cover += select(0.0, value.g * w, p.y < value.r);
    }
    return 1.0 - cover;
}
