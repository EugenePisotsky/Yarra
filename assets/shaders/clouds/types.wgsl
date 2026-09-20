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
}
