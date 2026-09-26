//! Sky composite: one full-screen pass over the resolved HDR image adds the physical sky and
//! aerial perspective from Bevy's atmosphere tables, the cloud layer and weather fog.
//!
//! Bevy keeps computing the atmosphere tables and still lights surfaces through them; only its
//! sky pass is replaced. That pass drew into the multisampled colour target between the opaque
//! and transparent passes, so the target had to be stored and reloaded every frame, and it
//! shaded each pixel from a single depth sample. This composite runs once on the resolved image
//! and classifies every depth sample, so silhouettes against the sky are exact.
use crate::{
    AtmospherePresentation,
    clouds::{CloudAssets, CloudPipelines, CloudQuality, CloudTarget, CloudView, refresh_clouds},
};
use bevy::{
    camera::MainPassResolutionOverride,
    core_pipeline::{
        Core3d, Core3dSystems, FullscreenShader,
        core_3d::{main_opaque_pass_3d, main_transparent_pass_3d},
    },
    ecs::{
        schedule::{
            InternedSystemSet, IntoSystemSet, NodeId, Schedule, SystemKey, SystemSet,
            SystemWithAccess, graph::Direction,
        },
        system::IntoSystem,
    },
    pbr::{
        ExtractedAtmosphere, GpuAtmosphereSettings, GpuLights, LightMeta, ViewLightsUniformOffset,
        main_transmissive_pass_3d,
        resources::{
            AtmosphereSampler, AtmosphereTextures, AtmosphereTransform, AtmosphereTransforms,
            AtmosphereTransformsOffset, GpuAtmosphere,
        },
    },
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_component::{
            ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
        },
        extract_resource::ExtractResourcePlugin,
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, ViewQuery},
        storage::GpuShaderBuffer,
        texture::FallbackImage,
        view::{ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
};

/// Views that get the sky composite. Required by `WorldEnvironmentView`.
#[derive(Component, Clone, Copy, Default, ExtractComponent)]
pub struct SkyCompositeView;

/// Render-world marker: Bevy's sky pass was found and replaced, so nothing but its opaque pass
/// writes the multisampled colour target on atmosphere views.
#[derive(Resource)]
pub struct BevySkyPassReplaced;

pub(crate) struct SkyCompositePlugin;
impl Plugin for SkyCompositePlugin {
    fn build(&self, app: &mut App) {
        if app.get_sub_app(RenderApp).is_none() {
            return;
        }
        app.add_plugins((
            ExtractComponentPlugin::<SkyCompositeView>::default(),
            ExtractResourcePlugin::<AtmospherePresentation>::default(),
        ));
        let render = app.sub_app_mut(RenderApp);
        render
            .init_resource::<SpecializedRenderPipelines<SkyPipelines>>()
            .add_systems(RenderStartup, init)
            .add_systems(Render, queue.in_set(RenderSystems::Queue))
            .add_systems(
                Core3d,
                draw.after(refresh_clouds)
                    .after(Core3dSystems::MainPass)
                    .before(Core3dSystems::EarlyPostProcess),
            );
    }

    fn finish(&self, app: &mut App) {
        let Some(render) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        let world = render.world_mut();
        let mut schedules = world.resource_mut::<Schedules>();
        let schedule = schedules
            .get_mut(Core3d)
            .expect("Core3d plugin is required");
        if replace_bevy_sky_pass(schedule) {
            world.insert_resource(BevySkyPassReplaced);
        } else {
            // Bevy leaves the atmosphere out when the GPU lacks compute or float storage.
            warn!("Bevy's atmosphere sky pass is not installed; the sky composite draws alone");
        }
    }
}

/// Swap Bevy's sky pass for a no-op in its existing graph slot, so ordering edges that other
/// plugins attached to it stay valid. The pass is private to `bevy_pbr`, so it is found by its
/// ordering: the only system whose sole constraints are "after the opaque pass" and "before the
/// transparent pass", besides the public transmissive pass. Like `MsaaColorStorePlugin`, this
/// adapter is specific to Bevy 0.19 and fails loudly if the graph changes shape. Returns
/// whether it was found.
fn replace_bevy_sky_pass(schedule: &mut Schedule) -> bool {
    assert!(!schedule.graph().systems.is_initialized());
    let matches = bevy_sky_pass_candidates(schedule);
    assert!(
        matches.len() <= 1,
        "expected at most one Bevy sky pass between the opaque and transparent passes"
    );
    let Some(&key) = matches.first() else {
        return false;
    };
    *schedule.graph_mut().systems.get_mut(key).unwrap() =
        SystemWithAccess::new(Box::new(IntoSystem::into_system(skip_bevy_sky)));
    true
}

fn bevy_sky_pass_candidates(schedule: &Schedule) -> Vec<SystemKey> {
    let graph = schedule.graph();
    let set = |set: InternedSystemSet| graph.system_sets.get_key(set).map(NodeId::Set);
    let (Some(opaque), Some(transparent)) = (
        set(main_opaque_pass_3d.into_system_set().intern()),
        set(main_transparent_pass_3d.into_system_set().intern()),
    ) else {
        return Vec::new();
    };
    let transmissive = IntoSystem::into_system(main_transmissive_pass_3d).system_type();
    let only = |node, direction, expected| {
        let mut neighbors = graph.dependency().neighbors_directed(node, direction);
        neighbors.next() == Some(expected) && neighbors.next().is_none()
    };
    graph
        .systems
        .iter()
        .filter(|(key, system, _)| {
            let node = NodeId::System(*key);
            system.system_type() != transmissive
                && only(node, Direction::Incoming, opaque)
                && only(node, Direction::Outgoing, transparent)
        })
        .map(|(key, _, _)| key)
        .collect()
}

fn skip_bevy_sky() {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Clouds {
    None,
    /// High quality: a screen-space image traced every frame.
    PerFrame,
    /// Balanced quality: the cross-faded panorama cache.
    Cached,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SkyKey {
    multisampled: bool,
    atmosphere: bool,
    clouds: Clouds,
}

#[derive(Resource)]
struct SkyPipelines {
    /// Group 0 with Bevy's atmosphere bindings, or with the view alone.
    atmosphere_layout: BindGroupLayoutDescriptor,
    view_layout: BindGroupLayoutDescriptor,
    /// Group 1, single-sample and multisampled depth.
    composite_layout: [BindGroupLayoutDescriptor; 2],
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
    dual_source_blending: bool,
}

impl SpecializedRenderPipeline for SkyPipelines {
    type Key = SkyKey;

    fn specialize(&self, key: SkyKey) -> RenderPipelineDescriptor {
        let mut shader_defs = Vec::new();
        if key.multisampled {
            shader_defs.push("MULTISAMPLED".into());
        }
        if key.atmosphere {
            shader_defs.push("ATMOSPHERE".into());
        }
        if key.clouds != Clouds::None {
            shader_defs.push("CLOUDS".into());
        }
        if key.clouds == Clouds::Cached {
            shader_defs.push("CACHED_CLOUDS".into());
        }
        if self.dual_source_blending {
            shader_defs.push("DUAL_SOURCE_BLENDING".into());
        }
        RenderPipelineDescriptor {
            label: Some("sky composite".into()),
            layout: vec![
                if key.atmosphere {
                    self.atmosphere_layout.clone()
                } else {
                    self.view_layout.clone()
                },
                self.composite_layout[usize::from(key.multisampled)].clone(),
            ],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs,
                targets: vec![Some(ColorTargetState {
                    format: TextureFormat::Rgba16Float,
                    // Added light plus the scene times transmittance, per channel when the GPU
                    // blends with a second source.
                    blend: Some(BlendState {
                        color: BlendComponent {
                            src_factor: BlendFactor::One,
                            dst_factor: if self.dual_source_blending {
                                BlendFactor::Src1
                            } else {
                                BlendFactor::SrcAlpha
                            },
                            operation: BlendOperation::Add,
                        },
                        alpha: BlendComponent {
                            src_factor: BlendFactor::Zero,
                            dst_factor: BlendFactor::One,
                            operation: BlendOperation::Add,
                        },
                    }),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}

fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    server: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
) {
    let atmosphere_layout = BindGroupLayoutDescriptor::new(
        "sky composite atmosphere",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (0, uniform_buffer::<GpuAtmosphere>(true)),
                (1, uniform_buffer::<GpuAtmosphereSettings>(true)),
                (2, uniform_buffer::<AtmosphereTransform>(true)),
                (3, uniform_buffer::<ViewUniform>(true)),
                (4, uniform_buffer::<GpuLights>(true)),
                (8, texture_2d(TextureSampleType::Float { filterable: true })),
                (
                    10,
                    texture_2d(TextureSampleType::Float { filterable: true }),
                ),
                (
                    11,
                    texture_3d(TextureSampleType::Float { filterable: true }),
                ),
                (12, sampler(SamplerBindingType::Filtering)),
            ),
        ),
    );
    let view_layout = BindGroupLayoutDescriptor::new(
        "sky composite view",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            ((3, uniform_buffer::<ViewUniform>(true)),),
        ),
    );
    let composite_layout = std::array::from_fn(|multisampled| {
        BindGroupLayoutDescriptor::new(
            "sky composite",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    if multisampled == 1 {
                        texture_depth_2d_multisampled()
                    } else {
                        texture_depth_2d()
                    },
                    storage_buffer_read_only_sized(
                        false,
                        std::num::NonZeroU64::new(
                            std::mem::size_of::<crate::clouds::CloudParams>() as u64,
                        ),
                    ),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    uniform_buffer_sized(false, std::num::NonZeroU64::new(16)),
                ),
            ),
        )
    });
    commands.insert_resource(SkyPipelines {
        atmosphere_layout,
        view_layout,
        composite_layout,
        fullscreen: fullscreen.clone(),
        shader: server.load("shaders/sky/composite.wgsl"),
        dual_source_blending: device
            .features()
            .contains(WgpuFeatures::DUAL_SOURCE_BLENDING),
    });
}

#[derive(Component)]
struct SkyPipeline {
    id: CachedRenderPipelineId,
    key: SkyKey,
}

/// View, sample count, whether Bevy prepares atmosphere tables for it, and whether it has clouds.
type QueuedView = (
    Entity,
    &'static Msaa,
    Has<ExtractedAtmosphere>,
    Has<CloudView>,
);

fn queue(
    mut commands: Commands,
    views: Query<QueuedView, With<SkyCompositeView>>,
    presentation: Option<Res<AtmospherePresentation>>,
    quality: Res<CloudQuality>,
    pipelines: Option<Res<SkyPipelines>>,
    mut specialized: ResMut<SpecializedRenderPipelines<SkyPipelines>>,
    cache: Res<PipelineCache>,
) {
    let Some(pipelines) = pipelines else {
        return;
    };
    let sky = presentation.is_none_or(|p| p.sky_and_haze);
    for (entity, msaa, atmosphere, clouds) in &views {
        let key = SkyKey {
            multisampled: msaa.samples() > 1,
            atmosphere: atmosphere && sky,
            clouds: match (clouds, *quality) {
                (false, _) | (true, CloudQuality::Off) => Clouds::None,
                (true, CloudQuality::Balanced) => Clouds::Cached,
                (true, CloudQuality::High) => Clouds::PerFrame,
            },
        };
        let id = specialized.specialize(&cache, &pipelines, key);
        commands.entity(entity).insert(SkyPipeline { id, key });
    }
}

type AtmosphereBindings = (
    &'static AtmosphereTextures,
    &'static DynamicUniformIndex<GpuAtmosphere>,
    &'static DynamicUniformIndex<GpuAtmosphereSettings>,
    &'static AtmosphereTransformsOffset,
    &'static ViewLightsUniformOffset,
);

/// Atmosphere uniform buffers shared by every view; absent until Bevy prepares them.
#[derive(bevy::ecs::system::SystemParam)]
struct AtmosphereBuffers<'w> {
    atmosphere: Option<Res<'w, ComponentUniforms<GpuAtmosphere>>>,
    settings: Option<Res<'w, ComponentUniforms<GpuAtmosphereSettings>>>,
    transforms: Option<Res<'w, AtmosphereTransforms>>,
    lights: Res<'w, LightMeta>,
    sampler: Option<Res<'w, AtmosphereSampler>>,
}

type CompositeView = (
    &'static SkyPipeline,
    &'static ViewTarget,
    &'static ViewDepthTexture,
    &'static ViewUniformOffset,
    Option<&'static MainPassResolutionOverride>,
    Option<AtmosphereBindings>,
);

#[allow(clippy::too_many_arguments)] // One pass: view, atmosphere, cloud and fallback inputs.
fn draw(
    view: ViewQuery<CompositeView>,
    pipelines: Res<SkyPipelines>,
    cache: Res<PipelineCache>,
    uniforms: Res<ViewUniforms>,
    atmosphere: AtmosphereBuffers,
    assets: Option<Res<CloudAssets>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    clouds: Res<CloudTarget>,
    cloud_pipelines: Res<CloudPipelines>,
    fallback: Res<FallbackImage>,
    mut ctx: RenderContext,
) {
    let (pipeline, target, depth, view_offset, resolution, bindings) = view.into_inner();
    if target.main_texture_format() != TextureFormat::Rgba16Float {
        return;
    }
    let (Some(render), Some(view_binding), Some(assets)) = (
        cache.get_render_pipeline(pipeline.id),
        uniforms.uniforms.binding(),
        assets,
    ) else {
        return;
    };
    let Some(parameters) = buffers.get(&assets.parameters) else {
        return;
    };
    let key = pipeline.key;
    let device = ctx.render_device().clone();
    let (view_group, offsets) = if key.atmosphere {
        let (
            Some((textures, atmosphere_index, settings_index, transforms_offset, lights_offset)),
            Some(atmosphere_binding),
            Some(settings_binding),
            Some(transforms_binding),
            Some(lights_binding),
            Some(sampler),
        ) = (
            bindings,
            atmosphere.atmosphere.as_ref().and_then(|u| u.binding()),
            atmosphere.settings.as_ref().and_then(|u| u.binding()),
            atmosphere
                .transforms
                .as_ref()
                .and_then(|t| t.uniforms().binding()),
            atmosphere.lights.view_gpu_lights.binding(),
            atmosphere.sampler.as_ref(),
        )
        else {
            return;
        };
        let group = device.create_bind_group(
            "sky composite atmosphere",
            &cache.get_bind_group_layout(&pipelines.atmosphere_layout),
            &BindGroupEntries::with_indices((
                (0, atmosphere_binding),
                (1, settings_binding),
                (2, transforms_binding),
                (3, view_binding),
                (4, lights_binding),
                (8, &textures.transmittance_lut.default_view),
                (10, &textures.sky_view_lut.default_view),
                (11, &textures.aerial_view_lut.default_view),
                (12, &***sampler),
            )),
        );
        let offsets = vec![
            atmosphere_index.index(),
            settings_index.index(),
            transforms_offset.index(),
            view_offset.offset,
            lights_offset.offset,
        ];
        (group, offsets)
    } else {
        let group = device.create_bind_group(
            "sky composite view",
            &cache.get_bind_group_layout(&pipelines.view_layout),
            &BindGroupEntries::with_indices(((3, view_binding),)),
        );
        (group, vec![view_offset.offset])
    };
    let cached = key.clouds == Clouds::Cached;
    let (newer, older, blend) = match clouds.display() {
        Some(display) if key.clouds != Clouds::None => display,
        _ => (&fallback.d2.texture_view, &fallback.d2.texture_view, 0.0),
    };
    let blend = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("sky composite cloud blend"),
        contents: bytemuck::bytes_of(&[blend, 0.0, 0.0, 0.0]),
        usage: BufferUsages::UNIFORM,
    });
    let composite_group = device.create_bind_group(
        "sky composite",
        &cache.get_bind_group_layout(&pipelines.composite_layout[usize::from(key.multisampled)]),
        &BindGroupEntries::sequential((
            depth.view(),
            parameters.buffer.as_entire_binding(),
            newer,
            older,
            cloud_pipelines.display_sampler(cached),
            blend.as_entire_binding(),
        )),
    );
    let size = resolution.map_or(
        UVec2::new(
            target.main_texture().width(),
            target.main_texture().height(),
        ),
        |r| r.0,
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("sky composite"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: target.main_texture_view(),
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Load,
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_viewport(0., 0., size.x as f32, size.y as f32, 0., 1.);
    pass.set_pipeline(render);
    pass.set_bind_group(0, &view_group, &offsets);
    pass.set_bind_group(1, &composite_group, &[]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::schedule::ScheduleLabel;

    #[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
    struct Probe;

    fn private_sky() {}
    fn after_opaque_only() {}
    fn also_before_transmissive() {}

    #[test]
    fn finds_the_one_private_pass_between_opaque_and_transparent() {
        let mut schedule = Schedule::new(Probe);
        schedule.add_systems((
            main_opaque_pass_3d,
            main_transparent_pass_3d.after(main_opaque_pass_3d),
            main_transmissive_pass_3d
                .after(main_opaque_pass_3d)
                .before(main_transparent_pass_3d),
            after_opaque_only.after(main_opaque_pass_3d),
            // Like the vegetation temporal draw: same slot, one more constraint.
            also_before_transmissive
                .after(main_opaque_pass_3d)
                .before(main_transmissive_pass_3d)
                .before(main_transparent_pass_3d),
            private_sky
                .after(main_opaque_pass_3d)
                .before(main_transparent_pass_3d),
        ));
        let system_type = |schedule: &Schedule, key| {
            let (_, system, _) = schedule
                .graph()
                .systems
                .iter()
                .find(|(k, _, _)| *k == key)
                .unwrap();
            system.system_type()
        };
        let candidates = bevy_sky_pass_candidates(&schedule);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            system_type(&schedule, candidates[0]),
            IntoSystem::into_system(private_sky).system_type()
        );
        assert!(replace_bevy_sky_pass(&mut schedule));
        assert_eq!(
            system_type(&schedule, candidates[0]),
            IntoSystem::into_system(skip_bevy_sky).system_type()
        );
        // Ordering edges survive the swap.
        assert_eq!(bevy_sky_pass_candidates(&schedule), candidates);

        let mut without = Schedule::new(Probe);
        without.add_systems((main_opaque_pass_3d, main_transparent_pass_3d));
        assert!(!replace_bevy_sky_pass(&mut without));
    }
}
