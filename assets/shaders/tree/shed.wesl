// Snow characters shake off trees (crates/engine/src/trees/gpu/shed.rs). Each snowy tree in the GPU
// trees' pool holds a state per primary limb: when it shed (negative while it holds its snow),
// how fast its bough moved, and its share of the snow. tree/shed_shake.wesl sheds a limb where
// the touch field pushes its bough; snowy vertices read their limb's state by the number they
// carry in uv_b.y (tree/material.wesl). Needles and bark lose their snow over SHED_SECONDS, the
// loads' chunks fall from the bough as it moved, and the bough, unloaded, springs up.

// What reads the states: the game clock and the frame's time step (x, y), and per batch origin
// slot of the pool its first state, its first pool instance and limbs per tree (x SHED_NONE: its
// trees hold no snow).
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

// A limb's state at `now`: seconds since it shed (negative while it holds its snow), its bough's
// velocity in x and z as it shed, and its share of the snow.
fn shed_since(now: f32, state: vec4<f32>) -> vec4<f32> {
    return vec4(select(-1.0, now - state.x, state.x >= 0.0), state.yzw);
}

// How far a limb's snow has slid off `since` seconds after it shed, 0 (all held) to 1 (gone).
fn shed_fallen(since: f32) -> f32 {
    return select(0.0, clamp(since / SHED_SECONDS, 0.0, 1.0), since >= 0.0);
}

// Unloaded, a bough rises this far (radians, for a whole share of snow), springing up past it
// (angular frequency, damping ratio).
const UNLOAD_LIFT: f32 = 0.12;
const UNLOAD_SPRING: vec2<f32> = vec2(9.0, 0.2);

// How far an unloaded bough has risen toward its unloaded pose `t` seconds after it shed: the
// step response of an underdamped spring, overshooting and settling at 1.
fn unload_rise(t: f32) -> f32 {
    let w = UNLOAD_SPRING.x;
    let z = UNLOAD_SPRING.y;
    let damped = w * sqrt(1.0 - z * z);
    return 1.0 - exp(-z * w * t) * (cos(damped * t) + z * w / damped * sin(damped * t));
}

// A snow load's chunk falling: how far it has gone from where it lay, how it has turned about
// its middle (axis, angle), and its size.
struct ChunkFall {
    offset: vec3<f32>,
    axis: vec3<f32>,
    angle: f32,
    scale: f32,
}

// Chunks leave with this share of their bough's velocity and up to CHUNK_KICK upward (m/s), drop
// toward a terminal speed between those of CHUNK_TERMINAL against the air's drag, and crumble
// over CHUNK_CRUMBLE seconds into the clumps and powder the ambient particles carry on
// (weather/ambient.wesl). They slide off nearly level, turning at most CHUNK_SPIN radians a
// second: a chunk is its load's top surface, thin and open below, and tumbling shows it as a
// shard.
const CHUNK_FLING: f32 = 0.6;
const CHUNK_KICK: f32 = 0.6;
const CHUNK_TERMINAL: vec2<f32> = vec2(3.0, 5.0);
const CHUNK_SPIN: vec2<f32> = vec2(0.5, 2.0);
const CHUNK_CRUMBLE: vec2<f32> = vec2(0.2, 0.55);

// How the chunk seeded `seed` (0..1) falls `t` seconds after its limb, in state `shed`
// (shed_since), shed.
fn chunk_fall(seed: f32, shed: vec4<f32>, t: f32) -> ChunkFall {
    let r = fract(sin(seed * vec3(127.1, 311.7, 74.7) + vec3(1.3, 4.1, 7.7)) * 43758.5453);
    let terminal = mix(CHUNK_TERMINAL.x, CHUNK_TERMINAL.y, r.x);
    let tau = terminal / SHED_GRAVITY;
    let thrown = vec3(shed.y * CHUNK_FLING, CHUNK_KICK * r.y, shed.z * CHUNK_FLING);
    let settled = vec3(0.0, -terminal, 0.0);
    var fall: ChunkFall;
    fall.offset = settled * t + (thrown - settled) * tau * (1.0 - exp(-t / tau));
    fall.axis = normalize(r - 0.5 + vec3(0.0, 0.0, 1e-3));
    fall.angle = mix(CHUNK_SPIN.x, CHUNK_SPIN.y, r.z) * t;
    fall.scale = 1.0 - smoothstep(CHUNK_CRUMBLE.x, CHUNK_CRUMBLE.y, t);
    return fall;
}
