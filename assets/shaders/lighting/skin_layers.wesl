// What a character's skin paints over itself as it draws: brows and makeup, from masks in the
// head's texture layout (crates/engine/src/character_skin.rs).
import bevy_pbr::render::pbr_types::PbrInput;

struct SkinLayers {
    // The part of the head's texture the brow mask covers: corner, size.
    brow_rect: vec4<f32>,
    // Each paint: linear colour, strength.
    brow: vec4<f32>,
    hairline: vec4<f32>,
    eyeshadow: vec4<f32>,
    liner: vec4<f32>,
    blush: vec4<f32>,
    lipstick: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<uniform> layers: SkinLayers;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var brow_mask: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var mask_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var makeup: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var hairline: texture_2d<f32>;

// How much a brow's halo shades the skin under it, how rough a hair is, how glossy lipstick is, and
// how far paint flattens the skin's relief where it lies.
const HALO_SHADE: f32 = 0.42;
const HAIR_ROUGHNESS: f32 = 0.62;
const LIPSTICK_ROUGHNESS: f32 = 0.3;
const HAIR_FLATTENS: f32 = 0.8;

// Samples are unconditional: texture sampling must stay in uniform control flow.
fn paint(pbr_input: ptr<function, PbrInput>, uv: vec2<f32>, world_normal: vec3<f32>) {
    let local = (uv - layers.brow_rect.xy) / layers.brow_rect.zw;
    let brow = textureSample(brow_mask, mask_sampler, local).rg;
    let m = textureSample(makeup, mask_sampler, uv);
    let hair_cover = textureSample(hairline, mask_sampler, uv).r * layers.hairline.a;
    let inside = f32(all(local >= vec2(0.0)) && all(local <= vec2(1.0)));

    var color = (*pbr_input).material.base_color.rgb;
    color = mix(color, layers.eyeshadow.rgb, m.r * layers.eyeshadow.a);
    // Blush tints what is there: it does not hide the skin's own marks.
    color = mix(color, color * layers.blush.rgb * 1.4, m.b * layers.blush.a);
    color = mix(color, layers.lipstick.rgb, m.a * layers.lipstick.a);
    color = mix(color, layers.liner.rgb, m.g * layers.liner.a);

    let halo = brow.g * inside * layers.brow.a;
    let hair = brow.r * inside * layers.brow.a;
    color = mix(color, color * layers.brow.rgb * 3.0, halo * HALO_SHADE);
    color = mix(color, layers.brow.rgb, hair);
    color = mix(color, layers.hairline.rgb, hair_cover);
    (*pbr_input).material.base_color = vec4(color, (*pbr_input).material.base_color.a);

    var roughness = (*pbr_input).material.perceptual_roughness;
    roughness = mix(roughness, LIPSTICK_ROUGHNESS, m.a * layers.lipstick.a);
    roughness = mix(roughness, HAIR_ROUGHNESS, max(hair, hair_cover));
    (*pbr_input).material.perceptual_roughness = roughness;
    (*pbr_input).N = normalize(mix((*pbr_input).N, normalize(world_normal), max(hair, hair_cover) * HAIR_FLATTENS));
}
