//! Blade poses: every emitted blade bent by trampling and the wind, this frame and last, once per
//! blade before the draw reads them (`shaders/vegetation/pose.wesl`). The wind moves each frame,
//! so the pass runs every frame, also while placement is reused.
use super::{buffers::VegetationBuffers, pipelines::VegetationPipelines};

use crate::{VegetationProfileMode, VegetationSettings};

use bevy::{
    prelude::*,
    render::{
        render_resource::{ComputePassDescriptor, PipelineCache},
        renderer::RenderContext,
    },
};

pub(super) fn pose(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<VegetationPipelines>,
    buffers: Res<VegetationBuffers>,
    settings: Res<VegetationSettings>,
) {
    // Only drawn blades need poses.
    let drawn = matches!(
        settings.profile_mode,
        VegetationProfileMode::Full | VegetationProfileMode::DrawFrozen
    );
    let Some(pipeline) = pipeline_cache.get_compute_pipeline(pipelines.pose) else {
        return;
    };

    if !buffers.active || !drawn {
        return;
    }

    let mut pass = render_context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("vegetation blade poses"),
            ..default()
        });
    pass.set_bind_group(0, &buffers.pose_bind_group, &[]);
    pass.set_pipeline(pipeline);
    pass.dispatch_workgroups_indirect(&buffers.pose_dispatch, 0);
}
