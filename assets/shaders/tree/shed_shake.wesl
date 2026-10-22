// Each frame's shake of the snowy trees near the touch field (crates/engine/src/trees/gpu/shed.rs):
// one invocation per limb of each listed tree reads the field (tree/touch.wesl) over its bough,
// across the width its foliage spreads, where its snow lies, and along its axis. A limb still
// holding its snow whose bough is pushed past SHED_PUSH sheds it, noting when and how fast the
// bough moved, and its snow falls from where it lay as a burst in the ambient particles' ring
// (weather/ambient.wesl).
import package::shaders::tree::instance::Instance;
import package::shaders::tree::shed::{Limb, SHED_NONE, ShedTrees, shed_slot};
import package::shaders::tree::touch::{TouchWindow, touch_canonical, touch_inside, touch_uv};
import package::shaders::weather::snow_fall::SnowFall;

struct Shake {
    // x: the game clock; y: trees listed; z: bursts the ring holds.
    frame: vec4<f32>,
}

@group(0) @binding(0) var<uniform> shake: Shake;
// The listed trees' pool instances.
@group(0) @binding(1) var<storage, read> trees: array<u32>;
@group(0) @binding(2) var<storage, read> instances: array<Instance>;
// Each pool instance's transform this frame (tree/lod.wesl).
@group(0) @binding(3) var<storage, read> transforms: array<array<vec4<f32>, 3>>;
@group(0) @binding(4) var<storage, read> shed: ShedTrees;
// Per form: its first limb in `limbs` and how many it has.
@group(0) @binding(5) var<storage, read> forms: array<vec4<u32>>;
@group(0) @binding(6) var<storage, read> limbs: array<Limb>;
@group(0) @binding(7) var<storage, read_write> states: array<vec4<f32>>;
@group(0) @binding(8) var touch_field: texture_3d<f32>;
@group(0) @binding(9) var touch_sampler: sampler;
@group(0) @binding(10) var<storage, read> touch_window: TouchWindow;
@group(0) @binding(11) var<storage, read_write> falls: array<SnowFall>;
@group(0) @binding(12) var<storage, read_write> next_fall: atomic<u32>;

// Invocations per listed tree: the most limbs a form may have.
const MAX_LIMBS: u32 = 128u;
// A bough pushed this far aside, metres, sheds its snow: it has visibly moved.
const SHED_PUSH: f32 = 0.06;
// Points read along the bough, each across its width at ACROSS of them, and along its axis. A
// body parts foliage within about 0.4 m of it, so the points lie closer than that.
const SAMPLES: u32 = 8u;
const ACROSS: u32 = 5u;

fn world_point(r: array<vec4<f32>, 3>, p: vec3<f32>) -> vec3<f32> {
    return vec3(dot(r[0].xyz, p) + r[0].w, dot(r[1].xyz, p) + r[1].w, dot(r[2].xyz, p) + r[2].w);
}

fn world_vector(r: array<vec4<f32>, 3>, v: vec3<f32>) -> vec3<f32> {
    return vec3(dot(r[0].xyz, v), dot(r[1].xyz, v), dot(r[2].xyz, v));
}

// The touch spring (push x and z, its velocity x and z) of foliage resting at render-space `p`.
fn spring_at(window: TouchWindow, p: vec3<f32>) -> vec4<f32> {
    let q = touch_canonical(window, p);
    let inside = touch_inside(window, q);
    if inside <= 0.0 {
        return vec4(0.0);
    }
    return textureSampleLevel(touch_field, touch_sampler, touch_uv(window, q), 0.0) * inside;
}

// The one of `a` and `b` pushed further.
fn further(a: vec4<f32>, b: vec4<f32>) -> vec4<f32> {
    return select(a, b, dot(b.xy, b.xy) > dot(a.xy, a.xy));
}

@compute @workgroup_size(64)
fn shake_limbs(@builtin(global_invocation_id) id: vec3<u32>) {
    let tree = id.x / MAX_LIMBS;
    let limb = id.x % MAX_LIMBS;
    let window = touch_window;
    if tree >= u32(shake.frame.y) || window.centre.w <= 0.0 {
        return;
    }
    let pool = trees[tree];
    let instance = instances[pool];
    let batch = shed.batches[instance.origin];
    if batch.x == SHED_NONE || limb >= batch.z {
        return;
    }
    let slot = shed_slot(batch, pool, limb);
    if states[slot].x >= 0.0 {
        return;
    }
    let l = limbs[forms[instance.form].x + limb];
    if l.root.w <= 0.0 {
        return;
    }

    let r = transforms[pool];
    let start = world_point(r, l.snow.xyz);
    let along = world_vector(r, l.along.xyz);
    let root = world_point(r, l.root.xyz);
    let tip = world_point(r, l.root.xyz + l.axis.xyz * l.root.w);
    // Across the bough, level: its sprays spread sideways from it.
    let axis = world_vector(r, l.axis.xyz);
    var side = cross(axis, vec3(0.0, 1.0, 0.0));
    side = select(vec3(1.0, 0.0, 0.0), normalize(side), dot(side, side) > 1e-6) * l.along.w * r[1].y;
    var pushed = vec4(0.0);
    for (var k = 0u; k < SAMPLES; k += 1u) {
        let t = (f32(k) + 0.5) / f32(SAMPLES);
        let on = start + along * t;
        for (var j = 0u; j < ACROSS; j += 1u) {
            let across = f32(j) / f32(ACROSS - 1u) * 2.0 - 1.0;
            pushed = further(pushed, spring_at(window, on + side * across));
        }
        pushed = further(pushed, spring_at(window, mix(root, tip, t)));
    }
    if dot(pushed.xy, pushed.xy) < SHED_PUSH * SHED_PUSH {
        return;
    }

    let now = shake.frame.x;
    let velocity = vec3(pushed.z, 0.0, pushed.w);
    states[slot] = vec4(now, velocity);
    let scale = r[1].y;
    let fall = atomicAdd(&next_fall, 1u) % u32(shake.frame.z);
    // The seed is a whole number below 2^24, which f32 holds exactly.
    let seed = f32((pool * MAX_LIMBS + limb) & 0xffffffu);
    falls[fall] = SnowFall(vec4(start, now), vec4(along, l.snow.w * scale), vec4(velocity, r[1].w),
        vec4(l.axis.w, seed, 0.0, 0.0));
}
