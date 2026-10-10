// Light shafts' depth-aware blur (crates/atmosphere/src/light_shafts.rs), run along x and then
// y over what sky/light_shafts.wesl marched.
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var distances: texture_2d<f32>;
@group(0) @binding(2) var destination: texture_storage_2d<rgba16float, write>;

// A 9-tap Gaussian (sigma 2 texels) that skips texels at a different distance, so beams do not
// bleed across silhouettes.
const RADIUS: i32 = 4;
fn blur(id: vec2<u32>, step: vec2<i32>) {
    let size = vec2<i32>(textureDimensions(destination));
    let texel = vec2<i32>(id);
    if any(texel >= size) {
        return;
    }
    let centre = textureLoad(distances, texel, 0).r;
    var sum = vec4(0.0);
    var weight = 0.0;
    for (var i = -RADIUS; i <= RADIUS; i += 1) {
        let at = clamp(texel + step * i, vec2(0), size - 1);
        let d = textureLoad(distances, at, 0).r;
        let w = exp(-f32(i * i) / 8.0) * exp(-abs(d - centre) / (0.05 * min(centre, d) + 0.3));
        sum += textureLoad(source, at, 0) * w;
        weight += w;
    }
    textureStore(destination, texel, sum / weight);
}

@compute @workgroup_size(8, 8)
fn blur_x(@builtin(global_invocation_id) id: vec3<u32>) {
    blur(id.xy, vec2(1, 0));
}

@compute @workgroup_size(8, 8)
fn blur_y(@builtin(global_invocation_id) id: vec3<u32>) {
    blur(id.xy, vec2(0, 1));
}
