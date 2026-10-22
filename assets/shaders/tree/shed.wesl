// Snow characters shook off trees (crates/engine/src/trees/shed.rs): a map wrapping around the
// player of the lowest and highest height at which they shook snowy boughs over each spot, and
// when. Snowy tree vertices read it along their limb (tree/material.wesl), which binds it: needles
// and bark lose their snow over SHED_SECONDS, and snow loads sink into their card as they drop.

// Where the map lies: shared by every tree material.
struct ShedWindow {
    // xy: canonical x and z of the window's first texel's corner; z: metres per texel; w: texels
    // per side (0: no map).
    area: vec4<f32>,
    // xy: the render origin's canonical x and z; z: the game clock, seconds.
    origin: vec4<f32>,
    // The canonical x and z every shaken texel lies within: lowest, then highest (the lowest
    // above the highest while none is).
    shaken: vec4<f32>,
}

// Seconds a shaken bough's snow takes to slide off it.
const SHED_SECONDS: f32 = 0.35;

const SHED_GRAVITY: f32 = 9.81;

// Whether the snow of a tree rooted at render-space `root`, its limbs reaching `reach` metres, may
// have been shaken.
fn shed_near(window: ShedWindow, root: vec3<f32>, reach: f32) -> bool {
    let q = root.xz + window.origin.xy;
    return window.area.w > 0.0 && all(q >= window.shaken.xy - reach) && all(q <= window.shaken.zw + reach);
}

// The map texel over render-space `p` (xy), and 1 in z where the window holds it.
fn shed_texel(window: ShedWindow, p: vec3<f32>) -> vec3<i32> {
    let texel = vec2<i32>(floor((p.xz + window.origin.xy) / window.area.z));
    let side = i32(window.area.w);
    let inside = texel - vec2<i32>(round(window.area.xy / window.area.z));
    let held = all(inside >= vec2(0)) && all(inside < vec2(side));
    return vec3(((texel % side) + side) % side, select(0, 1, held));
}

// How far snow has slid off, 0 (all held) to 1 (gone), `since` seconds after it was shaken
// (negative: never).
fn shed_fallen(since: f32) -> f32 {
    return select(0.0, clamp(since / SHED_SECONDS, 0.0, 1.0), since >= 0.0);
}
