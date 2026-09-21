//! Small readback probe for native command ordering, sRGB handling and resource recreation.
use super::*;
use bevy::render::{
    renderer::{RenderDevice, initialize_renderer},
    settings::{Backends, WgpuSettings},
    texture::GpuImage,
};

fn gpu_image(device: &RenderDevice, image: Image) -> GpuImage {
    let texture = device.create_texture(&image.texture_descriptor);
    let texture_view =
        texture.create_view(&image.texture_view_descriptor.clone().unwrap_or_default());
    GpuImage {
        texture,
        texture_view,
        sampler: device.create_sampler(&default()),
        texture_descriptor: image.texture_descriptor,
        texture_view_descriptor: image.texture_view_descriptor,
        had_data: false,
    }
}

#[test]
#[ignore = "requires a native GPU; small bounded readback test"]
fn spatial_backends_preserve_colour_orientation_and_updates() {
    let resources = bevy::tasks::block_on(initialize_renderer(
        Backends::PRIMARY,
        None,
        &WgpuSettings::default(),
    ));
    let device = &resources.0;
    let queue = &resources.1;
    let linear = backends::linear::Linear::new(device);
    for (width, height, out_width, out_height) in [(64, 32, 128, 64), (63, 35, 129, 73)] {
        let mut image =
            Image::new_target_texture(width, height, wgpu::TextureFormat::Bgra8UnormSrgb, None);
        image.texture_descriptor.usage |= wgpu::TextureUsages::COPY_SRC;
        let input = gpu_image(device, image.clone());
        let upload = gpu_image(device, image);
        let output = gpu_image(device, output_image(UVec2::new(out_width, out_height)));
        // A second render pass samples the result exactly as Bevy's UI will.
        let mut sampled_image = output_image(UVec2::new(out_width, out_height));
        sampled_image.texture_descriptor.usage |= wgpu::TextureUsages::COPY_SRC;
        let sampled = gpu_image(device, sampled_image);
        let src_bind = linear.bind(device, &input);
        let output_bind = linear.bind(device, &output);
        let mut spatial = {
            #[cfg(all(feature = "metalfx", any(target_os = "macos", target_os = "ios")))]
            {
                Some(
                    backends::Spatial::new(device, &input, &output)
                        .expect("native MetalFX must actually be available for this test"),
                )
            }
            #[cfg(not(all(feature = "metalfx", any(target_os = "macos", target_os = "ios"))))]
            {
                None::<backends::Spatial>
            }
        };
        let row_bytes = (out_width * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("upscale readback"),
            size: (row_bytes * out_height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        for frame in 0..4 {
            let mut colours = [
                [32u8, 82, 144, 255],
                [190, 120, 45, 255],
                [71, 183, 97, 255],
                [220, 213, 201, 255],
            ];
            colours.rotate_left(frame);
            let mut pixels = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    let [r, g, b, a] =
                        colours[(usize::from(y >= height / 2) * 2) + usize::from(x >= width / 2)];
                    pixels.extend_from_slice(&[b, g, r, a]);
                }
            }
            queue.write_texture(
                upload.texture.as_image_copy(),
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                upload.texture.size(),
            );
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("upscale colour test"),
            });
            encoder.copy_texture_to_texture(
                upload.texture.as_image_copy(),
                input.texture.as_image_copy(),
                input.texture.size(),
            );
            let native = frame % 2 == 0 && spatial.is_some();
            let mut buffers = vec![encoder.finish()];
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sample upscale result"),
            });
            if native {
                buffers.extend(
                    spatial
                        .as_mut()
                        .unwrap()
                        .encode(device, &input, &output)
                        .unwrap(),
                );
            } else {
                linear.encode(&mut encoder, &src_bind, &output);
            }
            linear.encode(&mut encoder, &output_bind, &sampled);
            encoder.copy_texture_to_buffer(
                sampled.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes),
                        rows_per_image: Some(out_height),
                    },
                },
                sampled.texture.size(),
            );
            buffers.push(encoder.finish());
            queue.submit(buffers);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(std::time::Duration::from_secs(10)),
                })
                .unwrap();
            rx.recv_timeout(std::time::Duration::from_secs(10))
                .unwrap()
                .unwrap();
            let bytes = readback.slice(..).get_mapped_range();
            for (quadrant, expected) in colours.iter().enumerate() {
                let x = out_width * (if quadrant % 2 == 0 { 1 } else { 3 }) / 4;
                let y = out_height * (if quadrant < 2 { 1 } else { 3 }) / 4;
                let offset = (y * row_bytes + x * 4) as usize;
                let actual = &bytes[offset..offset + 4];
                eprintln!(
                    "native={native} {width}x{height}, frame={frame}, quadrant={quadrant}: {actual:?}"
                );
                assert!(
                    actual
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| a.abs_diff(*b) <= 3),
                    "colour/orientation mismatch: got {actual:?}, expected {expected:?}"
                );
            }
            drop(bytes);
            readback.unmap();
        }
    }
}

#[test]
#[ignore = "requires native MetalFX; bounded 64x64 HDR/depth/motion probe"]
fn temporal_preserves_hdr_and_resets_history() {
    use bevy::render::render_resource::*;
    let resources = bevy::tasks::block_on(initialize_renderer(
        Backends::PRIMARY,
        None,
        &WgpuSettings::default(),
    ));
    let (device, queue) = (&resources.0, &resources.1);
    let input_size = UVec2::new(32, 32);
    let output_size = UVec2::new(64, 64);
    let texture = |format, extra| {
        let mut image = Image::new_target_texture(64, 64, format, None);
        image.texture_descriptor.usage |= extra;
        gpu_image(device, image)
    };
    let color = texture(TextureFormat::Rgba16Float, TextureUsages::empty());
    let depth = texture(TextureFormat::Depth32Float, TextureUsages::empty());
    let motion = texture(TextureFormat::Rg16Float, TextureUsages::empty());
    let output = texture(
        TextureFormat::Rgba16Float,
        TextureUsages::STORAGE_BINDING | TextureUsages::COPY_SRC,
    );
    let mut scaler = backends::Temporal::new(device, input_size, output_size)
        .expect("native temporal support required");
    let readback = device.create_buffer(&BufferDescriptor {
        label: None,
        size: 64 * 64 * 8,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for frame in 0..8 {
        let expected = if frame < 4 {
            [0.25, 0.5, 0.75]
        } else {
            [0.75, 0.25, 0.125]
        };
        let mut encoder = device.create_command_encoder(&default());
        {
            let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: None,
                color_attachments: &[
                    Some(RenderPassColorAttachment {
                        view: &color.texture_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: Operations {
                            load: LoadOp::Clear(wgpu::Color {
                                r: expected[0],
                                g: expected[1],
                                b: expected[2],
                                a: 1.,
                            }),
                            store: StoreOp::Store,
                        },
                    }),
                    Some(RenderPassColorAttachment {
                        view: &motion.texture_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: Operations {
                            load: LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                    view: &depth.texture_view,
                    depth_ops: Some(Operations {
                        load: LoadOp::Clear(0.5),
                        store: StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..default()
            });
        }
        let mut commands = vec![encoder.finish()];
        commands.extend(
            scaler
                .encode(
                    device,
                    temporal::Inputs {
                        color: &color.texture,
                        depth: &depth.texture,
                        motion: &motion.texture,
                        output: &output.texture,
                        output_view: &output.texture_view,
                        size: input_size,
                        jitter: temporal::jitter(frame),
                        reset: frame == 0 || frame == 4,
                    },
                )
                .unwrap(),
        );
        let mut encoder = device.create_command_encoder(&default());
        encoder.copy_texture_to_buffer(
            output.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(512),
                    rows_per_image: Some(64),
                },
            },
            output.texture.size(),
        );
        commands.push(encoder.finish());
        queue.submit(commands);
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
        let data = readback.slice(..).get_mapped_range();
        for (channel, expected) in expected.iter().enumerate() {
            let offset = 32 * 512 + 32 * 8 + channel * 2;
            let half = u16::from_le_bytes([data[offset], data[offset + 1]]);
            let exponent = ((half >> 10) & 31) as i32;
            let actual = (1. + f64::from(half & 1023) / 1024.) * 2.0_f64.powi(exponent - 15);
            assert!(
                half & 0x8000 == 0 && (actual - expected).abs() < 0.03,
                "frame {frame}, channel {channel}: {actual} vs {expected}"
            );
        }
        drop(data);
        readback.unmap();
    }
}
