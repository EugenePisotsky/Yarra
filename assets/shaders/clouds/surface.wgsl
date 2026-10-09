#import "shaders/clouds/types.wgsl"::{CloudParams, shelter_exposure}
@group(#{MATERIAL_BIND_GROUP}) @binding(120) var<storage,read> clouds: CloudParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(121) var cloud_shadow: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(122) var cloud_repeat: sampler;
// Rain shelter around the camera: highest object top above each texel, coverage.
@group(#{MATERIAL_BIND_GROUP}) @binding(123) var shelter_map: texture_2d<f32>;
// Even a thick deck passes on some of the sun's light (the bright patch of cloud around it), so
// overcast scenes keep a little modelling instead of going flat.
const CLOUD_DIFFUSE_FLOOR: f32 = 0.12;
// Sunlight or moonlight at `p` past the cloud layer, as shares of the light above it: x the
// direct beam, which casts shadows; y the light the clouds pass on diffusely, which still lights
// surfaces facing the bright sky around the sun but casts none. Kept as a floor under the beam,
// it gave the character a sharp shadow under a closed deck.
fn cloud_light(p: vec3<f32>, direction: vec3<f32>) -> vec2<f32> {
    if clouds.layer.w < 0.5 || direction.y <= 0.0 || p.y >= clouds.layer.x+clouds.layer.y { return vec2(1.0, 0.0); }
    let sun = dot(direction,clouds.sun.xyz)>0.999;
    let moon = dot(direction,clouds.moon.xyz)>0.999;
    if !sun && !moon {return vec2(1.0, 0.0);}
    let hit = p.xz+clouds.offset.xy+direction.xz*(clouds.layer.x-p.y)/max(direction.y,0.04);
    let t=textureSampleLevel(cloud_shadow,cloud_repeat,(hit-clouds.offset.zw)/(clouds.layer.z*4.0),0.0).rg;
    // A closing deck hides the beam whatever thin spots the shadow map finds, as it hides the
    // sun's disc.
    let open = 1.0-clouds.weather.z;
    let direct = select(t.y,t.x,sun)*open*open;
    return vec2(direct, max(CLOUD_DIFFUSE_FLOOR-direct, 0.0));
}
// Sun and moon light on flat ground at `p`, unexposed: their light after the atmosphere at the
// camera stands in for its light there (the ground around a surface lies within metres of it),
// past the clouds above `p`.
fn ground_celestial_irradiance(p: vec3<f32>) -> vec3<f32> {
    var irradiance = vec3(0.0);
    if clouds.sun.y > 0.0 {
        irradiance += clouds.near_sun.rgb * clouds.sun.y * cloud_visibility(p, clouds.sun.xyz);
    }
    if clouds.moon.w > 0.0 && clouds.moon.y > 0.0 {
        irradiance += clouds.moon_color.rgb * clouds.moon.w * clouds.moon.y
            * cloud_visibility(p, clouds.moon.xyz);
    }
    return irradiance;
}
// All of it, for light that casts no shadows anyway.
fn cloud_visibility(p: vec3<f32>, direction: vec3<f32>) -> f32 {
    let light = cloud_light(p, direction);
    return light.x + light.y;
}
/// Forest shadow map: origin xz, metres per texel (0 when off), tallest crown top.
fn forest_shadow_parameters() -> vec4<f32> {
    return clouds.forest_shadow;
}
/// Sky light under the crowns: x the share they hold back (0 off, 1 as their foliage does).
fn forest_sky_parameters() -> vec4<f32> {
    return clouds.forest_sky;
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
