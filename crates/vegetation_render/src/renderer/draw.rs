//! Queue visible vegetation and issue its indirect opaque draws.
use super::{
    buffers::VegetationBuffers,
    pipelines::{VegetationPipelineKey, VegetationPipelines},
    temporal,
};
use crate::{VegetationDebugSettings, VegetationDraw, VegetationProfileMode, VegetationView};
use bevy::{
    asset::AssetId,
    core_pipeline::core_3d::{Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey},
    ecs::{
        query::ROQueryItem,
        system::{SystemParamItem, lifetimeless::SRes},
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
        render_resource::{IndexFormat, PipelineCache},
        sync_world::MainEntity,
        view::ExtractedView,
    },
};

pub(super) fn queue(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<VegetationPipelines>,
    buffers: Res<VegetationBuffers>,
    settings: Res<VegetationDebugSettings>,
    mut opaque_phases: ResMut<ViewBinnedRenderPhases<Opaque3d>>,
    draw_functions: Res<DrawFunctions<Opaque3d>>,
    view_key_cache: Res<ViewKeyCache>,
    views: Query<
        (
            Entity,
            &ExtractedView,
            &Msaa,
            Option<&upscaling::temporal::TemporalView>,
        ),
        With<VegetationView>,
    >,
    draw_entity: Query<(Entity, &MainEntity), With<VegetationDraw>>,
    clouds: Option<Res<atmosphere::clouds::CloudShadowGpu>>,
) {
    let Ok((draw_entity, draw_main_entity)) = draw_entity.single() else {
        return;
    };
    let draw_function = draw_functions.read().id::<DrawVegetationDebug>();
    for (view_entity, view, msaa, temporal) in &views {
        commands.entity(view_entity).remove::<temporal::Pipeline>();
        let (Some(phase), Some(mesh_view_key)) = (
            opaque_phases.get_mut(&view.retained_view_entity),
            view_key_cache.get(&view.retained_view_entity),
        ) else {
            continue;
        };
        phase.remove(*draw_main_entity);
        if !buffers.active
            || matches!(
                settings.profile_mode,
                VegetationProfileMode::ComputeOnly
                    | VegetationProfileMode::ScheduleOnly
                    | VegetationProfileMode::Disabled
            )
        {
            continue;
        }
        let Ok(pipeline) = pipelines.draw_variants.specialize(
            &pipeline_cache,
            VegetationPipelineKey {
                msaa: *msaa,
                target_format: view.target_format,
                view_layout_bits: MeshPipelineViewLayoutKey::from(*mesh_view_key).bits(),
                blade_bands: settings.blade_bands,
                clouds: clouds.is_some(),
                temporal: temporal.is_some(),
            },
        ) else {
            continue;
        };
        if temporal.is_some() {
            commands
                .entity(view_entity)
                .insert(temporal::Pipeline(pipeline));
            continue;
        }
        phase.add(
            Opaque3dBatchSetKey {
                draw_function,
                pipeline,
                material_bind_group_index: None,
                lightmap_slab: None,
                slabs: default(),
            },
            Opaque3dBinKey {
                asset_id: AssetId::<Mesh>::invalid().untyped(),
            },
            (draw_entity, *draw_main_entity),
            InputUniformIndex::default(),
            BinnedRenderPhaseType::NonMesh,
        );
    }
}

pub(super) type DrawVegetationDebug = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    DrawVegetationDebugIndirect,
);

pub(super) struct DrawVegetationDebugIndirect;

impl<P: PhaseItem> RenderCommand<P> for DrawVegetationDebugIndirect {
    type Param = (
        SRes<VegetationBuffers>,
        Option<SRes<DiagnosticsRecorder>>,
        Option<SRes<atmosphere::clouds::CloudShadowGpu>>,
    );
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        _view: ROQueryItem<'w, '_, Self::ViewQuery>,
        _entity: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        (buffers, diagnostics, clouds): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let buffers = buffers.into_inner();
        let diagnostics = diagnostics.as_deref();
        let draw_span = diagnostics.pass_span(pass, "vegetation_v2_draw");
        pass.set_bind_group(1, &buffers.draw_bind_group, &[]);
        if let Some(clouds) = clouds {
            pass.set_bind_group(2, &clouds.into_inner().0, &[]);
        }
        pass.set_index_buffer(buffers.topology_indices.slice(..), IndexFormat::Uint16);
        pass.draw_indexed_indirect(&buffers.args, 0);
        pass.draw_indexed_indirect(&buffers.args, 20);
        pass.draw_indexed_indirect(&buffers.args, 40);
        pass.draw_indexed_indirect(&buffers.args, 60);
        draw_span.end(pass);
        RenderCommandResult::Success
    }
}
