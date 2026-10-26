//! Blowing snow's field (`shaders/sky/drift.wesl`): where and how thick the wind lifts snow off
//! the ground around the camera, and the light the snow takes there from the sky, the ground and
//! the sun. All of it depends on the ground alone, so one pass fills it each frame over the
//! ground map's texels (`crate::ground_albedo`), after the map, and the sky composite integrates
//! each view ray through it instead of working it out again at every stretch of every pixel.
use crate::{
    environment::{EnvironmentAssets, EnvironmentParams},
    ground_albedo::{FORMAT, GroundAlbedoFill, SIZE},
};

use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::schedule::camera_driver,
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderGraph},
        storage::GpuShaderBuffer,
        texture::GpuImage,
    },
};

const SHADER: &str = "shaders/sky/drift.wesl";

const WORKGROUP: u32 = 8;

/// The field, written on the GPU alone: in array layer 0 for the ground map's near layer and 1 for
/// its far one, each texel the snow's light over the frame's scale and its extinction.
pub(crate) fn image() -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 2,
        },
        TextureDimension::D2,
        FORMAT,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage |= TextureUsages::STORAGE_BINDING;
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    image.sampler = bevy::image::ImageSampler::linear();
    image
}

pub(crate) struct DriftPlugin;

impl Plugin for DriftPlugin {
    fn build(&self, app: &mut App) {
        let Some(render) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render.add_systems(RenderStartup, init).add_systems(
            RenderGraph,
            fill.after(GroundAlbedoFill).before(camera_driver),
        );
    }
}

#[derive(Resource)]
struct FillPass {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
    // Group 0 is the composite's view, which the field does not read.
    view: BindGroup,
}

fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    server: Res<AssetServer>,
) {
    let view = BindGroupLayoutDescriptor::new("blowing snow view", &[]);

    // The composite's slots for the maps it shares (sky/composite_common.wesl), and the field.
    let layout = BindGroupLayoutDescriptor::new(
        "blowing snow field",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (
                    1,
                    storage_buffer_read_only_sized(
                        false,
                        std::num::NonZeroU64::new(size_of::<EnvironmentParams>() as u64),
                    ),
                ),
                (7, sampler(SamplerBindingType::Filtering)),
                (8, texture_3d(TextureSampleType::Float { filterable: true })),
                (9, sampler(SamplerBindingType::Filtering)),
                (
                    17,
                    texture_2d_array(TextureSampleType::Float { filterable: true }),
                ),
                (
                    19,
                    texture_storage_2d_array(FORMAT, StorageTextureAccess::WriteOnly),
                ),
            ),
        ),
    );
    let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("blowing snow field".into()),
        layout: vec![view.clone(), layout.clone()],
        shader: server.load(SHADER),
        shader_defs: vec![crate::environment::mist_noise_period_def()],
        entry_point: Some("fill".into()),
        ..default()
    });
    let view = device.create_bind_group(
        Some("blowing snow view"),
        &cache.get_bind_group_layout(&view),
        &[],
    );
    commands.insert_resource(FillPass {
        layout,
        pipeline,
        view,
    });
}

fn fill(
    mut ctx: RenderContext,
    pass: Option<Res<FillPass>>,
    assets: Option<Res<EnvironmentAssets>>,
    images: Res<RenderAssets<GpuImage>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    cache: Res<PipelineCache>,
) {
    let (Some(pass), Some(assets)) = (pass, assets) else {
        return;
    };
    let (Some(pipeline), Some(parameters)) = (
        cache.get_compute_pipeline(pass.pipeline),
        buffers.get(&assets.parameters),
    ) else {
        return;
    };
    let (Some(ground), Some(noise), Some(field)) = (
        images.get(&assets.ground_albedo),
        images.get(&assets.noise),
        images.get(&assets.drift),
    ) else {
        return;
    };
    let group = ctx.render_device().create_bind_group(
        Some("blowing snow field"),
        &cache.get_bind_group_layout(&pass.layout),
        &BindGroupEntries::with_indices((
            (1, parameters.buffer.as_entire_binding()),
            (7, &ground.sampler),
            (8, &noise.texture_view),
            (9, &noise.sampler),
            (17, &ground.texture_view),
            (19, &field.texture_view),
        )),
    );
    let mut compute = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("blowing snow field"),
            ..default()
        });
    compute.set_pipeline(pipeline);
    compute.set_bind_group(0, &pass.view, &[]);
    compute.set_bind_group(1, &group, &[]);
    compute.dispatch_workgroups(SIZE / WORKGROUP, SIZE / WORKGROUP, 2);
}
