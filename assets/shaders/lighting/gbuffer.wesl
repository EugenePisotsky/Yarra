// The G-buffer: what a surface writes in the deferred pass and the deferred lighting reads back
// (docs/DEFERRED_LIGHTING.md). One `Rgba32Uint` texel, Bevy's deferred target, whose y word holds
// the lighting model in its top 8 bits; each model's kernel reads the rest in its own layout.
// The shared lighting's models' (lighting/deferred.wesl):
// - x: albedo, gamma 2.2, 8 bits a channel; perceptual roughness 8;
// - y: shading normal, octahedral 12 + 12; model 8;
// - z: diffuse occlusion, specular occlusion, reflectance and metallic, 8 each;
// - w: geometric normal (shadow bias), octahedral 12 + 12; lift, snorm 8 over LIFT_RANGE.
// Grass's is in vegetation/draw/gbuffer.wesl.
import bevy_render::utils::{octahedral_encode, octahedral_decode};

// Lighting models. NONE marks pixels a forward pass draws over; grass has its own lighting
// (vegetation/draw/gbuffer.wesl), the others share pbr_lighting.wesl.
const MODEL_NONE: u32 = 0u;
const MODEL_TERRAIN: u32 = 1u;
const MODEL_GRASS: u32 = 2u;
// Objects: rocks, props, buildings, characters, plain trees.
const MODEL_STANDARD: u32 = 3u;
// Tree foliage tagged `crown_v1` and `crown_v2`, and bark tagged `bark_sky_v1`
// (crates/engine/src/trees/material.rs).
const MODEL_CROWN: u32 = 4u;
const MODEL_CROWN_SKY: u32 = 5u;
const MODEL_BARK: u32 = 6u;
// Impostors (crates/engine/src/trees/impostor.rs): lit as `crown_v2` foliage, with their own
// self-shadow, at their crown's surface behind or before the quad.
const MODEL_IMPOSTOR: u32 = 7u;

// The model of the surface a material's pipeline shades, by the definitions that select its
// branches of the shared lighting.
@if(TERRAIN_SURFACE) const SHADED_MODEL: u32 = MODEL_TERRAIN;
@elif(IMPOSTOR_SELF_SHADOW) const SHADED_MODEL: u32 = MODEL_IMPOSTOR;
@elif(TREE_CROWN_SKY) const SHADED_MODEL: u32 = MODEL_CROWN_SKY;
@elif(TREE_CROWN_SHADING) const SHADED_MODEL: u32 = MODEL_CROWN;
@elif(TREE_BARK_SKY) const SHADED_MODEL: u32 = MODEL_BARK;
@else const SHADED_MODEL: u32 = MODEL_STANDARD;

fn shared_lighting(model: u32) -> bool {
    return model == MODEL_TERRAIN || (model >= MODEL_STANDARD && model <= MODEL_IMPOSTOR);
}

struct Surface {
    albedo: vec3<f32>,
    perceptual_roughness: f32,
    // Normal-mapped, and as the mesh had it.
    normal: vec3<f32>,
    geometric_normal: vec3<f32>,
    diffuse_occlusion: f32,
    specular_occlusion: f32,
    reflectance: f32,
    metallic: f32,
    model: u32,
    // Metres toward the viewer from the depth drawn to the point lit: an impostor's crown
    // surface, off its quad.
    lift: f32,
}

// The lift's range either way: an impostor's crown reaches its views' half size from the quad.
const LIFT_RANGE: f32 = 16.0;

// Four values in 0..1 as bytes, the first lowest. By hand: on Metal, `pack4x8unorm` stored zero
// for the occlusion word.
fn pack_unorm4(v: vec4<f32>) -> u32 {
    let q = vec4<u32>(round(saturate(v) * 255.0));
    return q.x | (q.y << 8u) | (q.z << 16u) | (q.w << 24u);
}

fn unpack_unorm4(bits: u32) -> vec4<f32> {
    return vec4<f32>(vec4(bits, bits >> 8u, bits >> 16u, bits >> 24u) & vec4(0xffu)) / 255.0;
}

// A unit vector in 24 bits, octahedral 12 + 12.
fn pack_normal(n: vec3<f32>) -> u32 {
    let e = vec2<u32>(round(octahedral_encode(n) * 4095.0));
    return e.x | (e.y << 12u);
}

fn unpack_normal(bits: u32) -> vec3<f32> {
    return octahedral_decode(vec2(f32(bits & 0xfffu), f32((bits >> 12u) & 0xfffu)) / 4095.0);
}

// A unit vector in 16 bits, octahedral 8 + 8, for directions only diffuse or broad lobes read.
fn pack_normal16(n: vec3<f32>) -> u32 {
    let e = vec2<u32>(round(octahedral_encode(n) * 255.0));
    return e.x | (e.y << 8u);
}

fn unpack_normal16(bits: u32) -> vec3<f32> {
    return octahedral_decode(vec2(f32(bits & 0xffu), f32((bits >> 8u) & 0xffu)) / 255.0);
}

fn pack(s: Surface) -> vec4<u32> {
    return vec4(
        pack_unorm4(vec4(pow(saturate(s.albedo), vec3(1.0 / 2.2)), s.perceptual_roughness)),
        pack_normal(s.normal) | (s.model << 24u),
        pack_unorm4(vec4(s.diffuse_occlusion, s.specular_occlusion, s.reflectance, s.metallic)),
        pack_normal(s.geometric_normal)
            | (u32(i32(round(clamp(s.lift / LIFT_RANGE, -1.0, 1.0) * 127.0)) & 0xff) << 24u),
    );
}

fn model(texel: vec4<u32>) -> u32 {
    return texel.y >> 24u;
}

fn unpack(texel: vec4<u32>) -> Surface {
    let base = unpack_unorm4(texel.x);
    let props = unpack_unorm4(texel.z);
    return Surface(pow(base.rgb, vec3(2.2)), base.a, unpack_normal(texel.y), unpack_normal(texel.w),
        props.x, props.y, props.z, props.w, model(texel),
        f32(bitcast<i32>(texel.w) >> 24u) / 127.0 * LIFT_RANGE);
}
