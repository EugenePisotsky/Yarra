// Ground prints' relief (crates/terrain_render/src/prints.rs): each workgroup resolves one 8×8
// tile of a window that the frame's presses, or a change of the soft layer under it, touched. From
// the caps feet left (terrain/print_map.wesl) and the soft layer's depth (world::soft_layer) it
// takes how far they lowered the ground, rims risen beside trenches, and its slope, which the
// terrain filters like any texture (terrain/prints.wesl); terrain/print_mips.wesl fills its mip
// levels after. Snow broken by feet is uneven: rims lump, crumbs lie thrown beside trenches and
// floors are trodden rough.
import package::shaders::terrain::print_map::{print_heights, snow_noise, wall_distance};

struct Relief {
    // Per window, as terrain/press.wesl reads them.
    windows: array<vec4<u32>, 4>,
    // The soft layer's depths (the leaves' clipmap texture): the render origin's texel (x, z),
    // texels per metre and texels per side; no side, no soft layer.
    ground: vec4<f32>,
    // x: tiles listed.
    counts: vec4<u32>,
    // Per window, the world's 2 cm texel holding its first texel (x, z).
    cells: vec4<i32>,
}

@group(0) @binding(0) var<uniform> relief: Relief;
// As terrain/press.wesl lists them.
@group(0) @binding(1) var<storage, read> tiles: array<vec4<u32>>;
@group(0) @binding(2) var map: texture_2d_array<u32>;
@group(0) @binding(3) var soft_depths: texture_2d<f32>;
@group(0) @binding(4) var soft_sampler: sampler;
// How far feet lowered the ground (metres; rims rise above it), its slope along x and z.
@group(0) @binding(5) var out: texture_storage_2d_array<rgba16float, write>;

// prints/gpu.rs DISPATCH_ROW.
const DISPATCH_ROW: u32 = 256u;
// The snow pushed aside rises beside a trench to this share of the trench's depth, over this many
// metres past its edge, either lumping by up to the given share of it.
const RIM_SHARE: f32 = 0.25;
const RIM_WIDTH: f32 = 0.08;
const RIM_LUMPS: f32 = 0.7;
// Crumbs a few centimetres across lie where the noise passes this, up to this share of the
// trench's depth high, thinning out over this many metres past its edge.
const CRUMBS: f32 = 0.72;
const CRUMB_SHARE: f32 = 0.12;
const CRUMB_REACH: f32 = 0.16;
// A trodden floor lies this many metres either way of its sole's.
const TRODDEN: f32 = 0.008;

struct Window { corner: vec2<f32>, wrap: vec2<i32>, texel: f32, side: i32, samples: i32, }

fn window(level: u32) -> Window {
    let a = relief.windows[2u * level];
    let b = relief.windows[2u * level + 1u];
    return Window(bitcast<vec2<f32>>(a.xy), vec2<i32>(a.zw), bitcast<f32>(b.x), i32(b.y), max(i32(b.w), 1));
}

// The snow's unevenness at a point: how its rim lumps (-1..1), how much of a crumb lies there
// (0..1) and how its trodden floor lies (-1..1).
struct Unevenness { rim: f32, crumb: f32, floor: f32, }

// The soft layer, `depth` deep where untouched, as feet left it under `caps`: no higher than the
// cap, and, where a sole went below its top, risen beside the trench into the snow pushed aside.
// How far past the sole's outline a point lies follows from its cap's rise over the sole.
fn pressed_layer(depth: f32, caps: vec2<f32>, snow: Unevenness) -> f32 {
    let deep = depth - caps.y;
    if deep <= 0.0 {
        return depth;
    }
    let past = wall_distance(caps.x - caps.y) - wall_distance(deep);
    let u = clamp(past / RIM_WIDTH, 0.0, 1.0);
    let rim = deep * RIM_SHARE * (1.0 + RIM_LUMPS * snow.rim) * 4.0 * u * (1.0 - u);
    let crumbs = deep * CRUMB_SHARE * snow.crumb * (1.0 - smoothstep(0.0, CRUMB_REACH, past));
    let thrown = select(0.0, rim + crumbs, past > 0.0);
    return min(depth + thrown, caps.x + TRODDEN * snow.floor);
}

// The snow's unevenness at the centre of texel `texel` of window `w`, whose first is `cells`.
fn unevenness(w: Window, cells: vec2<i32>, texel: vec2<i32>) -> Unevenness {
    let base = cells + texel * w.samples;
    let offset = vec2(0.5 * f32(w.samples));
    let crumb = snow_noise(base, offset, 1u);
    return Unevenness(
        snow_noise(base, offset, 3u) * 2.0 - 1.0,
        smoothstep(CRUMBS, 1.0, crumb),
        snow_noise(base + vec2(91, 37), offset, 2u) * 2.0 - 1.0,
    );
}

// How deep the soft layer lies at render xz `p`.
fn soft_depth(p: vec2<f32>) -> f32 {
    if relief.ground.w <= 0.0 {
        return 0.0;
    }
    let t = relief.ground.xy + p * relief.ground.z;
    return textureSampleLevel(soft_depths, soft_sampler, (t + 0.5) / relief.ground.w, 0.0).r;
}

// How far feet lowered the ground at a texel of a window, counted from its first.
fn lowered(level: u32, w: Window, texel: vec2<i32>) -> f32 {
    let at = (texel + w.wrap) & vec2(w.side - 1);
    let caps = print_heights(unpack2x16float(textureLoad(map, at, level, 0).r));
    let depth = soft_depth(w.corner + (vec2<f32>(texel) + 0.5) * w.texel);
    let cells = select(relief.cells.xy, relief.cells.zw, level == 1u);
    return pressed_layer(depth, caps, unevenness(w, cells, texel)) - depth;
}

@compute @workgroup_size(8, 8, 1)
fn resolve(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>) {
    let index = group.y * DISPATCH_ROW + group.x;
    if index >= relief.counts.x {
        return;
    }
    let tile = tiles[index];
    let level = tile.w & 1u;
    let w = window(level);
    let texel = vec2<i32>(tile.xy * 8u + local.xy);
    let e = vec2(1, 0);
    let slope = vec2(
        lowered(level, w, texel + e) - lowered(level, w, texel - e),
        lowered(level, w, texel + e.yx) - lowered(level, w, texel - e.yx),
    ) / (2.0 * w.texel);
    let at = (texel + w.wrap) & vec2(w.side - 1);
    textureStore(out, at, level, vec4(lowered(level, w, texel), slope, 0.0));
}
