#import bevy_render::view::View
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import "shaders/clouds/types.wgsl"::{CloudParams,sky_panorama_uv}
@group(0) @binding(0) var cloud_image: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;
#ifdef MULTISAMPLED
@group(0) @binding(2) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(2) var depth: texture_depth_2d;
#endif
@group(0) @binding(3) var<uniform> view: View;
@group(0) @binding(4) var<storage,read> clouds: CloudParams;
// Cached sky: previous complete refresh and x = share of the next sweep already refreshed.
@group(0) @binding(5) var cloud_previous: texture_2d<f32>;
@group(0) @binding(6) var<uniform> cloud_blend: vec4<f32>;
@fragment fn fragment(in:FullscreenVertexOutput)->@location(0) vec4<f32> {
    let uv=(in.position.xy-view.main_pass_viewport.xy)/view.main_pass_viewport.zw;
    if any(uv<vec2(0.0)) || any(uv>vec2(1.0)) {discard;}
    let ndc=uv*vec2(2.0,-2.0)+vec2(-1.0,1.0);
    let near=view.world_from_clip*vec4(ndc,1.0,1.0);
    let ray=normalize(near.xyz/near.w-view.world_position);
    let fogged=clouds.fog.w>0.0;
    let sky=ray.y>0.01;
    // Clear weather keeps the original early exit for everything below the horizon.
    if !sky && !fogged {discard;}
    let start=select(1.0e9,max(0.0,(clouds.layer.x-view.world_position.y)/ray.y),sky);
    let xy=vec2<i32>(in.position.xy);
#ifdef MULTISAMPLED
    let samples=textureNumSamples(depth);
#else
    let samples=1u;
#endif
    // Per sample: clouds show where the sky or geometry beyond the cloud base is visible, and
    // weather fog covers the path to the nearer of the surface and the cloud base.
    var visible=0.0;
    var fog_transmittance=0.0;
    var sky_samples=0u;
    for(var i=0u;i<samples;i+=1u) {
        let z=textureLoad(depth,xy,i32(i));
        var distance=start;
        if z!=0.0 {
            let world=view.world_from_clip*vec4(ndc,z,1.0);
            distance=length(world.xyz/world.w-view.world_position);
        } else {
            sky_samples+=1u;
        }
        if sky && distance>=start {visible+=1.0;}
        fog_transmittance+=exp(-clouds.fog.w*min(distance,start));
    }
    fog_transmittance/=f32(samples);
    // The resolved colour mixes geometry with the physical sky, which is brighter than weather
    // fog near the horizon. Averaged transmittance would leave part of that sky as a bright halo
    // around silhouettes, so an edge touching the sky takes the sky's transmittance instead.
    if sky_samples>0u {fog_transmittance=exp(-clouds.fog.w*start);}
    let fog=clouds.fog.rgb*view.exposure*(1.0-fog_transmittance);
    if visible==0.0 {
        if !fogged {discard;}
        return vec4(fog,fog_transmittance);
    }
#ifdef CACHED_SKY
    let cloud_uv=sky_panorama_uv(ray);
#else
    let cloud_uv=in.uv;
#endif
#ifdef CACHED_SKY
    let c=mix(textureSampleLevel(cloud_previous,linear_sampler,cloud_uv,0.0),
        textureSampleLevel(cloud_image,linear_sampler,cloud_uv,0.0),cloud_blend.x);
#else
    let c=textureSampleLevel(cloud_image,linear_sampler,cloud_uv,0.0);
#endif
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
    // Weather fog lies in front of both the clouds and the scene.
    return vec4(color*coverage*fog_transmittance+fog,(1.0-coverage*(1.0-c.a))*fog_transmittance);
}
