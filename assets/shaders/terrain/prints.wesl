// Ground prints as terrain v2 draws them (crates/terrain_render/src/prints.rs): the relief feet
// pressed into the ground, in a fine window and a coarse one centred on the player that wrap
// around it toroidally, read texel by texel and filtered bilinearly. The fine window blends into
// the coarse one where it ends; the coarse one fades out at its own edge.

// Relief in metres, one layer per window: below zero pressed down, above zero pushed up.
@group(constants::MATERIAL_BIND_GROUP) @binding(13) var prints: texture_2d_array<f32>;
// Two texels per window (prints.rs PrintFrame::windows).
@group(constants::MATERIAL_BIND_GROUP) @binding(14) var print_windows: texture_2d<u32>;

// A window: the render-space corner of its first texel, the toroidal texel holding it, metres per
// texel and texels per side (0 while not drawn).
struct PrintWindow { corner: vec2<f32>, wrap: vec2<i32>, texel: f32, side: i32, }

// Both windows and each one's share of the relief around a point.
struct PrintView { fine: PrintWindow, coarse: PrintWindow, shares: vec2<f32>, }

// Texels a point keeps from a window's edge: room for the filter and the parallax's shift.
const PRINT_MARGIN: f32 = 12.0;
// Texels over which a window gives way at its edge.
const PRINT_BLEND: f32 = 64.0;

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

fn level_texel(level: i32, w: PrintWindow, texel: vec2<i32>) -> f32 {
    return textureLoad(prints, (texel + w.wrap) & vec2(w.side - 1), level, 0).r;
}

fn level_relief(level: i32, w: PrintWindow, p: vec2<f32>) -> f32 {
    let t = (p - w.corner) / w.texel - 0.5;
    let i = vec2<i32>(floor(t));
    let f = t - floor(t);
    let a = level_texel(level, w, i);
    let b = level_texel(level, w, i + vec2(1, 0));
    let c = level_texel(level, w, i + vec2(0, 1));
    let d = level_texel(level, w, i + vec2(1, 1));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// The relief at render xz `p` as ground `soft` metres deep takes it: pressed no deeper than it
// is, and shallow ground pushed up as little as it is pressed.
fn print_relief(v: PrintView, p: vec2<f32>, soft: f32) -> f32 {
    var relief = 0.0;
    if v.shares.x > 0.0 {
        relief += v.shares.x * level_relief(0, v.fine, p);
    }
    if v.shares.y > 0.0 {
        relief += v.shares.y * level_relief(1, v.coarse, p);
    }
    return clamp(relief, -soft, soft);
}

// The relief at `p` and its slope along x and z.
fn print_relief_slope(v: PrintView, p: vec2<f32>, soft: f32) -> vec3<f32> {
    let e = select(v.coarse.texel, v.fine.texel, v.shares.x > 0.0);
    let x = print_relief(v, p + vec2(e, 0.0), soft) - print_relief(v, p - vec2(e, 0.0), soft);
    let z = print_relief(v, p + vec2(0.0, e), soft) - print_relief(v, p - vec2(0.0, e), soft);
    return vec3(print_relief(v, p, soft), vec2(x, z) / (2.0 * e));
}

// How far the ground at `p` is pressed below its surface; snow pushed up above it is not.
fn print_depth(v: PrintView, p: vec2<f32>, soft: f32) -> f32 {
    return max(-print_relief(v, p, soft), 0.0);
}

// Where the view ray (`view`: toward the eye) entering the ground at render xz `p` meets the
// pressed ground below it: the shift along the ground (xz, metres) and the depth there. `normal`
// is the terrain's; the march reaches `deepest` metres down. A print's walls are steep, so the
// step that passes below the ground is bisected: interpolating between steps put some wall
// pixels on the sunlit floor.
fn print_parallax(v: PrintView, p: vec2<f32>, soft: f32, view: vec3<f32>, normal: vec3<f32>, deepest: f32) -> vec3<f32> {
    let vn = max(dot(view, normal), 0.15);
    // Along the ground per metre down.
    let shift = -(view.xz - normal.xz * dot(view, normal)) / vn;
    var above = 0.0;
    for (var i = 1u; i <= 12u; i += 1u) {
        var below = deepest * f32(i) / 12.0;
        if below >= print_depth(v, p + shift * below, soft) {
            for (var j = 0u; j < 4u; j += 1u) {
                let middle = 0.5 * (above + below);
                if middle >= print_depth(v, p + shift * middle, soft) {
                    below = middle;
                } else {
                    above = middle;
                }
            }
            return vec3(shift * below, print_depth(v, p + shift * below, soft));
        }
        above = below;
    }
    return vec3(shift * deepest, print_depth(v, p + shift * deepest, soft));
}
