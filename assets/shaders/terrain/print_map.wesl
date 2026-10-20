// The print map's texels (crates/terrain_render/src/prints.rs), shared by the pass pressing it
// (terrain/press.wesl) and the terrain reading it (terrain/prints.wesl). A texel holds how high
// over the firm ground the soft layer may stand (its cap) and the lowest sole that reached it,
// each as how far below CAP_REFERENCE it lies, two half floats in a u32: untouched ground is
// zero, and the lower a cap or sole the larger its value, so presses combine by their maximum.

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
