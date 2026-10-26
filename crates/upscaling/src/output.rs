//! Fuse the final tone map, the UI and the output copy for temporal views.
//! Uses Bevy's tone-mapping shader, LUTs, grading and dithering unchanged.
use crate::temporal::TemporalView;

use bevy::{
    camera::{CameraOutputMode, CompositingSpace},
    core_pipeline::{
        FullscreenShader,
        schedule::Core3d,
        tonemapping::{self, DebandDither, Tonemapping, TonemappingLuts},
        upscaling::upscaling,
    },
    ecs::{
        query::QueryData,
        schedule::{BoxedCondition, ConditionWithAccess},
        system::SystemParam,
    },
    prelude::*,
    render::{
        GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems,
        camera::ExtractedCamera,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_asset::RenderAssets,
        render_phase::{SortedRenderPhase, ViewSortedRenderPhases},
        render_resource::{binding_types::*, *},
        renderer::{CurrentView, RenderContext, RenderDevice, ViewQuery},
        texture::{FallbackImage, GpuImage},
        view::{
            ExtractedView, ResolvedCompositingSpace, ViewTarget, ViewUniform, ViewUniformOffset,
            ViewUniforms, prepare_view_targets,
        },
    },
    shader::ShaderDefVal,
    ui_render::{TransparentUi, UiCameraView, ui_pass},
};

use std::any::TypeId;

/// Opt into direct output for a temporal camera with no effects after tone mapping: one pass
/// tone-maps the image onto the camera's target and draws the camera's UI over it, so the UI
/// needs no camera of its own. Bloom and HDR effects still run before this pass. The camera
/// must own its full output target; viewports, output blending and non-linear compositing use
/// Bevy's standard path. Remove this before adding LDR effects.
#[derive(Component, Clone, ExtractComponent)]
#[extract_app(bevy::render::RenderApp)]
pub struct DirectTonemapOutput;

/// A view that draws onto its output in this crate's pass. Its UI pipelines target the
/// output's format, so Bevy's UI pass, which draws into the HDR main texture, leaves it alone.
#[derive(Component)]
struct Direct;

/// The view's tone map pipeline, once compiled. Until then Bevy's tone map and copy write the
/// output, and the pass draws only the UI over it.
#[derive(Component)]
struct Ready(CachedRenderPipelineId);

#[derive(Resource)]
struct Pipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    format: TextureFormat,
    tone: Tonemapping,
    dither: DebandDither,
    hue: bool,
    balance: bool,
    sectional: bool,
}

impl SpecializedRenderPipeline for Pipeline {
    type Key = Key;

    fn specialize(&self, k: Key) -> RenderPipelineDescriptor {
        let mut defs = vec![
            ShaderDefVal::UInt("TONEMAPPING_LUT_TEXTURE_BINDING_INDEX".into(), 3),
            ShaderDefVal::UInt("TONEMAPPING_LUT_SAMPLER_BINDING_INDEX".into(), 4),
        ];

        for (enabled, name) in [
            (k.dither == DebandDither::Enabled, "DEBAND_DITHER"),
            (k.hue, "HUE_ROTATE"),
            (k.balance, "WHITE_BALANCE"),
            (k.sectional, "SECTIONAL_COLOR_GRADING"),
        ] {
            if enabled {
                defs.push(name.into());
            }
        }

        defs.push(
            match k.tone {
                Tonemapping::None | Tonemapping::Linear => "TONEMAP_METHOD_LINEAR",
                Tonemapping::Reinhard => "TONEMAP_METHOD_REINHARD",
                Tonemapping::ReinhardLuminance => "TONEMAP_METHOD_REINHARD_LUMINANCE",
                Tonemapping::AcesFitted => "TONEMAP_METHOD_ACES_FITTED",
                Tonemapping::AgX => "TONEMAP_METHOD_AGX",
                Tonemapping::SomewhatBoringDisplayTransform => {
                    "TONEMAP_METHOD_SOMEWHAT_BORING_DISPLAY_TRANSFORM"
                }
                Tonemapping::TonyMcMapface => "TONEMAP_METHOD_TONY_MC_MAPFACE",
                Tonemapping::BlenderFilmic => "TONEMAP_METHOD_BLENDER_FILMIC",
                Tonemapping::KhronosPbrNeutral => "TONEMAP_METHOD_PBR_NEUTRAL",
            }
            .into(),
        );
        RenderPipelineDescriptor {
            label: Some("temporal tone map to output".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: defs,
                targets: vec![Some(ColorTargetState {
                    format: k.format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}

pub(super) fn install(app: &mut App) {
    app.add_plugins(ExtractComponentPlugin::<DirectTonemapOutput>::default());
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render
        .init_gpu_resource::<SpecializedRenderPipelines<Pipeline>>()
        .add_systems(RenderStartup, init)
        .add_systems(
            Render,
            (
                // Before the UI queues its pipelines, which take the format chosen here.
                claim
                    .in_set(RenderSystems::PrepareViews)
                    .after(prepare_view_targets),
                prepare.in_set(RenderSystems::PrepareBindGroups),
            ),
        )
        // After Bevy's copy, which writes the image while the tone map pipeline compiles.
        .add_systems(Core3d, draw.after(upscaling));
}

pub(super) fn finish(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    let mut schedules = render.world_mut().resource_mut::<Schedules>();
    let schedule = schedules
        .get_mut(Core3d)
        .expect("Core3d plugin is required");

    // Add conditions in place: preserve all ordering edges and let the normal
    // startup initialize these systems, including the game's timing adapters.
    // This adapter, like the engine's opaque-pass adapter, targets Bevy 0.20.
    assert!(!schedule.graph().systems.is_initialized());
    let tone_map = IntoSystem::into_system(tonemapping::tonemapping).system_type();
    give_way(
        schedule,
        tone_map,
        Box::new(IntoSystem::into_system(use_builtin)),
    );
    let copy = IntoSystem::into_system(upscaling).system_type();
    give_way(
        schedule,
        copy,
        Box::new(IntoSystem::into_system(use_builtin)),
    );
    let ui = IntoSystem::into_system(ui_pass).system_type();
    give_way(
        schedule,
        ui,
        Box::new(IntoSystem::into_system(draws_own_ui)),
    );
}

/// Runs the one built-in output system of type `system` only while `condition` holds.
fn give_way(schedule: &mut Schedule, system: TypeId, condition: BoxedCondition) {
    let keys: Vec<_> = schedule
        .graph()
        .systems
        .iter()
        .filter(|(_, s, _)| s.system_type() == system)
        .map(|(k, _, _)| k)
        .collect();
    assert_eq!(keys.len(), 1, "expected one built-in output pass");
    schedule
        .graph_mut()
        .systems
        .get_conditions_mut(keys[0])
        .unwrap()
        .push(ConditionWithAccess::new(condition));
}

fn use_builtin(current: Res<CurrentView>, ready: Query<(), With<Ready>>) -> bool {
    !ready.contains(current.0)
}

fn draws_own_ui(current: Res<CurrentView>, direct: Query<(), With<Direct>>) -> bool {
    !direct.contains(current.0)
}

fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
    assets: Res<AssetServer>,
) {
    let lut = tonemapping::get_lut_bind_group_layout_entries();
    let entries = BindGroupLayoutEntries::with_indices(
        ShaderStages::FRAGMENT,
        (
            (0, uniform_buffer::<ViewUniform>(true)),
            (
                1,
                texture_2d(TextureSampleType::Float { filterable: false }),
            ),
            (2, sampler(SamplerBindingType::NonFiltering)),
            (3, lut[0]),
            (4, lut[1]),
        ),
    );
    commands.insert_resource(Pipeline {
        layout: BindGroupLayoutDescriptor::new("temporal tone map inputs", &entries),
        sampler: device.create_sampler(&SamplerDescriptor::default()),
        fullscreen: fullscreen.clone(),
        shader: assets.load("embedded://bevy_core_pipeline/tonemapping/tonemapping_frag.wesl"),
    });
}

/// What decides whether a view can draw onto its output directly.
#[derive(QueryData)]
struct Candidate {
    entity: Entity,
    camera: &'static ExtractedCamera,
    target: &'static ViewTarget,
    ui: Option<&'static UiCameraView>,
    temporal: Has<TemporalView>,
    direct: Has<DirectTonemapOutput>,
    tone: Option<&'static Tonemapping>,
    space: Option<&'static ResolvedCompositingSpace>,
}

impl CandidateItem<'_, '_> {
    /// The output's format, when nothing after the tone map needs the main texture.
    fn output_format(&self) -> Option<TextureFormat> {
        let supported = self.direct
            && self.temporal
            && self.camera.hdr
            && self.camera.viewport.is_none()
            && matches!(
                self.camera.output_mode,
                CameraOutputMode::Write {
                    blend_state: None,
                    ..
                }
            )
            && matches!(
                ResolvedCompositingSpace::space(self.space),
                None | Some(CompositingSpace::Linear)
            )
            && self.tone.is_some_and(Tonemapping::is_enabled);
        self.target.out_texture_view_format().filter(|_| supported)
    }
}

fn claim(mut commands: Commands, views: Query<Candidate>, mut ui_views: Query<&mut ExtractedView>) {
    for view in &views {
        let Some(format) = view.output_format() else {
            commands.entity(view.entity).remove::<(Direct, Ready)>();
            continue;
        };
        commands.entity(view.entity).insert(Direct);

        if let Some(ui) = view.ui
            && let Ok(mut ui_view) = ui_views.get_mut(ui.0)
        {
            ui_view.target_format = format;
        }
    }
}

fn prepare(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    pipeline: Res<Pipeline>,
    mut pipelines: ResMut<SpecializedRenderPipelines<Pipeline>>,
    views: Query<ToneMapped, With<Direct>>,
) {
    for view in &views {
        let Some(format) = view.target.out_texture_view_format() else {
            continue;
        };
        let grading = &view.view.color_grading;
        let key = Key {
            format,
            tone: *view.tone,
            dither: *view.dither.unwrap_or(&DebandDither::Disabled),
            hue: grading.global.hue != 0.,
            balance: grading.global.temperature != 0. || grading.global.tint != 0.,
            sectional: grading.all_sections().any(|section| *section != default()),
        };
        let id = pipelines.specialize(&cache, &pipeline, key);

        if cache.get_render_pipeline(id).is_some() {
            commands.entity(view.entity).insert(Ready(id));
        } else {
            commands.entity(view.entity).remove::<Ready>();
        }
    }
}

/// What the tone map pipeline is specialised by.
#[derive(QueryData)]
struct ToneMapped {
    entity: Entity,
    view: &'static ExtractedView,
    target: &'static ViewTarget,
    tone: &'static Tonemapping,
    dither: Option<&'static DebandDither>,
}

#[derive(Default)]
struct Bindings {
    // Temporal reconstruction alternates the HDR source every frame. Retain
    // both bindings, with a fixed bound across resizes/LUT changes/view switches.
    cached: Vec<((BufferId, TextureViewId, TextureViewId), BindGroup)>,
}

impl Bindings {
    fn get(
        &mut self,
        key: (BufferId, TextureViewId, TextureViewId),
        create: impl FnOnce() -> BindGroup,
    ) -> &BindGroup {
        let index = match self.cached.iter().position(|(k, _)| *k == key) {
            Some(index) => index,
            None => {
                if self.cached.len() == 2 {
                    self.cached.remove(0);
                }

                self.cached.push((key, create()));
                self.cached.len() - 1
            }
        };
        &self.cached[index].1
    }
}

/// What the tone map reads besides the view.
#[derive(SystemParam)]
struct ToneMapInputs<'w> {
    cache: Res<'w, PipelineCache>,
    pipeline: Res<'w, Pipeline>,
    uniforms: Res<'w, ViewUniforms>,
    images: Res<'w, RenderAssets<GpuImage>>,
    luts: Res<'w, TonemappingLuts>,
    fallback: Res<'w, FallbackImage>,
}

impl ToneMapInputs<'_> {
    /// The view's tone map pipeline and its bind group, once both exist.
    fn bind<'a>(
        &'a self,
        ready: &Ready,
        target: &ViewTarget,
        tone: &Tonemapping,
        bindings: &'a mut Bindings,
        device: &RenderDevice,
    ) -> Option<(&'a RenderPipeline, &'a BindGroup)> {
        let pipeline = self.cache.get_render_pipeline(ready.0)?;
        let buffer = self.uniforms.uniforms.buffer()?;
        let source = target.main_texture_view();
        let (lut, lut_sampler) =
            tonemapping::get_lut_bindings(&self.images, &self.luts, tone, &self.fallback);
        let group = bindings.get((buffer.id(), source.id(), lut.id()), || {
            device.create_bind_group(
                Some("temporal tone map inputs"),
                &self.cache.get_bind_group_layout(&self.pipeline.layout),
                &BindGroupEntries::sequential((
                    &self.uniforms.uniforms,
                    source,
                    &self.pipeline.sampler,
                    lut,
                    lut_sampler,
                )),
            )
        });
        Some((pipeline, group))
    }
}

/// The UI views and what they draw.
#[derive(SystemParam)]
struct UiPhases<'w, 's> {
    views: Query<'w, 's, &'static ExtractedView>,
    phases: Res<'w, ViewSortedRenderPhases<TransparentUi>>,
}

impl UiPhases<'_, '_> {
    /// The UI view's phase, when it has anything to draw.
    fn of(&self, ui: &UiCameraView) -> Option<(Entity, &SortedRenderPhase<TransparentUi>)> {
        let view = self.views.get(ui.0).ok()?;
        let phase = self.phases.get(&view.retained_view_entity)?;
        (!phase.items.is_empty()).then_some((ui.0, phase))
    }
}

/// What the output pass draws of its view.
#[derive(QueryData)]
struct Output {
    ready: Option<&'static Ready>,
    target: &'static ViewTarget,
    offset: &'static ViewUniformOffset,
    tone: &'static Tonemapping,
    ui: Option<&'static UiCameraView>,
}

fn draw(
    world: &World,
    view: ViewQuery<Output, With<Direct>>,
    inputs: ToneMapInputs,
    ui_phases: UiPhases,
    mut bindings: Local<Bindings>,
    mut ctx: RenderContext,
) {
    let view = view.into_inner();
    let image = view.ready.and_then(|ready| {
        inputs.bind(
            ready,
            view.target,
            view.tone,
            &mut bindings,
            ctx.render_device(),
        )
    });
    let ui = view.ui.and_then(|ui| ui_phases.of(ui));

    if image.is_none() && ui.is_none() {
        return;
    }

    // The tone map overwrites the whole output, and a clear permits a tile GPU to skip loading
    // last frame's image. Without it Bevy's copy has written the image, and the UI loads it.
    let clear = image.is_some().then_some(LinearRgba::BLACK);
    let Some(attachment) = view.target.out_texture_color_attachment(clear) else {
        return;
    };
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("tone map and UI to output"),
        color_attachments: &[Some(attachment)],
        ..default()
    });

    if let Some((pipeline, group)) = image {
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(0, group, &[view.offset.offset]);
        pass.draw(0..3, 0..1);
    }

    if let Some((ui_view, phase)) = ui
        && let Err(error) = phase.render(&mut pass, world, ui_view)
    {
        error!("UI on the output: {error:?}");
    }
}

#[cfg(test)]
mod tests;
