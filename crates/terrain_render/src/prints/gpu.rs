//! The windows' textures and the passes filling them each frame. The map (R32Uint, one layer per
//! window, two half floats a texel: `terrain/print_map.wesl`) takes the frame's presses
//! (`terrain/press.wesl`); the relief (Rgba16Float with mip levels: how far feet lowered the
//! ground and its slope) is resolved from it and the soft layer's depth over the same tiles
//! (`terrain/print_relief.wesl`), then filtered level by level (`terrain/print_mips.wesl`). A small
//! texture tells the terrain material where the windows lie. All are GPU-only images under fixed
//! handles, so every terrain material binds them from the start.
use super::{LEVELS, PrintFrame, SIDE};

use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::schedule::camera_driver,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::{
        ExtractSchedule, MainWorld, Render, RenderApp, RenderStartup, RenderSystems,
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        texture::{FallbackImage, GpuImage},
    },
};

use bytemuck::{Pod, Zeroable};

const PRESS_SHADER: &str = "shaders/terrain/press.wesl";
const RELIEF_SHADER: &str = "shaders/terrain/print_relief.wesl";
const MIPS_SHADER: &str = "shaders/terrain/print_mips.wesl";

/// Tiles per row of the dispatch: a full window's tiles exceed one dimension's limit.
const DISPATCH_ROW: u32 = 256;

/// Mip levels of the relief: the coarsest is 32 texels, 2.56 m in the coarse window, as far
/// apart as the leaves' vertices morphing to the next level.
pub(crate) const RELIEF_MIPS: u32 = 6;

pub(crate) fn map_handle() -> Handle<Image> {
    bevy::asset::uuid_handle!("8f0f1c52-55b8-4d9b-9a8e-6f0f3c1d2a71")
}

pub(crate) fn windows_handle() -> Handle<Image> {
    bevy::asset::uuid_handle!("c3a7e2f4-0b6d-4c1e-8d52-7a9b6e4f1c03")
}

pub(crate) fn relief_handle() -> Handle<Image> {
    bevy::asset::uuid_handle!("5d2e9b17-3c4a-4f8e-b061-29e7a4c8d3f5")
}

pub(super) fn init_images(mut images: ResMut<Assets<Image>>) {
    let layers = |format, mips, sampler| {
        let mut image = Image::new_uninit(
            Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: LEVELS as u32,
            },
            TextureDimension::D2,
            format,
            RenderAssetUsages::RENDER_WORLD,
        );
        image.texture_descriptor.usage |= TextureUsages::STORAGE_BINDING;
        image.texture_descriptor.mip_level_count = mips;
        image.texture_view_descriptor = Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::D2Array),
            ..default()
        });
        image.sampler = sampler;
        image
    };
    let map = layers(TextureFormat::R32Uint, 1, ImageSampler::nearest());

    // Toroidal: filtering wraps across the texture edge as the windows do.
    let relief = layers(
        TextureFormat::Rgba16Float,
        RELIEF_MIPS,
        ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            mipmap_filter: ImageFilterMode::Linear,
            ..default()
        }),
    );
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

    for (handle, image) in [
        (map_handle(), map),
        (relief_handle(), relief),
        (windows_handle(), windows),
    ] {
        images
            .insert(handle.id(), image)
            .expect("the prints' handles are free");
    }
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
    /// Each window's first texel among the world's fine ones.
    cells: [i32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct ReliefGpu {
    windows: [[u32; 8]; LEVELS],
    /// Where the soft layer's depths lie (`PrintFrame::ground`).
    ground: [f32; 4],
    /// Tiles listed.
    counts: [u32; 4],
    /// Each window's first texel among the world's fine ones.
    cells: [i32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct MipGpu {
    /// Each window's toroidal first texel.
    wraps: [u32; 4],
    /// Tiles listed, the level written, texels per side.
    counts: [u32; 4],
}

#[derive(Resource, Default)]
struct RenderPrintFrame(PrintFrame);

/// One compute pipeline and its layout.
struct Stage {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
}

impl Stage {
    fn new(
        cache: &PipelineCache,
        server: &AssetServer,
        name: &'static str,
        shader: &'static str,
        entry: &'static str,
        entries: &[BindGroupLayoutEntry],
    ) -> Self {
        let layout = BindGroupLayoutDescriptor::new(name, entries);
        let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(name.into()),
            layout: vec![layout.clone()],
            shader: server.load(shader),
            entry_point: Some(entry.into()),
            ..default()
        });
        Self { layout, pipeline }
    }
}

#[derive(Resource)]
struct PrintPass {
    press: Stage,
    relief: Stage,
    mips: Stage,
    press_uniform: Buffer,
    relief_uniform: Buffer,
    /// One per mip level past the first.
    mip_uniforms: Vec<Buffer>,
    tiles: RawBufferVec<[u32; 4]>,
    indices: RawBufferVec<u32>,
    stamps: RawBufferVec<[f32; 12]>,
    soft_sampler: Sampler,
    map_view: Option<TextureView>,
    /// The relief's levels, one view each.
    relief_views: Vec<TextureView>,
    /// Pressing, resolving, then each mip level.
    bind_groups: Vec<BindGroup>,
    workgroups: [u32; 2],
}

fn init_pass(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    server: Res<AssetServer>,
) {
    let relief_storage =
        || texture_storage_2d_array(TextureFormat::Rgba16Float, StorageTextureAccess::WriteOnly);
    let press = Stage::new(
        &cache,
        &server,
        "ground prints",
        PRESS_SHADER,
        "press",
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
    let relief = Stage::new(
        &cache,
        &server,
        "ground print relief",
        RELIEF_SHADER,
        "resolve",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                texture_2d_array(TextureSampleType::Uint),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                relief_storage(),
            ),
        ),
    );
    let mips = Stage::new(
        &cache,
        &server,
        "ground print relief levels",
        MIPS_SHADER,
        "downsample",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                texture_2d_array(TextureSampleType::Float { filterable: false }),
                relief_storage(),
            ),
        ),
    );
    let uniform = |label, size| {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: size as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    commands.insert_resource(PrintPass {
        press,
        relief,
        mips,
        press_uniform: uniform("ground prints", size_of::<PressGpu>()),
        relief_uniform: uniform("ground print relief", size_of::<ReliefGpu>()),
        mip_uniforms: (1..RELIEF_MIPS)
            .map(|_| uniform("ground print relief level", size_of::<MipGpu>()))
            .collect(),
        tiles: storage("ground print tiles"),
        indices: storage("ground print stamp lists"),
        stamps: storage("ground print stamps"),
        soft_sampler: device.create_sampler(&SamplerDescriptor {
            label: Some("ground print soft layer"),
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::Repeat,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
        map_view: None,
        relief_views: Vec::new(),
        bind_groups: Vec::new(),
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
    fallback: Res<FallbackImage>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
) {
    pass.workgroups = [0, 0];
    pass.bind_groups.clear();
    let (Some(map), Some(relief), Some(windows)) = (
        images.get(&map_handle()),
        images.get(&relief_handle()),
        images.get(&windows_handle()),
    ) else {
        return;
    };
    let frame = std::mem::take(&mut frame.0);
    write_windows(&queue, windows, &frame);

    // Without the pipelines the work is dropped: until they first compile the map is still the
    // untouched ground the windows reset to.
    let compiled = [&pass.press, &pass.relief, &pass.mips]
        .iter()
        .all(|s| cache.get_compute_pipeline(s.pipeline).is_some());

    if frame.tiles.is_empty() || !compiled {
        return;
    }

    let tiles = frame.tiles.len() as u32;
    write_uniforms(&pass, &queue, &frame, tiles);

    // Bound buffers must not be empty.
    let pass = pass.as_mut();
    *pass.tiles.values_mut() = frame.tiles;
    *pass.indices.values_mut() = frame.indices;
    *pass.stamps.values_mut() = frame.stamps;
    pass.indices.values_mut().push(0);
    pass.stamps.values_mut().push([0.0; 12]);
    pass.tiles.write_buffer(&device, &queue);
    pass.indices.write_buffer(&device, &queue);
    pass.stamps.write_buffer(&device, &queue);
    let map_view = pass
        .map_view
        .get_or_insert_with(|| array_view(map, "ground prints", 0))
        .clone();

    if pass.relief_views.is_empty() {
        pass.relief_views = (0..RELIEF_MIPS)
            .map(|level| array_view(relief, "ground print relief", level))
            .collect();
    }

    let soft = frame
        .soft
        .and_then(|id| images.get(id))
        .map_or(&fallback.d2.texture_view, |image| &image.texture_view);
    let (Some(tiles_buffer), Some(indices), Some(stamps)) = (
        pass.tiles.buffer(),
        pass.indices.buffer(),
        pass.stamps.buffer(),
    ) else {
        return;
    };
    let layout = |stage: &Stage| cache.get_bind_group_layout(&stage.layout);
    let mut groups = vec![
        device.create_bind_group(
            Some("ground prints"),
            &layout(&pass.press),
            &BindGroupEntries::sequential((
                pass.press_uniform.as_entire_binding(),
                tiles_buffer.as_entire_binding(),
                indices.as_entire_binding(),
                stamps.as_entire_binding(),
                &map_view,
            )),
        ),
        device.create_bind_group(
            Some("ground print relief"),
            &layout(&pass.relief),
            &BindGroupEntries::sequential((
                pass.relief_uniform.as_entire_binding(),
                tiles_buffer.as_entire_binding(),
                &map_view,
                soft,
                &pass.soft_sampler,
                &pass.relief_views[0],
            )),
        ),
    ];

    for (level, uniform) in pass.mip_uniforms.iter().enumerate() {
        groups.push(device.create_bind_group(
            Some("ground print relief level"),
            &layout(&pass.mips),
            &BindGroupEntries::sequential((
                uniform.as_entire_binding(),
                tiles_buffer.as_entire_binding(),
                &pass.relief_views[level],
                &pass.relief_views[level + 1],
            )),
        ));
    }

    pass.bind_groups = groups;
    pass.workgroups = [tiles.min(DISPATCH_ROW), tiles.div_ceil(DISPATCH_ROW)];
}

fn write_windows(queue: &RenderQueue, windows: &GpuImage, frame: &PrintFrame) {
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
}

fn write_uniforms(pass: &PrintPass, queue: &RenderQueue, frame: &PrintFrame, tiles: u32) {
    let press = PressGpu {
        windows: frame.windows,
        shape: super::shape(),
        counts: [tiles, 0, 0, 0],
        cells: frame.cells,
    };
    let relief = ReliefGpu {
        windows: frame.windows,
        ground: frame.ground,
        counts: [tiles, 0, 0, 0],
        cells: frame.cells,
    };
    queue.write_buffer(&pass.press_uniform, 0, bytemuck::bytes_of(&press));
    queue.write_buffer(&pass.relief_uniform, 0, bytemuck::bytes_of(&relief));
    let [fine, coarse] = frame.windows;

    for (level, uniform) in (1..).zip(&pass.mip_uniforms) {
        let mip = MipGpu {
            wraps: [fine[2], fine[3], coarse[2], coarse[3]],
            counts: [tiles, level, SIDE, 0],
        };
        queue.write_buffer(uniform, 0, bytemuck::bytes_of(&mip));
    }
}

/// Every layer of one mip level.
fn array_view(image: &GpuImage, label: &'static str, level: u32) -> TextureView {
    image.texture.create_view(&TextureViewDescriptor {
        label: Some(label),
        dimension: Some(TextureViewDimension::D2Array),
        base_mip_level: level,
        mip_level_count: Some(1),
        ..default()
    })
}

fn press(mut render_context: RenderContext, pass: Res<PrintPass>, cache: Res<PipelineCache>) {
    let [x, y] = pass.workgroups;

    if x == 0 {
        return;
    }

    let pipeline = |stage: &Stage| cache.get_compute_pipeline(stage.pipeline);
    let (Some(press), Some(relief), Some(mips)) = (
        pipeline(&pass.press),
        pipeline(&pass.relief),
        pipeline(&pass.mips),
    ) else {
        return;
    };
    let mut compute = render_context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("ground prints"),
            ..default()
        });

    // Each dispatch sees the last one's writes.
    for (i, bind_group) in pass.bind_groups.iter().enumerate() {
        compute.set_pipeline(match i {
            0 => press,
            1 => relief,
            _ => mips,
        });
        compute.set_bind_group(0, bind_group, &[]);
        compute.dispatch_workgroups(x, y, 1);
    }
}
