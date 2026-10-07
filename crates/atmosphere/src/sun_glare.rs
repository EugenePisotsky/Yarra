//! How much of the sun each view sees, for the glare the sky composite draws around it. A
//! single workgroup samples the depth buffer across the sun's disc when it is on screen, or the
//! sun's shadow map at the camera when it is not, and dims both by the cloud shadow at the
//! camera. The result stays on the GPU in a small per-view buffer the composite reads.
use crate::{clouds::CloudAssets, sky::SkyCompositeView};
use bevy::{
    core_pipeline::{Core3d, Core3dSystems},
    pbr::{GpuLights, LightMeta, ShadowSamplers, ViewLightsUniformOffset, ViewShadowBindings},
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, ViewQuery},
        storage::GpuShaderBuffer,
        texture::GpuImage,
        view::{ViewDepthTexture, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
};

/// Per view: x the share of the sun seen, y 1 when its disc is on screen, zw its position in
/// main-pass uv.
#[derive(Component)]
pub(crate) struct SunState(pub(crate) Buffer);

pub(crate) struct SunGlarePlugin;
impl Plugin for SunGlarePlugin {
    fn build(&self, app: &mut App) {
        let Some(render) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render
            .add_systems(RenderStartup, init)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
            .add_systems(
                Core3d,
                draw.after(Core3dSystems::MainPass)
                    .before(crate::sky::SkyCompositeDraw),
            );
    }
}

fn prepare(
    mut commands: Commands,
    views: Query<Entity, (With<SkyCompositeView>, Without<SunState>)>,
    device: Res<RenderDevice>,
) {
    for entity in &views {
        // Fully visible until the first measurement.
        let buffer = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("sun state"),
            contents: bytemuck::bytes_of(&[1.0_f32, 0.0, 0.5, 0.5]),
            usage: BufferUsages::STORAGE,
        });
        commands.entity(entity).insert(SunState(buffer));
    }
}

#[derive(Resource)]
struct Pipelines {
    layout: [BindGroupLayoutDescriptor; 2],
    occlusion: [CachedComputePipelineId; 2],
    repeat: Sampler,
}

fn init(
    mut commands: Commands,
    server: Res<AssetServer>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
) {
    let layout = std::array::from_fn(|multisampled| {
        BindGroupLayoutDescriptor::new(
            "sun occlusion",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer::<ViewUniform>(true),
                    storage_buffer_read_only_sized(
                        false,
                        std::num::NonZeroU64::new(
                            std::mem::size_of::<crate::clouds::CloudParams>() as u64,
                        ),
                    ),
                    if multisampled == 1 {
                        texture_depth_2d_multisampled()
                    } else {
                        texture_depth_2d()
                    },
                    uniform_buffer::<GpuLights>(true),
                    texture_2d_array(TextureSampleType::Depth),
                    sampler(SamplerBindingType::Comparison),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    storage_buffer_sized(false, std::num::NonZeroU64::new(16)),
                ),
            ),
        )
    });
    let shader = server.load("shaders/sky/sun_occlusion.wgsl");
    let occlusion = std::array::from_fn(|multisampled| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("sun occlusion".into()),
            layout: vec![layout[multisampled].clone()],
            shader: shader.clone(),
            shader_defs: if multisampled == 1 {
                vec!["MULTISAMPLED".into()]
            } else {
                vec![]
            },
            entry_point: Some("occlusion".into()),
            ..default()
        })
    });
    let repeat = device.create_sampler(&SamplerDescriptor {
        label: Some("sun occlusion"),
        address_mode_u: AddressMode::Repeat,
        address_mode_v: AddressMode::Repeat,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(Pipelines {
        layout,
        occlusion,
        repeat,
    });
}

#[allow(clippy::too_many_arguments)] // Independent render-world resources.
fn draw(
    view: ViewQuery<(
        &SunState,
        &ViewDepthTexture,
        &ViewUniformOffset,
        &ViewLightsUniformOffset,
        &ViewShadowBindings,
        &Msaa,
    )>,
    assets: Option<Res<CloudAssets>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    images: Res<RenderAssets<GpuImage>>,
    pipelines: Res<Pipelines>,
    cache: Res<PipelineCache>,
    uniforms: Res<ViewUniforms>,
    lights: Res<LightMeta>,
    shadow_samplers: Res<ShadowSamplers>,
    mut ctx: RenderContext,
) {
    let (state, depth, offset, light_offset, shadows, msaa) = view.into_inner();
    let index = usize::from(msaa.samples() > 1);
    let (Some(assets), Some(pipeline), Some(view_binding), Some(light_binding)) = (
        assets,
        cache.get_compute_pipeline(pipelines.occlusion[index]),
        uniforms.uniforms.binding(),
        lights.view_gpu_lights.binding(),
    ) else {
        return;
    };
    let (Some(clouds), Some(cloud_shadow)) =
        (buffers.get(&assets.parameters), images.get(&assets.shadows))
    else {
        return;
    };
    let group = ctx.render_device().create_bind_group(
        "sun occlusion",
        &cache.get_bind_group_layout(&pipelines.layout[index]),
        &BindGroupEntries::sequential((
            view_binding,
            clouds.buffer.as_entire_binding(),
            depth.view(),
            light_binding,
            &shadows.directional_light_depth_texture_view,
            &shadow_samplers.directional_light_comparison_sampler,
            &cloud_shadow.texture_view,
            &pipelines.repeat,
            state.0.as_entire_binding(),
        )),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("sun occlusion"),
            timestamp_writes: None,
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &group, &[offset.offset, light_offset.offset]);
    pass.dispatch_workgroups(1, 1, 1);
}
