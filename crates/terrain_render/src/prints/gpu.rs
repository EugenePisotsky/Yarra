//! The windows' texture (R32Uint, one layer per window, two half floats a texel:
//! `terrain/print_map.wesl`), the compute pass pressing each frame's tiles into it, and a small
//! texture telling the terrain material where the windows lie. Both textures are GPU-only
//! images under fixed handles, so every terrain material binds them from the start.
use super::{LEVELS, PrintFrame, SIDE};

use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::schedule::camera_driver,
    image::ImageSampler,
    prelude::*,
    render::{
        ExtractSchedule, MainWorld, Render, RenderApp, RenderStartup, RenderSystems,
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        texture::GpuImage,
    },
};

use bytemuck::{Pod, Zeroable};

const SHADER_PATH: &str = "shaders/terrain/press.wesl";

/// Tiles per row of the dispatch: a full window's tiles exceed one dimension's limit.
const DISPATCH_ROW: u32 = 256;

pub(crate) fn map_handle() -> Handle<Image> {
    bevy::asset::uuid_handle!("8f0f1c52-55b8-4d9b-9a8e-6f0f3c1d2a71")
}

pub(crate) fn windows_handle() -> Handle<Image> {
    bevy::asset::uuid_handle!("c3a7e2f4-0b6d-4c1e-8d52-7a9b6e4f1c03")
}

pub(super) fn init_images(mut images: ResMut<Assets<Image>>) {
    let mut map = Image::new_uninit(
        Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: LEVELS as u32,
        },
        TextureDimension::D2,
        TextureFormat::R32Uint,
        RenderAssetUsages::RENDER_WORLD,
    );
    map.texture_descriptor.usage |= TextureUsages::STORAGE_BINDING;
    map.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    map.sampler = ImageSampler::nearest();
    let mut windows = Image::new_uninit(
        Extent3d {
            width: 2 * LEVELS as u32,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba32Uint,
        RenderAssetUsages::RENDER_WORLD,
    );
    windows.sampler = ImageSampler::nearest();
    images
        .insert(map_handle().id(), map)
        .expect("the print map's handle is free");
    images
        .insert(windows_handle().id(), windows)
        .expect("the print windows' handle is free");
}

pub(super) fn install(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render
        .init_resource::<RenderPrintFrame>()
        .add_systems(RenderStartup, init_pass)
        .add_systems(ExtractSchedule, extract)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
        .add_systems(RenderGraph, press.before(camera_driver));
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct PressGpu {
    windows: [[u32; 8]; LEVELS],
    /// The sole's reach.
    shape: [f32; 4],
    /// Tiles listed.
    counts: [u32; 4],
}

#[derive(Resource, Default)]
struct RenderPrintFrame(PrintFrame);

#[derive(Resource)]
struct PrintPass {
    uniform: Buffer,
    tiles: RawBufferVec<[u32; 4]>,
    indices: RawBufferVec<u32>,
    stamps: RawBufferVec<[f32; 8]>,
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
    view: Option<TextureView>,
    bind_group: Option<BindGroup>,
    workgroups: [u32; 2],
}

fn init_pass(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    server: Res<AssetServer>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "ground prints",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                texture_storage_2d_array(TextureFormat::R32Uint, StorageTextureAccess::ReadWrite),
            ),
        ),
    );
    let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("ground prints".into()),
        layout: vec![layout.clone()],
        shader: server.load(SHADER_PATH),
        entry_point: Some("press".into()),
        ..default()
    });
    let uniform = device.create_buffer(&BufferDescriptor {
        label: Some("ground prints"),
        size: size_of::<PressGpu>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    commands.insert_resource(PrintPass {
        uniform,
        tiles: storage("ground print tiles"),
        indices: storage("ground print stamp lists"),
        stamps: storage("ground print stamps"),
        layout,
        pipeline,
        view: None,
        bind_group: None,
        workgroups: [0, 0],
    });
}

fn storage<T: Pod>(label: &str) -> RawBufferVec<T> {
    let mut buffer = RawBufferVec::new(BufferUsages::STORAGE);
    buffer.set_label(Some(label));
    buffer
}

fn extract(mut main: ResMut<MainWorld>, mut frame: ResMut<RenderPrintFrame>) {
    if let Some(mut prints) = main.get_resource_mut::<super::GroundPrints>() {
        frame.0 = std::mem::take(&mut prints.frame);
    }
}

fn prepare(
    mut pass: ResMut<PrintPass>,
    mut frame: ResMut<RenderPrintFrame>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
) {
    pass.workgroups = [0, 0];
    let (Some(map), Some(windows)) = (images.get(&map_handle()), images.get(&windows_handle()))
    else {
        return;
    };
    let frame = std::mem::take(&mut frame.0);
    let words: Vec<u8> = frame
        .windows
        .iter()
        .flatten()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    queue.write_texture(
        windows.texture.as_image_copy(),
        &words,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(2 * LEVELS as u32 * 16),
            rows_per_image: Some(1),
        },
        Extent3d {
            width: 2 * LEVELS as u32,
            height: 1,
            depth_or_array_layers: 1,
        },
    );

    // Without the pipeline the work is dropped: until it first compiles the map is still the
    // untouched ground the windows reset to.
    if frame.tiles.is_empty() || cache.get_compute_pipeline(pass.pipeline).is_none() {
        return;
    }

    let tiles = frame.tiles.len() as u32;
    let gpu = PressGpu {
        windows: frame.windows,
        shape: super::shape(),
        counts: [tiles, 0, 0, 0],
    };
    queue.write_buffer(&pass.uniform, 0, bytemuck::bytes_of(&gpu));

    // Bound buffers must not be empty.
    let pass = pass.as_mut();
    *pass.tiles.values_mut() = frame.tiles;
    *pass.indices.values_mut() = frame.indices;
    *pass.stamps.values_mut() = frame.stamps;
    pass.indices.values_mut().push(0);
    pass.stamps.values_mut().push([0.0; 8]);
    pass.tiles.write_buffer(&device, &queue);
    pass.indices.write_buffer(&device, &queue);
    pass.stamps.write_buffer(&device, &queue);
    let view = pass.view.get_or_insert_with(|| {
        map.texture.create_view(&TextureViewDescriptor {
            label: Some("ground prints"),
            dimension: Some(TextureViewDimension::D2Array),
            ..default()
        })
    });
    let (Some(tiles_buffer), Some(indices), Some(stamps)) = (
        pass.tiles.buffer(),
        pass.indices.buffer(),
        pass.stamps.buffer(),
    ) else {
        return;
    };
    pass.bind_group = Some(device.create_bind_group(
        Some("ground prints"),
        &cache.get_bind_group_layout(&pass.layout),
        &BindGroupEntries::sequential((
            pass.uniform.as_entire_binding(),
            tiles_buffer.as_entire_binding(),
            indices.as_entire_binding(),
            stamps.as_entire_binding(),
            &*view,
        )),
    ));
    pass.workgroups = [tiles.min(DISPATCH_ROW), tiles.div_ceil(DISPATCH_ROW)];
}

fn press(mut render_context: RenderContext, pass: Res<PrintPass>, cache: Res<PipelineCache>) {
    let ([x, y], Some(bind_group), Some(pipeline)) = (
        pass.workgroups,
        pass.bind_group.as_ref(),
        cache.get_compute_pipeline(pass.pipeline),
    ) else {
        return;
    };

    if x == 0 {
        return;
    }

    let mut compute = render_context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("ground prints"),
            ..default()
        });
    compute.set_pipeline(pipeline);
    compute.set_bind_group(0, bind_group, &[]);
    compute.dispatch_workgroups(x, y, 1);
}
