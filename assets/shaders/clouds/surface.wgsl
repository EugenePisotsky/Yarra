#import "shaders/clouds/types.wgsl"::CloudParams
@group(#{MATERIAL_BIND_GROUP}) @binding(120) var<storage,read> clouds: CloudParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(121) var cloud_shadow: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(122) var cloud_repeat: sampler;
fn cloud_visibility(p: vec3<f32>, direction: vec3<f32>) -> f32 {
    if clouds.layer.w < 0.5 || direction.y <= 0.0 || p.y >= clouds.layer.x+clouds.layer.y { return 1.0; }
    let sun = dot(direction,clouds.sun.xyz)>0.999;
    let moon = dot(direction,clouds.moon.xyz)>0.999;
    if !sun && !moon {return 1.0;}
    let hit = p.xz+clouds.offset.xy+direction.xz*(clouds.layer.x-p.y)/max(direction.y,0.04);
    let t=textureSampleLevel(cloud_shadow,cloud_repeat,(hit-clouds.offset.zw)/(clouds.layer.z*4.0),0.0).rg;
    return select(t.y,t.x,sun);
}
