// The trample map (vegetation_render renderer/trample.rs): each texel of a world-aligned texture
// wrapping around the camera is a damped spring holding how the grass rooted there is displaced
// by characters: xy, the way its tips are pushed (length 1 lies flat), zw, their velocity.
// Characters set the spring's target: flat along their motion under their feet, leaning away
// from their legs to either side. The spring pulls blades down within a fraction of a second
// and lets them spring back over a second or two, a little past upright.

struct Trample {
    // xy: canonical centre this frame, zw: last frame's
    centres: vec4<f32>,
    // x: metres per texel, y: texels per side, z: seconds since last frame, w: benders
    params: vec4<f32>,
    // Per bender: (canonical x, z, radius, 0), (velocity x, z, 0, 0).
    benders: array<vec4<f32>, 32>,
}

@group(0) @binding(0) var<uniform> trample: Trample;
@group(0) @binding(1) var previous: texture_2d<f32>;
@group(0) @binding(2) var current: texture_storage_2d<rgba16float, write>;

// Spring while a character presses (fast, no overshoot) and while grass recovers (slower,
// slightly underdamped): angular frequency in radians per second and damping ratio.
const PRESS_SPRING: vec2<f32> = vec2(20.0, 1.0);
const RECOVER_SPRING: vec2<f32> = vec2(3.5, 0.6);
// Shares of a bender's radius: under its feet grass lies flat; out to the radius it leans away.
const FEET_SHARE: vec2<f32> = vec2(0.2, 0.5);
const ASIDE_LEAN: f32 = 0.45;
// Integration step bound, seconds.
const MAX_STEP: f32 = 1.0 / 120.0;

// The canonical position, along one axis, of the texel's ground nearest a centre.
fn nearest(cell: f32, centre: f32) -> f32 {
    let texel = trample.params.x;
    let side = trample.params.y;
    let k = round((centre / texel - cell - 0.5) / side);
    return (cell + 0.5 + k * side) * texel;
}

// Where characters push the grass at `here`, and whether any does.
fn goal_at(here: vec2<f32>) -> vec3<f32> {
    var best = vec2<f32>(0.0);
    var pressed = 0.0;
    for (var i = 0u; i < u32(trample.params.w); i += 1u) {
        let disc = trample.benders[2u * i];
        let velocity = trample.benders[2u * i + 1u].xy;
        let offset = here - disc.xy;
        let distance = length(offset);
        let radius = max(disc.z, 0.05);
        if (distance >= radius) {
            continue;
        }
        let radial = select(vec2<f32>(1.0, 0.0), offset / distance, distance > 1e-4);
        let feet = 1.0 - smoothstep(FEET_SHARE.x * radius, FEET_SHARE.y * radius, distance);
        let aside = ASIDE_LEAN * (1.0 - smoothstep(FEET_SHARE.y * radius, radius, distance));
        // Under the feet blades lie along the motion; beside the legs they lean away from them.
        let speed = length(velocity);
        let along = select(radial, velocity / speed, speed > 0.2);
        let under = normalize(mix(radial, along, min(speed, 1.0)) + 1e-5);
        let push = under * feet + radial * aside * (1.0 - feet);
        if (dot(push, push) > dot(best, best)) {
            best = push;
        }
        pressed = 1.0;
    }
    return vec3<f32>(best, pressed);
}

@compute @workgroup_size(8, 8, 1)
fn stamp(@builtin(global_invocation_id) id: vec3<u32>) {
    let side = u32(trample.params.y);
    if (id.x >= side || id.y >= side) {
        return;
    }
    let cell = vec2<f32>(id.xy);
    let here = vec2(nearest(cell.x, trample.centres.x), nearest(cell.y, trample.centres.y));
    let before = vec2(nearest(cell.x, trample.centres.z), nearest(cell.y, trample.centres.w));
    // A texel the map's movement wrapped onto new ground starts at rest.
    var state = vec4<f32>(0.0);
    if (all(here == before)) {
        state = textureLoad(previous, vec2<i32>(id.xy), 0);
    }
    let goal = goal_at(here);
    // Nothing to do for grass at rest with no one near.
    if (goal.z == 0.0 && dot(state, state) < 1e-8) {
        textureStore(current, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }
    let spring = select(RECOVER_SPRING, PRESS_SPRING, goal.z > 0.0 && dot(goal.xy, goal.xy) > 1e-4);
    let total = clamp(trample.params.z, 0.0, 0.1);
    let steps = max(1u, u32(ceil(total / MAX_STEP)));
    let h = total / f32(steps);
    var position = state.xy;
    var velocity = state.zw;
    for (var i = 0u; i < steps; i += 1u) {
        let acceleration = spring.x * spring.x * (goal.xy - position)
            - 2.0 * spring.y * spring.x * velocity;
        velocity += acceleration * h;
        position += velocity * h;
    }
    // Settled grass leaves the map.
    if (goal.z == 0.0 && dot(position, position) < 1e-6 && dot(velocity, velocity) < 1e-6) {
        position = vec2<f32>(0.0);
        velocity = vec2<f32>(0.0);
    }
    textureStore(current, vec2<i32>(id.xy), vec4<f32>(position, velocity));
}
