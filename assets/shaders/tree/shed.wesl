// Snow characters shake off trees (crates/engine/src/trees/gpu/shed.rs). Each snowy tree in the GPU
// trees' pool holds a state per primary limb: when it shed (negative while it holds its snow) and
// how fast its bough moved. tree/shed_shake.wesl sheds a limb where the touch field pushes its
// bough; snowy vertices read their limb's state by the number they carry in uv_b.y
// (tree/material.wesl): needles and bark lose their snow over SHED_SECONDS, and snow loads sink
// into their card as they drop.

// What reads the states: the game clock (x), and per batch origin slot of the pool its first
// state, its first pool instance and limbs per tree (x SHED_NONE: its trees hold no snow).
struct ShedTrees {
    now: vec4<f32>,
    batches: array<vec4<u32>>,
}

const SHED_NONE: u32 = 0xffffffffu;

// A limb of a snowy form at unit scale (`Limb` in shed.rs).
struct Limb {
    // xyz: its root; w: how far it reaches along its axis (0: no such limb).
    root: vec4<f32>,
    // xyz: its direction; w: its share of the snow, 0..1.
    axis: vec4<f32>,
    // xyz: where its snow starts along it; w: how far across the snow lies.
    snow: vec4<f32>,
    // xyz: from there to where the snow ends; w: how far its foliage spreads from its axis.
    along: vec4<f32>,
}

// Seconds a shaken bough's snow takes to slide off it.
const SHED_SECONDS: f32 = 0.35;

const SHED_GRAVITY: f32 = 9.81;

// The state of `limb` of pool instance `pool`, whose batch is `batch`.
fn shed_slot(batch: vec4<u32>, pool: u32, limb: u32) -> u32 {
    return batch.x + (pool - batch.y) * batch.z + limb;
}

// How far a limb's snow has slid off at `now`, 0 (all held) to 1 (gone), for a limb that shed at
// `shed` (negative: never).
fn shed_fallen(now: f32, shed: f32) -> f32 {
    return select(0.0, clamp((now - shed) / SHED_SECONDS, 0.0, 1.0), shed >= 0.0);
}
