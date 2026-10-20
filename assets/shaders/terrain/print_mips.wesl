// Ground prints' relief (terrain/print_relief.wesl), one mip level over the tiles resolved this
// frame: each texel the mean of the four it covers one level finer. A tile holds 8 >> level texels
// a side, and one texel at the levels above.
struct Mip {
    // The windows' toroidal first texels: the fine window's x and z, then the coarse one's.
    wraps: vec4<u32>,
    // Tiles listed, the level written, texels per side at level 0.
    counts: vec4<u32>,
}

@group(0) @binding(0) var<uniform> mip: Mip;
// As terrain/press.wesl lists them.
@group(0) @binding(1) var<storage, read> tiles: array<vec4<u32>>;
@group(0) @binding(2) var finer: texture_2d_array<f32>;
@group(0) @binding(3) var out: texture_storage_2d_array<rgba16float, write>;

// prints/gpu.rs DISPATCH_ROW.
const DISPATCH_ROW: u32 = 256u;

@compute @workgroup_size(8, 8, 1)
fn downsample(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>) {
    let index = group.y * DISPATCH_ROW + group.x;
    let level = mip.counts.y;
    let n = max(8u >> level, 1u);
    if index >= mip.counts.x || any(local.xy >= vec2(n)) {
        return;
    }
    let tile = tiles[index];
    let layer = tile.w & 1u;
    let wrap = select(mip.wraps.xy, mip.wraps.zw, layer == 1u);
    let first = ((tile.xy * 8u + wrap) & vec2(mip.counts.z - 1u)) >> vec2(level);
    let at = vec2<i32>(first + local.xy);
    let s = at * 2;
    let sum = textureLoad(finer, s, layer, 0) + textureLoad(finer, s + vec2(1, 0), layer, 0)
        + textureLoad(finer, s + vec2(0, 1), layer, 0) + textureLoad(finer, s + vec2(1, 1), layer, 0);
    textureStore(out, at, layer, sum * 0.25);
}
