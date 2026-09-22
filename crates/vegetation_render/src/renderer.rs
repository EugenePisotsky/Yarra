//! Vegetation render-world composition. Resource ownership lives in the stage modules below.
#[cfg(not(target_os = "ios"))]
use bevy::render::ExtractSchedule;
use bevy::{
    core_pipeline::{core_3d::Opaque3d, schedule::camera_driver},
    prelude::*,
    render::{Render, RenderSystems, render_phase::AddRenderCommand, renderer::RenderGraph},
};

mod blade_preparation;
mod buffers;
mod candidate_cache;
mod canopy_boundary;
mod draw;
mod generation;
mod gpu_types;
mod packing;
mod pipelines;
#[cfg(test)]
mod placement_gpu_tests;
mod prepare;
mod telemetry;
mod temporal;
#[cfg(test)]
mod tests;
mod topology;

pub use packing::terrain_contact_radius;

pub(crate) fn install(render_app: &mut SubApp) {
    temporal::install(render_app);
    render_app
        .add_render_command::<Opaque3d, draw::DrawVegetationDebug>()
        .add_systems(
            Render,
            (
                prepare::prepare.in_set(RenderSystems::PrepareBindGroups),
                draw::queue.in_set(RenderSystems::Queue),
            ),
        )
        .add_systems(
            RenderGraph,
            (
                candidate_cache::build,
                generation::generate,
                blade_preparation::run,
                telemetry::finish_telemetry,
            )
                .chain()
                .before(camera_driver),
        );

    // Mapping a GPU buffer immediately after submitting vegetation work is intentionally excluded
    // from iOS while the Metal corruption is isolated. The staging resource remains initialized,
    // so `finish_telemetry` simply skips its copy when no staging buffer has been prepared.
    #[cfg(not(target_os = "ios"))]
    render_app
        .add_systems(ExtractSchedule, telemetry::begin_telemetry_readback)
        .add_systems(
            Render,
            telemetry::prepare_telemetry_staging.in_set(RenderSystems::PrepareResourcesFlush),
        );
}

pub(crate) fn initialize(world: &mut World) {
    world.init_resource::<pipelines::VegetationPipelines>();
    world.init_resource::<blade_preparation::BladePreparation>();
    world.init_resource::<candidate_cache::CandidateCache>();
    world.init_resource::<buffers::VegetationBuffers>();
    world.init_resource::<telemetry::VegetationTelemetryStaging>();
}
