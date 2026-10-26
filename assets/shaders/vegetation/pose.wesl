// Blade poses: each emitted blade trampled and bent by the wind, this frame and last, with the
// rest of what drawing needs of it (blade.wesl `DrawnBlade`), once per blade for drawing
// (draw/geometry.wesl) instead of once in each of its vertices. A twin root
// poses both blades: the instance's own at twice its slot, its sibling next to it. Placement's
// finalize sizes the dispatch: a workgroup row per detail bin, as wide as the fuller bin's
// batches of BLADES_PER_BATCH. The last batch's slots past its bin's blades get zero poses,
// which their vertices collapse onto one point.
import package::shaders::vegetation::types::{Instance, GrassType, Camera, TemporalGrass, BladePose};
import package::shaders::vegetation::blade::{
    BladeInstance, decode_instance, twin_blade, rest_blade, trample_blade, rest_curve, wind_curve,
    drawn_blade, pack_pose,
};

struct DrawIndexedIndirectArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: u32,
    first_instance: u32,
}

// placement/bindings.wesl `Draws`.
struct Draws {
    args: array<DrawIndexedIndirectArgs, 2>,
    blades: array<u32, 2>,
}

@group(0) @binding(0) var<storage, read> instances: array<Instance>;
@group(0) @binding(1) var<storage, read> types: array<GrassType>;
@group(0) @binding(2) var<uniform> camera: Camera;
@group(0) @binding(3) var<uniform> temporal_grass: TemporalGrass;
// The trample map (trample.wesl) this frame and last.
@group(0) @binding(4) var trample_now: texture_2d<f32>;
@group(0) @binding(5) var trample_before: texture_2d<f32>;
@group(0) @binding(6) var trample_sampler: sampler;
@group(0) @binding(7) var<storage, read> draws: Draws;
@group(0) @binding(8) var<storage, read_write> poses: array<BladePose>;

const WORKGROUP_SIZE: u32 = constants::WORKGROUP_SIZE;
const BLADES_PER_BATCH: u32 = constants::BLADES_PER_BATCH;

// How characters push the grass rooted at `root`, from the trample map of `frame`: the
// way its tips are pushed, length 1 lying flat. The map wraps, so roots near or beyond its edge
// read nothing.
fn trampled(root: vec3<f32>, frame: Camera, previous: bool) -> vec2<f32> {
    if (frame.trample.w <= 0.0) {
        return vec2<f32>(0.0);
    }
    let canonical = root.xz + frame.render_origin.xy;
    let away = abs(canonical - frame.trample.xy);
    let edge = max(away.x, away.y);
    let half_side = frame.trample.z * 0.5;
    let inside = 1.0 - smoothstep(half_side - 4.0, half_side - 1.0, edge);
    if (inside <= 0.0) {
        return vec2<f32>(0.0);
    }
    let uv = canonical / frame.trample.z;
    var value = textureSampleLevel(trample_now, trample_sampler, uv, 0.0);
    if previous {
        value = textureSampleLevel(trample_before, trample_sampler, uv, 0.0);
    }
    return value.xy * inside;
}

fn posed(b: BladeInstance, grass: GrassType) -> BladePose {
    let rest = rest_blade(b, grass);
    let now = trample_blade(rest, trampled(b.root, camera, false));
    let current = wind_curve(rest_curve(now), now, grass, camera);
    // Last frame posed the same blade, rooted where last frame's render origin put it.
    let last_camera = temporal_grass.previous;
    let shift = camera.render_origin.xy - last_camera.render_origin.xy;
    var earlier = b;
    earlier.root += vec3(shift.x, 0.0, shift.y);
    let then = trample_blade(rest_blade(earlier, grass), trampled(earlier.root, last_camera, true));
    let before = wind_curve(rest_curve(then), then, grass, last_camera);
    return pack_pose(current, before, now.side, now.pressed, drawn_blade(b, rest));
}

@compute @workgroup_size(WORKGROUP_SIZE, 1, 1)
fn pose(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let args = draws.args[invocation.y];
    if (invocation.x >= args.instance_count * BLADES_PER_BATCH) {
        return;
    }
    let slot = args.first_instance * BLADES_PER_BATCH + invocation.x;
    if (invocation.x >= draws.blades[invocation.y]) {
        poses[2u * slot] = BladePose();
        poses[2u * slot + 1u] = BladePose();
        return;
    }
    let b = decode_instance(instances[slot]);
    let grass = types[b.type_index];
    if (grass.form.x > 0.5) {
        poses[2u * slot] = posed(twin_blade(b, false), grass);
        poses[2u * slot + 1u] = posed(twin_blade(b, true), grass);
    } else {
        poses[2u * slot] = posed(b, grass);
    }
}
