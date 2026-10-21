// The touch field (crates/engine/src/trees/touch.rs): a world-aligned 3D texture wrapping around
// the player in which characters push branches aside and from which they swing back. Each voxel is
// a spring holding how far the foliage resting there is pushed: xy along world x and z in metres,
// zw its velocity. Inside a character's body, an upright capsule, the spring follows the way out
// quickly; once the body has gone it swings back, underdamped, so a branch settles after a wobble.
// Tree vertices read it at their rest position and turn about their limb's root by it
// (tree/material.wesl, tree/wind.wesl); tree/touch_stamp.wesl steps it each frame. Bindings stay
// with those shaders.

// Where the field lies: shared by the pass and every tree material.
struct TouchWindow {
    // xyz: the window's centre in canonical coordinates (render x and z plus `origin`, y as is),
    // w: metres per voxel (0: no field).
    centre: vec4<f32>,
    // Voxels across, voxels high, seconds since the last frame.
    sides: vec4<f32>,
    // xz: the render origin's canonical position, in x and y.
    origin: vec4<f32>,
    // The voxels that move this frame, as columns from the window's first: lowest x and z,
    // highest x and z (none while the lowest exceed the highest). The pass fills it.
    moving: vec4<u32>,
}

// This frame's pass: the window, where it lay last frame (xyz) and the bodies (w).
struct Touch {
    window: TouchWindow,
    previous: vec4<f32>,
    // Per body: (canonical feet x, y, z, radius), (velocity x, z, height, 0).
    bodies: array<vec4<f32>, 32>,
}

// Foliage this far outside a body is already pushed: needles and leaves stand off their twigs.
const TOUCH_MARGIN: f32 = 0.12;
// A moving body drags what it pushes along its way by this share of the push.
const TOUCH_DRAG: f32 = 0.35;
// Furthest a voxel holds foliage pushed, metres.
const TOUCH_REACH: f32 = 0.9;
// Spring while a body pushes (fast, critically damped) and once it lets go (about a second a
// swing, underdamped): angular frequency in radians per second and damping ratio.
const PRESS_SPRING: vec2<f32> = vec2(18.0, 1.0);
const RELEASE_SPRING: vec2<f32> = vec2(6.5, 0.28);
// Integration step bound, seconds.
const MAX_STEP: f32 = 1.0 / 120.0;

// The canonical position of render-space `p`.
fn touch_canonical(window: TouchWindow, p: vec3<f32>) -> vec3<f32> {
    return vec3(p.x + window.origin.x, p.y, p.z + window.origin.y);
}

// How much of the field holds at canonical `q`: none at and past the window's edge, all from two
// voxels inside it, so the wrap never shows.
fn touch_inside(window: TouchWindow, q: vec3<f32>) -> f32 {
    let half = vec3(window.sides.x, window.sides.y, window.sides.x) * window.centre.w * 0.5;
    let room = min(min(half.x - abs(q.x - window.centre.x), half.y - abs(q.y - window.centre.y)),
        half.z - abs(q.z - window.centre.z));
    return clamp(room / (2.0 * window.centre.w) - 1.0, 0.0, 1.0);
}

// The canonical xz the field's moving voxels cover, grown by `margin`: lowest, then highest; the
// lowest exceed the highest while none moves.
fn touch_moving(window: TouchWindow, margin: f32) -> vec4<f32> {
    let voxel = window.centre.w;
    let first = window.centre.xz - window.sides.x * voxel * 0.5;
    let low = first + vec2<f32>(window.moving.xy) * voxel - margin;
    let high = first + vec2<f32>(window.moving.zw + 1u) * voxel + margin;
    return select(vec4(low, high), vec4(1.0, 1.0, -1.0, -1.0), any(window.moving.xy > window.moving.zw));
}

// Whether canonical `xz` lies in a box from touch_moving.
fn within(area: vec4<f32>, xz: vec2<f32>) -> bool {
    return all(xz >= area.xy) && all(xz <= area.zw);
}

// Texture coordinates of canonical `q`: the texture repeats, so texels hold fixed world points.
fn touch_uv(window: TouchWindow, q: vec3<f32>) -> vec3<f32> {
    return q / (vec3(window.sides.x, window.sides.y, window.sides.x) * window.centre.w);
}

// The push the body `feet` (canonical feet, radius) moving as `motion` (velocity xz, height)
// gives foliage resting at canonical `q`, and 1 in w where `q` lies within its reach.
fn body_push(q: vec3<f32>, feet: vec4<f32>, motion: vec4<f32>) -> vec4<f32> {
    let radius = max(feet.w, 0.05);
    let low = feet.y + radius;
    let high = max(feet.y + motion.z - radius, low);
    let axis = vec3(feet.x, clamp(q.y, low, high), feet.z);
    let offset = q - axis;
    let reach = radius + TOUCH_MARGIN;
    let depth = reach - length(offset);
    if depth <= 0.0 {
        return vec4(0.0);
    }
    let speed = length(motion.xy);
    let along = select(vec2(0.0), motion.xy / speed, speed > 1e-3);
    let across = length(offset.xz);
    // Straight above or below the axis the way out is the way the body moves, or any.
    let out = select(select(vec2(1.0, 0.0), along, speed > 1e-3), offset.xz / across, across > 1e-3);
    let push = (out + along * TOUCH_DRAG * smoothstep(0.2, 1.0, speed)) * depth;
    return vec4(push.x, 0.0, push.y, 1.0);
}

// A voxel's spring `state` (push, velocity) after `seconds` towards `goal`, pressed while a body
// holds it and swinging back to rest once none does.
fn touch_spring(state: vec4<f32>, goal: vec2<f32>, held: bool, seconds: f32) -> vec4<f32> {
    let spring = select(RELEASE_SPRING, PRESS_SPRING, held);
    let aim = select(vec2(0.0), goal, held);
    let total = clamp(seconds, 0.0, 0.1);
    let steps = max(1u, u32(ceil(total / MAX_STEP)));
    let h = total / f32(steps);
    var position = state.xy;
    var velocity = state.zw;
    for (var i = 0u; i < steps; i += 1u) {
        let acceleration = spring.x * spring.x * (aim - position)
            - 2.0 * spring.y * spring.x * velocity;
        velocity += acceleration * h;
        position += velocity * h;
    }
    let pushed = length(position);
    if pushed > TOUCH_REACH {
        position *= TOUCH_REACH / pushed;
    }
    return vec4(position, velocity);
}
