//! The ground albedo map's pass (`shaders/terrain/ground_albedo.wesl`): each frame it fills the
//! environment's map (`atmosphere::ground_albedo`) where it now lies around the camera, from the
//! streamed world's clipmap ([`GroundAlbedoSource`]: its control samples, surfaces, heights and
//! normals) and the terrain's horizon towards the sun, with the height of the ground it draws.
//! Materials take the map once the pass has filled it.
use crate::clipmap::ClipmapSettings;

use atmosphere::{
    environment::EnvironmentAssets,
    ground_albedo::{FORMAT, GroundAlbedo, SIZE, TEXELS},
};
use bevy::{
    core_pipeline::schedule::camera_driver,
    prelude::*,
    render::{
        ExtractSchedule, MainWorld, RenderApp, RenderStartup,
        render_asset::RenderAssets,
        render_resource::{binding_types::*, encase, *},
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        storage::{GpuShaderBuffer, ShaderBuffer},
        texture::GpuImage,
    },
    shader::ShaderDefVal,
};

use bytemuck::{Pod, Zeroable};

const SHADER: &str = "shaders/terrain/ground_albedo.wesl";

/// Texels per side of a workgroup.
const WORKGROUP: u32 = 8;

/// The clipmap the ground albedo map is filled from (`ClipmapTextures::ground_albedo_source`):
/// the streamed world's, while it is drawn.
#[derive(Resource, Clone)]
pub struct GroundAlbedoSource {
    pub(crate) settings: ClipmapSettings,
    pub(crate) heights: Handle<Image>,
    pub(crate) normals: Handle<Image>,
    pub(crate) state: Handle<Image>,
    pub(crate) control: Handle<Image>,
    pub(crate) surfaces: Handle<ShaderBuffer>,
    pub(crate) base_color: Handle<Image>,
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct PlacementGpu {
    /// Each layer's first corner in render XZ: near, far.
    corners: [f32; 4],
    /// Metres per texel of each layer, the open sea's level, 1 with a sea.
    texels: [f32; 4],
    /// x: render height the ground's heights are stored above.
    reference: [f32; 4],
}

/// The frame's source and where the map lies, and whether the last frame filled it.
#[derive(Resource, Default)]
struct FillFrame {
    source: Option<GroundAlbedoSource>,
    albedo: GroundAlbedo,
    filled: bool,
}

#[derive(Resource)]
struct FillPass {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
    settings: Buffer,
    placement: Buffer,
}

pub(crate) fn install(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render
        .init_resource::<FillFrame>()
        .add_systems(RenderStartup, init_pass)
        .add_systems(ExtractSchedule, extract)
        .add_systems(RenderGraph, fill.before(camera_driver));
}

/// Takes the frame's source and placement, and tells the environment whether the map holds the
/// ground: materials read it from the next frame on.
fn extract(mut main: ResMut<MainWorld>, mut frame: ResMut<FillFrame>) {
    frame.source = main.get_resource::<GroundAlbedoSource>().cloned();
    let filled = frame.filled && frame.source.is_some();

    if let Some(mut albedo) = main.get_resource_mut::<GroundAlbedo>() {
        albedo.filled = filled;
        frame.albedo = *albedo;
    }
}

fn init_pass(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    server: Res<AssetServer>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "ground albedo",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (0, uniform_buffer::<ClipmapSettings>(false)),
                (
                    1,
                    texture_2d_array(TextureSampleType::Float { filterable: false }),
                ),
                (
                    2,
                    texture_2d_array(TextureSampleType::Float { filterable: true }),
                ),
                (3, sampler(SamplerBindingType::Filtering)),
                (4, texture_2d(TextureSampleType::Uint)),
                (5, texture_2d_array(TextureSampleType::Uint)),
                (6, storage_buffer_read_only_sized(false, None)),
                (
                    7,
                    texture_2d_array(TextureSampleType::Float { filterable: true }),
                ),
                (8, sampler(SamplerBindingType::Filtering)),
                (
                    16,
                    texture_storage_2d_array(FORMAT, StorageTextureAccess::WriteOnly),
                ),
                (17, uniform_buffer_sized(false, None)),
                (120, storage_buffer_read_only_sized(false, None)),
                (
                    126,
                    texture_2d(TextureSampleType::Float { filterable: true }),
                ),
                (127, sampler(SamplerBindingType::Filtering)),
            ),
        ),
    );
    let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("ground albedo".into()),
        layout: vec![layout.clone()],
        shader: server.load(SHADER),
        shader_defs: vec![ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 0)],
        entry_point: Some("fill".into()),
        ..default()
    });
    let uniform = |label, size: u64| {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    commands.insert_resource(FillPass {
        layout,
        pipeline,
        settings: uniform("ground albedo clipmap", ClipmapSettings::min_size().get()),
        placement: uniform("ground albedo placement", size_of::<PlacementGpu>() as u64),
    });
}

#[allow(clippy::too_many_arguments)] // The pass, its inputs and their GPU assets.
fn fill(
    mut ctx: RenderContext,
    pass: Option<Res<FillPass>>,
    mut frame: ResMut<FillFrame>,
    assets: Option<Res<EnvironmentAssets>>,
    images: Res<RenderAssets<GpuImage>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    cache: Res<PipelineCache>,
    queue: Res<RenderQueue>,
) {
    frame.filled = false;
    let (Some(pass), Some(assets), Some(source)) = (pass, assets, frame.source.as_ref()) else {
        return;
    };
    let (Some(pipeline), Some(map), Some(horizon)) = (
        cache.get_compute_pipeline(pass.pipeline),
        images.get(&assets.ground_albedo),
        images.get(&assets.terrain_shadow),
    ) else {
        return;
    };
    let (Some(heights), Some(normals), Some(state), Some(control), Some(base)) = (
        images.get(&source.heights),
        images.get(&source.normals),
        images.get(&source.state),
        images.get(&source.control),
        images.get(&source.base_color),
    ) else {
        return;
    };
    let (Some(surfaces), Some(environment)) = (
        buffers.get(&source.surfaces),
        buffers.get(&assets.parameters),
    ) else {
        return;
    };

    let mut settings = encase::UniformBuffer::new(Vec::<u8>::new());
    settings
        .write(&source.settings)
        .expect("settings fit their uniform");
    queue.write_buffer(&pass.settings, 0, settings.as_ref());
    let [near, far] = frame.albedo.corners;
    let sea = frame.albedo.sea;
    let placement = PlacementGpu {
        corners: [near.x, near.y, far.x, far.y],
        texels: [
            TEXELS[0],
            TEXELS[1],
            sea.unwrap_or(0.0),
            f32::from(u8::from(sea.is_some())),
        ],
        reference: [frame.albedo.reference, 0.0, 0.0, 0.0],
    };
    queue.write_buffer(&pass.placement, 0, bytemuck::bytes_of(&placement));

    let bind_group = ctx.render_device().create_bind_group(
        Some("ground albedo"),
        &cache.get_bind_group_layout(&pass.layout),
        &BindGroupEntries::with_indices((
            (0, pass.settings.as_entire_binding()),
            (1, &heights.texture_view),
            (2, &normals.texture_view),
            (3, &normals.sampler),
            (4, &state.texture_view),
            (5, &control.texture_view),
            (6, surfaces.buffer.as_entire_binding()),
            (7, &base.texture_view),
            (8, &base.sampler),
            (16, &map.texture_view),
            (17, pass.placement.as_entire_binding()),
            (120, environment.buffer.as_entire_binding()),
            (126, &horizon.texture_view),
            (127, &horizon.sampler),
        )),
    );
    let mut compute = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("ground albedo"),
            ..default()
        });
    compute.set_pipeline(pipeline);
    compute.set_bind_group(0, &bind_group, &[]);
    compute.dispatch_workgroups(SIZE / WORKGROUP, SIZE / WORKGROUP, 2);
    frame.filled = true;
}
