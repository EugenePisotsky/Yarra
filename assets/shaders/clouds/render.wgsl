#import bevy_render::view::View
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import "shaders/clouds/types.wgsl"::CloudParams
#import "shaders/clouds/density.wgsl"::{density_at,light_transmission}
@group(0) @binding(0) var<storage,read> clouds: CloudParams;
@group(0) @binding(1) var noise: texture_3d<f32>;
@group(0) @binding(2) var repeat: sampler;
@group(0) @binding(3) var<uniform> view: View;
// Dimensions only: preserve exact ray directions for odd target sizes and
// viewport offsets, independently of the selected cloud resolution.
@group(0) @binding(4) var scene: texture_2d<f32>;
fn light_at(p:vec3<f32>, ray:vec3<f32>, light:vec4<f32>, color:vec3<f32>) -> vec3<f32> {
    if light.w<=0.0 || light.y<=0.0 {return vec3(0.0);}
    let t=light_transmission(p,light.xyz,clouds,noise,repeat,5u);
    let mu=dot(ray,light.xyz);
    let g=0.55;
    let phase=(1.0-g*g)/pow(1.0+g*g-2.0*g*mu,1.5);
    // Approximate multiple scattering keeps deep cloud interiors readable.
    return color*light.w*0.065*(t*phase+0.22*sqrt(t));
}
@fragment fn fragment(in:FullscreenVertexOutput)->@location(0) vec4<f32> {
#ifdef CACHED_SKY
    // Stereographic hemisphere atlas. It covers every camera orientation; turns
    // only change the lookup in the full-resolution composite, never the cache.
    let disk=in.uv*2.0-vec2(1.0);
    let radius2=dot(disk,disk);
    if radius2>=1.0 {return vec4(0.0,0.0,0.0,1.0);}
    let ray=vec3(2.0*disk.x,1.0-radius2,2.0*disk.y)/(1.0+radius2);
#else
    let uv=(in.uv*vec2<f32>(textureDimensions(scene))-view.viewport.xy)/view.viewport.zw;
    if any(uv<vec2(0.0)) || any(uv>vec2(1.0)) {return vec4(0.0,0.0,0.0,1.0);}
    let ndc=vec4(uv*vec2(2.0,-2.0)+vec2(-1.0,1.0),1.0,1.0);
    let point=view.world_from_clip*ndc;
    let ray=normalize(point.xyz/point.w-view.world_position);
#endif
    let camera=view.world_position+vec3(clouds.offset.x,0.0,clouds.offset.y);
    if ray.y<=0.01 || camera.y>=clouds.layer.x+clouds.layer.y {return vec4(0.0,0.0,0.0,1.0);}
    let start=max(0.0,(clouds.layer.x-camera.y)/ray.y);
    let end=min((clouds.layer.x+clouds.layer.y-camera.y)/ray.y,40000.0);
    if end<=start {return vec4(0.0,0.0,0.0,1.0);}
    let ds=(end-start)/48.0;
    var t=1.0;var color=vec3(0.0);
    for(var i=0u;i<48u;i+=1u) {
        let p=camera+ray*(start+(f32(i)+0.5)*ds);
        let density=density_at(p,clouds,noise,repeat);
        if density>0.001 {
            let a=1.0-exp(-density*clouds.shape.y*ds);
            let h=clamp((p.y-clouds.layer.x)/clouds.layer.y,0.0,1.0);
            let ambient=clouds.ambient.rgb*clouds.ambient.w*mix(0.06,0.22,h);
            let illumination=ambient+light_at(p,ray,clouds.sun,clouds.sun_color.rgb)+light_at(p,ray,clouds.moon,clouds.moon_color.rgb);
            color += t*a*illumination;
            t *= 1.0-a;
            // Once opaque, discard the tiny residual transmission too. Leaving a
            // 1% floor leaks extremely bright HDR sun/moon disks through solid clouds.
            if t<0.003 {t=0.0;break;}
        }
    }
    // Store unexposed radiance at a fixed scale, including authored 200 klux
    // sunlight without overflowing rgba16float. Exposure stays live at composite.
    return vec4(color/1024.0,t);
}
