// Generated once per resident page/input change, before any camera draws it.
struct ControlSettings {
    minimum: vec2<f32>,
    extent: vec2<f32>,
    scales: vec4<f32>,
    mip: vec4<u32>,
}
@group(0) @binding(0) var<uniform> settings: ControlSettings;
@group(0) @binding(1) var weights: texture_2d<f32>;
@group(0) @binding(2) var weight_sampler: sampler;
@group(0) @binding(3) var macro_texture: texture_2d<f32>;
@group(0) @binding(4) var macro_sampler: sampler;
@group(0) @binding(5) var output: texture_storage_2d<rgba8unorm, write>;

fn band(world: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, scale: f32, offset: vec2<f32>) -> f32 {
    let inverse_scale = 1.0 / max(scale, 0.001);
    return textureSampleGrad(macro_texture, macro_sampler, world * inverse_scale + offset,
        dx * inverse_scale, dy * inverse_scale).r * 2.0 - 1.0;
}
fn medium(v: vec2<f32>) -> vec2<f32> {
    return vec2(v.x * 0.819 - v.y * 0.574, v.x * 0.574 + v.y * 0.819);
}
fn large(v: vec2<f32>) -> vec2<f32> {
    return vec2(v.x * -0.342 - v.y * 0.940, v.x * 0.940 - v.y * 0.342);
}
@compute @workgroup_size(8, 8)
fn prepare(@builtin(global_invocation_id) id: vec3<u32>) {
    let dimensions = textureDimensions(output);
    if any(id.xy >= dimensions) { return; }
    // Eight base-level gutter texels; every supplied mip retains a gutter.
    // Evaluate each mip from the original inputs, including the world-space halo.
    let uv = ((vec2<f32>(id.xy) + 0.5) / vec2<f32>(dimensions) * 272.0 - 8.0) / 256.0;
    let world = settings.minimum + uv * settings.extent;
    let step = settings.extent / 256.0 * f32(1u << settings.mip.x);
    let dx = vec2(step.x, 0.0);
    let dy = vec2(0.0, step.y);
    let wdim = vec2<f32>(textureDimensions(weights));
    let wuv = (0.5 + clamp(uv, vec2(0.0), vec2(1.0)) * (wdim - 1.0)) / wdim;
    let blend = textureSampleLevel(weights, weight_sampler, wuv, 0.0).rg;
    let signal = band(world, dx, dy, settings.scales.x, vec2(0.11, 0.73)) * 0.28
        + band(medium(world), medium(dx), medium(dy), settings.scales.y, vec2(0.37, 0.61)) * 0.36
        + band(large(world), large(dx), large(dy), settings.scales.z, vec2(0.83, 0.19)) * 0.36;
    // Linear decoding also works after bilinear/trilinear filtering across byte carries.
    let packed = u32(round(clamp(signal * 0.5 + 0.5, 0.0, 1.0) * 65535.0));
    textureStore(output, id.xy, vec4(blend, f32(packed >> 8u) / 255.0, f32(packed & 255u) / 255.0));
}
