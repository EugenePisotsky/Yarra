//! Fuse the final tone map and output copy for temporal views.
//! Uses Bevy's tone-mapping shader, LUTs, grading and dithering unchanged.
use crate::temporal::TemporalView;
use bevy::{
    camera::{CameraOutputMode, CompositingSpace},
    core_pipeline::{
        Core3dSystems, FullscreenShader,
        schedule::Core3d,
        tonemapping::{self, DebandDither, Tonemapping, TonemappingLuts},
        upscaling::upscaling,
    },
    ecs::schedule::ConditionWithAccess,
    prelude::*,
    render::{
        GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems,
        camera::ExtractedCamera,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{CurrentView, RenderContext, RenderDevice, ViewQuery},
        texture::{FallbackImage, GpuImage},
        view::{ExtractedView, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
    shader::ShaderDefVal,
};

/// Opt into direct tone-mapped output for a temporal camera with no effects after
/// tone mapping. Bloom and HDR effects still run before this pass. The camera
/// must own its full output target; viewports, output blending and non-linear
/// compositing use Bevy's standard path. Remove this before adding LDR effects.
#[derive(Component, Clone, ExtractComponent)]
pub struct DirectTonemapOutput;

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
                Tonemapping::None => "TONEMAP_METHOD_NONE",
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
#[derive(Component)]
struct Ready(CachedRenderPipelineId);

pub(super) fn install(app: &mut App) {
    app.add_plugins(ExtractComponentPlugin::<DirectTonemapOutput>::default());
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render
        .init_gpu_resource::<SpecializedRenderPipelines<Pipeline>>()
        .add_systems(RenderStartup, init)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
        .add_systems(
            Core3d,
            draw.after(Core3dSystems::PostProcess).before(upscaling),
        );
}
pub(super) fn finish(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    let mut schedules = render.world_mut().resource_mut::<Schedules>();
    let schedule = schedules
        .get_mut(Core3d)
        .expect("Core3d plugin is required");
    // Add a condition in place: preserve all ordering edges and let the normal
    // startup initialize these systems, including the game's timing adapters.
    // This adapter, like the engine's opaque-pass adapter, targets Bevy 0.19.
    assert!(!schedule.graph().systems.is_initialized());
    for system_type in [
        IntoSystem::into_system(tonemapping::tonemapping).system_type(),
        IntoSystem::into_system(upscaling).system_type(),
    ] {
        let keys: Vec<_> = schedule
            .graph()
            .systems
            .iter()
            .filter(|(_, s, _)| s.system_type() == system_type)
            .map(|(k, _, _)| k)
            .collect();
        assert_eq!(keys.len(), 1, "expected one built-in output pass");
        schedule
            .graph_mut()
            .systems
            .get_conditions_mut(keys[0])
            .unwrap()
            .push(ConditionWithAccess::new(Box::new(IntoSystem::into_system(
                use_builtin,
            ))));
    }
}
fn use_builtin(current: Res<CurrentView>, ready: Query<(), With<Ready>>) -> bool {
    !ready.contains(current.0)
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
        shader: assets.load("embedded://bevy_core_pipeline/tonemapping/tonemapping.wgsl"),
    });
}
#[allow(clippy::type_complexity)]
fn prepare(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    pipeline: Res<Pipeline>,
    mut pipelines: ResMut<SpecializedRenderPipelines<Pipeline>>,
    views: Query<(
        Entity,
        &ExtractedCamera,
        &ExtractedView,
        &ViewTarget,
        Option<&TemporalView>,
        Option<&DirectTonemapOutput>,
        Option<&Tonemapping>,
        Option<&DebandDither>,
    )>,
) {
    for (e, camera, view, target, temporal, direct, tone, dither) in &views {
        let supported = direct.is_some()
            && temporal.is_some()
            && camera.hdr
            && camera.viewport.is_none()
            && matches!(
                camera.output_mode,
                CameraOutputMode::Write {
                    blend_state: None,
                    ..
                }
            )
            && matches!(
                camera.compositing_space,
                None | Some(CompositingSpace::Linear)
            )
            && tone.is_some_and(Tonemapping::is_enabled);
        let Some(format) = target.out_texture_view_format().filter(|_| supported) else {
            commands.entity(e).remove::<Ready>();
            continue;
        };
        let key = Key {
            format,
            tone: *tone.unwrap(),
            dither: *dither.unwrap_or(&DebandDither::Disabled),
            hue: view.color_grading.global.hue != 0.,
            balance: view.color_grading.global.temperature != 0.
                || view.color_grading.global.tint != 0.,
            sectional: view
                .color_grading
                .all_sections()
                .any(|section| *section != default()),
        };
        let id = pipelines.specialize(&cache, &pipeline, key);
        if cache.get_render_pipeline(id).is_some() {
            commands.entity(e).insert(Ready(id));
        } else {
            commands.entity(e).remove::<Ready>();
        }
    }
}
#[derive(Default)]
struct Bindings {
    // Temporal reconstruction alternates the HDR source every frame. Retain
    // both bindings, with a fixed bound across resizes/LUT changes/view switches.
    cached: Vec<(BufferId, TextureViewId, TextureViewId, BindGroup)>,
}
fn draw(
    view: ViewQuery<(&Ready, &ViewTarget, &ViewUniformOffset, &Tonemapping)>,
    cache: Res<PipelineCache>,
    pipeline: Res<Pipeline>,
    uniforms: Res<ViewUniforms>,
    images: Res<RenderAssets<GpuImage>>,
    luts: Res<TonemappingLuts>,
    fallback: Res<FallbackImage>,
    mut bindings: Local<Bindings>,
    mut ctx: RenderContext,
) {
    let (ready, target, offset, tone) = view.into_inner();
    let Some(render_pipeline) = cache.get_render_pipeline(ready.0) else {
        return;
    };
    let Some(buffer) = uniforms.uniforms.buffer() else {
        return;
    };
    // This pass owns the entire target and overwrites it without blending.
    // Clear permits a tile GPU to avoid loading last frame's display image.
    let Some(attachment) = target.out_texture_color_attachment(Some(LinearRgba::BLACK)) else {
        return;
    };
    let source = target.main_texture_view();
    let (lut, lut_sampler) = tonemapping::get_lut_bindings(&images, &luts, tone, &fallback);
    let key = (buffer.id(), source.id(), lut.id());
    let cached_index = bindings
        .cached
        .iter()
        .position(|(b, s, l, _)| (*b, *s, *l) == key);
    let index = if let Some(index) = cached_index {
        index
    } else {
        let group = ctx.render_device().create_bind_group(
            Some("temporal tone map inputs"),
            &cache.get_bind_group_layout(&pipeline.layout),
            &BindGroupEntries::sequential((
                &uniforms.uniforms,
                source,
                &pipeline.sampler,
                lut,
                lut_sampler,
            )),
        );
        if bindings.cached.len() == 2 {
            bindings.cached.remove(0);
        }
        bindings.cached.push((key.0, key.1, key.2, group));
        bindings.cached.len() - 1
    };
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("temporal tone map to output"),
            color_attachments: &[Some(attachment)],
            ..default()
        });
    pass.set_pipeline(render_pipeline);
    pass.set_bind_group(0, &bindings.cached[index].3, &[offset.offset]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests;
