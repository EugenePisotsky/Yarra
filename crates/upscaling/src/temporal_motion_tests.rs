//! Analytic moving-image probe: exercises the production MetalFX backend with
//! known colour sampling positions and motion, independently of scene complexity.
use super::*;
use bevy::render::{
    render_resource::*,
    renderer::initialize_renderer,
    settings::{Backends, WgpuSettings},
};

#[test]
#[ignore = "requires native MetalFX; bounded 128-to-256 pixel reconstruction probe"]
fn temporal_motion_sampling_probe() {
    let resources = bevy::tasks::block_on(initialize_renderer(
        Backends::METAL,
        None,
        &WgpuSettings::default(),
    ));
    let (device, queue) = (&resources.0, &resources.1);
    let motion_gain: f32 =
        std::env::var("YARRA_PROBE_MOTION_GAIN").map_or(1.0, |s| s.parse().unwrap());
    let frequency: f32 = std::env::var("YARRA_PROBE_FREQUENCY").map_or(1.0, |s| s.parse().unwrap());
    let diagnostic = motion_gain != 1.0 || frequency != 1.0;
    let input = UVec2::splat(128);
    let output_size = UVec2::splat(256);
    let make = |format, extra| {
        device.create_texture(&TextureDescriptor {
            label: None,
            size: Extent3d {
                width: 256,
                height: 256,
                depth_or_array_layers: 1,
            },
            dimension: TextureDimension::D2,
            format,
            mip_level_count: 1,
            sample_count: 1,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | extra,
            view_formats: &[],
        })
    };
    let color = make(TextureFormat::Rgba16Float, TextureUsages::empty());
    let depth = make(TextureFormat::Depth32Float, TextureUsages::empty());
    let motion = make(TextureFormat::Rg16Float, TextureUsages::empty());
    let output = make(
        TextureFormat::Rgba16Float,
        TextureUsages::STORAGE_BINDING | TextureUsages::COPY_SRC,
    );
    let (cv, dv, mv, ov) = (
        color.create_view(&default()),
        depth.create_view(&default()),
        motion.create_view(&default()),
        output.create_view(&default()),
    );
    let shader = device
        .wgpu_device()
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("analytic temporal sampling probe"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
struct Params { position_jitter: vec4<f32>, delta_size: vec4<f32> }
@group(0) @binding(0) var<uniform> params: Params;
@vertex fn vertex(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
    return vec4(vec2(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0, 0.5, 1.0);
}
struct Output { @location(0) color:vec4<f32>, @location(1) motion:vec2<f32> }
@fragment fn fragment(@builtin(position) p:vec4<f32>) -> Output {
    // Bevy's projection displaces geometry by -jitter, so pixel p samples p+jitter.
    let q = p.xy + params.position_jitter.zw - params.position_jitter.xy;
    let c = 0.5 + 0.35 * sin(q.x * 0.7853981634) * cos(q.y * 0.4833219467);
    var out:Output;
    out.color = vec4(vec3(c), 1.0);
    out.motion = params.delta_size.xy / params.delta_size.zw;
    return out;
}
"#
                .replace(
                    "0.7853981634",
                    &format!("{:.9}", std::f32::consts::FRAC_PI_4 * frequency),
                )
                .replace(
                    "0.4833219467",
                    &format!("{:.9}", std::f32::consts::TAU / 13.0 * frequency),
                )
                .into(),
            ),
        });
    let pipeline = device
        .wgpu_device()
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                buffers: &[],
                compilation_options: default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: default(),
                targets: &[
                    Some(ColorTargetState {
                        format: TextureFormat::Rgba16Float,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    }),
                    Some(ColorTargetState {
                        format: TextureFormat::Rg16Float,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    }),
                ],
            }),
            depth_stencil: Some(DepthStencilState {
                format: TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::Always),
                stencil: default(),
                bias: default(),
            }),
            primitive: default(),
            multisample: default(),
            multiview_mask: None,
            cache: None,
        });
    let readback = device.create_buffer(&BufferDescriptor {
        label: None,
        size: 256 * 256 * 8,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for (name, delta) in [
        ("static", Vec2::ZERO),
        ("horizontal", Vec2::new(1.25, 0.0)),
        ("vertical", Vec2::new(0.0, 1.25)),
    ] {
        let mut errors = Vec::new();
        for sign in [1.0, -1.0] {
            let mut scaler = backends::Temporal::new(device, input, output_size).unwrap();
            for frame in 0..32 {
                let position = delta * frame as f32;
                let jitter = temporal::jitter(frame);
                let uniform = device.create_buffer_with_data(&BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&[
                        position.x,
                        position.y,
                        jitter.x,
                        jitter.y,
                        delta.x * motion_gain,
                        delta.y * motion_gain,
                        128.0,
                        128.0,
                    ]),
                    usage: BufferUsages::UNIFORM,
                });
                let group = device
                    .wgpu_device()
                    .create_bind_group(&BindGroupDescriptor {
                        label: None,
                        layout: &pipeline.get_bind_group_layout(0),
                        entries: &[BindGroupEntry {
                            binding: 0,
                            resource: uniform.as_entire_binding(),
                        }],
                    });
                let mut encoder = device.create_command_encoder(&default());
                {
                    let attachment = |view| {
                        Some(RenderPassColorAttachment {
                            view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: Operations {
                                load: LoadOp::Clear(wgpu::Color::BLACK),
                                store: StoreOp::Store,
                            },
                        })
                    };
                    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                        label: None,
                        color_attachments: &[attachment(&cv), attachment(&mv)],
                        depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                            view: &dv,
                            depth_ops: Some(Operations {
                                load: LoadOp::Clear(0.0),
                                store: StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        ..default()
                    });
                    pass.set_pipeline(&pipeline);
                    pass.set_bind_group(0, &group, &[]);
                    pass.set_viewport(0.0, 0.0, 128.0, 128.0, 0.0, 1.0);
                    pass.draw(0..3, 0..1);
                }
                let mut commands = vec![encoder.finish()];
                commands.extend(
                    scaler
                        .encode(
                            device,
                            temporal::Inputs {
                                color: &color,
                                depth: &depth,
                                motion: &motion,
                                output: &output,
                                output_view: &ov,
                                size: input,
                                jitter: jitter * sign,
                                reset: frame == 0,
                            },
                        )
                        .unwrap(),
                );
                if frame == 31 {
                    let mut encoder = device.create_command_encoder(&default());
                    encoder.copy_texture_to_buffer(
                        output.as_image_copy(),
                        TexelCopyBufferInfo {
                            buffer: &readback,
                            layout: TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(256 * 8),
                                rows_per_image: Some(256),
                            },
                        },
                        output.size(),
                    );
                    commands.push(encoder.finish());
                }
                queue.submit(commands);
            }
            let (tx, rx) = std::sync::mpsc::channel();
            readback.slice(..).map_async(MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            device
                .poll(PollType::Wait {
                    submission_index: None,
                    timeout: Some(std::time::Duration::from_secs(10)),
                })
                .unwrap();
            rx.recv_timeout(std::time::Duration::from_secs(10))
                .unwrap()
                .unwrap();
            let bytes = readback.slice(..).get_mapped_range();
            let mut squared = 0.0f64;
            let mut n = 0;
            for y in 48..208 {
                for x in 48..208 {
                    let q = (Vec2::new(x as f32, y as f32) + Vec2::splat(0.5)) * 0.5 - delta * 31.0;
                    let expected = 0.5
                        + 0.35
                            * (q.x * std::f32::consts::FRAC_PI_4 * frequency).sin()
                            * (q.y * std::f32::consts::TAU / 13.0 * frequency).cos();
                    let offset = (y * 256 + x) * 8;
                    let h = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
                    let exponent = i32::from((h >> 10) & 31);
                    let actual = if exponent == 0 {
                        f32::from(h & 1023) * 2.0f32.powi(-24)
                    } else {
                        (1.0 + f32::from(h & 1023) / 1024.0) * 2.0f32.powi(exponent - 15)
                    } * if h & 0x8000 == 0 { 1.0 } else { -1.0 };
                    assert!(actual.is_finite());
                    squared += f64::from(actual - expected).powi(2);
                    n += 1;
                }
            }
            let rmse = (squared / f64::from(n)).sqrt();
            errors.push(rmse);
            println!(
                "TEMPORAL_SAMPLING {name} motion_gain={motion_gain} frequency={frequency} backend_argument_sign={sign} rmse={:.6}",
                rmse
            );
            drop(bytes);
            readback.unmap();
        }
        if diagnostic {
            continue;
        }
        assert!(errors[0] < 0.02, "{name}: reconstruction error {errors:?}");
        assert!(
            errors[0] < errors[1] * 0.5,
            "{name}: production sampling convention must outperform reversed jitter: {errors:?}"
        );
    }
}
