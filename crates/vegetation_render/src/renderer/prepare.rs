//! Upload scene revisions and prepare per-view inputs without changing render scheduling.
use super::{
    blade_preparation,
    buffers::{
        VegetationBuffers, create_compute_bind_group, create_draw_bind_group,
        create_schedule_bind_group, diagnostic_instances, grow_storage, update_storage,
        writable_storage,
    },
    candidate_cache,
    generation::GenerationInputs,
    gpu_types::{
        CameraGpu, ConfigGpu, DRAW_ARGS_SIZE, DiagnosticInstanceGpu, PROCEDURAL_INSTANCE_CAPACITY,
        ProceduralInstanceGpu, SINGLE_HIGH_CAPACITY, SPLIT_HIGH_CAPACITY, SurfaceSampleGpu,
    },
    packing::{apply_terrain_gate, pack_coverage, pack_items, pack_lod_focus, pack_surface},
    pipelines::VegetationPipelines,
};
use crate::{
    VegetationAmbientGain, VegetationDiagnosticMode, VegetationDiagnostics, VegetationLighting,
    VegetationLightingMode, VegetationLodFocus, VegetationProfileMode, VegetationRenderOrigin,
    VegetationSceneState, VegetationSettings, VegetationSun, VegetationTerrainGate, VegetationView,
    VegetationWind,
};
use bevy::{
    ecs::system::SystemParam,
    prelude::*,
    render::{
        render_resource::{Buffer, PipelineCache},
        renderer::{RenderDevice, RenderQueue},
        view::ExtractedView,
    },
};
use std::mem::size_of;

/// What a view's camera and lighting inputs are built from, besides the view itself.
#[derive(SystemParam)]
pub(super) struct ViewInputs<'w> {
    lighting: Res<'w, VegetationLighting>,
    wind: Res<'w, VegetationWind>,
    sun: Res<'w, VegetationSun>,
    lod_focus: Res<'w, VegetationLodFocus>,
    render_origin: Res<'w, VegetationRenderOrigin>,
    ambient_gain: Option<Res<'w, VegetationAmbientGain>>,
}

/// `ConfigGpu::workload.w` bits, read as `config.workload.w & 1u` and `& 2u` in
/// `shaders/vegetation/placement.wesl`.
const EARLY_REJECTION: u32 = 1;
const CANDIDATE_CACHE: u32 = 1 << 1;
/// Bounds on the exposure gain applied to grass's ambient fill.
const AMBIENT_GAIN_RANGE: (f32, f32) = (0.0625, 16.0);

#[allow(clippy::too_many_arguments)] // Bevy render-world system parameters are independent resources.
pub(super) fn prepare(
    scene: Option<Res<VegetationSceneState>>,
    settings: Res<VegetationSettings>,
    blade_settings: Res<crate::VegetationBladePreparation>,
    blade_preparation: Res<blade_preparation::BladePreparation>,
    inputs: ViewInputs,
    terrain_gate: Res<VegetationTerrainGate>,
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
    let mut rebind =
        buffers
            .canopy_boundary
            .update(&scene, &inputs.lighting, &render_device, &render_queue);
    let diagnostic = settings.diagnostic_mode != VegetationDiagnosticMode::ProceduralGeometry;
    if (buffers.diagnostic_instances.size() > size_of::<DiagnosticInstanceGpu>() as u64)
        != diagnostic
    {
        buffers.diagnostic_instances = diagnostic_instances(&render_device, diagnostic);
        rebind = true;
    }
    let gate_changed = *terrain_gate != buffers.uploaded_terrain_gate;
    if gate_changed {
        // Even DrawFrozen must stop showing roots whose ground is no longer
        // certified. Readiness does not change authored candidate acceptance.
        render_queue.write_buffer(&buffers.args, 0, &[0_u8; DRAW_ARGS_SIZE as usize]);
        buffers.last_generation = None;
    }
    let origin = inputs.render_origin.world_xz;
    if scene.revision() != buffers.uploaded_revision || origin != buffers.uploaded_origin {
        let position = views
            .iter()
            .next()
            .map_or(Vec3::ZERO, |(view, _)| view.world_from_view.translation());
        rebind |= upload_sources(
            &mut buffers,
            &mut candidate_cache,
            &scene,
            &terrain_gate,
            origin,
            position,
            (&render_device, &render_queue),
            &diagnostics,
        );
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
    if rebind {
        rebind_groups(
            &mut buffers,
            &render_device,
            &pipeline_cache,
            &pipelines,
            &candidate_cache,
            &blade_preparation,
        );
    }

    let Some((view, resolution)) = views.iter().next() else {
        buffers.active = false;
        return;
    };
    let view_size = resolution.map_or(view.viewport.zw(), |r| r.0);
    let camera_gpu = camera(view, view_size, &inputs, settings.far_width_compensation);
    buffers.preparation_enabled = blade_settings.enabled
        && !diagnostic
        && settings.lighting_mode != VegetationLightingMode::VertexOnlyDiagnostic
        && matches!(
            settings.profile_mode,
            VegetationProfileMode::Full | VegetationProfileMode::DrawFrozen
        )
        && blade_preparation.available(&pipeline_cache);
    buffers.preparation_camera = Some(camera_gpu);
    let config_gpu = ConfigGpu {
        values: [
            settings.diagnostic_mode as u32,
            settings.density_mode as u32,
            settings.lighting_mode as u32,
            buffers.low_detail_capacities[0],
        ],
        workload: [
            buffers.work_item_count,
            u32::from(settings.gpu_counters_enabled),
            u32::from(buffers.preparation_enabled),
            (u32::from(settings.early_rejection) * EARLY_REJECTION)
                | (u32::from(settings.candidate_cache_enabled) * CANDIDATE_CACHE),
        ],
    };
    render_queue.write_buffer(&buffers.camera, 0, bytemuck::bytes_of(&camera_gpu));
    render_queue.write_buffer(&buffers.config, 0, bytemuck::bytes_of(&config_gpu));
    buffers.generation_inputs = Some(GenerationInputs::new(
        buffers.source_serial,
        camera_gpu,
        config_gpu,
    ));
    buffers.active = buffers.work_item_count > 0 && buffers.maximum_candidate_count > 0;
}

/// Uploads a new scene revision or render origin: page samples stay in place while their
/// content key is unchanged, so only arriving or edited pages are packed and written; work
/// items and species are rebuilt. True when a buffer was replaced and bind groups must follow.
#[allow(clippy::too_many_arguments)] // The renderer's buffers and the scene's inputs.
fn upload_sources(
    buffers: &mut VegetationBuffers,
    candidate_cache: &mut candidate_cache::CandidateCache,
    scene: &VegetationSceneState,
    terrain_gate: &VegetationTerrainGate,
    origin: [f64; 2],
    camera_position: Vec3,
    (render_device, render_queue): (&RenderDevice, &RenderQueue),
    diagnostics: &VegetationDiagnostics,
) -> bool {
    buffers.source_serial = buffers.source_serial.wrapping_add(1);
    let pages = &scene.scene().pages;
    let mut replaced_buffers = 0_u64;
    let capacity = |buffers: &VegetationBuffers| {
        let elements = |bytes: u64, size: usize| (bytes / size as u64).min(u64::from(u32::MAX));
        (
            elements(buffers.surfaces_capacity, size_of::<SurfaceSampleGpu>()) as u32,
            elements(buffers.coverage_capacity, size_of::<f32>()) as u32,
        )
    };
    let current = capacity(buffers);
    let plan = match buffers.page_slots.place(pages, scene.page_keys(), current) {
        Ok(plan) => plan,
        Err(regrow) => {
            (buffers.surfaces, buffers.surfaces_capacity) = writable_storage(
                render_device,
                "vegetation surface fields",
                regrow.surface_samples * size_of::<SurfaceSampleGpu>() as u64,
            );
            (buffers.coverage, buffers.coverage_capacity) = writable_storage(
                render_device,
                "vegetation coverage fields",
                regrow.coverage_samples * size_of::<f32>() as u64,
            );
            replaced_buffers += 2;
            let capacity = capacity(buffers);
            buffers.page_slots.reset(capacity);
            buffers
                .page_slots
                .place(pages, scene.page_keys(), capacity)
                .expect("regrown page buffers hold the whole scene")
        }
    };
    let mut uploaded_bytes = write_runs(
        render_queue,
        &buffers.surfaces,
        plan.uploads
            .iter()
            .map(|&page| (plan.layouts[page].surface, pack_surface(&pages[page])))
            .collect(),
    );
    uploaded_bytes += write_runs(
        render_queue,
        &buffers.coverage,
        plan.uploads
            .iter()
            .flat_map(|&page| {
                plan.layouts[page]
                    .coverage
                    .iter()
                    .zip(&pages[page].fields)
                    .map(|(&offset, field)| (offset, pack_coverage(field)))
            })
            .collect(),
    );
    let packed = pack_items(scene.scene(), terrain_gate, origin, &plan.layouts);
    let cache_replaced = candidate_cache.prepare(
        render_device,
        render_queue,
        &packed.work_items,
        camera_position,
    );
    replaced_buffers += u64::from(cache_replaced);

    let b = &mut *buffers;
    for (buffer, capacity, label, contents) in [
        (
            &mut b.work_items,
            &mut b.work_items_capacity,
            "vegetation work items",
            bytemuck::cast_slice(&packed.work_items),
        ),
        (
            &mut b.choices,
            &mut b.choices_capacity,
            "vegetation species choices",
            bytemuck::cast_slice(&packed.choices),
        ),
        (
            &mut b.species,
            &mut b.species_capacity,
            "vegetation species",
            bytemuck::cast_slice(&packed.species),
        ),
    ] {
        uploaded_bytes += contents.len() as u64;
        if let Some(replacement) = update_storage(
            render_device,
            render_queue,
            buffer,
            *capacity,
            label,
            contents,
        ) {
            (*buffer, *capacity) = replacement;
            replaced_buffers += 1;
        }
    }
    if let Some(replacement) = grow_storage(
        render_device,
        buffers.visible_work_items_capacity,
        "vegetation visible work queue",
        packed.work_items.len() as u64 * size_of::<u32>() as u64,
    ) {
        (
            buffers.visible_work_items,
            buffers.visible_work_items_capacity,
        ) = replacement;
        replaced_buffers += 1;
    }

    buffers.work_item_count = packed.work_items.len() as u32;
    buffers.maximum_candidate_count = packed.maximum_candidate_count;
    buffers.low_detail_capacities = packed.low_detail_capacities;
    buffers.uploaded_revision = scene.revision();
    buffers.uploaded_origin = origin;
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
    replaced_buffers > 0
}

/// Rebuilds the schedule, placement and draw bind groups after a buffer was replaced.
fn rebind_groups(
    buffers: &mut VegetationBuffers,
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    pipelines: &VegetationPipelines,
    candidate_cache: &candidate_cache::CandidateCache,
    blade_preparation: &blade_preparation::BladePreparation,
) {
    buffers.schedule_bind_group = create_schedule_bind_group(
        render_device,
        &pipeline_cache.get_bind_group_layout(&pipelines.schedule_layout),
        &buffers.work_items,
        &buffers.visible_work_items,
        &buffers.candidate_dispatch_args,
        &buffers.camera,
        &buffers.gpu_telemetry,
        &buffers.config,
    );
    buffers.compute_bind_group = create_compute_bind_group(
        render_device,
        &pipeline_cache.get_bind_group_layout(&pipelines.compute_layout),
        [
            &buffers.work_items,
            &buffers.choices,
            &buffers.coverage,
            &buffers.surfaces,
            &buffers.procedural_instances,
            &buffers.diagnostic_instances,
            &buffers.args,
            &buffers.visible_work_items,
            &buffers.config,
            &buffers.camera,
            &buffers.gpu_telemetry,
            &candidate_cache.entries,
            &candidate_cache.acceptance_bits,
            &candidate_cache.build_items,
        ],
    );
    buffers.draw_bind_group = create_draw_bind_group(
        render_device,
        &pipeline_cache.get_bind_group_layout(&pipelines.draw_layout),
        &buffers.procedural_instances,
        &buffers.diagnostic_instances,
        &buffers.species,
        &buffers.camera,
        &buffers.config,
        &blade_preparation.arena,
        &buffers.canopy_boundary.buffer,
    );
}

/// The view's camera, projection, light and wind as the grass shaders read them.
fn camera(
    view: &ExtractedView,
    view_size: UVec2,
    inputs: &ViewInputs,
    far_width_compensation: bool,
) -> CameraGpu {
    let ViewInputs {
        lighting,
        wind,
        sun,
        lod_focus,
        render_origin,
        ambient_gain,
    } = inputs;
    let clip_from_world = view
        .clip_from_world
        .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
    let direction = wind.direction.try_normalize().unwrap_or(Vec2::X);
    CameraGpu {
        render_origin: [
            render_origin.world_xz[0] as f32,
            render_origin.world_xz[1] as f32,
            0.,
            0.,
        ],
        lod_focus: pack_lod_focus(
            lod_focus.position,
            view.world_from_view.translation(),
            *view.world_from_view.forward(),
        ),
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
            if far_width_compensation { 1.0 } else { 0.0 },
        ],
        sun_direction: sun
            .direction_to_light
            .extend(if sun.active { 1.0 } else { 0.0 })
            .to_array(),
        sun_radiance: sun.radiance.extend(0.0).to_array(),
        // w: ambient bound gain; the shader treats non-positive values as 1.
        ambient_radiance: sun
            .ambient_radiance
            .extend(
                ambient_gain
                    .as_ref()
                    .map_or(1.0, |gain| gain.0)
                    .clamp(AMBIENT_GAIN_RANGE.0, AMBIENT_GAIN_RANGE.1),
            )
            .to_array(),
        lighting: [
            lighting.diffuse_strength.max(0.0),
            lighting.specular_strength.max(0.0),
            lighting.transmission_strength.max(0.0),
            lighting.received_shadow_strength.clamp(0.0, 1.0),
        ],
        wind: [
            direction.x,
            direction.y,
            if wind.enabled {
                wind.strength.max(0.0)
            } else {
                0.0
            },
            wind.phase_seconds(),
        ],
        wind_shape: [
            wind.spatial_frequency.max(0.001),
            wind.speed.max(0.0),
            wind.gustiness.clamp(0.0, 1.0),
            wind.flutter.max(0.0),
        ],
    }
}

/// Writes `(element offset, samples)` ranges, merging adjacent ones into single uploads, and
/// returns the bytes written. A regrown buffer is written in one upload.
fn write_runs<T: bytemuck::Pod>(
    queue: &RenderQueue,
    buffer: &Buffer,
    mut ranges: Vec<(u32, impl Iterator<Item = T>)>,
) -> u64 {
    ranges.sort_by_key(|&(offset, _)| offset);
    let mut written = 0;
    let mut flush = |start: u32, run: &mut Vec<T>| {
        if !run.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(run);
            queue.write_buffer(buffer, u64::from(start) * size_of::<T>() as u64, bytes);
            written += bytes.len() as u64;
            run.clear();
        }
    };
    let mut start = 0;
    let mut run = Vec::new();
    for (offset, samples) in ranges {
        if offset as usize != start as usize + run.len() {
            flush(start, &mut run);
            start = offset;
        }
        run.extend(samples);
    }
    flush(start, &mut run);
    written
}
