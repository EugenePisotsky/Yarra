// Ground prints (crates/terrain_render/src/prints.rs): each workgroup updates one 8×8 tile of a
// window, starting it untouched when the window has just moved onto it, then pressing the stamps
// listed for it (terrain/print_map.wesl): a sole caps the soft layer at its floor under it, its
// wall rising steeply beyond, and marks its floor as far as it reaches.
import package::shaders::terrain::print_map::{print_values, wall_rise};

struct Prints {
    // Per window, two texels (prints.rs PrintFrame::windows): the render-space corner of its first
    // texel (f32 bits) and the toroidal texel holding it; metres per texel (f32 bits), texels per
    // side, drawn, and samples per texel side.
    windows: array<vec4<u32>, 4>,
    // x: how far past its outline a sole reaches, metres.
    shape: vec4<f32>,
    // x: tiles listed.
    counts: vec4<u32>,
}

// Heel and ball centres (render xz); their radii and floors over the firm ground; x: how far
// the outline draws in at its middle, as a share of its width.
struct Stamp { ends: vec4<f32>, sole: vec4<f32>, shape: vec4<f32>, }

@group(0) @binding(0) var<uniform> prints: Prints;
// Per tile: x and z within its window, first stamp in `indices`, then level (bit 0), reset (bit 1)
// and stamp count (from bit 2).
@group(0) @binding(1) var<storage, read> tiles: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read> indices: array<u32>;
@group(0) @binding(3) var<storage, read> stamps: array<Stamp>;
@group(0) @binding(4) var map: texture_storage_2d_array<r32uint, read_write>;

// prints/gpu.rs DISPATCH_ROW.
const DISPATCH_ROW: u32 = 256u;
const PI: f32 = 3.14159265;

// The values a stamp gives render xz `p`: its cap there and, within its reach, its floor.
fn stamp_values(s: Stamp, p: vec2<f32>) -> vec2<f32> {
    let heel = s.ends.xy;
    let along = s.ends.zw - heel;
    let length2 = dot(along, along);
    let t = select(0.0, clamp(dot(p - heel, along) / length2, 0.0, 1.0), length2 > 1e-8);
    let radius = mix(s.sole.x, s.sole.y, t) * (1.0 - s.shape.x * sin(PI * t));
    // Distance outside the sole's outline; negative inside.
    let edge = length(p - heel - along * t) - radius;
    if edge >= prints.shape.x {
        return vec2(0.0);
    }
    let floor = max(mix(s.sole.z, s.sole.w, t), 0.0);
    return print_values(vec2(floor + wall_rise(max(edge, 0.0)), floor));
}

@compute @workgroup_size(8, 8, 1)
fn press(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>) {
    let index = group.y * DISPATCH_ROW + group.x;
    if index >= prints.counts.x {
        return;
    }
    let tile = tiles[index];
    let level = tile.w & 1u;
    let count = tile.w >> 2u;
    let a = prints.windows[2u * level];
    let b = prints.windows[2u * level + 1u];
    let corner = bitcast<vec2<f32>>(a.xy);
    let texel_metres = bitcast<f32>(b.x);
    let samples = max(b.w, 1u);
    let texel = tile.xy * 8u + local.xy;
    let at = vec2<i32>((texel + a.zw) % b.y);
    var held = vec2(0.0);
    if (tile.w & 2u) == 0u {
        held = unpack2x16float(textureLoad(map, at, level).r);
    }
    // A coarse texel takes a stamp's mean over the fine texels it covers.
    let weight = 1.0 / f32(samples * samples);
    for (var i = 0u; i < count; i += 1u) {
        let s = stamps[indices[tile.z + i]];
        var values = vec2(0.0);
        for (var y = 0u; y < samples; y += 1u) {
            for (var x = 0u; x < samples; x += 1u) {
                let offset = (vec2(f32(x), f32(y)) + 0.5) / f32(samples);
                values += stamp_values(s, corner + (vec2<f32>(texel) + offset) * texel_metres);
            }
        }
        held = max(held, values * weight);
    }
    textureStore(map, at, level, vec4(pack2x16float(held), 0u, 0u, 0u));
}
