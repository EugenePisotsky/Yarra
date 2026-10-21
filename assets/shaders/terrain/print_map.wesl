// The print map's texels (crates/terrain_render/src/prints.rs), shared by the pass pressing it
// (terrain/press.wesl) and the one resolving it into the relief the terrain draws
// (terrain/print_relief.wesl). A texel holds how high over the firm ground the soft layer may
// stand (its cap) and the lowest sole that reached it, each as how far below CAP_REFERENCE it
// lies, two half floats in a u32: untouched ground is zero, and the lower a cap or sole the larger
// its value, so presses combine by their maximum.

// Higher than any soft layer.
const CAP_REFERENCE: f32 = 2.0;

// A trench's wall beside a sole: how far it rises over the sole's floor `x` metres past its
// outline, steep and steepening.
const WALL_SLOPE: f32 = 2.0;
const WALL_CURVE: f32 = 20.0;

fn wall_rise(x: f32) -> f32 {
    return x * (WALL_SLOPE + x * WALL_CURVE);
}

// How far past a sole's outline its wall has risen `rise` metres.
fn wall_distance(rise: f32) -> f32 {
    return (sqrt(WALL_SLOPE * WALL_SLOPE + 4.0 * WALL_CURVE * max(rise, 0.0)) - WALL_SLOPE) / (2.0 * WALL_CURVE);
}

// A texel's cap and lowest sole (metres over the firm ground) from its values, and back.
fn print_heights(values: vec2<f32>) -> vec2<f32> {
    return vec2(CAP_REFERENCE) - values;
}

fn print_values(heights: vec2<f32>) -> vec2<f32> {
    return max(vec2(CAP_REFERENCE) - heights, vec2(0.0));
}

// Snow's own unevenness where feet broke it: value noise over the world's 2 cm texels
// (prints.rs TEXEL_METRES), so a trail a window takes again comes back the same. `base` is a
// world texel and `offset` how many texels on the point lies, cells are 2^`shift` texels a side;
// in 0..1.
fn snow_noise(base: vec2<i32>, offset: vec2<f32>, shift: u32) -> f32 {
    let whole = floor(offset);
    let at = base + vec2<i32>(whole);
    let cell = at >> vec2(shift);
    let t = (vec2<f32>(at - (cell << vec2(shift))) + offset - whole) / f32(1u << shift);
    let f = t * t * (3.0 - 2.0 * t);
    let a = mix(snow_hash(cell), snow_hash(cell + vec2(1, 0)), f.x);
    let b = mix(snow_hash(cell + vec2(0, 1)), snow_hash(cell + vec2(1, 1)), f.x);
    return mix(a, b, f.y);
}

fn snow_hash(cell: vec2<i32>) -> f32 {
    var h = (bitcast<u32>(cell.x) * 0x8da6b343u) ^ (bitcast<u32>(cell.y) * 0xd8163841u);
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = (h ^ (h >> 12u)) * 0x297a2d39u;
    return f32(h ^ (h >> 15u)) * (1.0 / 4294967295.0);
}
