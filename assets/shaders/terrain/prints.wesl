// Ground prints as terrain v2 draws them (crates/terrain_render/src/prints.rs): how far feet
// lowered the ground and its slope, resolved per texel of a fine window and a coarse one centred on
// the player that wrap around it toroidally (terrain/print_relief.wesl), and filtered like any
// texture, mip levels and all. The fine window blends into the coarse one where it ends; the
// coarse one fades out at its own edge. Vertices stand on the coarse window's relief filtered to
// their own spacing, so the ground they draw holds still while they morph; the material tilts and
// packs the ground by the relief at the pixel's footprint.

// How far feet lowered the ground (x, metres; rims rise above it) and its slope along x and z
// (y, z), one layer per window.
@group(constants::MATERIAL_BIND_GROUP) @binding(13) var print_relief: texture_2d_array<f32>;
// Two texels per window (prints.rs PrintFrame::windows).
@group(constants::MATERIAL_BIND_GROUP) @binding(14) var print_windows: texture_2d<u32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(15) var print_sampler: sampler;

// A window: the render-space corner of its first texel, the toroidal texel holding it, metres per
// texel and texels per side (0 while not drawn).
struct PrintWindow { corner: vec2<f32>, wrap: vec2<i32>, texel: f32, side: i32, }

// Both windows and each one's share of the prints around a point.
struct PrintView { fine: PrintWindow, coarse: PrintWindow, shares: vec2<f32>, }

// Texels a point keeps from a window's edge: room for the coarsest mip level's footprint, which
// reaches across the toroidal seam.
const PRINT_MARGIN: f32 = 32.0;
// Texels over which a window gives way at its edge.
const PRINT_BLEND: f32 = 64.0;
// prints/gpu.rs RELIEF_MIPS: past its coarsest level, a vertex's footprint takes no relief.
const PRINT_MIPS: f32 = 6.0;

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

fn relief_uv(w: PrintWindow, p: vec2<f32>) -> vec2<f32> {
    return ((p - w.corner) / w.texel + vec2<f32>(w.wrap)) / f32(w.side);
}

// The relief at render xz `p` (lowering, slope along x and z), over the footprint of a pixel
// whose xz changes by `dx` and `dy` across it.
fn print_relief_at(v: PrintView, p: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> vec3<f32> {
    var relief = vec3(0.0);
    if v.shares.x > 0.0 {
        let s = 1.0 / (v.fine.texel * f32(v.fine.side));
        relief += v.shares.x * textureSampleGrad(print_relief, print_sampler, relief_uv(v.fine, p), 0, dx * s, dy * s).xyz;
    }
    if v.shares.y > 0.0 {
        let s = 1.0 / (v.coarse.texel * f32(v.coarse.side));
        relief += v.shares.y * textureSampleGrad(print_relief, print_sampler, relief_uv(v.coarse, p), 1, dx * s, dy * s).xyz;
    }
    return relief;
}

// How far feet lowered the ground at render xz `p`, filtered over `footprint` metres: the coarse
// window's, for vertices that far apart.
fn print_lowering(p: vec2<f32>, footprint: f32) -> f32 {
    let w = print_window(1);
    let share = window_share(w, p);
    if share <= 0.0 {
        return 0.0;
    }
    let lod = log2(max(footprint / w.texel, 1.0));
    let reach = 1.0 - smoothstep(PRINT_MIPS - 1.0, PRINT_MIPS, lod);
    if reach <= 0.0 {
        return 0.0;
    }
    return share * reach * textureSampleLevel(print_relief, print_sampler, relief_uv(w, p), 1, lod).x;
}
