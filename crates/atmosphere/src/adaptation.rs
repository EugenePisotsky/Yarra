//! Eye adaptation in the game: the exposure follows part of a change in the scene's brightness,
//! around the profile's exposure, so forest shade stays readable and open ground in sun calms
//! down while the forest still reads darker than the beach.
//!
//! A pass on the rendered image, before the temporal upscaler (`shaders/post/exposure.wesl`),
//! meters the average log2 luminance of the exposed image (the 10-90% band of its histogram)
//! and eases the exposure by [`FOLLOW`] times the distance from [`REFERENCE`], clamped, into the
//! view's colour grading, where the tone map reads it. Measured at EV 13 with no tonemapping
//! through Bevy's auto exposure, whose metering the pass keeps, the start view's beach meters
//! -2.4, the forest edge -3.2, the pine forest -3.9, the spruce forest -5.5 and the summit,
//! mostly sky, -1.3. The sky counts for less than the land ([`SKY_WEIGHT`]).
use crate::sky::SkyCompositeDraw;

use bevy::{
    camera::{MainPassResolutionOverride, Viewport},
    core_pipeline::{Core3d, Core3dSystems},
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        camera::ExtractedCamera,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        globals::{GlobalsBuffer, GlobalsUniform},
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        view::{ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
};

use bytemuck::{Pod, Zeroable};

use std::num::NonZeroU64;

/// Share of a change in metered brightness the exposure follows; the rest stays visible.
pub(crate) const FOLLOW: f32 = 0.5;

/// Metered log2 exposed luminance left as authored, a stop below a forest edge in sun at EV 13
/// (-3.25), so daylight shows half a stop darker than it meters: the game read too bright in
/// sun and under cloud.
pub(crate) const REFERENCE: f32 = -4.25;

/// The most the exposure brightens (deep shade) and darkens (bright open ground), in stops.
pub(crate) const BRIGHTEN: f32 = 1.5;

pub(crate) const DARKEN: f32 = 1.0;

/// The most it brightens at full night. A moonless night meters far below [`REFERENCE`] (dark
/// grass, a near-black sky; snow is in the top tenth the meter leaves out), so the full day value
/// lifted snow to lavender white; a moonlit night meters brighter and kept snow grey.
pub(crate) const NIGHT_BRIGHTEN: f32 = 0.5;

/// Stops per second towards a brighter scene, which eyes adapt to quickly, and towards a
/// darker one, which takes longer.
const TO_BRIGHT: f32 = 2.0;

const TO_DARK: f32 = 0.75;

/// Metering weight of the top of the frame (mostly sky), rising to 1 at [`LAND_FROM`] of its
/// height.
const SKY_WEIGHT: f32 = 0.3;

const LAND_FROM: f32 = 0.4;

/// The meter's histogram workgroup, 16×16 threads each reading every other pixel each way.
const WORKGROUP_PIXELS: u32 = 32;

/// A view's eye adaptation. A view keeps it once adapting, and stops by following nothing, so the
/// exposure eases back to the profile's.
#[derive(Component, Clone, Copy, PartialEq, ExtractComponent)]
#[extract_app(bevy::render::RenderApp)]
#[require(bevy::camera::Hdr)]
pub struct EyeAdaptation {
    follow: f32,
    brighten: f32,
}

impl EyeAdaptation {
    /// `night` is the atmosphere's adaptation, 0 by day and 1 at night.
    pub(crate) fn new(adapt: bool, night: f32) -> Self {
        Self {
            follow: if adapt { FOLLOW } else { 0.0 },
            brighten: BRIGHTEN + (NIGHT_BRIGHTEN - BRIGHTEN) * night.clamp(0.0, 1.0),
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }

    app.add_plugins(ExtractComponentPlugin::<EyeAdaptation>::default());
    app.sub_app_mut(RenderApp)
        .add_systems(RenderStartup, init)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
        .add_systems(
            Core3d,
            draw.after(SkyCompositeDraw)
                .after(crate::bloom::BloomDraw)
                .before(Core3dSystems::EarlyPostProcess),
        );
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Params {
    origin: [u32; 2],
    size: [u32; 2],
    follow: f32,
    reference: f32,
    darken: f32,
    brighten: f32,
    to_bright: f32,
    to_dark: f32,
    sky_weight: f32,
    land_from: f32,
}

/// The meter's histogram bins and eased exposure, in stops.
const METER_SIZE: u64 = 64 * 4 + 4;

/// Per view: the meter, which keeps the exposure from frame to frame, and this frame's region
/// and settings.
#[derive(Component)]
struct Meter {
    state: Buffer,
    params: Buffer,
    size: UVec2,
}

#[derive(Resource)]
struct Pipelines {
    layout: BindGroupLayoutDescriptor,
    measure: CachedComputePipelineId,
    adapt: CachedComputePipelineId,
}

fn init(mut commands: Commands, server: Res<AssetServer>, cache: Res<PipelineCache>) {
    let layout = BindGroupLayoutDescriptor::new(
        "eye adaptation",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<GlobalsUniform>(false),
                uniform_buffer_sized(false, NonZeroU64::new(size_of::<Params>() as u64)),
                texture_2d(TextureSampleType::Float { filterable: false }),
                storage_buffer_sized(false, NonZeroU64::new(METER_SIZE)),
                storage_buffer::<ViewUniform>(true),
            ),
        ),
    );
    let shader = server.load("shaders/post/exposure.wesl");
    let pipeline = |entry: &'static str| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(format!("eye adaptation {entry}").into()),
            layout: vec![layout.clone()],
            shader: shader.clone(),
            entry_point: Some(entry.into()),
            ..default()
        })
    };
    commands.insert_resource(Pipelines {
        measure: pipeline("measure"),
        adapt: pipeline("adapt"),
        layout,
    });
}

type PreparedView = (
    Entity,
    &'static EyeAdaptation,
    &'static ExtractedCamera,
    &'static ViewTarget,
    Option<&'static MainPassResolutionOverride>,
    Option<&'static Meter>,
);

fn prepare(
    mut commands: Commands,
    views: Query<PreparedView>,
    stale: Query<Entity, (With<Meter>, Without<EyeAdaptation>)>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    for entity in &stale {
        commands.entity(entity).remove::<Meter>();
    }

    for (entity, eye, camera, target, resolution, meter) in &views {
        let viewport = Viewport::from_viewport_and_override(camera.viewport.as_ref(), resolution);
        let region = crate::rendered_region(viewport.as_ref(), target);
        let params = Params {
            origin: region.min.to_array(),
            size: region.size().to_array(),
            follow: eye.follow,
            reference: REFERENCE,
            darken: DARKEN,
            brighten: eye.brighten,
            to_bright: TO_BRIGHT,
            to_dark: TO_DARK,
            sky_weight: SKY_WEIGHT,
            land_from: LAND_FROM,
        };

        if let Some(meter) = meter {
            queue.write_buffer(&meter.params, 0, bytemuck::bytes_of(&params));

            if meter.size != region.size() {
                commands.entity(entity).insert(Meter {
                    size: region.size(),
                    state: meter.state.clone(),
                    params: meter.params.clone(),
                });
            }

            continue;
        }

        // Starts at the profile's exposure, with empty bins.
        let state = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("eye adaptation meter"),
            contents: &[0; METER_SIZE as usize],
            usage: BufferUsages::STORAGE,
        });
        let params = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("eye adaptation"),
            contents: bytemuck::bytes_of(&params),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });
        commands.entity(entity).insert(Meter {
            state,
            params,
            size: region.size(),
        });
    }
}

fn draw(
    view: ViewQuery<(&Meter, &ViewTarget, &ViewUniformOffset)>,
    pipelines: Res<Pipelines>,
    cache: Res<PipelineCache>,
    globals: Res<GlobalsBuffer>,
    uniforms: Res<ViewUniforms>,
    mut ctx: RenderContext,
) {
    let (meter, target, offset) = view.into_inner();
    let (Some(measure), Some(adapt), Some(globals), Some(views)) = (
        cache.get_compute_pipeline(pipelines.measure),
        cache.get_compute_pipeline(pipelines.adapt),
        globals.buffer.binding(),
        uniforms.uniforms.buffer(),
    ) else {
        return;
    };
    let group = ctx.render_device().create_bind_group(
        "eye adaptation",
        &cache.get_bind_group_layout(&pipelines.layout),
        &BindGroupEntries::sequential((
            globals,
            meter.params.as_entire_binding(),
            target.main_texture_view(),
            meter.state.as_entire_binding(),
            BufferBinding {
                buffer: views,
                offset: 0,
                size: Some(ViewUniform::min_size()),
            },
        )),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("eye adaptation"),
            timestamp_writes: None,
        });
    pass.set_bind_group(0, &group, &[offset.offset]);
    pass.set_pipeline(measure);
    pass.dispatch_workgroups(
        meter.size.x.div_ceil(WORKGROUP_PIXELS),
        meter.size.y.div_ceil(WORKGROUP_PIXELS),
        1,
    );
    pass.set_pipeline(adapt);
    pass.dispatch_workgroups(1, 1, 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapting_follows_half_the_change_and_stopping_follows_none() {
        assert_eq!(EyeAdaptation::new(true, 0.0).follow, FOLLOW);
        assert_eq!(EyeAdaptation::new(false, 0.0).follow, 0.0);
    }

    #[test]
    fn night_brightens_less_and_daylight_is_unchanged() {
        assert_eq!(EyeAdaptation::new(true, 0.0).brighten, BRIGHTEN);
        assert_eq!(EyeAdaptation::new(true, 1.0).brighten, NIGHT_BRIGHTEN);
        assert_eq!(EyeAdaptation::new(true, 2.0).brighten, NIGHT_BRIGHTEN);
        let dusk = EyeAdaptation::new(true, 0.5).brighten;
        assert!(NIGHT_BRIGHTEN < dusk && dusk < BRIGHTEN);
    }

    #[test]
    fn the_params_match_the_shader_layout() {
        assert_eq!(size_of::<Params>(), 48);
    }
}
