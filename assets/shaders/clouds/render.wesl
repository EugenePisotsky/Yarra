import bevy_render::view::View;
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import package::shaders::clouds::types::{CloudParams,sky_panorama_ray};
import package::shaders::clouds::density::{density_at,light_transmission};
@group(0) @binding(0) var<storage,read> clouds: CloudParams;
@group(0) @binding(1) var noise: texture_3d<f32>;
@group(0) @binding(2) var repeat: sampler;
@group(0) @binding(3) var<uniform> view: View;
// Weight of the light scattered many times inside the cloud, against the direct beam's phase.
const MULTIPLE: f32 = 0.35;
// Sunlit land and sea below light the clouds from beneath (albedo about 0.1 over the island): at
// midday as much as the sky lights their bases from above.
const GROUND_BOUNCE: f32 = 0.02;
// Clouds above the ground's horizon still see a sun that has set below it; `light.w` already
// holds what reaches the cloud layer (none once the sun is below the layer's own horizon), so
// afterglow lights them from below.
fn light_at(p:vec3<f32>, ray:vec3<f32>, light:vec4<f32>, color:vec3<f32>) -> vec3<f32> {
    if light.w<=0.0 {return vec3(0.0);}
    let t=light_transmission(p,light.xyz,clouds,noise,repeat,5u);
    let mu=dot(ray,light.xyz);
    let g=0.55;
    let phase=(1.0-g*g)/pow(1.0+g*g-2.0*g*mu,1.5);
    // Light diffusing through the cloud fades far slower than the direct beam: a thick cumulus
    // still passes about a third of it (two-stream diffusion, g 0.85), so its base is grey.
    let diffuse=1.0/(1.0-0.11*log(max(t,1e-6)));
    return color*light.w*0.065*(t*phase+MULTIPLE*diffuse);
}
@fragment fn fragment(in:FullscreenVertexOutput)->@location(0) vec4<f32> {
    // Hemisphere panorama. It covers every camera orientation; turns only change
    // the lookup in the full-resolution composite, never the cache.
    @if(CACHED_SKY)
    let ray=sky_panorama_ray(in.uv);
    @if(!CACHED_SKY)
    let uv=in.uv;
    @if(!CACHED_SKY)
    if any(uv<vec2(0.0)) || any(uv>vec2(1.0)) {return vec4(0.0,0.0,0.0,1.0);}
    @if(!CACHED_SKY)
    let ndc=vec4(uv*vec2(2.0,-2.0)+vec2(-1.0,1.0),1.0,1.0);
    @if(!CACHED_SKY)
    let point=view.world_from_clip*ndc;
    @if(!CACHED_SKY)
    let ray=normalize(point.xyz/point.w-view.world_position);
    let camera=view.world_position+vec3(clouds.offset.x,0.0,clouds.offset.y);
    if ray.y<=0.01 || camera.y>=clouds.layer.x+clouds.layer.y {return vec4(0.0,0.0,0.0,1.0);}
    let start=max(0.0,(clouds.layer.x-camera.y)/ray.y);
    let end=min((clouds.layer.x+clouds.layer.y-camera.y)/ray.y,40000.0);
    if end<=start {return vec4(0.0,0.0,0.0,1.0);}
    // About 25 m steps, at least 48: a long grazing path through the layer needs more of them,
    // or sharp-edged clouds break into slices. Traced every frame (High), at most 80.
    @if(CACHED_SKY)
    let most=160u;
    @else
    let most=80u;
    let steps=clamp(u32((end-start)/25.0),48u,most);
    let ds=(end-start)/f32(steps);
    var t=1.0;var color=vec3(0.0);
    for(var i=0u;i<steps;i+=1u) {
        let p=camera+ray*(start+(f32(i)+0.5)*ds);
        let density=density_at(p,clouds,noise,repeat);
        if density>0.001 {
            let a=1.0-exp(-density*clouds.shape.y*ds);
            let h=clamp((p.y-clouds.layer.x)/clouds.layer.y,0.0,1.0);
            let sky=clouds.ambient.rgb*clouds.ambient.w;
            let ground=clouds.near_sun.rgb*max(clouds.sun.y,0.0)*(1.0-clouds.weather.z)+sky;
            let ambient=sky*mix(0.06,0.22,h)+ground*GROUND_BOUNCE*(1.0-h);
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
