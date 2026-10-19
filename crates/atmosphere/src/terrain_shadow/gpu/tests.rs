//! The GPU searches against the CPU reference (`terrain_shadow::reference`).
use super::*;
use crate::terrain_shadow::{TerrainRelief, reference};
use bevy::render::{
    render_resource::{
        BindGroupDescriptor, BindGroupEntry, BindingResource, BufferInitDescriptor,
        CommandEncoderDescriptor, Extent3d, MapMode, Origin3d, PollType,
        RawComputePipelineDescriptor, ShaderModuleDescriptor, ShaderSource, TexelCopyBufferInfo,
        TexelCopyBufferLayout, TexelCopyTextureInfo,
    },
    renderer::initialize_renderer,
    settings::{Backends, WgpuSettings},
};

/// Rolling hills with a ridge, 8 m texels.
fn hills() -> TerrainRelief {
    let size = UVec2::new(150, 110);
    let heights: Vec<Option<f32>> = (0..size.x * size.y)
        .map(|i| {
            let (x, z) = ((i % size.x) as f32, (i / size.x) as f32);
            let ridge = 180.0 * (-((x - 0.6 * z - 60.0) / 9.0).powi(2)).exp();
            Some(40.0 * (x / 13.0).sin() * (z / 17.0).cos() + ridge)
        })
        .collect();
    TerrainRelief::from_heights([0.0, 0.0], size, 8.0, &heights, None)
}

#[test]
#[ignore = "requires a native GPU adapter; run explicitly on a development machine"]
fn gpu_searches_match_the_cpu_reference() {
    for (relief, azimuth) in [
        (crate::terrain_shadow::tests::wall(), Vec2::X),
        (hills(), Vec2::new(0.6, -0.8)),
        (hills(), Vec2::new(-0.96, 0.28)),
    ] {
        let expected = reference::map(&relief, azimuth);
        let got = run(&relief, azimuth);
        let mut far = 0;
        for (i, (e, g)) in expected.iter().zip(&got).enumerate() {
            // Half floats: about 0.1% of the value.
            let close = |a: f32, b: f32, abs: f32| (a - b).abs() <= abs + 0.002 * a.abs();
            assert!(close(e[0], g[0], 0.01), "tangent at {i}: {e:?} vs {g:?}");
            assert!(close(e[1], g[1], 0.05), "height at {i}: {e:?} vs {g:?}");
            assert!(close(e[2], g[2], 0.004), "sky at {i}: {e:?} vs {g:?}");
            // Two nearly equal peaks may swap the distance.
            far += usize::from(!close(e[3], g[3], 4.0));
        }
        assert!(far * 200 <= got.len(), "{far} distances differ");
    }
}

fn run(relief: &TerrainRelief, azimuth: Vec2) -> Vec<[f32; 4]> {
    let wgsl = shader_check::Shaders::get()
        .compose(SHADER, &[])
        .unwrap_or_else(|e| panic!("{e}"));
    let resources = bevy::tasks::block_on(initialize_renderer(
        Backends::PRIMARY,
        None,
        &WgpuSettings::default(),
    ));
    let (device, queue) = (resources.0, resources.1);
    let wgpu = device.wgpu_device();
    let module = wgpu.create_shader_module(ShaderModuleDescriptor {
        label: Some("terrain horizon"),
        source: ShaderSource::Wgsl(wgsl.into()),
    });
    let pipeline = |entry| {
        wgpu.create_compute_pipeline(&RawComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &module,
            entry_point: Some(entry),
            compilation_options: default(),
            cache: None,
        })
    };
    let (sky, horizon) = (pipeline("sky"), pipeline("horizon"));
    let (w, h) = (relief.size.x, relief.size.y);
    let params = EnvironmentParams {
        terrain_shadow: [0.0, 0.0, relief.metres_per_texel, 1.0],
        terrain_light: [azimuth.x, azimuth.y, 0.0, 0.0],
        ..default()
    };
    let params = device.create_buffer_with_data(&BufferInitDescriptor {
        label: None,
        contents: bytemuck::bytes_of(&params),
        usage: BufferUsages::STORAGE,
    });
    let band = device.create_buffer_with_data(&BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&[0_u32, h, 0, 0]),
        usage: BufferUsages::UNIFORM,
    });
    let texture = |format, usage| {
        device.create_texture(&TextureDescriptor {
            label: None,
            size: Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let heights = texture(
        TextureFormat::R32Float,
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
    );
    queue.write_texture(
        heights.as_image_copy(),
        bytemuck::cast_slice(&relief.heights),
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        heights.size(),
    );
    let both = TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING;
    let relief_texture = texture(TextureFormat::Rg32Float, both);
    let map = texture(TextureFormat::Rgba16Float, both | TextureUsages::COPY_SRC);
    let views = [&heights, &relief_texture, &map].map(|t| t.create_view(&default()));
    let group = |layout, textures: [(u32, usize); 2]| {
        let mut entries = vec![
            BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: band.as_entire_binding(),
            },
        ];
        entries.extend(textures.map(|(binding, view)| BindGroupEntry {
            binding,
            resource: BindingResource::TextureView(&views[view]),
        }));
        wgpu.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &entries,
        })
    };
    let sky_group = group(sky.get_bind_group_layout(0), [(2, 0), (3, 1)]);
    let horizon_group = group(horizon.get_bind_group_layout(0), [(4, 1), (5, 2)]);
    let row = (w * 8).next_multiple_of(256);
    let readback = device.create_buffer(&BufferDescriptor {
        label: None,
        size: u64::from(row * h),
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = wgpu.create_command_encoder(&CommandEncoderDescriptor::default());
    for (pipeline, group) in [(&sky, &sky_group), (&horizon, &horizon_group)] {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
    }
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &map,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        map.size(),
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(MapMode::Read, move |result| sender.send(result).unwrap());
    wgpu.poll(PollType::wait_indefinitely()).unwrap();
    receiver.recv().unwrap().unwrap();
    let data = readback
        .slice(..)
        .get_mapped_range()
        .expect("readback is mapped");
    (0..h as usize)
        .flat_map(|z| {
            let start = z * row as usize;
            let texels: &[u16] = bytemuck::cast_slice(&data[start..start + w as usize * 8]);
            texels
                .chunks(4)
                .map(|t| std::array::from_fn(|c| half::f16::from_bits(t[c]).to_f32()))
                .collect::<Vec<_>>()
        })
        .collect()
}
