// Each frame's step of the touch field (tree/touch.wesl, crates/engine/src/trees/touch.rs): every
// voxel takes last frame's spring, unless the window's movement wrapped it onto new ground, and
// moves it towards the push of the bodies around it.
import package::shaders::tree::touch::{Touch, body_push, touch_spring};

@group(0) @binding(0) var<uniform> touch: Touch;
@group(0) @binding(1) var previous: texture_3d<f32>;
@group(0) @binding(2) var current: texture_storage_3d<rgba16float, write>;

// The materials' TouchWindow, whose moving columns this pass marks.
struct Marked {
    centre: vec4<f32>,
    sides: vec4<f32>,
    origin: vec4<f32>,
    moving: array<atomic<u32>, 4>,
}
@group(0) @binding(3) var<storage, read_write> marked: Marked;

// Pushes this small or less count as rest, metres.
const STILL: f32 = 1e-3;

// The canonical position, along one axis, of the voxel nearest `centre` among those the texture's
// cell `cell` holds, `side` cells apart.
fn nearest(cell: f32, centre: f32, side: f32) -> f32 {
    let voxel = touch.window.centre.w;
    let k = round((centre / voxel - cell - 0.5) / side);
    return (cell + 0.5 + k * side) * voxel;
}

fn holds(cell: vec3<f32>, centre: vec3<f32>) -> vec3<f32> {
    let sides = touch.window.sides;
    return vec3(nearest(cell.x, centre.x, sides.x), nearest(cell.y, centre.y, sides.y),
        nearest(cell.z, centre.z, sides.x));
}

@compute @workgroup_size(4, 4, 4)
fn stamp(@builtin(global_invocation_id) id: vec3<u32>) {
    let sides = vec3<u32>(u32(touch.window.sides.x), u32(touch.window.sides.y), u32(touch.window.sides.x));
    if any(id >= sides) {
        return;
    }
    let cell = vec3<f32>(id);
    let here = holds(cell, touch.window.centre.xyz);
    var state = vec4(0.0);
    if all(here == holds(cell, touch.previous.xyz)) {
        state = textureLoad(previous, vec3<i32>(id), 0);
    }
    var goal = vec4(0.0);
    for (var i = 0u; i < u32(touch.previous.w); i += 1u) {
        let push = body_push(here, touch.bodies[2u * i], touch.bodies[2u * i + 1u]);
        if push.w > 0.0 && dot(push.xz, push.xz) >= dot(goal.xz, goal.xz) {
            goal = push;
        }
    }
    // Foliage at rest with no one near stays at rest.
    if goal.w == 0.0 && dot(state, state) < 1e-10 {
        textureStore(current, vec3<i32>(id), vec4(0.0));
        return;
    }
    var next = touch_spring(state, goal.xz, goal.w > 0.0, touch.window.sides.z);
    if goal.w == 0.0 && dot(next, next) < 1e-8 {
        next = vec4(0.0);
    }
    textureStore(current, vec3<i32>(id), next);
    if dot(next.xy, next.xy) > STILL * STILL {
        let voxel = touch.window.centre.w;
        let first = touch.window.centre.xz - touch.window.sides.x * voxel * 0.5;
        let column = vec2<u32>(clamp(floor((here.xz - first) / voxel), vec2(0.0), vec2(touch.window.sides.x - 1.0)));
        atomicMin(&marked.moving[0], column.x);
        atomicMin(&marked.moving[1], column.y);
        atomicMax(&marked.moving[2], column.x);
        atomicMax(&marked.moving[3], column.y);
    }
}
