//! Exercise the actual preparation shader on both sides of the topology boundary.
use bevy::{
    prelude::*,
    render::{
        render_resource::{
            BindGroupDescriptor, BindGroupEntry, BufferDescriptor, BufferUsages,
            CommandEncoderDescriptor, ComputePassDescriptor, MapMode, PollType,
            RawComputePipelineDescriptor, ShaderModuleDescriptor, ShaderSource,
        },
        renderer::initialize_renderer,
        settings::{Backends, WgpuSettings},
    },
};

#[test]
#[ignore = "requires a native GPU; run when changing grass LOD morphing"]
fn lod_boundary_preserves_width_and_endpoints() {
    let resources = bevy::tasks::block_on(initialize_renderer(
        Backends::PRIMARY,
        None,
        &WgpuSettings::default(),
    ));
    let device = resources.0.wgpu_device();
    let shader = format!(
        "{}\n{}",
        include_str!("../../../../../assets/shaders/vegetation_blade.wgsl"),
        include_str!("lod_comparison.wgsl")
    );
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("grass LOD boundary comparison"),
        source: ShaderSource::Wgsl(shader.into()),
    });
    let pipeline = device.create_compute_pipeline(&RawComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &module,
        entry_point: Some("compare_lod"),
        compilation_options: default(),
        cache: None,
    });
    const CASES: u64 = 99;
    let bytes = CASES * 16;
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
        entries: &[BindGroupEntry {
            binding: 0,
            resource: output.as_entire_binding(),
        }],
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(CASES.div_ceil(64) as u32, 1, 1);
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
    let results: &[[f32; 4]] = bytemuck::cast_slice(&data);
    let mut partial_fades = 0;
    for (case, &[high_width, low_width, full_width, endpoint_error]) in results.iter().enumerate() {
        assert!(
            full_width.is_finite() && full_width > 0.005,
            "case {case}: empty geometry"
        );
        assert!(
            (high_width - low_width).abs() < 1e-6,
            "case {case}: high width {high_width} does not meet low width {low_width}"
        );
        assert!(
            endpoint_error < 1e-6,
            "case {case}: endpoint jump {endpoint_error}"
        );
        partial_fades += usize::from(low_width > 0.00001 && low_width < full_width * 0.9);
    }
    assert!(
        partial_fades > 0,
        "must exercise the partially retained density band"
    );
    eprintln!(
        "99 GPU LOD boundary cases: continuous widths/endpoints, including density fade band"
    );
}
