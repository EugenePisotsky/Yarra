//! Light shafts: sunbeams through the air near the camera. At a quarter of the main-pass
//! resolution, each view ray is marched through the sun's shadow-map cascades over the range they
//! cover. Haze and valley mist in shadow lose the sunlight the sky composite gives them
//! everywhere, and the humid air under crowns, which only this pass draws, scatters sunlight
//! where it reaches and sky light everywhere. The result is blurred along the image and the sky
//! composite blends it in with a depth-aware upsample, so there is no extra full-screen pass.
use crate::{AtmospherePresentation, clouds::CloudAssets, sky::SkyCompositeView};
use bevy::{
    camera::MainPassResolutionOverride,
    core_pipeline::{Core3d, Core3dSystems},
    pbr::{GpuLights, LightMeta, ShadowSamplers, ViewLightsUniformOffset, ViewShadowBindings},
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, ViewQuery},
        storage::GpuShaderBuffer,
        texture::{CachedTexture, GpuImage, TextureCache},
        view::{ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
};

/// Main-pass pixels per shaft texel along each axis.
pub(crate) const SCALE: u32 = 4;
/// Steps along each ray, packed towards the camera where beams are widest on screen.
const STEPS: u32 = 32;
const WORKGROUP: u32 = 8;

/// Per view, this frame's shafts: light added and transmittance of the air under crowns, and
/// the distance each texel was marched to, for the depth-aware blur and upsample.
#[derive(Component)]
pub(crate) struct LightShaftTargets {
    pub(crate) light: CachedTexture,
    scratch: CachedTexture,
    pub(crate) distance: CachedTexture,
    size: UVec2,
}

pub(crate) struct LightShaftsPlugin;
impl Plugin for LightShaftsPlugin {
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

/// Shaft texels for a main pass of `main` pixels.
pub(crate) fn shaft_size(main: UVec2) -> UVec2 {
    UVec2::new(main.x.div_ceil(SCALE), main.y.div_ceil(SCALE)).max(UVec2::ONE)
}

type PreparedView = (
    Entity,
    &'static ViewTarget,
    Option<&'static MainPassResolutionOverride>,
);

fn prepare(
    mut commands: Commands,
    views: Query<PreparedView, With<SkyCompositeView>>,
    presentation: Option<Res<AtmospherePresentation>>,
    mut cache: ResMut<TextureCache>,
    device: Res<RenderDevice>,
) {
    let enabled = presentation.is_none_or(|p| p.light_shafts && p.low_air);
    for (entity, target, resolution) in &views {
        if !enabled {
            commands.entity(entity).remove::<LightShaftTargets>();
            continue;
        }
        let main = resolution.map_or(
            UVec2::new(
                target.main_texture().width(),
                target.main_texture().height(),
            ),
            |r| r.0,
        );
        let size = shaft_size(main);
        let mut texture = |label, format| {
            cache.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size: Extent3d {
                        width: size.x,
                        height: size.y,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format,
                    usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        commands.entity(entity).insert(LightShaftTargets {
            light: texture("light shafts", TextureFormat::Rgba16Float),
            scratch: texture("light shafts blur", TextureFormat::Rgba16Float),
            // Half floats, filterable, so the composite gathers four distances at once.
            distance: texture("light shaft distance", TextureFormat::Rgba16Float),
            size,
        });
    }
}

#[derive(Resource)]
struct Pipelines {
    march_layout: [BindGroupLayoutDescriptor; 2],
    blur_layout: BindGroupLayoutDescriptor,
    /// Single-sample and multisampled depth.
    march: [CachedComputePipelineId; 2],
    /// Horizontal, then vertical.
    blur: [CachedComputePipelineId; 2],
    repeat: Sampler,
    clamp: Sampler,
}

fn init(
    mut commands: Commands,
    server: Res<AssetServer>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
) {
    let march_layout = std::array::from_fn(|multisampled| {
        BindGroupLayoutDescriptor::new(
            "light shaft march",
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
                    // Cloud shadow, forest map, mist map, cloud noise.
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_3d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    sampler(SamplerBindingType::Filtering),
                    texture_storage_2d(TextureFormat::Rgba16Float, StorageTextureAccess::WriteOnly),
                    texture_storage_2d(TextureFormat::Rgba16Float, StorageTextureAccess::WriteOnly),
                ),
            ),
        )
    });
    let blur_layout = BindGroupLayoutDescriptor::new(
        "light shaft blur",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_storage_2d(TextureFormat::Rgba16Float, StorageTextureAccess::WriteOnly),
            ),
        ),
    );
    let shader = server.load("shaders/sky/light_shafts.wgsl");
    let compute = |label: &'static str, layout: &BindGroupLayoutDescriptor, entry, defs| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            shader: shader.clone(),
            shader_defs: defs,
            entry_point: Some(entry),
            ..default()
        })
    };
    let march = std::array::from_fn(|multisampled| {
        let mut defs = vec![
            "MARCH".into(),
            bevy::shader::ShaderDefVal::UInt("SHAFT_SCALE".into(), SCALE),
            bevy::shader::ShaderDefVal::UInt("SHAFT_STEPS".into(), STEPS),
        ];
        if multisampled == 1 {
            defs.push("MULTISAMPLED".into());
        }
        compute(
            "light shaft march",
            &march_layout[multisampled],
            "march".into(),
            defs,
        )
    });
    let blur = [
        compute("light shaft blur", &blur_layout, "blur_x".into(), vec![]),
        compute("light shaft blur", &blur_layout, "blur_y".into(), vec![]),
    ];
    let filtered = |address_mode| {
        device.create_sampler(&SamplerDescriptor {
            label: Some("light shafts"),
            address_mode_u: address_mode,
            address_mode_v: address_mode,
            address_mode_w: address_mode,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            ..default()
        })
    };
    commands.insert_resource(Pipelines {
        march_layout,
        blur_layout,
        march,
        blur,
        repeat: filtered(AddressMode::Repeat),
        clamp: filtered(AddressMode::ClampToEdge),
    });
}

#[allow(clippy::too_many_arguments)] // Independent render-world resources.
fn draw(
    view: ViewQuery<(
        &LightShaftTargets,
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
    let (targets, depth, offset, light_offset, shadows, msaa) = view.into_inner();
    let index = usize::from(msaa.samples() > 1);
    let (Some(assets), Some(march), Some(blur_x), Some(blur_y), Some(view_binding)) = (
        assets,
        cache.get_compute_pipeline(pipelines.march[index]),
        cache.get_compute_pipeline(pipelines.blur[0]),
        cache.get_compute_pipeline(pipelines.blur[1]),
        uniforms.uniforms.binding(),
    ) else {
        return;
    };
    let (Some(light_binding), Some(clouds)) = (
        lights.view_gpu_lights.binding(),
        buffers.get(&assets.parameters),
    ) else {
        return;
    };
    let (Some(cloud_shadow), Some(forest), Some(mist), Some(noise)) = (
        images.get(&assets.shadows),
        images.get(&assets.forest_shadow),
        images.get(&assets.mist),
        images.get(&assets.noise),
    ) else {
        return;
    };
    let device = ctx.render_device().clone();
    let march_group = device.create_bind_group(
        "light shaft march",
        &cache.get_bind_group_layout(&pipelines.march_layout[index]),
        &BindGroupEntries::sequential((
            view_binding,
            clouds.buffer.as_entire_binding(),
            depth.view(),
            light_binding,
            &shadows.directional_light_depth_texture_view,
            &shadow_samplers.directional_light_comparison_sampler,
            &cloud_shadow.texture_view,
            &forest.texture_view,
            &mist.texture_view,
            &noise.texture_view,
            &pipelines.repeat,
            &pipelines.clamp,
            &targets.light.default_view,
            &targets.distance.default_view,
        )),
    );
    let blur_group = |from: &CachedTexture, to: &CachedTexture| {
        device.create_bind_group(
            "light shaft blur",
            &cache.get_bind_group_layout(&pipelines.blur_layout),
            &BindGroupEntries::sequential((
                &from.default_view,
                &targets.distance.default_view,
                &to.default_view,
            )),
        )
    };
    let horizontal = blur_group(&targets.light, &targets.scratch);
    let vertical = blur_group(&targets.scratch, &targets.light);
    let groups = UVec2::new(
        targets.size.x.div_ceil(WORKGROUP),
        targets.size.y.div_ceil(WORKGROUP),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("light shafts"),
            timestamp_writes: None,
        });
    pass.set_pipeline(march);
    pass.set_bind_group(0, &march_group, &[offset.offset, light_offset.offset]);
    pass.dispatch_workgroups(groups.x, groups.y, 1);
    pass.set_pipeline(blur_x);
    pass.set_bind_group(0, &horizontal, &[]);
    pass.dispatch_workgroups(groups.x, groups.y, 1);
    pass.set_pipeline(blur_y);
    pass.set_bind_group(0, &vertical, &[]);
    pass.dispatch_workgroups(groups.x, groups.y, 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaft_texels_cover_the_main_pass() {
        assert_eq!(shaft_size(UVec2::new(3456, 2168)), UVec2::new(864, 542));
        assert_eq!(shaft_size(UVec2::new(1729, 1)), UVec2::new(433, 1));
        assert_eq!(shaft_size(UVec2::ZERO), UVec2::ONE);
    }
}
