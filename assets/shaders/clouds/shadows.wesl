import package::shaders::environment::params::EnvironmentParams;
import package::shaders::clouds::density::light_transmission;
@group(0) @binding(0) var<storage,read> environment: EnvironmentParams;
@group(0) @binding(1) var noise: texture_3d<f32>;
@group(0) @binding(2) var repeat: sampler;
@group(0) @binding(3) var output: texture_storage_2d<rgba8unorm,write>;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size=textureDimensions(output);
    if any(id.xy>=size) {return;}
    let uv=(vec2<f32>(id.xy)+0.5)/vec2<f32>(size);
    let p=vec3(uv.x*environment.layer.z*4.0, environment.layer.x,uv.y*environment.layer.z*4.0);
    var t=vec2(1.0);
    var field=environment;
    field.offset=vec4(environment.offset.xy,0.0,0.0);
    if environment.layer.w>0.5 {
        if environment.sun.w>0.0 { t.x=light_transmission(p,environment.sun.xyz,field,noise,repeat,16u); }
        if environment.moon.w>0.0 { t.y=light_transmission(p,environment.moon.xyz,field,noise,repeat,16u); }
    }
    textureStore(output,vec2<i32>(id.xy),vec4(t,1.0,1.0));
}
