// What rain and ambient particles share (atmosphere/src/precipitation.rs, ambient_particles.rs):
// bindings 1-3 of their group, random numbers and the scene's depth for their soft depth test.
// Binding 0 is each pass's own uniform.
import bevy_render::view::View;
import package::shaders::environment::params::EnvironmentParams;

@group(0) @binding(1) var<storage, read> environment: EnvironmentParams;
@group(0) @binding(2) var<uniform> view: View;
@if(MULTISAMPLED)
@group(0) @binding(3) var depth: texture_depth_multisampled_2d;
@else
@group(0) @binding(3) var depth: texture_depth_2d;

fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}
fn random3(seed: u32) -> vec3<f32> {
    let a = pcg(seed);
    let b = pcg(a);
    let c = pcg(b);
    return vec3(f32(a), f32(b), f32(c)) / 4294967295.0;
}

// Linear scene depth under a fragment. Scene depth may be at a lower resolution after
// temporal reconstruction.
fn scene_depth(fragment: vec2<f32>) -> f32 {
    let scale = view.main_pass_viewport.zw / view.viewport.zw;
    let texel = vec2<i32>((fragment - view.viewport.xy) * scale + view.main_pass_viewport.xy);
    let z = textureLoad(depth, texel, 0);
    return select(1.0e9, view.clip_from_view[3][2] / z, z > 0.0);
}
