//! Compare every accepted candidate's data on the GPU, independent of atomic append order.
use super::{
    gpu_types::{CameraGpu, DebugConfigGpu, GPU_TELEMETRY_SIZE, SurfaceSampleGpu},
    packing::pack_scene,
};
use bevy::{
    prelude::*,
    render::{
        render_resource::{
            BindGroupDescriptor, BindGroupEntry, BufferDescriptor, BufferInitDescriptor,
            BufferUsages, CommandEncoderDescriptor, ComputePassDescriptor, MapMode, PollType,
            RawComputePipelineDescriptor, ShaderModuleDescriptor, ShaderSource,
        },
        renderer::initialize_renderer,
        settings::{Backends, WgpuSettings},
    },
};
use bytemuck::Zeroable;
use vegetation::decode_octahedral_normal;

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

#[test]
#[ignore = "requires a native GPU; run when changing surface interpolation"]
fn surface_sampling_matches_cpu_on_a_nonplanar_quad() {
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
        r#"
@group(0) @binding(14) var<storage, read_write> surface_comparison: array<vec4<f32>>;
@compute @workgroup_size(64)
fn compare_surface(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= 169u) { return; }
    var item: WorkItem;
    item.page = vec4<f32>(32.0, -16.0, 8.0, 0.0);
    item.surface = vec4<u32>(0u, 2u, 0u, 0u);
    let uv = vec2<f32>(f32(id.x % 13u), f32(id.x / 13u)) / 8.0 - vec2<f32>(0.25);
    let result = sample_surface(item, item.page.xy + uv * item.page.z);
    surface_comparison[id.x * 2u] = vec4<f32>(result.height, result.normal);
    surface_comparison[id.x * 2u + 1u] = vec4<f32>(result.validity, 0.0, 0.0, 0.0);
}
"#
    );
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("terrain triangle interpolation comparison"),
        source: ShaderSource::Wgsl(shader.into()),
    });
    let pipeline = device.create_compute_pipeline(&RawComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &module,
        entry_point: Some("compare_surface"),
        compilation_options: default(),
        cache: None,
    });
    let surface = vegetation::VegetationSurfaceField {
        resolution: 2,
        heights: vec![0.0, 10.0, 20.0, 0.0],
        normals_oct: [
            [0.0, 1.0, 0.0],
            [0.4, 1.0, 0.0],
            [0.0, 1.0, -0.5],
            [0.2, 1.0, 0.2],
        ]
        .map(vegetation::encode_octahedral_normal)
        .to_vec(),
        validity: vec![255, 128, 0, 255],
    };
    let samples: Vec<_> = (0..4)
        .map(|i| {
            let n = decode_octahedral_normal(surface.normals_oct[i]);
            SurfaceSampleGpu {
                height_validity: [
                    surface.heights[i],
                    f32::from(surface.validity[i]) / 255.0,
                    0.0,
                    0.0,
                ],
                normal: [n[0], n[1], n[2], 0.0],
            }
        })
        .collect();
    let input = resources.0.create_buffer_with_data(&BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&samples),
        usage: BufferUsages::STORAGE,
    });
    let bytes = 169 * 2 * 16;
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
                binding: 3,
                resource: input.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 14,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(3, 1, 1);
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
        let values: &[f32] = bytemuck::cast_slice(&data);
        for i in 0..169 {
            let p = [
                32.0 + ((i % 13) as f32 / 8.0 - 0.25) * 8.0,
                -16.0 + ((i / 13) as f32 / 8.0 - 0.25) * 8.0,
            ];
            let cpu = surface.sample([32.0, -16.0], 8.0, p);
            let expected = [
                cpu.height,
                cpu.normal[0],
                cpu.normal[1],
                cpu.normal[2],
                cpu.validity,
            ];
            for (j, value) in expected.into_iter().enumerate() {
                assert!(
                    (values[i * 8 + j] - value).abs() < 1e-5,
                    "sample {i}, component {j}: GPU {} CPU {value}",
                    values[i * 8 + j]
                );
            }
        }
        // The nonplanar diagonal must be flat, unlike bilinear interpolation.
        assert_eq!(values[(6 * 13 + 6) * 8], 0.0);
    }
    readback.unmap();
}

/// Exercises the real scheduler against a retained allocation full of valid, visible old
/// records. The old arrayLength guard schedules all 64 records after the live set shrinks.
#[test]
#[ignore = "requires a native GPU; run explicitly when changing scheduler bounds"]
fn gpu_scheduler_ignores_retired_records_and_keeps_dispatch_without_telemetry() {
    use bevy::render::{
        render_resource::{
            BindGroupDescriptor, BindGroupEntry, CommandEncoderDescriptor, MapMode, PollType,
            RawComputePipelineDescriptor, ShaderModuleDescriptor, ShaderSource,
        },
        renderer::initialize_renderer,
        settings::{Backends, WgpuSettings},
    };
    let resources = bevy::tasks::block_on(initialize_renderer(
        Backends::PRIMARY,
        None,
        &WgpuSettings::default(),
    ));
    let device = resources.0.wgpu_device();
    let queue = &resources.1;
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("scheduler regression"),
        source: ShaderSource::Wgsl(
            include_str!("../../../../assets/shaders/vegetation_schedule_compute.wgsl").into(),
        ),
    });
    let pipeline = device.create_compute_pipeline(&RawComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("schedule"),
        compilation_options: default(),
        cache: None,
    });
    let layout = pipeline.get_bind_group_layout(0);
    let mut item = pack_scene(&vegetation::fixtures::reference_scene()).work_items[0];
    item.page = [-2.0, -2.0, 4.0, 1.0];
    item.layout[1] = 64;
    let records = [item; 64];
    let work = resources.0.create_buffer_with_data(&BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&records),
        usage: BufferUsages::STORAGE,
    });
    let camera = resources.0.create_buffer_with_data(&BufferInitDescriptor {
        label: None,
        contents: bytemuck::bytes_of(&CameraGpu::zeroed()),
        usage: BufferUsages::UNIFORM,
    });
    for (live_count, counters) in [(64u32, 1u32), (5, 1), (5, 0), (0, 1)] {
        let config = resources.0.create_buffer_with_data(&BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&DebugConfigGpu {
                values: [0, 1, 0, 0],
                workload: [live_count, counters, 0, 0],
            }),
            usage: BufferUsages::UNIFORM,
        });
        let storage = |size| {
            resources.0.create_buffer(&BufferDescriptor {
                label: None,
                size,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let visible = storage(64 * 4);
        let dispatch = storage(12);
        let telemetry = storage(GPU_TELEMETRY_SIZE);
        let buffers = [&work, &visible, &dispatch, &camera, &telemetry, &config];
        let entries = buffers
            .iter()
            .enumerate()
            .map(|(binding, buffer)| BindGroupEntry {
                binding: binding as u32,
                resource: buffer.as_entire_binding(),
            })
            .collect::<Vec<_>>();
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &entries,
        });
        let readback = resources.0.create_buffer(&BufferDescriptor {
            label: None,
            size: 12 + GPU_TELEMETRY_SIZE,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&dispatch, 0, &readback, 0, 12);
        encoder.copy_buffer_to_buffer(&telemetry, 0, &readback, 12, GPU_TELEMETRY_SIZE);
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
            assert_eq!(
                words[1], live_count,
                "retired work must not enter the indirect dispatch"
            );
            assert_eq!(
                words[3],
                live_count * counters,
                "diagnostic atomics must be optional"
            );
            assert_eq!(words[0], u32::from(live_count > 0));
        }
        readback.unmap();
    }
}
