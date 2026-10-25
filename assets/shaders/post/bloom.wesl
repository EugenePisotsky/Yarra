// Bloom on the rendered image, before the upscaler (crates/atmosphere/src/bloom.rs): the pyramid
// of Jimenez's "Next Generation Post Processing in Call of Duty: Advanced Warfare" (SIGGRAPH
// 2014) with the filters and weights of Bevy's bloom. Each level is a 13-tap downsample of the one
// above it, the first with Karis's average against fireflies. Going back up, each level blends a
// 3×3 tent of the coarser one over its own image, and the composite blends the finest over the
// view's image.
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

struct Level {
    // A destination texel at uv u reads the source at origin + u * extent, each tap clamped to
    // [low, high]. The first level reads the view's rendered region, which under the temporal
    // upscaler is a corner of a window-sized texture whose rest holds an older image.
    origin: vec2<f32>,
    extent: vec2<f32>,
    low: vec2<f32>,
    high: vec2<f32>,
    // Upsampling: the share of the coarser level's tent in this one.
    blend: f32,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_clamp: sampler;
@group(0) @binding(2) var<uniform> level: Level;
@group(0) @binding(3) var destination: texture_storage_2d<rgba16float, write>;
// Upsampling: the level's own downsampled image.
@group(0) @binding(4) var own: texture_2d<f32>;

fn tap(uv: vec2<f32>, texels: vec2<f32>, texel: vec2<f32>) -> vec3<f32> {
    let at = clamp(uv + texels * texel, level.low, level.high);
    return textureSampleLevel(source, linear_clamp, at, 0.0).rgb;
}

fn source_uv(id: vec2<u32>) -> vec2<f32> {
    let uv = (vec2<f32>(id) + 0.5) / vec2<f32>(textureDimensions(destination));
    return level.origin + uv * level.extent;
}

// Rec. 709 luminance.
fn luminance(colour: vec3<f32>) -> f32 {
    return dot(colour, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// A group of taps weighted down by its brightness, so a single bright pixel does not bloom.
fn karis(group: vec3<f32>) -> vec3<f32> {
    return group / (1.0 + luminance(group) / 4.0);
}

@compute @workgroup_size(8, 8, 1)
fn downsample_first(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(destination)) {
        return;
    }
    let uv = source_uv(id.xy);
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    let a = tap(uv, vec2(-2.0, 2.0), texel);
    let b = tap(uv, vec2(0.0, 2.0), texel);
    let c = tap(uv, vec2(2.0, 2.0), texel);
    let d = tap(uv, vec2(-2.0, 0.0), texel);
    let e = tap(uv, vec2(0.0, 0.0), texel);
    let f = tap(uv, vec2(2.0, 0.0), texel);
    let g = tap(uv, vec2(-2.0, -2.0), texel);
    let h = tap(uv, vec2(0.0, -2.0), texel);
    let i = tap(uv, vec2(2.0, -2.0), texel);
    let j = tap(uv, vec2(-1.0, 1.0), texel);
    let k = tap(uv, vec2(1.0, 1.0), texel);
    let l = tap(uv, vec2(-1.0, -1.0), texel);
    let m = tap(uv, vec2(1.0, -1.0), texel);
    let corners = karis((a + b + d + e) * (0.125 / 4.0)) + karis((b + c + e + f) * (0.125 / 4.0))
        + karis((d + e + g + h) * (0.125 / 4.0)) + karis((e + f + h + i) * (0.125 / 4.0));
    let colour = corners + karis((j + k + l + m) * (0.5 / 4.0));

    // The floor keeps a black region from multiplying through the levels into black boxes.
    textureStore(destination, id.xy, vec4(clamp(colour, vec3(0.0001), vec3(3.4e37)), 1.0));
}

@compute @workgroup_size(8, 8, 1)
fn downsample(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(destination)) {
        return;
    }
    let uv = source_uv(id.xy);
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    var colour = (tap(uv, vec2(-2.0, 2.0), texel) + tap(uv, vec2(2.0, 2.0), texel)
        + tap(uv, vec2(-2.0, -2.0), texel) + tap(uv, vec2(2.0, -2.0), texel)) * 0.03125;
    colour += (tap(uv, vec2(0.0, 2.0), texel) + tap(uv, vec2(-2.0, 0.0), texel)
        + tap(uv, vec2(2.0, 0.0), texel) + tap(uv, vec2(0.0, -2.0), texel)) * 0.0625;
    colour += (tap(uv, vec2(0.0, 0.0), texel) + tap(uv, vec2(-1.0, 1.0), texel)
        + tap(uv, vec2(1.0, 1.0), texel) + tap(uv, vec2(-1.0, -1.0), texel)
        + tap(uv, vec2(1.0, -1.0), texel)) * 0.125;
    textureStore(destination, id.xy, vec4(colour, 1.0));
}

// The coarser level at uv through a 3×3 tent of its texels.
fn tent(uv: vec2<f32>) -> vec3<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    var colour = sample(uv, vec2(0.0, 0.0), texel) * 0.25;
    colour += (sample(uv, vec2(0.0, 1.0), texel) + sample(uv, vec2(-1.0, 0.0), texel)
        + sample(uv, vec2(1.0, 0.0), texel) + sample(uv, vec2(0.0, -1.0), texel)) * 0.125;
    colour += (sample(uv, vec2(-1.0, 1.0), texel) + sample(uv, vec2(1.0, 1.0), texel)
        + sample(uv, vec2(-1.0, -1.0), texel) + sample(uv, vec2(1.0, -1.0), texel)) * 0.0625;
    return colour;
}

fn sample(uv: vec2<f32>, texels: vec2<f32>, texel: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(source, linear_clamp, uv + texels * texel, 0.0).rgb;
}

@compute @workgroup_size(8, 8, 1)
fn upsample(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(destination)) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(textureDimensions(destination));
    let below = textureLoad(own, id.xy, 0).rgb;
    textureStore(destination, id.xy, vec4(mix(below, tent(uv), level.blend), 1.0));
}

// Drawn over the rendered region with the finest level's blend as the blend constant.
@fragment
fn composite(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return vec4(tent(in.uv), 1.0);
}
