//! Grass motion under MetalFX Temporal, which the G-buffer pass writes (`deferred.rs`): the pose
//! pass bends every blade twice, with this frame's camera and wind and with last frame's
//! (`pose.rs`); this module decides what last frame was.
use super::{
    buffers::VegetationBuffers,
    gpu_types::{CameraGpu, TemporalGrassGpu},
};
use crate::{VegetationSceneState, VegetationView};

use bevy::{
    prelude::*,
    render::{
        Render, RenderSystems,
        render_resource::{
            BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
            PipelineCache, ShaderStages, binding_types::uniform_buffer_sized,
        },
        renderer::{RenderDevice, RenderQueue},
    },
};

use upscaling::temporal::TemporalFrame;

#[derive(Resource, Default)]
pub(super) struct State {
    group: Option<BindGroup>,
    previous: Option<CameraGpu>,
    catalog_revision: u64,
    was_active: bool,
}

impl State {
    /// Last frame's camera and this frame's raster transform, for the G-buffer pass.
    pub(super) fn group(&self) -> Option<&BindGroup> {
        self.group.as_ref()
    }
}

pub(super) fn layout() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "grass temporal data",
        &BindGroupLayoutEntries::single(ShaderStages::VERTEX, uniform_buffer_sized(false, None)),
    )
}

pub(super) fn install(app: &mut SubApp) {
    app.init_resource::<State>().add_systems(
        Render,
        prepare
            .after(super::prepare::prepare)
            .in_set(RenderSystems::PrepareBindGroups),
    );
}

#[allow(clippy::too_many_arguments)] // Independent render-world resources.
fn prepare(
    mut state: ResMut<State>,
    buffers: Res<VegetationBuffers>,
    queue: Res<RenderQueue>,
    mut frames: Query<&mut TemporalFrame, With<VegetationView>>,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    scene: Option<Res<VegetationSceneState>>,
    diagnostics: Option<Res<upscaling::UpscalingDiagnostics>>,
) {
    let (Some(camera), Ok(mut frame)) = (buffers.camera_values, frames.single_mut()) else {
        state.previous = None;
        state.was_active = false;
        state.group = None;
        return;
    };

    if state.group.is_none() {
        state.group = Some(device.create_bind_group(
            "grass temporal data",
            &cache.get_bind_group_layout(&layout()),
            &BindGroupEntries::single(buffers.temporal_grass.as_entire_binding()),
        ));
    }

    let catalog_revision = scene.as_ref().map_or(0, |s| s.catalog_revision());
    let catalog_changed = buffers.active && state.catalog_revision != catalog_revision;
    let active_changed = state.was_active != buffers.active;
    let pose_discontinuity = state.was_active
        && buffers.active
        && state.previous.is_some_and(|p| {
            p.render_origin != camera.render_origin || (p.wind[3] - camera.wind[3]).abs() > 0.25
        });
    let reset = frame.reset || catalog_changed || pose_discontinuity;

    // Residency repacks instance indices, not blade identity: both poses come from the same
    // root and seed, so page loads need no reset.
    if diagnostics.as_ref().is_some_and(|d| d.temporal_timing)
        && (active_changed || catalog_changed || pose_discontinuity)
    {
        info!(
            "GRASS_TEMPORAL_HISTORY source_revision={} catalog_changed={catalog_changed} active_changed={active_changed} pose_discontinuity={pose_discontinuity} reset={reset}",
            buffers.uploaded_revision
        );
    }

    frame.reset |= reset;
    let mut previous = if reset || active_changed {
        camera
    } else {
        state.previous.unwrap_or(camera)
    };

    if active_changed && !reset {
        // Newly enabled grass has no old deformation, but still has camera motion.
        previous.clip_from_world = frame.previous_clip_from_world.to_cols_array();
    }

    queue.write_buffer(
        &buffers.temporal_grass,
        0,
        bytemuck::bytes_of(&TemporalGrassGpu {
            previous,
            raster_clip: frame.raster_clip_from_world.to_cols_array(),
        }),
    );
    state.previous = Some(camera);

    // An edit while disabled must still invalidate when the new catalog is first drawn.
    if buffers.active {
        state.catalog_revision = catalog_revision;
    }

    state.was_active = buffers.active;
}
