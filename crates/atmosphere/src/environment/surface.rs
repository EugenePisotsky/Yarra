//! The environment as lit surfaces read it, in their material bind group (slots 120-125,
//! `shaders/lighting/surface.wesl`): the parameter buffer, the cloud shadow map, rain shelter and
//! the forest shadow map. Pipelines outside Bevy's materials (grass) bind the same group.
use super::*;
use bevy::render::{
    Render, RenderApp, RenderStartup, RenderSystems,
    render_asset::RenderAssets,
    renderer::{RenderDevice, RenderQueue},
    storage::GpuShaderBuffer,
    texture::GpuImage,
};
use binding_types::*;

#[derive(Resource)]
pub struct EnvironmentSurfaceLayout(pub BindGroupLayoutDescriptor);
#[derive(Resource)]
pub struct EnvironmentSurfaceGpu(pub BindGroup);
pub(super) fn install(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render
        .add_systems(RenderStartup, init)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
        .add_systems(
            Render,
            upload_forest_sky_levels.in_set(RenderSystems::PrepareResources),
        );
}
fn init(mut commands: Commands) {
    commands.insert_resource(EnvironmentSurfaceLayout(surface_layout()));
}
/// Writes the forest map's sky levels into its texture once Bevy has rewritten level 0, and
/// again whenever the map or its texture changes.
fn upload_forest_sky_levels(
    levels: Option<Res<ForestSkyLevels>>,
    assets: Option<Res<EnvironmentAssets>>,
    images: Res<RenderAssets<GpuImage>>,
    queue: Res<RenderQueue>,
    mut written: Local<Option<(u64, TextureId)>>,
) {
    let (Some(levels), Some(assets)) = (levels, assets) else {
        return;
    };
    let Some(image) = images.get(&assets.forest_shadow) else {
        return;
    };
    let key = (levels.revision, image.texture.id());
    if levels.levels.is_empty() || *written == Some(key) {
        return;
    }
    for (index, data) in levels.levels.iter().enumerate() {
        let level = index as u32 + 1;
        let side = crate::forest_shadow::SIZE >> level;
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &image.texture,
                mip_level: level,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            data,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(side * 8),
                rows_per_image: Some(side),
            },
            Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
        );
    }
    *written = Some(key);
}

fn prepare(
    mut commands: Commands,
    assets: Option<Res<EnvironmentAssets>>,
    params: Res<EnvironmentParams>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    layout: Res<EnvironmentSurfaceLayout>,
    mut previous: Local<Option<(BufferId, TextureViewId, TextureViewId, TextureViewId)>>,
) {
    let Some(assets) = assets else {
        return;
    };
    let (Some(buffer), Some(image), Some(shelter), Some(forest)) = (
        buffers.get(&assets.parameters),
        images.get(&assets.shadows),
        images.get(&assets.shelter),
        images.get(&assets.forest_shadow),
    ) else {
        return;
    };
    queue.write_buffer(&buffer.buffer, 0, bytemuck::bytes_of(&*params));
    let key = (
        buffer.buffer.id(),
        image.texture_view.id(),
        shelter.texture_view.id(),
        forest.texture_view.id(),
    );
    if *previous != Some(key) {
        commands.insert_resource(EnvironmentSurfaceGpu(device.create_bind_group(
            "environment surface bind group",
            &cache.get_bind_group_layout(&layout.0),
            &BindGroupEntries::with_indices((
                (120, buffer.buffer.as_entire_binding()),
                (121, &image.texture_view),
                (122, &image.sampler),
                (123, &shelter.texture_view),
                (124, &forest.texture_view),
                (125, &forest.sampler),
            )),
        )));
        *previous = Some(key);
    }
}
pub fn surface_layout() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "environment surface input",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (
                    120,
                    storage_buffer_read_only_sized(
                        false,
                        Some(
                            std::num::NonZeroU64::new(
                                std::mem::size_of::<EnvironmentParams>() as u64
                            )
                            .unwrap(),
                        ),
                    ),
                ),
                (
                    121,
                    texture_2d(TextureSampleType::Float { filterable: true }),
                ),
                (122, sampler(SamplerBindingType::Filtering)),
                (
                    123,
                    texture_2d(TextureSampleType::Float { filterable: false }),
                ),
                (
                    124,
                    texture_2d(TextureSampleType::Float { filterable: true }),
                ),
                (125, sampler(SamplerBindingType::Filtering)),
            ),
        ),
    )
}
