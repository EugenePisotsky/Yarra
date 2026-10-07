//! Run the production deformation functions on a native GPU, including previous poses.
use super::*;
use bevy::render::{
    render_resource::{
        BindGroupDescriptor, BindGroupEntry, BufferDescriptor, BufferInitDescriptor, BufferUsages,
        CommandEncoderDescriptor, ComputePassDescriptor, MapMode, PollType,
        RawComputePipelineDescriptor, ShaderModuleDescriptor, ShaderSource,
    },
    renderer::initialize_renderer,
    settings::{Backends, WgpuSettings},
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Case {
    frames: WindFrames,
    model: [[f32; 4]; 4],
    previous_model: [[f32; 4]; 4],
    weights: [f32; 4],
}

#[test]
#[ignore = "requires native GPU access; verifies tree deformation and temporal inputs"]
fn deformation_history_handles_animation_pause_disable_and_rebase() {
    let mut wind = VegetationWind::default();
    let pose =
        |wind: &VegetationWind, origin| WindPose::sample(wind, TreeWindResponse::default(), origin);
    wind.set_phase_seconds(6.8);
    let previous = pose(&wind, [0.; 2]);
    wind.set_phase_seconds(7.);
    let current = pose(&wind, [0.; 2]);
    let model = Mat4::from_translation(Vec3::new(6., 0., -8.)).to_cols_array_2d();
    let animated = Case {
        frames: WindFrames { current, previous },
        model,
        previous_model: model,
        weights: [0.4, 1., 0., 0.],
    };
    let paused = Case {
        frames: WindFrames {
            current,
            previous: current,
        },
        ..animated
    };
    let fixed_bark = Case {
        weights: [0.; 4],
        ..animated
    };
    wind.enabled = false;
    let disabled = Case {
        frames: WindFrames {
            current: pose(&wind, [0.; 2]),
            previous: current,
        },
        ..animated
    };
    wind.enabled = true;
    let rebased = Case {
        frames: WindFrames {
            current: pose(&wind, [256., 0.]),
            previous: current,
        },
        model: Mat4::from_translation(Vec3::new(-250., 0., -8.)).to_cols_array_2d(),
        ..animated
    };
    let cases = [animated, paused, fixed_bark, disabled, rebased];

    let source = include_str!("../../../../assets/shaders/tree_wind.wgsl");
    // Only the Bevy matrix helper needs a stand-in; evaluate the actual field/deformation
    // implementation, not a second copy of its math. Full vertex variants run in game QA.
    let body = format!(
        "{}\n{}",
        &source[source.find("struct WindPose").unwrap()..source.find("fn facing_camera").unwrap()],
        &source[source.find("fn displaced_position").unwrap()
            ..source.find("// Structural wind helpers.").unwrap()]
    );
    let body = body
        .replace(
            "@group(#{MATERIAL_BIND_GROUP}) @binding(100)\nvar<storage, read> wind: WindFrames;",
            "",
        )
        .replace(
            "mesh_functions::mesh_position_local_to_world",
            "position_local_to_world",
        );
    let shader = format!(
        "{body}\n{}",
        r#"
fn position_local_to_world(model: mat4x4<f32>, p: vec4<f32>) -> vec4<f32> { return model * p; }
struct Case { frames: WindFrames, model: mat4x4<f32>, previous_model: mat4x4<f32>, weights: vec4<f32> }
struct Result { current: vec4<f32>, previous: vec4<f32>, full_facing: vec4<f32>, keep_tilt: vec4<f32> }
@group(0) @binding(0) var<storage, read> inputs: array<Case>;
@group(0) @binding(1) var<storage, read_write> results: array<Result>;
@compute @workgroup_size(1)
fn evaluate(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = inputs[id.x];
    let p = vec4(2.0, 10.0, 3.0, 1.0);
    results[id.x].current = displaced_position(c.model * p, c.model, c.weights.xy, c.frames.current);
    results[id.x].previous = displaced_position(c.previous_model * p, c.previous_model, c.weights.xy, c.frames.previous);
    let directions = array<vec3<f32>, 5>(vec3(1., .4, -2.), vec3(0., 1., 0.), vec3(0., -1., 0.), vec3(-2., 4., 3.), vec3(0., 0., 1.));
    let rest = normalize(vec3(.3, .6, .7));
    results[id.x].full_facing = vec4(turn_camera(rest, rest, directions[id.x], 1.), 0.);
    results[id.x].keep_tilt = vec4(turn_camera(rest, rest, directions[id.x], 0.), 0.);
}
"#
    );
    let results = run_shader(&cases, &shader);
    let delta = |i: usize, offset: Vec3| {
        let r = results[i];
        Vec3::new(r[0], r[1], r[2]) + offset - Vec3::new(r[4], r[5], r[6])
    };
    assert!(
        delta(0, Vec3::ZERO).length() > 0.001,
        "moving leaves must supply nonzero deformation motion"
    );
    assert!(
        delta(1, Vec3::ZERO).length() < 1e-6,
        "paused transport must be motionless"
    );
    assert!(
        delta(2, Vec3::ZERO).length() < 1e-6,
        "zero-weight bark must be motionless"
    );
    assert!(
        delta(3, Vec3::ZERO).length() > 0.001,
        "disabling wind must retain the previous deformed pose"
    );
    assert_eq!(&results[3][..3], &[8., 10., -5.]);
    assert!(
        delta(4, Vec3::X * 256.).length() < 0.0001,
        "rebasing must not change the deformation"
    );
    assert!(results.iter().flatten().all(|v| v.is_finite()));
    let directions = [
        Vec3::new(1., 0.4, -2.),
        Vec3::Y,
        -Vec3::Y,
        Vec3::new(-2., 4., 3.),
        Vec3::Z,
    ];
    let rest = Vec3::new(0.3, 0.6, 0.7).normalize();
    for (result, direction) in results.iter().zip(directions) {
        assert!(
            (Vec3::from_slice(&result[8..11]) - direction.normalize()).length() < 0.0001,
            "full-facing normal must face the camera, including directly overhead"
        );
        assert!(
            (result[13] - rest.y).abs() < 0.0001,
            "yaw-only mode must preserve tilt"
        );
    }
}

fn run_shader(cases: &[Case], shader: &str) -> Vec<[f32; 16]> {
    let resources = bevy::tasks::block_on(initialize_renderer(
        Backends::PRIMARY,
        None,
        &WgpuSettings::default(),
    ));
    let device = resources.0.wgpu_device();
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("tree temporal deformation"),
        source: ShaderSource::Wgsl(shader.into()),
    });
    let pipeline = device.create_compute_pipeline(&RawComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &module,
        entry_point: Some("evaluate"),
        compilation_options: default(),
        cache: None,
    });
    let input = resources.0.create_buffer_with_data(&BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(cases),
        usage: BufferUsages::STORAGE,
    });
    let bytes = cases.len() as u64 * 64;
    let output = resources.0.create_buffer(&BufferDescriptor {
        label: None,
        size: bytes,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = resources.0.create_buffer(&BufferDescriptor {
        label: None,
        size: bytes,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(cases.len() as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
    resources.1.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(MapMode::Read, move |result| sender.send(result).unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    receiver.recv().unwrap().unwrap();
    let data = readback.slice(..).get_mapped_range();
    bytemuck::cast_slice(&data).to_vec()
}

#[test]
#[ignore = "requires native GPU; evaluates production hierarchy and pinned card roots"]
fn structural_cards_follow_wood_and_keep_roots_and_history() {
    let source = include_str!("../../../../assets/shaders/tree_wind.wgsl");
    let source = &source
        [source.find("struct WindPose").unwrap()..source.find("@vertex\nfn vertex").unwrap()];
    // Enable the actual production card path without Bevy's vertex IO/imports.
    let mut stack = vec![true];
    let mut body = String::new();
    for line in source.lines() {
        if let Some(key) = line.strip_prefix("#ifdef ") {
            stack.push(
                *stack.last().unwrap()
                    && matches!(
                        key,
                        "TREE_HIERARCHY"
                            | "TREE_BRANCH_CARDS"
                            | "TREE_CARD_FACING"
                            | "VERTEX_UVS_B"
                    ),
            );
        } else if line == "#else" {
            let i = stack.len() - 1;
            stack[i] = stack[i - 1] && !stack[i];
        } else if line == "#endif" {
            stack.pop();
        } else if *stack.last().unwrap() {
            body.push_str(line);
            body.push('\n');
        }
    }
    body = body
        .replace(
            "@group(#{MATERIAL_BIND_GROUP}) @binding(100)\nvar<storage, read> wind: WindFrames;",
            "",
        )
        .replace(
            "@group(#{MATERIAL_BIND_GROUP}) @binding(105) var<uniform> tree_profile: vec4<f32>;",
            "const tree_profile = vec4(1., 3., 1., 1.);",
        )
        .replace("view.lod_view_world_position", "vec3(0., 4., 20.)");
    let shader = format!(
        "{body}\n{}",
        r#"
struct Vertex { position: vec3<f32>, wind_pivot: vec4<f32>, wind_axis: vec4<f32>, card_pivot: vec3<f32>, card_axis: vec3<f32>, card_normal: vec3<f32>, card_facing: vec2<f32>, uv_b: vec2<f32> }
struct Case { frames: WindFrames, model: mat4x4<f32>, previous_model: mat4x4<f32>, weights: vec4<f32> }
struct Result { card: vec4<f32>, previous: vec4<f32>, wood: vec4<f32>, root: vec4<f32> }
@group(0) @binding(0) var<storage, read> inputs: array<Case>;
@group(0) @binding(1) var<storage, read_write> results: array<Result>;
@compute @workgroup_size(1)
fn evaluate(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = inputs[id.x];
    var card: Vertex;
    card.position = vec3(3., 8.5, 0.);
    card.wind_pivot = vec4(0., 8., 0., 14.);
    card.wind_axis = vec4(normalize(vec3(1., .2, 0.)), 1.);
    card.card_pivot = card.position;
    card.card_axis = vec3(0., 1., 0.);
    card.card_normal = vec3(0., 0., 1.);
    card.card_facing = vec2(1., 1.);
    card.uv_b = vec2(1.);
    results[id.x].card = animated_position(card, c.model, c.frames.current);
    results[id.x].previous = animated_position(card, c.previous_model, c.frames.previous);
    let limb = structural_frame(card.wind_pivot.xyz, card.wind_axis, 14., c.model, c.frames.current, tree_profile);
    results[id.x].wood = vec4(sway_point((c.model * vec4(card.position, 1.)).xyz, limb), 1.);
    let root = structural_frame(vec3(0.), vec4(0.), 14., c.model, c.frames.current, tree_profile);
    results[id.x].root = vec4(sway_point(c.model[3].xyz, root), 1.);
}
"#
    );
    let mut wind = VegetationWind::default();
    let sample = |w: &VegetationWind, o| WindPose::sample(w, TreeWindResponse::default(), o);
    wind.set_phase_seconds(7.);
    let current = sample(&wind, [0.; 2]);
    wind.set_phase_seconds(6.8);
    let previous = sample(&wind, [0.; 2]);
    let model = Mat4::from_scale_rotation_translation(
        Vec3::splat(1.3),
        Quat::from_rotation_y(0.7),
        Vec3::new(6., 0., -8.),
    );
    let animated = Case {
        frames: WindFrames { current, previous },
        model: model.to_cols_array_2d(),
        previous_model: model.to_cols_array_2d(),
        weights: [0.; 4],
    };
    let paused = Case {
        frames: WindFrames {
            current,
            previous: current,
        },
        ..animated
    };
    wind.enabled = false;
    let calm = Case {
        frames: WindFrames {
            current: sample(&wind, [0.; 2]),
            previous: current,
        },
        ..animated
    };
    wind.enabled = true;
    wind.set_phase_seconds(7.);
    let rebased = Case {
        frames: WindFrames {
            current: sample(&wind, [256., 0.]),
            previous: current,
        },
        model: (Mat4::from_translation(-Vec3::X * 256.) * model).to_cols_array_2d(),
        ..animated
    };
    wind.strength = 2.;
    let mut strong = sample(&wind, [0.; 2]);
    strong.hierarchy = [3., 3., 3., 0.4];
    let strong = Case {
        frames: WindFrames {
            current: strong,
            previous: current,
        },
        ..animated
    };
    let cases = [animated, paused, calm, rebased, strong];
    let results = run_shader(&cases, &shader);
    for (c, r) in cases.iter().zip(&results) {
        assert!(r.iter().all(|v| v.is_finite()));
        assert!(
            (Vec3::from_slice(&r[..3]) - Vec3::from_slice(&r[8..11])).length() < 0.0001,
            "facing/flutter must leave the card attachment on its wood"
        );
        assert!(
            (Vec3::from_slice(&r[12..15]) - Vec3::from_slice(&c.model[3][..3])).length() < 0.0001,
            "trunk root must stay planted"
        );
    }
    let delta = |i: usize| Vec3::from_slice(&results[i][..3]) - Vec3::from_slice(&results[i][4..7]);
    assert!(
        delta(0).length() > 0.001,
        "structural movement must generate temporal motion"
    );
    assert!(
        delta(1).length() < 0.0001,
        "paused structural pose must stop exactly"
    );
    assert!(
        (Vec3::from_slice(&results[2][..3]) - model.transform_point3(Vec3::new(3., 8.5, 0.)))
            .length()
            < 0.0001,
        "calm preserves authored structure"
    );
    assert!(
        (delta(3) + Vec3::X * 256.).length() < 0.0002,
        "world rebasing must preserve structural pose"
    );
}

#[test]
#[ignore = "requires native GPU; compares the impostor's trunk lean with the mesh LODs'"]
fn impostors_lean_as_the_mesh_trunk_does() {
    let source = include_str!("../../../../assets/shaders/tree_wind.wgsl");
    let impostor = include_str!("../../../../assets/shaders/tree_impostor.wgsl");
    let body = format!(
        "{}\n{}\n{}",
        &source[source.find("struct WindPose").unwrap()
            ..source.find("// Use the main camera").unwrap()],
        &source[source.find("// Structural wind helpers.").unwrap()
            ..source.find("// End structural helpers.").unwrap()],
        &impostor[impostor.find("// Impostor wind: begin").unwrap()
            ..impostor.find("// Impostor wind: end").unwrap()],
    )
    .replace(
        "@group(#{MATERIAL_BIND_GROUP}) @binding(100)\nvar<storage, read> wind: WindFrames;",
        "",
    );
    let shader = format!(
        "{body}\n{}",
        r#"
struct Case { frames: WindFrames, model: mat4x4<f32>, previous_model: mat4x4<f32>, weights: vec4<f32> }
@group(0) @binding(0) var<storage, read> inputs: array<Case>;
@group(0) @binding(1) var<storage, read_write> results: array<array<vec4<f32>, 4>>;
@compute @workgroup_size(1)
fn evaluate(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = inputs[id.x];
    let profile = vec4(1., 3., 1., 1.);
    let scale = length(c.model[1].xyz);
    // Along the stem, beside it in the crown, above its top and below the root.
    let points = array<vec3<f32>, 4>(vec3(0., 7., 0.), vec3(1.5, 10., -.8), vec3(.3, 16., .2), vec3(.5, -.5, .2));
    for (var i = 0; i < 4; i += 1) {
        let rest = (c.model * vec4(points[i], 1.)).xyz;
        let mesh = sway_point(rest, structural_frame(points[i], vec4(0.), 14., c.model, c.frames.current, profile));
        let quad = sway_point_of_trunk(rest, c.model[3].xyz, 14. * scale, profile, c.frames.current, 1.);
        results[id.x][i] = vec4(mesh - quad, length(mesh - rest));
    }
}
"#
    );
    let mut wind = VegetationWind::default();
    let sample = |w: &VegetationWind, o| WindPose::sample(w, TreeWindResponse::default(), o);
    wind.set_phase_seconds(7.);
    let current = sample(&wind, [0.; 2]);
    let model = Mat4::from_scale_rotation_translation(
        Vec3::splat(1.3),
        Quat::from_rotation_y(0.7),
        Vec3::new(6., 0., -8.),
    );
    let animated = Case {
        frames: WindFrames {
            current,
            previous: current,
        },
        model: model.to_cols_array_2d(),
        previous_model: model.to_cols_array_2d(),
        weights: [0.; 4],
    };
    let rebased = Case {
        frames: WindFrames {
            current: sample(&wind, [256., 0.]),
            previous: current,
        },
        model: (Mat4::from_translation(-Vec3::X * 256.) * model).to_cols_array_2d(),
        ..animated
    };
    wind.strength = 2.;
    let mut strong = sample(&wind, [0.; 2]);
    strong.hierarchy = [3., 3., 3., 0.4];
    let strong = Case {
        frames: WindFrames {
            current: strong,
            previous: current,
        },
        ..animated
    };
    let cases = [animated, rebased, strong];
    let results = run_shader(&cases, &shader);
    for r in &results {
        assert!(r.iter().all(|v| v.is_finite()));
        for point in r.chunks(4) {
            assert!(
                Vec3::from_slice(&point[..3]).length() < 0.001,
                "an impostor must lean as its mesh trunk does: {point:?}"
            );
        }
        assert!(r[11] > 0.05, "the treetop must move in the wind");
        assert!(r[15] < 0.001, "below the root nothing moves");
    }
}
