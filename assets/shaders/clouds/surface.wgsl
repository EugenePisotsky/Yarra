#import "shaders/clouds/types.wgsl"::{CloudParams, shelter_exposure}
@group(#{MATERIAL_BIND_GROUP}) @binding(120) var<storage,read> clouds: CloudParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(121) var cloud_shadow: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(122) var cloud_repeat: sampler;
// Rain shelter around the camera: highest object top above each texel, coverage.
@group(#{MATERIAL_BIND_GROUP}) @binding(123) var shelter_map: texture_2d<f32>;
fn cloud_visibility(p: vec3<f32>, direction: vec3<f32>) -> f32 {
    if clouds.layer.w < 0.5 || direction.y <= 0.0 || p.y >= clouds.layer.x+clouds.layer.y { return 1.0; }
    let sun = dot(direction,clouds.sun.xyz)>0.999;
    let moon = dot(direction,clouds.moon.xyz)>0.999;
    if !sun && !moon {return 1.0;}
    let hit = p.xz+clouds.offset.xy+direction.xz*(clouds.layer.x-p.y)/max(direction.y,0.04);
    let t=textureSampleLevel(cloud_shadow,cloud_repeat,(hit-clouds.offset.zw)/(clouds.layer.z*4.0),0.0).rg;
    return select(t.y,t.x,sun);
}
/// 1 where rain reaches `p`, 0 under full cover. Bilinear coverage, as on the CPU.
fn rain_shelter(p: vec3<f32>) -> f32 {
    return shelter_exposure(shelter_map, clouds.shelter, p);
}

/// Rain wetness of a surface: sky-facing surfaces soak fully, vertical ones partly, and
/// anything under a canopy stays drier.
fn surface_wetness(p: vec3<f32>, normal: vec3<f32>) -> f32 {
    return clamp(clouds.weather.x, 0.0, 1.0) * rain_shelter(p)
        * mix(0.35, 1.0, smoothstep(-0.2, 0.7, normal.y));
}

/// x: wetness, y: precipitation intensity.
fn surface_weather() -> vec4<f32> {
    return clouds.weather;
}
/// Wrapped world origin XZ: world-anchored surface patterns survive origin rebases.
fn surface_origin() -> vec2<f32> {
    return clouds.offset.xy;
}
