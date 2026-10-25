//! Queue visible vegetation and issue its indirect opaque draws: lit in the main pass on forward
//! views, into the G-buffer on deferred ones (`deferred.rs`).
use super::{
    buffers::VegetationBuffers,
    deferred::{self, GrassLighting},
    pipelines::{DrawOutput, VegetationPipelineKey, VegetationPipelines},
};

use crate::{VegetationDraw, VegetationProfileMode, VegetationSettings, VegetationView};

use atmosphere::environment::EnvironmentSurfaceGpu;

use bevy::{
    asset::AssetId,
    core_pipeline::{
        core_3d::{Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey},
        prepass::DeferredPrepass,
    },
    ecs::{
        query::ROQueryItem,
        system::{SystemParam, SystemParamItem, lifetimeless::SRes},
    },
    mesh::Mesh,
    pbr::{MeshPipelineViewLayoutKey, SetMeshViewBindGroup, ViewKeyCache},
    prelude::*,
    render::{
        diagnostic::{DiagnosticsRecorder, RecordDiagnostics},
        render_phase::{
            BinnedRenderPhaseType, DrawFunctions, InputUniformIndex, PhaseItem, RenderCommand,
            RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewBinnedRenderPhases,
        },
        render_resource::{CachedRenderPipelineId, IndexFormat, PipelineCache},
        sync_world::MainEntity,
        view::ExtractedView,
    },
};

/// What grass draws are specialized with.
#[derive(SystemParam)]
pub(super) struct GrassPipelines<'w> {
    cache: Res<'w, PipelineCache>,
    draw: ResMut<'w, VegetationPipelines>,
    lighting: ResMut<'w, GrassLighting>,
    environment: Option<Res<'w, EnvironmentSurfaceGpu>>,
}

impl GrassPipelines<'_> {
    /// The lit draw of a forward view.
    fn forward(
        &mut self,
        view: &ExtractedView,
        view_layout_bits: u32,
    ) -> Option<CachedRenderPipelineId> {
        let key = VegetationPipelineKey {
            output: DrawOutput::Colour {
                format: view.target_format,
                view_layout_bits,
            },
            environment: self.environment.is_some(),
        };
        self.draw.draw_variants.specialize(&self.cache, key).ok()
    }

    /// The G-buffer and lighting pipelines of a deferred view. Deferred grass reads the
    /// environment for its wetness and frost.
    fn deferred(
        &mut self,
        view: &ExtractedView,
        view_layout_bits: u32,
        motion: bool,
    ) -> Option<deferred::ViewPipelines> {
        self.environment.as_ref()?;
        deferred::specialize(
            &self.cache,
            &mut self.draw,
            &mut self.lighting,
            view,
            view_layout_bits,
            motion,
        )
        .ok()
    }
}

/// The main pass's opaque phase, where forward views draw grass.
#[derive(SystemParam)]
pub(super) struct OpaquePhase<'w> {
    phases: ResMut<'w, ViewBinnedRenderPhases<Opaque3d>>,
    functions: Res<'w, DrawFunctions<Opaque3d>>,
}

type QueuedView = (
    Entity,
    &'static ExtractedView,
    Has<DeferredPrepass>,
    Has<upscaling::temporal::TemporalView>,
);

pub(super) fn queue(
    mut commands: Commands,
    mut pipelines: GrassPipelines,
    mut opaque: OpaquePhase,
    settings: Res<VegetationSettings>,
    view_key_cache: Res<ViewKeyCache>,
    views: Query<QueuedView, With<VegetationView>>,
    draw_entity: Query<(Entity, &MainEntity), With<VegetationDraw>>,
) {
    let Ok((draw_entity, draw_main_entity)) = draw_entity.single() else {
        return;
    };
    let draw_function = opaque.functions.read().id::<DrawVegetation>();

    for (view_entity, view, deferred, temporal) in &views {
        commands
            .entity(view_entity)
            .remove::<deferred::ViewPipelines>();
        let (Some(phase), Some(mesh_view_key)) = (
            opaque.phases.get_mut(&view.retained_view_entity),
            view_key_cache.get(&view.retained_view_entity),
        ) else {
            continue;
        };
        phase.remove(*draw_main_entity);

        // Queue precedes PrepareBindGroups: buffers.active still describes the last
        // frame here. Decide residency at draw time so newly resident grass does not
        // miss its first frame (including after an empty scene or a visibility toggle).
        if matches!(
            settings.profile_mode,
            VegetationProfileMode::ComputeOnly
                | VegetationProfileMode::ScheduleOnly
                | VegetationProfileMode::Disabled
        ) {
            continue;
        }

        let view_layout_bits = MeshPipelineViewLayoutKey::from(*mesh_view_key).bits();

        if deferred {
            if let Some(view_pipelines) = pipelines.deferred(view, view_layout_bits, temporal) {
                commands.entity(view_entity).insert(view_pipelines);
            }
            continue;
        }

        let Some(pipeline) = pipelines.forward(view, view_layout_bits) else {
            continue;
        };
        phase.add(
            Opaque3dBatchSetKey {
                draw_function,
                pipeline,
                material_bind_group_index: None,
                lightmap_slab: None,
                slabs: default(),
            },
            Opaque3dBinKey {
                asset_id: AssetId::<Mesh>::default().untyped(),
            },
            (draw_entity, *draw_main_entity),
            InputUniformIndex::default(),
            BinnedRenderPhaseType::NonMesh,
        );
    }
}

pub(super) type DrawVegetation = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    DrawVegetationIndirect,
);

pub(super) struct DrawVegetationIndirect;

impl<P: PhaseItem> RenderCommand<P> for DrawVegetationIndirect {
    type Param = (
        SRes<VegetationBuffers>,
        Option<SRes<DiagnosticsRecorder>>,
        Option<SRes<EnvironmentSurfaceGpu>>,
    );

    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        _view: ROQueryItem<'w, '_, Self::ViewQuery>,
        _entity: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        (buffers, diagnostics, environment): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let buffers = buffers.into_inner();

        if !buffers.active {
            return RenderCommandResult::Success;
        }

        let diagnostics = diagnostics.as_deref();
        let draw_span = diagnostics.pass_span(pass, "vegetation_draw");
        pass.set_bind_group(1, &buffers.draw_bind_group, &[]);

        if let Some(environment) = environment {
            pass.set_bind_group(2, &environment.into_inner().0, &[]);
        }

        pass.set_index_buffer(buffers.topology_indices.slice(..), IndexFormat::Uint16);
        pass.draw_indexed_indirect(&buffers.args, 0);
        pass.draw_indexed_indirect(&buffers.args, 20);
        draw_span.end(pass);
        RenderCommandResult::Success
    }
}
