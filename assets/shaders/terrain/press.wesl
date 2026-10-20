// Ground prints (crates/terrain_render/src/prints.rs): each workgroup updates one 8×8 tile of a
// window, starting it undisturbed when the window has just moved onto it, then pressing the
// stamps listed for it in log order. A texel holds the ground's relief in metres: below zero a
// sole pressed it down, above zero the snow pushed aside rises beside the sole.

struct Prints {
    // Per window, two texels (prints.rs PrintFrame::windows): the render-space corner of its first
    // texel (f32 bits) and the toroidal texel holding it; metres per texel (f32 bits), texels per
    // side, drawn, and samples per texel side.
    windows: array<vec4<u32>, 4>,
    // The sole's wall and how far its rim reaches past the outline, metres; the rim's height as a
    // share of the print's depth.
    shape: vec4<f32>,
    // x: tiles listed.
    counts: vec4<u32>,
}

// Heel and ball centres (render xz); their radii and how deep each presses.
struct Stamp { ends: vec4<f32>, sole: vec4<f32>, }

@group(0) @binding(0) var<uniform> prints: Prints;
// Per tile: x and z within its window, first stamp in `indices`, then level (bit 0), reset (bit 1)
// and stamp count (from bit 2).
@group(0) @binding(1) var<storage, read> tiles: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read> indices: array<u32>;
@group(0) @binding(3) var<storage, read> stamps: array<Stamp>;
@group(0) @binding(4) var map: texture_storage_2d_array<r32float, read_write>;

// prints/gpu.rs DISPATCH_ROW.
const DISPATCH_ROW: u32 = 256u;
// The sole narrows by this share of its width at the arch.
const ARCH: f32 = 0.22;
const PI: f32 = 3.14159265;

// How far a stamp presses the ground at render xz `p` down (x), and how far the snow it pushed
// aside rises there (y).
fn stamp_at(s: Stamp, p: vec2<f32>) -> vec2<f32> {
    let heel = s.ends.xy;
    let along = s.ends.zw - heel;
    let length2 = dot(along, along);
    let t = select(0.0, clamp(dot(p - heel, along) / length2, 0.0, 1.0), length2 > 1e-8);
    let radius = mix(s.sole.x, s.sole.y, t) * (1.0 - ARCH * sin(PI * t));
    // Distance outside the sole's outline; negative inside.
    let edge = length(p - heel - along * t) - radius;
    let depth = max(mix(s.sole.z, s.sole.w, t), 0.0);
    let down = depth * smoothstep(0.0, prints.shape.x, -edge);
    let u = clamp(edge / prints.shape.y, 0.0, 1.0);
    let up = select(0.0, depth * prints.shape.z * 4.0 * u * (1.0 - u), edge > 0.0);
    return vec2(down, up);
}

// Deeper presses win and nothing raises pressed ground again; pushed-aside snow rises only
// where no sole pressed the ground down.
fn combine(relief: f32, pressed: vec2<f32>) -> f32 {
    if pressed.x > 0.0005 {
        return min(relief, -pressed.x);
    }
    if relief > -0.002 {
        return max(relief, pressed.y);
    }
    return relief;
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
    var relief = 0.0;
    if (tile.w & 2u) == 0u {
        relief = textureLoad(map, at, level).r;
    }
    // A coarse texel takes a stamp's mean over the fine texels it covers.
    let weight = 1.0 / f32(samples * samples);
    for (var i = 0u; i < count; i += 1u) {
        let s = stamps[indices[tile.z + i]];
        var pressed = vec2(0.0);
        for (var y = 0u; y < samples; y += 1u) {
            for (var x = 0u; x < samples; x += 1u) {
                let offset = (vec2(f32(x), f32(y)) + 0.5) / f32(samples);
                pressed += stamp_at(s, corner + (vec2<f32>(texel) + offset) * texel_metres);
            }
        }
        relief = combine(relief, pressed * weight);
    }
    textureStore(map, at, level, vec4(relief, 0.0, 0.0, 0.0));
}
