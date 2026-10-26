// Grass in the G-buffer, model GRASS (lighting/gbuffer.wesl): the blade's surface
// (draw/lighting.wesl `GrassSurface`) in the `Rgba32Uint` texel and an `Rgba8Unorm` texel of the
// grass's own:
// - x: albedo, gamma 2.2, 8 bits a channel; occlusion 8;
// - y: flat normal, octahedral 12 + 12; model 8;
// - z: rounded normal, octahedral 12 + 12; sheen alpha's square root 8;
// - w: clump normal, octahedral 8 + 8; blade alpha's square root 8; sheen strength / 2 8;
// - own texel: transmission, canopy depth, canopy height / 2 m, spare.
import package::shaders::lighting::gbuffer::{
    MODEL_GRASS, pack_normal, pack_normal16, pack_unorm4, unpack_normal, unpack_normal16,
    unpack_unorm4,
};
import package::shaders::vegetation::draw::lighting::GrassSurface;

// Metres of canopy the own texel's byte spans: blades stand at most 3 m, their canopy 1.65 m.
const CANOPY_RANGE: f32 = 2.0;

// A value in 0..1 as a byte.
fn byte(v: f32) -> u32 {
    return u32(round(saturate(v) * 255.0));
}

struct GrassTexels {
    surface: vec4<u32>,
    own: vec4<f32>,
}

// `clump_normal`: the surface's, in its 16 bits as drawing carries it.
fn pack(s: GrassSurface, clump_normal: u32) -> GrassTexels {
    return GrassTexels(
        vec4(
            pack_unorm4(vec4(pow(saturate(s.albedo), vec3(1.0 / 2.2)), s.occlusion)),
            pack_normal(s.flat_normal) | (MODEL_GRASS << 24u),
            pack_normal(s.rounded_normal) | (byte(sqrt(s.sheen_alpha)) << 24u),
            (clump_normal & 0xffffu) | (byte(sqrt(s.blade_alpha)) << 16u)
                | (byte(s.sheen * 0.5) << 24u),
        ),
        vec4(s.transmission, s.canopy_depth, s.canopy_height / CANOPY_RANGE, 0.0),
    );
}

fn unpack(surface: vec4<u32>, own: vec4<f32>) -> GrassSurface {
    let base = unpack_unorm4(surface.x);
    let alphas = unpack_unorm4(surface.w);
    let sheen_root = unpack_unorm4(surface.z).w;
    var s: GrassSurface;
    s.albedo = pow(base.rgb, vec3(2.2));
    s.occlusion = base.a;
    s.flat_normal = unpack_normal(surface.y);
    s.rounded_normal = unpack_normal(surface.z);
    s.clump_normal = unpack_normal16(surface.w);
    s.sheen_alpha = sheen_root * sheen_root;
    s.blade_alpha = alphas.z * alphas.z;
    s.sheen = alphas.w * 2.0;
    s.transmission = own.x;
    s.canopy_depth = own.y;
    s.canopy_height = own.z * CANOPY_RANGE;
    return s;
}
