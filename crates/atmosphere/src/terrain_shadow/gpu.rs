//! The terrain shadow map's searches on the GPU (`shaders/lighting/terrain_horizon.wesl`),
//! written into the environment's map in place: the sky share once per relief, into a relief
//! texture beside the heights, then the horizon towards the map's light whenever it moves. Each
//! runs a band of rows a frame, so no frame takes a whole search.
use crate::environment::{EnvironmentAssets, EnvironmentParams};

use bevy::{
    core_pipeline::schedule::camera_driver,
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        storage::GpuShaderBuffer,
        texture::GpuImage,
    },
};

const SHADER: &str = "shaders/lighting/terrain_horizon.wesl";

/// Rows a frame. The sky's eight short searches cost about as much as the horizon's long one.
const SKY_ROWS: u32 = 96;

const HORIZON_ROWS: u32 = 160;

#[derive(Resource)]
struct Pipelines {
    sky_layout: BindGroupLayoutDescriptor,
    horizon_layout: BindGroupLayoutDescriptor,
    sky: CachedComputePipelineId,
    horizon: CachedComputePipelineId,
    /// First row and row count of this frame's band.
    band: Buffer,
}

/// Heights and sky share of one relief, and the sky's rows done.
struct Relief {
    heights: TextureViewId,
    view: TextureView,
    next_row: u32,
}

#[derive(Clone, Copy, PartialEq)]
struct HorizonKey {
    map: TextureViewId,
    heights: TextureViewId,
    azimuth: [u32; 2],
    spacing: u32,
}

#[derive(Resource, Default)]
struct Searches {
    relief: Option<Relief>,
    /// The horizon search under way and its next row, or the one finished.
    horizon: Option<(HorizonKey, u32)>,
    done: Option<HorizonKey>,
}

pub(crate) fn install(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render
        .init_resource::<Searches>()
        .add_systems(RenderStartup, init)
        .add_systems(RenderGraph, search.before(camera_driver));
}

fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    server: Res<AssetServer>,
    cache: Res<PipelineCache>,
) {
    let params = || {
        storage_buffer_read_only_sized(
            false,
            Some(std::num::NonZeroU64::new(size_of::<EnvironmentParams>() as u64).unwrap()),
        )
    };
    let unfiltered = || texture_2d(TextureSampleType::Float { filterable: false });
    let sky_layout = BindGroupLayoutDescriptor::new(
        "terrain sky share",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (0, params()),
                (1, uniform_buffer_sized(false, None)),
                (2, unfiltered()),
                (
                    3,
                    texture_storage_2d(TextureFormat::Rg32Float, StorageTextureAccess::WriteOnly),
                ),
            ),
        ),
    );
    let horizon_layout = BindGroupLayoutDescriptor::new(
        "terrain horizon",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (0, params()),
                (1, uniform_buffer_sized(false, None)),
                (4, unfiltered()),
                (
                    5,
                    texture_storage_2d(TextureFormat::Rgba16Float, StorageTextureAccess::WriteOnly),
                ),
            ),
        ),
    );
    let pipeline =
        |label: &'static str, layout: &BindGroupLayoutDescriptor, entry: &'static str| {
            cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some(label.into()),
                layout: vec![layout.clone()],
                shader: server.load(SHADER),
                entry_point: Some(entry.into()),
                ..default()
            })
        };
    commands.insert_resource(Pipelines {
        sky: pipeline("terrain sky share", &sky_layout, "sky"),
        horizon: pipeline("terrain horizon", &horizon_layout, "horizon"),
        sky_layout,
        horizon_layout,
        band: device.create_buffer(&BufferDescriptor {
            label: Some("terrain search band"),
            size: 16,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
    });
}

#[allow(clippy::too_many_arguments)] // Independent render-world inputs of one pass.
fn search(
    assets: Option<Res<EnvironmentAssets>>,
    params: Res<EnvironmentParams>,
    pipelines: Option<Res<Pipelines>>,
    cache: Res<PipelineCache>,
    images: Res<RenderAssets<GpuImage>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    queue: Res<RenderQueue>,
    mut searches: ResMut<Searches>,
    mut ctx: RenderContext,
) {
    let (Some(assets), Some(pipelines)) = (assets, pipelines) else {
        return;
    };
    let searches = &mut *searches;

    if params.terrain_shadow[2] <= 0.0 {
        return;
    }

    let (Some(buffer), Some(heights), Some(map), Some(sky), Some(horizon)) = (
        buffers.get(&assets.parameters),
        images.get(&assets.terrain_heights),
        images.get(&assets.terrain_shadow),
        cache.get_compute_pipeline(pipelines.sky),
        cache.get_compute_pipeline(pipelines.horizon),
    ) else {
        return;
    };
    let size = heights.texture_descriptor.size;

    // Between a new relief's two images.
    if map.texture_descriptor.size != size {
        return;
    }

    let device = ctx.render_device().clone();
    let band = |first: u32, rows: u32| {
        queue.write_buffer(
            &pipelines.band,
            0,
            bytemuck::cast_slice(&[first, rows, 0, 0]),
        );
    };

    if searches.relief.as_ref().map(|r| r.heights) != Some(heights.texture_view.id()) {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("terrain relief and sky share"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rg32Float,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        searches.relief = Some(Relief {
            heights: heights.texture_view.id(),
            view: texture.create_view(&default()),
            next_row: 0,
        });
        searches.horizon = None;
        searches.done = None;
    }

    let relief = searches.relief.as_mut().unwrap();
    let encoder = ctx.command_encoder();

    // The sky share first: the horizon search carries it into the map.
    if relief.next_row < size.height {
        let rows = SKY_ROWS.min(size.height - relief.next_row);
        band(relief.next_row, rows);
        let group = device.create_bind_group(
            "terrain sky share",
            &cache.get_bind_group_layout(&pipelines.sky_layout),
            &BindGroupEntries::with_indices((
                (0, buffer.buffer.as_entire_binding()),
                (1, pipelines.band.as_entire_binding()),
                (2, &heights.texture_view),
                (3, &relief.view),
            )),
        );
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("terrain sky share"),
            timestamp_writes: None,
        });
        pass.set_pipeline(sky);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(size.width.div_ceil(8), rows.div_ceil(8), 1);
        relief.next_row += rows;
        return;
    }

    let key = HorizonKey {
        map: map.texture_view.id(),
        heights: relief.heights,
        azimuth: [
            params.terrain_light[0].to_bits(),
            params.terrain_light[1].to_bits(),
        ],
        spacing: params.terrain_shadow[2].to_bits(),
    };

    if searches.done == Some(key) {
        return;
    }

    // A light that moved again restarts the search from the first row.
    let first = match searches.horizon {
        Some((k, row)) if k == key => row,
        _ => 0,
    };
    let rows = HORIZON_ROWS.min(size.height - first);
    band(first, rows);
    let group = device.create_bind_group(
        "terrain horizon",
        &cache.get_bind_group_layout(&pipelines.horizon_layout),
        &BindGroupEntries::with_indices((
            (0, buffer.buffer.as_entire_binding()),
            (1, pipelines.band.as_entire_binding()),
            (4, &relief.view),
            (5, &map.texture_view),
        )),
    );
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("terrain horizon"),
        timestamp_writes: None,
    });
    pass.set_pipeline(horizon);
    pass.set_bind_group(0, &group, &[]);
    pass.dispatch_workgroups(size.width.div_ceil(8), rows.div_ceil(8), 1);
    drop(pass);

    if first + rows >= size.height {
        searches.horizon = None;
        searches.done = Some(key);
    } else {
        searches.horizon = Some((key, first + rows));
    }
}

#[cfg(test)]
mod tests;
