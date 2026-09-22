//! Placement cache identity and ordered scheduling, generation and indirect-argument finalization.
use super::{
    buffers::VegetationBuffers,
    candidate_cache,
    gpu_types::{CameraGpu, DebugConfigGpu, WORKGROUP_SIZE},
    pipelines::VegetationPipelines,
};
use crate::{VegetationDebugSettings, VegetationDiagnostics, VegetationProfileMode};
use bevy::{
    prelude::*,
    render::{
        diagnostic::RecordDiagnostics,
        render_resource::{ComputePassDescriptor, ComputePipelineId, PipelineCache},
        renderer::RenderContext,
    },
};

#[derive(Clone, Copy, PartialEq)]
pub(super) struct GenerationInputs {
    source_revision: u64,
    camera: CameraGpu,
    pub(super) config: DebugConfigGpu,
}

impl GenerationInputs {
    pub(super) fn new(
        source_revision: u64,
        mut camera: CameraGpu,
        mut config: DebugConfigGpu,
    ) -> Self {
        // Placement uses wind strength for conservative bounds, but wind phase is evaluated
        // only by blade preparation/drawing. Animate existing blades without rebuilding them.
        camera.wind[3] = 0.0;
        camera = camera.geometry_cache_key();
        config.workload[3] &= !(255 << 16);
        Self {
            source_revision,
            camera,
            config,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct GenerationKey {
    cache_serial: u64,
    inputs: GenerationInputs,
    // A hot-reloaded compute shader must regenerate even if its inputs did not change.
    pipelines: [ComputePipelineId; 3],
}

pub(super) fn generate(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<VegetationPipelines>,
    mut buffers: ResMut<VegetationBuffers>,
    settings: Res<VegetationDebugSettings>,
    candidate_cache: Res<candidate_cache::CandidateCache>,
    vegetation_diagnostics: Res<VegetationDiagnostics>,
) {
    // Isolation modes deliberately keep executing their selected workload. Returning to full
    // rendering must also rebuild after ScheduleOnly cleared the draw arguments.
    if settings.profile_mode != VegetationProfileMode::Full {
        buffers.last_generation = None;
    }
    if settings.profile_mode == VegetationProfileMode::Disabled {
        return;
    }
    if settings.profile_mode == VegetationProfileMode::DrawFrozen {
        return;
    }
    let ready_pipelines = (
        pipeline_cache.get_compute_pipeline(pipelines.schedule),
        pipeline_cache.get_compute_pipeline(pipelines.generate),
        pipeline_cache.get_compute_pipeline(pipelines.finalize),
    );
    let generation_key = match ready_pipelines {
        (Some(schedule), Some(generate), Some(finalize)) if buffers.active => {
            buffers.generation_inputs.map(|inputs| GenerationKey {
                inputs,
                cache_serial: candidate_cache.serial,
                pipelines: [schedule.id(), generate.id(), finalize.id()],
            })
        }
        _ => None,
    };
    if settings.profile_mode == VegetationProfileMode::Full
        && generation_key.is_some()
        && buffers.last_generation == generation_key
    {
        vegetation_diagnostics.update(|snapshot| snapshot.generation_reuses += 1);
        return;
    }
    buffers.last_generation = None;
    buffers.generation_serial = buffers.generation_serial.wrapping_add(1);
    render_context
        .command_encoder()
        .clear_buffer(&buffers.args, 0, None);
    render_context
        .command_encoder()
        .clear_buffer(&buffers.candidate_dispatch_args, 0, None);
    render_context
        .command_encoder()
        .clear_buffer(&buffers.gpu_telemetry, 0, None);
    if !buffers.active {
        return;
    }
    let (Some(schedule_pipeline), Some(generate_pipeline), Some(finalize_pipeline)) =
        ready_pipelines
    else {
        return;
    };
    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let mut schedule_pass =
        render_context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("vegetation-v2 visible work scheduling"),
                ..default()
            });
    let schedule_span = diagnostics.pass_span(&mut schedule_pass, "vegetation_v2_schedule");
    schedule_pass.set_bind_group(0, &buffers.schedule_bind_group, &[]);
    schedule_pass.set_pipeline(schedule_pipeline);
    schedule_pass.dispatch_workgroups(buffers.work_item_count.div_ceil(WORKGROUP_SIZE), 1, 1);
    schedule_span.end(&mut schedule_pass);
    drop(schedule_pass);
    if settings.profile_mode == VegetationProfileMode::ScheduleOnly {
        return;
    }

    let mut pass = render_context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("vegetation-v2 deterministic visible generation"),
            ..default()
        });
    let generate_span = diagnostics.pass_span(&mut pass, "vegetation_v2_generate");
    pass.set_bind_group(0, &buffers.compute_bind_group, &[]);
    pass.set_pipeline(generate_pipeline);
    pass.dispatch_workgroups_indirect(&buffers.candidate_dispatch_args, 0);
    generate_span.end(&mut pass);
    let finalize_span = diagnostics.pass_span(&mut pass, "vegetation_v2_finalize");
    pass.set_pipeline(finalize_pipeline);
    pass.dispatch_workgroups(1, 1, 1);
    finalize_span.end(&mut pass);
    drop(pass);
    vegetation_diagnostics.update(|snapshot| snapshot.generation_dispatches += 1);
    if settings.profile_mode == VegetationProfileMode::Full {
        buffers.last_generation = generation_key;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    #[test]
    fn placement_cache_ignores_animation_phase_but_tracks_generation_inputs() {
        let camera = CameraGpu::zeroed();
        let config = DebugConfigGpu::zeroed();
        let baseline = GenerationInputs::new(1, camera, config);
        let mut shaded = camera;
        shaded.canopy = vegetation::CanopyShading::experiment().packed([32.0, -64.0]);
        shaded.sun_direction = [0.2, 0.8, 0.3, 1.0];
        shaded.sun_radiance = [20.0, 18.0, 15.0, 0.0];
        shaded.ambient_radiance = [0.1, 0.2, 0.3, 0.0];
        shaded.lighting = [1.0, 0.5, 0.8, 1.0];
        assert!(baseline == GenerationInputs::new(1, shaded, config));
        let mut animated = camera;
        animated.wind[3] = 12.5;
        assert!(baseline == GenerationInputs::new(1, animated, config));

        let mut material_only = config;
        material_only.workload[3] |= 173 << 16;
        assert!(baseline == GenerationInputs::new(1, camera, material_only));

        let mut moved = camera;
        moved.clip_from_world[12] = 0.01;
        assert!(baseline != GenerationInputs::new(1, moved, config));
        moved = camera;
        moved.camera_position[0] = 0.01;
        assert!(baseline != GenerationInputs::new(1, moved, config));
        let mut resized = camera;
        resized.projection[0] = 720.0;
        assert!(baseline != GenerationInputs::new(1, resized, config));
        let mut stronger_wind = camera;
        stronger_wind.wind[2] = 0.5;
        assert!(baseline != GenerationInputs::new(1, stronger_wind, config));
        assert!(baseline != GenerationInputs::new(2, camera, config));
        let mut changed_config = config;
        changed_config.values[1] = 1;
        assert!(baseline != GenerationInputs::new(1, camera, changed_config));
        changed_config = config;
        changed_config.workload[1] = 1;
        assert!(baseline != GenerationInputs::new(1, camera, changed_config));

        let key = GenerationKey {
            cache_serial: 0,
            inputs: baseline,
            pipelines: [
                ComputePipelineId::new(),
                ComputePipelineId::new(),
                ComputePipelineId::new(),
            ],
        };
        let mut recompiled = key;
        recompiled.pipelines[1] = ComputePipelineId::new();
        assert!(key != recompiled);
    }
}
