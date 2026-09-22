//! Optional GPU counter readback. iOS deliberately omits staging allocation and mapping.
use super::{
    buffers::VegetationBuffers,
    gpu_types::{DRAW_ARGS_SIZE, GPU_TELEMETRY_SIZE},
};
#[cfg(not(target_os = "ios"))]
use super::{
    gpu_types::{
        DRAW_INDEXED_ARGS_WORD_COUNT, GPU_TELEMETRY_WORD_COUNT, TELEMETRY_READBACK_SIZE,
        WORKGROUP_SIZE,
    },
    topology::{
        SINGLE_HIGH_INDEX_COUNT, SINGLE_LOW_INDEX_COUNT, SPLIT_HIGH_INDEX_COUNT,
        SPLIT_LOW_INDEX_COUNT,
    },
};
#[cfg(not(target_os = "ios"))]
use crate::{VegetationDebugSettings, VegetationDiagnostics, VegetationSceneState};
#[cfg(not(target_os = "ios"))]
use bevy::render::{
    render_resource::{BufferDescriptor, BufferUsages, MapMode},
    renderer::RenderDevice,
};
use bevy::{
    prelude::*,
    render::{render_resource::Buffer, renderer::RenderContext},
};
#[cfg(not(target_os = "ios"))]
use std::mem::size_of;

#[cfg(not(target_os = "ios"))]
const TELEMETRY_CAPTURE_INTERVAL_FRAMES: u32 = 30;

#[derive(Resource)]
pub(super) struct VegetationTelemetryStaging {
    buffer: Option<Buffer>,
    scene_revision: u64,
    #[cfg(not(target_os = "ios"))]
    frames_until_capture: u32,
}

impl Default for VegetationTelemetryStaging {
    fn default() -> Self {
        Self {
            buffer: None,
            scene_revision: 0,
            #[cfg(not(target_os = "ios"))]
            frames_until_capture: 0,
        }
    }
}

#[cfg(not(target_os = "ios"))]
pub(super) fn prepare_telemetry_staging(
    render_device: Res<RenderDevice>,
    settings: Res<VegetationDebugSettings>,
    scene: Option<Res<VegetationSceneState>>,
    mut staging: ResMut<VegetationTelemetryStaging>,
) {
    if !settings.gpu_counters_enabled {
        staging.frames_until_capture = 0;
        return;
    }
    if staging.buffer.is_some() {
        return;
    }
    if staging.frames_until_capture > 0 {
        staging.frames_until_capture -= 1;
        return;
    }
    staging.buffer = Some(render_device.create_buffer(&BufferDescriptor {
        label: Some("vegetation-v2 telemetry readback"),
        size: TELEMETRY_READBACK_SIZE,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }));
    staging.scene_revision = scene.as_ref().map_or(0, |s| s.revision());
    staging.frames_until_capture = TELEMETRY_CAPTURE_INTERVAL_FRAMES;
}

#[cfg(not(target_os = "ios"))]
pub(super) fn begin_telemetry_readback(
    mut staging: ResMut<VegetationTelemetryStaging>,
    diagnostics: Res<VegetationDiagnostics>,
) {
    let Some(buffer) = staging.buffer.take() else {
        return;
    };
    let map_buffer = buffer.clone();
    let scene_revision = staging.scene_revision;
    let diagnostics = diagnostics.clone();
    map_buffer
        .slice(..)
        .map_async(MapMode::Read, move |result| {
            if let Err(error) = result {
                warn!("vegetation-v2 telemetry readback failed: {error}");
                return;
            }

            {
                let mapped = buffer.slice(..).get_mapped_range();
                let words: &[u32] = bytemuck::cast_slice(&mapped);
                let required_words = (TELEMETRY_READBACK_SIZE / size_of::<u32>() as u64) as usize;
                if words.len() >= required_words {
                    let args_offset = GPU_TELEMETRY_WORD_COUNT as usize;
                    let emitted_instances = std::array::from_fn(|bin| {
                        words[args_offset + bin * DRAW_INDEXED_ARGS_WORD_COUNT as usize + 1]
                    });
                    diagnostics.update(|snapshot| {
                        snapshot.prepared_blades = words[11];
                        snapshot.preparation_fallback_blades = words[12];
                        snapshot.scheduled_work_items = words[0];
                        snapshot.dispatched_candidate_lanes = words[10]
                            .saturating_mul(WORKGROUP_SIZE)
                            .saturating_mul(words[0]);
                        snapshot.candidate_evaluations = words[1];
                        snapshot.eligible_instances.copy_from_slice(&words[2..6]);
                        snapshot
                            .capacity_dropped_instances
                            .copy_from_slice(&words[6..10]);
                        snapshot.emitted_instances = emitted_instances;
                        snapshot.submitted_indices = emitted_instances
                            .into_iter()
                            .zip([
                                SINGLE_HIGH_INDEX_COUNT,
                                SINGLE_LOW_INDEX_COUNT,
                                SPLIT_HIGH_INDEX_COUNT,
                                SPLIT_LOW_INDEX_COUNT,
                            ])
                            .map(|(instances, indices)| u64::from(instances) * u64::from(indices))
                            .sum();
                        snapshot.topology_vertex_inputs = emitted_instances
                            .into_iter()
                            .zip([18_u32, 8, 15, 7])
                            .map(|(instances, vertices)| u64::from(instances) * u64::from(vertices))
                            .sum();
                        snapshot.gpu_samples = snapshot.gpu_samples.saturating_add(1);
                        snapshot.gpu_scene_revision = scene_revision;
                    });
                }
            }
            buffer.unmap();
        });
}

pub(super) fn finish_telemetry(
    mut context: RenderContext,
    buffers: Res<VegetationBuffers>,
    staging: Res<VegetationTelemetryStaging>,
) {
    copy_telemetry_to_staging(&mut context, &buffers, &staging);
}

fn copy_telemetry_to_staging(
    render_context: &mut RenderContext,
    buffers: &VegetationBuffers,
    staging: &VegetationTelemetryStaging,
) {
    let Some(staging) = staging.buffer.as_ref() else {
        return;
    };
    render_context.command_encoder().copy_buffer_to_buffer(
        &buffers.gpu_telemetry,
        0,
        staging,
        0,
        GPU_TELEMETRY_SIZE,
    );
    render_context.command_encoder().copy_buffer_to_buffer(
        &buffers.args,
        0,
        staging,
        GPU_TELEMETRY_SIZE,
        DRAW_ARGS_SIZE,
    );
}
