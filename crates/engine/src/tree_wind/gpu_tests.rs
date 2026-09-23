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
    let body = &source[source.find("struct WindPose").unwrap()..source.find("@vertex").unwrap()];
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
struct Result { current: vec4<f32>, previous: vec4<f32> }
@group(0) @binding(0) var<storage, read> inputs: array<Case>;
@group(0) @binding(1) var<storage, read_write> results: array<Result>;
@compute @workgroup_size(1)
fn evaluate(@builtin(global_invocation_id) id: vec3<u32>) {
    let c = inputs[id.x];
    let p = vec3(2.0, 10.0, 3.0);
    results[id.x].current = displaced_position(p, c.model, c.weights.xy, c.frames.current);
    results[id.x].previous = displaced_position(p, c.previous_model, c.weights.xy, c.frames.previous);
}
"#
    );
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
        contents: bytemuck::cast_slice(&cases),
        usage: BufferUsages::STORAGE,
    });
    let bytes = cases.len() as u64 * 32;
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
    let results: &[[f32; 8]] = bytemuck::cast_slice(&data);
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
}
