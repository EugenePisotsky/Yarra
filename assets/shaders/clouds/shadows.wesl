// The share of the sun's and the moon's beam the cloud layer lets through to its base, over the
// moving cloud field (crates/atmosphere/src/clouds): what shades the ground.
import package::shaders::environment::params::EnvironmentParams;
import package::shaders::clouds::density::light_depth;
@group(0) @binding(0) var<storage, read> environment: EnvironmentParams;
@group(0) @binding(1) var noise: texture_3d<f32>;
@group(0) @binding(2) var repeat: sampler;
@group(0) @binding(3) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(4) var shape: texture_3d<f32>;
@group(0) @binding(5) var detail: texture_3d<f32>;

// Steps from the layer's base towards each light.
const STEPS: u32 = 8u;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output);
    if any(id.xy >= size) { return; }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let p = vec3(uv.x * environment.layer.z * 4.0, environment.layer.x, uv.y * environment.layer.z * 4.0);
    var t = vec2(1.0);
    var field = environment;
    field.offset = vec4(environment.offset.xy, 0.0, 0.0);
    if environment.layer.w > 0.5 {
        if environment.sun.w > 0.0 {
            t.x = exp(-light_depth(p, environment.sun.xyz, field, noise, shape, detail, repeat, STEPS));
        }
        if environment.moon.w > 0.0 {
            t.y = exp(-light_depth(p, environment.moon.xyz, field, noise, shape, detail, repeat, STEPS));
        }
    }
    textureStore(output, vec2<i32>(id.xy), vec4(t, 1.0, 1.0));
}
