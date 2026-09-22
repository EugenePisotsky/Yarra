//! Upload scene revisions and prepare per-view inputs without changing render scheduling.
use super::{
    blade_preparation,
    buffers::{
        VegetationBuffers, create_compute_bind_group, create_draw_bind_group,
        create_schedule_bind_group, grow_storage, update_storage,
    },
    candidate_cache,
    generation::GenerationInputs,
    gpu_types::{
        CameraGpu, DRAW_ARGS_SIZE, DebugConfigGpu, PROCEDURAL_INSTANCE_CAPACITY,
        ProceduralInstanceGpu, SINGLE_HIGH_CAPACITY, SPLIT_HIGH_CAPACITY,
    },
    packing::{apply_terrain_gate, pack_lod_focus, pack_scene_with_gate},
    pipelines::VegetationPipelines,
};
use crate::{
    VegetationBladeBands, VegetationDebugSettings, VegetationDiagnostics, VegetationLighting,
    VegetationProfileMode, VegetationSceneState, VegetationSun, VegetationView, VegetationWind,
};
use bevy::{
    prelude::*,
    render::{
        render_resource::PipelineCache,
        renderer::{RenderDevice, RenderQueue},
        view::ExtractedView,
    },
};
use std::mem::size_of;

#[allow(clippy::too_many_arguments)] // Bevy render-world system parameters are independent resources.
pub(super) fn prepare(
    scene: Option<Res<VegetationSceneState>>,
    settings: Res<VegetationDebugSettings>,
    blade_settings: Res<crate::VegetationBladePreparation>,
    blade_preparation: Res<blade_preparation::BladePreparation>,
    lighting: Res<VegetationLighting>,
    wind: Res<VegetationWind>,
    sun: Res<VegetationSun>,
    (lod_focus, terrain_gate, render_origin): (
        Res<crate::VegetationLodFocus>,
        Res<crate::VegetationTerrainGate>,
        Res<crate::VegetationRenderOrigin>,
    ),
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<VegetationPipelines>,
    diagnostics: Res<VegetationDiagnostics>,
    views: Query<
        (
            &ExtractedView,
            Option<&bevy::camera::MainPassResolutionOverride>,
        ),
        With<VegetationView>,
    >,
    mut buffers: ResMut<VegetationBuffers>,
    mut candidate_cache: ResMut<candidate_cache::CandidateCache>,
) {
    if settings.profile_mode == VegetationProfileMode::Disabled {
        buffers.active = false;
        return;
    }
    let Some(scene) = scene else {
        buffers.active = false;
        return;
    };
    if buffers
        .canopy_boundary
        .update(&scene, &lighting, &render_device, &render_queue)
    {
        let layout = pipeline_cache.get_bind_group_layout(&pipelines.draw_layout);
        buffers.draw_bind_group = create_draw_bind_group(
            &render_device,
            &layout,
            &buffers.procedural_instances,
            &buffers.diagnostic_instances,
            &buffers.species,
            &buffers.camera,
            &buffers.debug_config,
            &blade_preparation.arena,
            &buffers.canopy_boundary.buffer,
        );
    }
    let gate_changed = *terrain_gate != buffers.uploaded_terrain_gate;
    if gate_changed {
        // Even DrawFrozen must stop showing roots whose ground is no longer
        // certified. Readiness does not change authored candidate acceptance.
        render_queue.write_buffer(&buffers.args, 0, &[0_u8; DRAW_ARGS_SIZE as usize]);
        buffers.last_generation = None;
    }
    if scene.revision() != buffers.uploaded_revision
        || render_origin.world_xz != buffers.uploaded_origin
    {
        buffers.source_serial = buffers.source_serial.wrapping_add(1);
        let packed = pack_scene_with_gate(scene.scene(), &terrain_gate, render_origin.world_xz);
        let schedule_layout = pipeline_cache.get_bind_group_layout(&pipelines.schedule_layout);
        let compute_layout = pipeline_cache.get_bind_group_layout(&pipelines.compute_layout);
        let draw_layout = pipeline_cache.get_bind_group_layout(&pipelines.draw_layout);
        let position = views
            .iter()
            .next()
            .map_or(Vec3::ZERO, |(view, _)| view.world_from_view.translation());
        let cache_replaced =
            candidate_cache.prepare(&render_device, &render_queue, &packed.work_items, position);
        let mut replaced_buffers = u64::from(cache_replaced);
        let mut uploaded_bytes = 0_u64;

        let work_items = bytemuck::cast_slice(&packed.work_items);
        uploaded_bytes += work_items.len() as u64;
        if let Some((buffer, capacity)) = update_storage(
            &render_device,
            &render_queue,
            &buffers.work_items,
            buffers.work_items_capacity,
            "vegetation-v2 work items",
            work_items,
        ) {
            buffers.work_items = buffer;
            buffers.work_items_capacity = capacity;
            replaced_buffers += 1;
        }
        if let Some((buffer, capacity)) = grow_storage(
            &render_device,
            buffers.visible_work_items_capacity,
            "vegetation-v2 visible work queue",
            packed.work_items.len() as u64 * size_of::<u32>() as u64,
        ) {
            buffers.visible_work_items = buffer;
            buffers.visible_work_items_capacity = capacity;
            replaced_buffers += 1;
        }

        let choices = bytemuck::cast_slice(&packed.choices);
        uploaded_bytes += choices.len() as u64;
        if let Some((buffer, capacity)) = update_storage(
            &render_device,
            &render_queue,
            &buffers.choices,
            buffers.choices_capacity,
            "vegetation-v2 species choices",
            choices,
        ) {
            buffers.choices = buffer;
            buffers.choices_capacity = capacity;
            replaced_buffers += 1;
        }

        let coverage = bytemuck::cast_slice(&packed.coverage);
        uploaded_bytes += coverage.len() as u64;
        if let Some((buffer, capacity)) = update_storage(
            &render_device,
            &render_queue,
            &buffers.coverage,
            buffers.coverage_capacity,
            "vegetation-v2 coverage fields",
            coverage,
        ) {
            buffers.coverage = buffer;
            buffers.coverage_capacity = capacity;
            replaced_buffers += 1;
        }

        let surfaces = bytemuck::cast_slice(&packed.surfaces);
        uploaded_bytes += surfaces.len() as u64;
        if let Some((buffer, capacity)) = update_storage(
            &render_device,
            &render_queue,
            &buffers.surfaces,
            buffers.surfaces_capacity,
            "vegetation-v2 surface fields",
            surfaces,
        ) {
            buffers.surfaces = buffer;
            buffers.surfaces_capacity = capacity;
            replaced_buffers += 1;
        }

        let species = bytemuck::cast_slice(&packed.species);
        uploaded_bytes += species.len() as u64;
        if let Some((buffer, capacity)) = update_storage(
            &render_device,
            &render_queue,
            &buffers.species,
            buffers.species_capacity,
            "vegetation-v2 species",
            species,
        ) {
            buffers.species = buffer;
            buffers.species_capacity = capacity;
            replaced_buffers += 1;
        }

        if replaced_buffers > 0 {
            buffers.schedule_bind_group = create_schedule_bind_group(
                &render_device,
                &schedule_layout,
                &buffers.work_items,
                &buffers.visible_work_items,
                &buffers.candidate_dispatch_args,
                &buffers.camera,
                &buffers.gpu_telemetry,
                &buffers.debug_config,
            );
            buffers.compute_bind_group = create_compute_bind_group(
                &render_device,
                &compute_layout,
                [
                    &buffers.work_items,
                    &buffers.choices,
                    &buffers.coverage,
                    &buffers.surfaces,
                    &buffers.procedural_instances,
                    &buffers.diagnostic_instances,
                    &buffers.args,
                    &buffers.visible_work_items,
                    &buffers.debug_config,
                    &buffers.camera,
                    &buffers.gpu_telemetry,
                    &candidate_cache.entries,
                    &candidate_cache.acceptance_bits,
                    &candidate_cache.build_items,
                ],
            );
            buffers.draw_bind_group = create_draw_bind_group(
                &render_device,
                &draw_layout,
                &buffers.procedural_instances,
                &buffers.diagnostic_instances,
                &buffers.species,
                &buffers.camera,
                &buffers.debug_config,
                &blade_preparation.arena,
                &buffers.canopy_boundary.buffer,
            );
        }
        buffers.work_item_count = packed.work_items.len() as u32;
        buffers.maximum_candidate_count = packed.maximum_candidate_count;
        buffers.low_detail_capacities = packed.low_detail_capacities;
        buffers.uploaded_revision = scene.revision();
        buffers.uploaded_origin = render_origin.world_xz;
        buffers.uploaded_terrain_gate = terrain_gate.clone();
        diagnostics.update(|snapshot| {
            snapshot.scene_revision = scene.revision();
            snapshot.source_repacks = snapshot.source_repacks.saturating_add(1);
            snapshot.source_buffer_reallocations = snapshot
                .source_buffer_reallocations
                .saturating_add(replaced_buffers);
            snapshot.source_uploaded_bytes = uploaded_bytes;
            snapshot.source_buffer_capacity_bytes = buffers.work_items_capacity
                + buffers.visible_work_items_capacity
                + buffers.choices_capacity
                + buffers.coverage_capacity
                + buffers.surfaces_capacity
                + buffers.species_capacity;
            snapshot.source_pages = scene.scene().pages.len() as u32;
            // CPU allocation metadata is available even when iOS GPU readback is disabled.
            snapshot.procedural_instance_capacity = PROCEDURAL_INSTANCE_CAPACITY;
            snapshot.procedural_instance_bytes =
                u64::from(PROCEDURAL_INSTANCE_CAPACITY) * size_of::<ProceduralInstanceGpu>() as u64;
            snapshot.source_work_items = buffers.work_item_count;
            snapshot.maximum_candidates_per_item = buffers.maximum_candidate_count;
            snapshot.topology_instance_capacities = [
                SINGLE_HIGH_CAPACITY,
                buffers.low_detail_capacities[0],
                SPLIT_HIGH_CAPACITY,
                buffers.low_detail_capacities[1],
            ];
        });
        buffers.source_work_items = packed.work_items;
    } else if gate_changed {
        // Keep surface/coverage/species buffers and stable candidate masks intact.
        // Only the compact scheduler input changes as terrain patches become ready.
        if apply_terrain_gate(&mut buffers.source_work_items, &terrain_gate) {
            render_queue.write_buffer(
                &buffers.work_items,
                0,
                bytemuck::cast_slice(&buffers.source_work_items),
            );
        }
        buffers.source_serial = buffers.source_serial.wrapping_add(1);
        buffers.uploaded_terrain_gate = terrain_gate.clone();
    }

    let Some((view, resolution)) = views.iter().next() else {
        buffers.active = false;
        return;
    };
    let clip_from_world = view
        .clip_from_world
        .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
    let view_size = resolution.map_or(view.viewport.zw(), |r| r.0);
    let focus_gpu = pack_lod_focus(
        lod_focus.position,
        view.world_from_view.translation(),
        *view.world_from_view.forward(),
    );
    let camera_gpu = CameraGpu {
        render_origin: [
            render_origin.world_xz[0] as f32,
            render_origin.world_xz[1] as f32,
            0.,
            0.,
        ],
        lod_focus: focus_gpu,
        canopy: lighting.canopy.packed(lighting.canopy_origin),
        clip_from_world: clip_from_world.to_cols_array(),
        camera_position: view
            .world_from_view
            .translation()
            .extend(view_size.y.max(1) as f32)
            .to_array(),
        projection: [
            view.clip_from_view.y_axis.y.abs() * view_size.y.max(1) as f32 * 0.5,
            view_size.x.max(1) as f32,
            view_size.y.max(1) as f32,
            if settings.far_width_compensation {
                1.0
            } else {
                0.0
            },
        ],
        sun_direction: sun
            .direction_to_light
            .extend(if sun.active { 1.0 } else { 0.0 })
            .to_array(),
        sun_radiance: sun.radiance.extend(0.0).to_array(),
        ambient_radiance: sun.ambient_radiance.extend(0.0).to_array(),
        lighting: [
            lighting.diffuse_strength.max(0.0),
            lighting.specular_strength.max(0.0),
            lighting.transmission_strength.max(0.0),
            lighting.received_shadow_strength.clamp(0.0, 1.0),
        ],
        wind: {
            let direction = wind.direction.try_normalize().unwrap_or(Vec2::X);
            [
                direction.x,
                direction.y,
                if wind.enabled {
                    wind.strength.max(0.0)
                } else {
                    0.0
                },
                wind.phase_seconds(),
            ]
        },
        wind_shape: [
            wind.spatial_frequency.max(0.001),
            wind.speed.max(0.0),
            wind.gustiness.clamp(0.0, 1.0),
            wind.flutter.max(0.0),
        ],
    };
    buffers.preparation_enabled = blade_settings.enabled
        && settings.mode as u32 == 0
        && settings.lighting_mode as u32 != 3
        && matches!(
            settings.profile_mode,
            VegetationProfileMode::Full | VegetationProfileMode::DrawFrozen
        )
        && blade_preparation.available(&pipeline_cache);
    buffers.preparation_camera = Some(camera_gpu);
    let config_gpu = DebugConfigGpu {
        values: [
            settings.mode as u32,
            settings.density_mode as u32,
            settings.lighting_mode as u32,
            buffers.low_detail_capacities[0],
        ],
        workload: [
            buffers.work_item_count,
            u32::from(settings.gpu_counters_enabled),
            u32::from(buffers.preparation_enabled),
            u32::from(settings.early_rejection)
                | (u32::from(settings.candidate_cache_enabled) << 1)
                | ((settings.shape_inspection as u32) << 4)
                | (u32::from(settings.inspection_disable_opening) << 8)
                | (u32::from(settings.blade_bands == VegetationBladeBands::MotionMask) << 9)
                | (((settings.blade_band_density.clamp(0.0, 1.0) * 255.0).round() as u32) << 16),
        ],
    };
    render_queue.write_buffer(&buffers.camera, 0, bytemuck::bytes_of(&camera_gpu));
    render_queue.write_buffer(&buffers.debug_config, 0, bytemuck::bytes_of(&config_gpu));
    buffers.generation_inputs = Some(GenerationInputs::new(
        buffers.source_serial,
        camera_gpu,
        config_gpu,
    ));
    buffers.active = buffers.work_item_count > 0 && buffers.maximum_candidate_count > 0;
}
