//! The touch field's pieces on a native GPU: a body's push, a voxel's spring and a pushed limb's
//! turn, run from the production shaders (`shaders/tree/touch.wesl`, `shaders/tree/wind.wesl`).
use crate::trees::wind::gpu_tests::{harness, run_shader};

use bevy::math::Vec3;

#[test]
#[ignore = "requires native GPU access; evaluates the touch field's pushes and springs"]
fn bodies_push_foliage_out_of_them_and_springs_swing_it_back() {
    let shader = harness(
        r#"
import package::shaders::tree::touch::{body_push, touch_spring};
@group(0) @binding(0) var<storage, read> inputs: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> results: array<array<vec4<f32>, 4>>;
@compute @workgroup_size(1)
fn evaluate(@builtin(global_invocation_id) id: vec3<u32>) {
    _ = inputs[id.x];
    let feet = vec4(0.0, 0.0, 0.0, 0.3);
    let still = vec4(0.0, 0.0, 1.8, 0.0);
    if id.x == 0u {
        // Beside the axis inside the body; out of its reach; beside a walking body.
        results[0][0] = body_push(vec3(0.1, 1.0, 0.0), feet, still);
        results[0][1] = body_push(vec3(0.0, 1.0, 0.5), feet, still);
        results[0][2] = body_push(vec3(0.0, 1.0, 0.1), feet, vec4(1.5, 0.0, 1.8, 0.0));
        // Over the body's head.
        results[0][3] = body_push(vec3(0.0, 2.5, 0.0), feet, still);
        return;
    }
    // Held towards 0.3 m for 0.2 s, then let go for 3 s.
    var s = vec4(0.0);
    for (var i = 0; i < 20; i += 1) {
        s = touch_spring(s, vec2(0.3, 0.0), true, 0.01);
    }
    results[1][0] = s;
    var lowest = s.x;
    for (var i = 0; i < 300; i += 1) {
        s = touch_spring(s, vec2(0.3, 0.0), false, 0.01);
        lowest = min(lowest, s.x);
    }
    results[1][1] = vec4(lowest);
    results[1][2] = s;
}
"#,
        "",
    );
    let results = run_shader(&[[0.0_f32; 4]; 2], &shader);
    let push = |case: usize, i: usize| Vec3::from_slice(&results[case][4 * i..4 * i + 3]);
    let reached = |case: usize, i: usize| results[case][4 * i + 3];

    // The body's 0.3 m radius and 0.12 m margin: foliage 0.1 m off its axis moves 0.32 m out.
    assert!((push(0, 0) - Vec3::new(0.32, 0.0, 0.0)).length() < 1e-4);
    assert_eq!(reached(0, 0), 1.0);
    assert_eq!(reached(0, 1), 0.0, "foliage beyond the margin stays");
    assert_eq!(push(0, 1), Vec3::ZERO);
    let walked = push(0, 2);
    assert!(
        walked.z > 0.31 && walked.x > 0.1,
        "a walking body drags along its way: {walked}"
    );
    assert_eq!(reached(0, 3), 0.0, "foliage over the head stays");

    assert!(
        results[1][0] > 0.25,
        "a held spring follows the push in a fifth of a second: {}",
        results[1][0]
    );
    assert!(
        results[1][4] < -0.05,
        "a released branch swings past rest: {}",
        results[1][4]
    );
    assert!(
        results[1][8].abs() < 0.01 && results[1][10].abs() < 0.05,
        "and settles within three seconds: {:?}",
        &results[1][8..12]
    );
}

#[test]
#[ignore = "requires native GPU access; evaluates a pushed limb's turn in the wind hierarchy"]
fn a_pushed_limb_turns_about_its_root_and_the_trunk_stands() {
    let shader = harness(
        r#"
import package::shaders::tree::wind::{TreeVertex, WindPose, animated_position};
const PROFILE = vec4(1., 3., 1., 1.);
const VIEWER = vec3(0., 4., 20.);
@group(0) @binding(0) var<storage, read> inputs: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> results: array<array<vec4<f32>, 4>>;
@compute @workgroup_size(1)
fn evaluate(@builtin(global_invocation_id) id: vec3<u32>) {
    _ = inputs[id.x];
    var calm: WindPose;
    let model = mat4x4(vec4(1., 0., 0., 0.), vec4(0., 1., 0., 0.), vec4(0., 0., 1., 0.), vec4(0., 0., 0., 1.));
    var limb: TreeVertex;
    limb.position = vec3(1.0, 1.0, 0.0);
    limb.wind_pivot = vec4(0.0, 0.5, 0.0, 3.0);
    limb.wind_axis = vec4(normalize(vec3(1.0, 0.5, 0.0)), 1.0);
    var trunk = limb;
    trunk.wind_axis.w = 0.0;
    results[0][0] = animated_position(limb, model, calm, PROFILE, VIEWER, vec3(0.0, 0.0, 0.3));
    results[0][1] = animated_position(limb, model, calm, PROFILE, VIEWER, vec3(0.0, 0.0, 2.0));
    results[0][2] = animated_position(trunk, model, calm, PROFILE, VIEWER, vec3(0.0, 0.0, 0.3));
    results[0][3] = animated_position(limb, model, calm, PROFILE, VIEWER, vec3(0.0));
}
"#,
        "TREE_HIERARCHY",
    );
    let results = run_shader(&[[0.0_f32; 4]], &shader);
    let at = |i: usize| Vec3::from_slice(&results[0][4 * i..4 * i + 3]);
    let root = Vec3::new(0.0, 0.5, 0.0);
    let rest = Vec3::new(1.0, 1.0, 0.0);
    let turned = |angle: f32| {
        let (s, c) = angle.sin_cos();
        let r = rest - root;
        root + Vec3::new(r.x * c - r.y * s, r.x * s + r.y * c, r.z)
    };

    assert!(
        (at(0) - turned(0.3)).length() < 1e-4,
        "the limb turns about its root: {}",
        at(0)
    );
    assert!(
        (at(1) - turned(0.6)).length() < 1e-4,
        "no further than TOUCH_TURN: {}",
        at(1)
    );
    assert!((at(2) - rest).length() < 1e-6, "the trunk stands");
    assert!((at(3) - rest).length() < 1e-6, "an untouched limb rests");
}
