#import bevy_render::view::View
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import "shaders/clouds/types.wgsl"::CloudParams
@group(0) @binding(0) var cloud_image: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;
#ifdef MULTISAMPLED
@group(0) @binding(2) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(2) var depth: texture_depth_2d;
#endif
@group(0) @binding(3) var<uniform> view: View;
@group(0) @binding(4) var<storage,read> clouds: CloudParams;
@fragment fn fragment(in:FullscreenVertexOutput)->@location(0) vec4<f32> {
    let uv=(in.position.xy-view.viewport.xy)/view.viewport.zw;
    if any(uv<vec2(0.0)) || any(uv>vec2(1.0)) {discard;}
    let ndc=uv*vec2(2.0,-2.0)+vec2(-1.0,1.0);
    let near=view.world_from_clip*vec4(ndc,1.0,1.0);
    let ray=normalize(near.xyz/near.w-view.world_position);
    if ray.y<=0.01 {discard;}
    let start=max(0.0,(clouds.layer.x-view.world_position.y)/ray.y);
    let xy=vec2<i32>(in.position.xy);
#ifdef MULTISAMPLED
    let samples=textureNumSamples(depth);
#else
    let samples=1u;
#endif
    var visible=0.0;
    for(var i=0u;i<samples;i+=1u) {
        let z=textureLoad(depth,xy,i32(i));
        if z==0.0 {visible+=1.0;}
        else {
            let world=view.world_from_clip*vec4(ndc,z,1.0);
            if length(world.xyz/world.w-view.world_position)>=start {visible+=1.0;}
        }
    }
    if visible==0.0 {discard;}
#ifdef CACHED_SKY
    let cloud_uv=ray.xz/(1.0+ray.y)*0.5+vec2(0.5);
#else
    let cloud_uv=in.uv;
#endif
    let c=textureSampleLevel(cloud_image,linear_sampler,cloud_uv,0.0);
    let haze=exp(-start*3.912/max(clouds.haze.w,50.0));
    // Haze in front of an opaque cloud must not reintroduce the sun/moon disk
    // from the background texture.
    let air=clouds.haze.rgb*(clouds.ambient.rgb*clouds.ambient.w*0.3
        +clouds.sun_color.rgb*clouds.sun.w*0.025
        +clouds.moon_color.rgb*clouds.moon.w*0.025)*view.exposure;
    let color=mix(air*(1.0-c.a),c.rgb*(1024.0*view.exposure),haze);
    // Fade the finite ground-view tracing range into the horizon rather than
    // exposing a straight edge at the end of the cloud layer.
    let horizon=1.0-smoothstep(20000.0,40000.0,start);
    let coverage=horizon*visible/f32(samples);
    // Premultiplied in-scattering + destination * transmission. Blend directly
    // into the existing HDR target; no full-scene sampling or ping-pong copy.
    return vec4(color*coverage,1.0-coverage*(1.0-c.a));
}
