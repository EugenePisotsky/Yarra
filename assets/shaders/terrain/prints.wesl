// Ground prints as terrain v2 draws them (crates/terrain_render/src/prints.rs): how feet left the
// soft layer, read from a fine window and a coarse one centred on the player that wrap around it
// toroidally (terrain/print_map.wesl), texel by texel and filtered bilinearly. The fine window
// blends into the coarse one where it ends; the coarse one fades out at its own edge. The
// vertices stand on the layer as feet left it, and the material slopes and packs it.
import package::shaders::terrain::print_map::{print_heights, wall_distance};

// Two half floats per texel, one layer per window.
@group(constants::MATERIAL_BIND_GROUP) @binding(13) var prints: texture_2d_array<u32>;
// Two texels per window (prints.rs PrintFrame::windows).
@group(constants::MATERIAL_BIND_GROUP) @binding(14) var print_windows: texture_2d<u32>;

// A window: the render-space corner of its first texel, the toroidal texel holding it, metres per
// texel and texels per side (0 while not drawn).
struct PrintWindow { corner: vec2<f32>, wrap: vec2<i32>, texel: f32, side: i32, }

// Both windows and each one's share of the prints around a point.
struct PrintView { fine: PrintWindow, coarse: PrintWindow, shares: vec2<f32>, }

// Texels a point keeps from a window's edge: room for the filter and the slope's reach.
const PRINT_MARGIN: f32 = 4.0;
// Texels over which a window gives way at its edge.
const PRINT_BLEND: f32 = 64.0;
// The snow pushed aside rises beside a trench to this share of the trench's depth, over this many
// metres past its edge.
const RIM_SHARE: f32 = 0.25;
const RIM_WIDTH: f32 = 0.08;

fn print_window(level: i32) -> PrintWindow {
    let a = textureLoad(print_windows, vec2(2 * level, 0), 0);
    let b = textureLoad(print_windows, vec2(2 * level + 1, 0), 0);
    return PrintWindow(bitcast<vec2<f32>>(a.xy), vec2<i32>(a.zw), bitcast<f32>(b.x), select(0, i32(b.y), b.z == 1u));
}

// How far inside a window render xz `p` is, as a share that falls to 0 near its edge.
fn window_share(w: PrintWindow, p: vec2<f32>) -> f32 {
    if w.side <= 0 {
        return 0.0;
    }
    let t = (p - w.corner) / w.texel;
    let edge = min(min(t.x, t.y), min(f32(w.side) - t.x, f32(w.side) - t.y));
    return smoothstep(PRINT_MARGIN, PRINT_MARGIN + PRINT_BLEND, edge);
}

fn print_view(p: vec2<f32>) -> PrintView {
    let fine = print_window(0);
    let coarse = print_window(1);
    let f = window_share(fine, p);
    return PrintView(fine, coarse, vec2(f, (1.0 - f) * window_share(coarse, p)));
}

fn level_texel(level: i32, w: PrintWindow, texel: vec2<i32>) -> vec2<f32> {
    return unpack2x16float(textureLoad(prints, (texel + w.wrap) & vec2(w.side - 1), level, 0).r);
}

fn level_values(level: i32, w: PrintWindow, p: vec2<f32>) -> vec2<f32> {
    let t = (p - w.corner) / w.texel - 0.5;
    let i = vec2<i32>(floor(t));
    let f = t - floor(t);
    let a = level_texel(level, w, i);
    let b = level_texel(level, w, i + vec2(1, 0));
    let c = level_texel(level, w, i + vec2(0, 1));
    let d = level_texel(level, w, i + vec2(1, 1));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// How high over the firm ground the soft layer may stand at render xz `p` (x), and the lowest sole
// that reached it (y).
fn print_caps(v: PrintView, p: vec2<f32>) -> vec2<f32> {
    var values = vec2(0.0);
    if v.shares.x > 0.0 {
        values += v.shares.x * level_values(0, v.fine, p);
    }
    if v.shares.y > 0.0 {
        values += v.shares.y * level_values(1, v.coarse, p);
    }
    return print_heights(values);
}

// The soft layer, `depth` deep where untouched, as feet left it under `caps`: no higher than the
// cap, and, where a sole went below its top, risen beside the trench into the snow pushed aside.
// How far past the sole's outline a point lies follows from its cap's rise over the sole.
fn pressed_layer(depth: f32, caps: vec2<f32>) -> f32 {
    let deep = depth - caps.y;
    if deep <= 0.0 {
        return depth;
    }
    let past = wall_distance(caps.x - caps.y) - wall_distance(deep);
    let u = clamp(past / RIM_WIDTH, 0.0, 1.0);
    let rim = select(0.0, deep * RIM_SHARE * 4.0 * u * (1.0 - u), past > 0.0);
    return min(depth + rim, caps.x);
}

// The pressed layer at render xz `p`, `depth` deep there untouched.
fn pressed_layer_at(v: PrintView, p: vec2<f32>, depth: f32) -> f32 {
    if dot(v.shares, vec2(1.0)) <= 0.0 {
        return depth;
    }
    return pressed_layer(depth, print_caps(v, p));
}
