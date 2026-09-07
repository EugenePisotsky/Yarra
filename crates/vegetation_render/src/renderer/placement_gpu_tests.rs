//! Compare every accepted candidate's data on the GPU, independent of atomic append order.
use super::*;
use bevy::render::{
    render_resource::{
        BindGroupDescriptor, BindGroupEntry, CommandEncoderDescriptor, MapMode, PollType,
        RawComputePipelineDescriptor, ShaderModuleDescriptor, ShaderSource,
    },
    renderer::initialize_renderer,
    settings::{Backends, WgpuSettings},
};

#[test]
#[ignore = "requires a native GPU; run when changing placement rejection"]
fn early_rejection_preserves_accepted_candidates_and_diagnostics() {
    let resources = bevy::tasks::block_on(initialize_renderer(
        Backends::PRIMARY,
        None,
        &WgpuSettings::default(),
    ));
    let device = resources.0.wgpu_device();
    let queue = &resources.1;
    let shader = format!(
        "{}\n{}",
        include_str!("../../../../assets/shaders/vegetation_debug_compute.wgsl"),
        include_str!("placement_comparison.wgsl")
    );
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("placement rejection comparison"),
        source: ShaderSource::Wgsl(shader.into()),
    });
    let pipeline = device.create_compute_pipeline(&RawComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &module,
        entry_point: Some("compare_candidates"),
        compilation_options: default(),
        cache: None,
    });
    let layout = pipeline.get_bind_group_layout(0);
    let mut packed = pack_scene(&vegetation::fixtures::reference_scene());
    // Exercise invalid surfaces and partially grown populations as well as occupancy/frustum
    // rejection. Both implementations read exactly the same modified source data.
    for (index, surface) in packed.surfaces.iter_mut().enumerate() {
        if index % 7 == 0 {
            surface.height_validity[1] = 0.0;
        }
    }
    for (index, item) in packed.work_items.iter_mut().enumerate() {
        if index % 2 == 0 {
            item.growth[3] = 0.55;
        }
    }
    let storage = |contents: &[u8]| {
        resources.0.create_buffer_with_data(&BufferInitDescriptor {
            label: None,
            contents,
            usage: BufferUsages::STORAGE,
        })
    };
    let work = storage(bytemuck::cast_slice(&packed.work_items));
    let choices = storage(bytemuck::cast_slice(&packed.choices));
    let coverage = storage(bytemuck::cast_slice(&packed.coverage));
    let surfaces = storage(bytemuck::cast_slice(&packed.surfaces));
    let slots = packed.maximum_candidate_count as usize * packed.work_items.len();
    let bytes = slots as u64 * 4;
    let output = resources.0.create_buffer(&BufferDescriptor {
        label: None,
        size: bytes,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let readback = resources.0.create_buffer(&BufferDescriptor {
        label: None,
        size: bytes,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut total_accepted = 0;
    let mut total_rejected = 0;
    for (position, target) in [
        (Vec3::new(12.0, 7.0, 20.0), Vec3::new(12.0, 0.0, 8.0)),
        (Vec3::new(-2.876, 1.730, 2.384), Vec3::new(10.0, 0.0, -7.0)),
        (Vec3::new(40.0, 1.730, -20.0), Vec3::new(50.0, 0.0, -20.0)),
    ] {
        let projection =
            Mat4::perspective_infinite_reverse_rh(std::f32::consts::FRAC_PI_3, 2.16, 0.1);
        let transform = Transform::from_translation(position).looking_at(target, Vec3::Y);
        let camera = CameraGpu {
            clip_from_world: (projection * transform.to_matrix().inverse()).to_cols_array(),
            camera_position: position.extend(967.0).to_array(),
            projection: [projection.y_axis.y * 967.0 * 0.5, 2097.0, 967.0, 1.0],
            wind: [0.92, 0.38, 0.82, 3.5],
            ..CameraGpu::zeroed()
        };
        let camera = resources.0.create_buffer_with_data(&BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&camera),
            usage: BufferUsages::UNIFORM,
        });
        for (mode, density) in [(0, 1), (0, 0), (0, 2), (1, 1), (2, 1), (3, 1), (4, 1)] {
            let config = resources.0.create_buffer_with_data(&BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&DebugConfigGpu {
                    values: [mode, density, 0, packed.low_detail_capacities[0]],
                    workload: [
                        packed.work_items.len() as u32,
                        1,
                        0,
                        packed.maximum_candidate_count,
                    ],
                }),
                usage: BufferUsages::UNIFORM,
            });
            let entries = [
                (0, &work),
                (1, &choices),
                (2, &coverage),
                (3, &surfaces),
                (8, &config),
                (9, &camera),
                (14, &output),
            ]
            .map(|(binding, buffer)| BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            });
            let group = device.create_bind_group(&BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &entries,
            });
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
            encoder.clear_buffer(&output, 0, None);
            {
                let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(
                    packed.maximum_candidate_count.div_ceil(64),
                    packed.work_items.len() as u32,
                    1,
                );
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
            queue.submit([encoder.finish()]);
            let (sender, receiver) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(MapMode::Read, move |result| sender.send(result).unwrap());
            device.poll(PollType::wait_indefinitely()).unwrap();
            receiver.recv().unwrap().unwrap();
            {
                let data = readback.slice(..).get_mapped_range();
                let words: &[u32] = bytemuck::cast_slice(&data);
                assert!(
                    words.iter().all(|&word| word < 4),
                    "changed eligibility or accepted data: mode={mode}, density={density}, position={position:?}"
                );
                total_accepted += words.iter().filter(|&&word| word == 3).count();
                total_rejected += words.iter().filter(|&&word| word == 1).count();
            }
            readback.unmap();
        }
    }
    assert!(total_accepted > 100 && total_rejected > 100);
    eprintln!(
        "placement comparisons: {total_accepted} accepted, {total_rejected} rejected, zero mismatches across 21 camera/mode/density cases"
    );
}
