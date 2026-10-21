// Snow settled on a tree (material extras `yarra_snow: "settled_v1"`, TREE_SNOW,
// crates/engine/src/trees/material.rs). Its vegetation kit bakes how deep snow lies on each texel
// of the needle sprays into the foliage ORM's blue channel, which the foliage's zero metallic
// factor leaves unused (YarraVegetation scripts/needle_snow.py): the depth is the share of a
// spray's needles lying shallower, so a card covers the texels deeper than a threshold set by
// how much snow it holds. Bark takes snow on its upper sides. The kit's impostor bake
// (scripts/bake_impostor.py) repeats these rules for the far trees.
import bevy_pbr::render::{pbr_bindings, pbr_types, pbr_types::PbrInput};
import bevy_pbr::render::mesh_view_bindings::view;

const SNOW_ALBEDO: vec3<f32> = vec3(0.80, 0.82, 0.86);

const SNOW_ROUGHNESS: f32 = 0.85;

// How much snow a card holds, 0-1: the crown shelters its inner cards from the falling snow as
// from the sky (`sky`, its baked sky visibility), and steep cards shed theirs. Cards that turn
// toward the camera hold their snow in the texture, which shows them from the side.
fn card_amount(sky: f32, card_up: f32) -> f32 {
    return smoothstep(0.1, 0.45, sky) * smoothstep(0.3, 0.85, card_up);
}

// The share of needles a full card covers is about half; none at no amount.
fn needle_cover(depth: f32, amount: f32) -> f32 {
    let threshold = 1.08 - 0.6 * amount;
    return smoothstep(threshold - 0.06, threshold + 0.06, depth);
}

// Snow seen from below a card shows only where it bridges the gaps between needles; on top it
// hides them. `top` is the side snow falls on.
fn needle_shown(cover: f32, needle_alpha: f32, cutoff: f32, top: bool) -> f32 {
    return select(cover * (1.0 - step(cutoff, needle_alpha)), cover, top);
}

fn hash(cell: vec3<i32>) -> f32 {
    var h = (u32(cell.x) * 73856093u) ^ (u32(cell.y) * 19349663u) ^ (u32(cell.z) * 83492791u);
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h & 0xffffffu) / 16777216.0;
}

fn lumps(p: vec3<f32>) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let x00 = mix(hash(i), hash(i + vec3(1, 0, 0)), u.x);
    let x10 = mix(hash(i + vec3(0, 1, 0)), hash(i + vec3(1, 1, 0)), u.x);
    let x01 = mix(hash(i + vec3(0, 0, 1)), hash(i + vec3(1, 0, 1)), u.x);
    let x11 = mix(hash(i + vec3(0, 1, 1)), hash(i + vec3(1, 1, 1)), u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}

// Snow on bark lies along the tops of limbs the crown leaves open to the sky (`sky`, bark_sky_v1),
// in lumps of a few centimetres.
fn bark_cover(normal_up: f32, sky: f32, p: vec3<f32>) -> f32 {
    let lumpy = normal_up + 0.35 * (lumps(p * 14.0) - 0.5);
    return smoothstep(0.55, 0.8, lumpy) * smoothstep(0.03, 0.15, sky);
}

// A snow surface's normal: mostly up (`side` 1) or, for its underside, down (-1), keeping a
// little of the surface it lies on.
fn snow_normal(n: vec3<f32>, side: f32) -> vec3<f32> {
    return normalize(n * 0.4 + vec3(0.0, side, 0.0));
}

// The baked snow depth under a foliage texel; none without the kit's texture.
fn needle_depth(uv_in: vec2<f32>) -> f32 {
    let standard = pbr_bindings::material;
    if (standard.flags & pbr_types::STANDARD_MATERIAL_FLAGS_METALLIC_ROUGHNESS_TEXTURE_BIT) == 0u {
        return 0.0;
    }
    let uv = (standard.uv_transform * vec3(uv_in, 1.0)).xy;
    return textureSampleBias(pbr_bindings::metallic_roughness_texture,
        pbr_bindings::metallic_roughness_sampler, uv, view.mip_bias).b;
}

// Lays snow over `shown` of a surface, seen from above (`side` 1) or below (-1).
fn settle(input: ptr<function, PbrInput>, shown: f32, side: f32) {
    let base = (*input).material.base_color;
    (*input).material.base_color = vec4(mix(base.rgb, SNOW_ALBEDO, shown), base.a);
    (*input).material.perceptual_roughness = mix((*input).material.perceptual_roughness, SNOW_ROUGHNESS, shown);
    (*input).N = normalize(mix((*input).N, snow_normal((*input).N, side), shown));
}
