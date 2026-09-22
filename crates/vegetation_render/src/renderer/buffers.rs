//! Resident GPU allocation, bind groups and geometric storage growth.
use super::{
    blade_preparation, candidate_cache, canopy_boundary,
    generation::{GenerationInputs, GenerationKey},
    gpu_types::{
        CameraGpu, DRAW_ARGS_SIZE, DebugConfigGpu, DebugInstanceGpu, GPU_TELEMETRY_SIZE,
        LOW_DETAIL_CAPACITY, MAX_DIAGNOSTIC_INSTANCES, PROCEDURAL_INSTANCE_CAPACITY,
        ProceduralInstanceGpu, WorkItemGpu,
    },
    pipelines::VegetationPipelines,
    topology::build_topology_indices,
};
use bevy::{
    prelude::*,
    render::{
        render_resource::{
            BindGroup, BindGroupEntries, BindGroupLayout, Buffer, BufferDescriptor,
            BufferInitDescriptor, BufferUsages, PipelineCache,
        },
        renderer::{RenderDevice, RenderQueue},
    },
};
use std::mem::size_of;

#[derive(Resource)]
pub(super) struct VegetationBuffers {
    pub(super) work_items: Buffer,
    pub(super) work_items_capacity: u64,
    pub(super) visible_work_items: Buffer,
    pub(super) visible_work_items_capacity: u64,
    pub(super) candidate_dispatch_args: Buffer,
    pub(super) choices: Buffer,
    pub(super) choices_capacity: u64,
    pub(super) coverage: Buffer,
    pub(super) coverage_capacity: u64,
    pub(super) canopy_boundary: canopy_boundary::CanopyBoundary,
    pub(super) surfaces: Buffer,
    pub(super) surfaces_capacity: u64,
    pub(super) species: Buffer,
    pub(super) species_capacity: u64,
    pub(super) procedural_instances: Buffer,
    pub(super) diagnostic_instances: Buffer,
    pub(super) topology_indices: Buffer,
    pub(super) args: Buffer,
    pub(super) gpu_telemetry: Buffer,
    pub(super) camera: Buffer,
    pub(super) debug_config: Buffer,
    pub(super) schedule_bind_group: BindGroup,
    pub(super) compute_bind_group: BindGroup,
    pub(super) draw_bind_group: BindGroup,
    pub(super) uploaded_revision: u64,
    pub(super) uploaded_origin: [f64; 2],
    pub(super) source_serial: u64,
    pub(super) uploaded_terrain_gate: crate::VegetationTerrainGate,
    pub(super) source_work_items: Vec<WorkItemGpu>,
    pub(super) generation_inputs: Option<GenerationInputs>,
    pub(super) last_generation: Option<GenerationKey>,
    pub(super) generation_serial: u64,
    pub(super) preparation_enabled: bool,
    pub(super) preparation_camera: Option<CameraGpu>,
    pub(super) work_item_count: u32,
    pub(super) maximum_candidate_count: u32,
    pub(super) low_detail_capacities: [u32; 2],
    pub(super) active: bool,
}

impl FromWorld for VegetationBuffers {
    fn from_world(world: &mut World) -> Self {
        let pipelines = world.resource::<VegetationPipelines>();
        let pipeline_cache = world.resource::<PipelineCache>();
        let schedule_layout = pipeline_cache.get_bind_group_layout(&pipelines.schedule_layout);
        let compute_layout = pipeline_cache.get_bind_group_layout(&pipelines.compute_layout);
        let draw_layout = pipeline_cache.get_bind_group_layout(&pipelines.draw_layout);
        let render_device = world.resource::<RenderDevice>();
        let candidate_cache = world.resource::<candidate_cache::CandidateCache>();
        let work_items = dummy_storage(render_device, "vegetation-v2 empty work items");
        let visible_work_items =
            dummy_storage(render_device, "vegetation-v2 empty visible work queue");
        let candidate_dispatch_args = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 candidate dispatch args"),
            size: 12,
            usage: BufferUsages::STORAGE | BufferUsages::INDIRECT | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let choices = dummy_storage(render_device, "vegetation-v2 empty choices");
        let coverage = dummy_storage(render_device, "vegetation-v2 empty coverage");
        let surfaces = dummy_storage(render_device, "vegetation-v2 empty surfaces");
        let species = dummy_storage(render_device, "vegetation-v2 empty species");
        let procedural_instances = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 compact topology-bin instances"),
            size: u64::from(PROCEDURAL_INSTANCE_CAPACITY)
                * size_of::<ProceduralInstanceGpu>() as u64,
            usage: BufferUsages::STORAGE
                | if cfg!(test) {
                    BufferUsages::COPY_SRC | BufferUsages::COPY_DST
                } else {
                    BufferUsages::empty()
                },
            mapped_at_creation: false,
        });
        let diagnostic_instances = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 placement diagnostic instances"),
            size: u64::from(MAX_DIAGNOSTIC_INSTANCES) * size_of::<DebugInstanceGpu>() as u64,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let topology_indices = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("vegetation-v2 fixed-budget topology indices"),
            contents: bytemuck::cast_slice(&build_topology_indices()),
            usage: BufferUsages::INDEX,
        });
        let args = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 topology-bin indirect args"),
            size: DRAW_ARGS_SIZE,
            usage: BufferUsages::STORAGE
                | BufferUsages::INDIRECT
                | BufferUsages::COPY_DST
                | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let gpu_telemetry = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 GPU telemetry"),
            size: GPU_TELEMETRY_SIZE,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let camera = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 debug camera"),
            size: size_of::<CameraGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let debug_config = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 debug configuration"),
            size: size_of::<DebugConfigGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let schedule_bind_group = create_schedule_bind_group(
            render_device,
            &schedule_layout,
            &work_items,
            &visible_work_items,
            &candidate_dispatch_args,
            &camera,
            &gpu_telemetry,
            &debug_config,
        );
        let compute_bind_group = create_compute_bind_group(
            render_device,
            &compute_layout,
            [
                &work_items,
                &choices,
                &coverage,
                &surfaces,
                &procedural_instances,
                &diagnostic_instances,
                &args,
                &visible_work_items,
                &debug_config,
                &camera,
                &gpu_telemetry,
                &candidate_cache.entries,
                &candidate_cache.acceptance_bits,
                &candidate_cache.build_items,
            ],
        );
        let canopy_boundary = canopy_boundary::CanopyBoundary::new(render_device);
        let draw_bind_group = create_draw_bind_group(
            render_device,
            &draw_layout,
            &procedural_instances,
            &diagnostic_instances,
            &species,
            &camera,
            &debug_config,
            &world
                .resource::<blade_preparation::BladePreparation>()
                .arena,
            &canopy_boundary.buffer,
        );
        Self {
            work_items,
            work_items_capacity: 16,
            visible_work_items,
            visible_work_items_capacity: 16,
            candidate_dispatch_args,
            choices,
            choices_capacity: 16,
            coverage,
            coverage_capacity: 16,
            canopy_boundary,
            surfaces,
            surfaces_capacity: 16,
            species,
            species_capacity: 16,
            procedural_instances,
            diagnostic_instances,
            topology_indices,
            args,
            gpu_telemetry,
            camera,
            debug_config,
            schedule_bind_group,
            compute_bind_group,
            draw_bind_group,
            uploaded_revision: 0,
            uploaded_origin: [0.; 2],
            source_serial: 0,
            uploaded_terrain_gate: default(),
            source_work_items: Vec::new(),
            generation_inputs: None,
            last_generation: None,
            generation_serial: 0,
            preparation_enabled: false,
            preparation_camera: None,
            work_item_count: 0,
            maximum_candidate_count: 0,
            low_detail_capacities: [LOW_DETAIL_CAPACITY / 2; 2],
            active: false,
        }
    }
}

pub(super) fn create_schedule_bind_group(
    render_device: &RenderDevice,
    layout: &BindGroupLayout,
    work_items: &Buffer,
    visible_work_items: &Buffer,
    candidate_dispatch_args: &Buffer,
    camera: &Buffer,
    gpu_telemetry: &Buffer,
    debug_config: &Buffer,
) -> BindGroup {
    render_device.create_bind_group(
        Some("vegetation-v2 visible work scheduling"),
        layout,
        &BindGroupEntries::sequential((
            work_items.as_entire_binding(),
            visible_work_items.as_entire_binding(),
            candidate_dispatch_args.as_entire_binding(),
            camera.as_entire_binding(),
            gpu_telemetry.as_entire_binding(),
            debug_config.as_entire_binding(),
        )),
    )
}

pub(super) fn dummy_storage(render_device: &RenderDevice, label: &'static str) -> Buffer {
    render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some(label),
        contents: &[0; 16],
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
    })
}

pub(super) fn create_compute_bind_group(
    render_device: &RenderDevice,
    layout: &BindGroupLayout,
    buffers: [&Buffer; 14],
) -> BindGroup {
    render_device.create_bind_group(
        Some("vegetation-v2 debug generation"),
        layout,
        &BindGroupEntries::sequential((
            buffers[0].as_entire_binding(),
            buffers[1].as_entire_binding(),
            buffers[2].as_entire_binding(),
            buffers[3].as_entire_binding(),
            buffers[4].as_entire_binding(),
            buffers[5].as_entire_binding(),
            buffers[6].as_entire_binding(),
            buffers[7].as_entire_binding(),
            buffers[8].as_entire_binding(),
            buffers[9].as_entire_binding(),
            buffers[10].as_entire_binding(),
            buffers[11].as_entire_binding(),
            buffers[12].as_entire_binding(),
            buffers[13].as_entire_binding(),
        )),
    )
}

#[allow(clippy::too_many_arguments)] // One named buffer per shader binding.
pub(super) fn create_draw_bind_group(
    render_device: &RenderDevice,
    layout: &BindGroupLayout,
    procedural_instances: &Buffer,
    diagnostic_instances: &Buffer,
    species: &Buffer,
    camera: &Buffer,
    debug_config: &Buffer,
    prepared_arena: &Buffer,
    canopy_boundary: &Buffer,
) -> BindGroup {
    render_device.create_bind_group(
        Some("vegetation-v2 debug draw"),
        layout,
        &BindGroupEntries::sequential((
            procedural_instances.as_entire_binding(),
            diagnostic_instances.as_entire_binding(),
            species.as_entire_binding(),
            camera.as_entire_binding(),
            debug_config.as_entire_binding(),
            prepared_arena.as_entire_binding(),
            canopy_boundary.as_entire_binding(),
        )),
    )
}

pub(super) fn update_storage(
    render_device: &RenderDevice,
    render_queue: &RenderQueue,
    buffer: &Buffer,
    capacity: u64,
    label: &'static str,
    contents: &[u8],
) -> Option<(Buffer, u64)> {
    let required = (contents.len() as u64).max(16);
    if required <= capacity {
        if !contents.is_empty() {
            render_queue.write_buffer(buffer, 0, contents);
        }
        return None;
    }
    let new_capacity = storage_capacity_for(required);
    let buffer = render_device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: new_capacity,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if !contents.is_empty() {
        render_queue.write_buffer(&buffer, 0, contents);
    }
    Some((buffer, new_capacity))
}

pub(super) fn grow_storage(
    render_device: &RenderDevice,
    capacity: u64,
    label: &'static str,
    required: u64,
) -> Option<(Buffer, u64)> {
    let required = required.max(16);
    if required <= capacity {
        return None;
    }
    let new_capacity = storage_capacity_for(required);
    Some((
        render_device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: new_capacity,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        }),
        new_capacity,
    ))
}

fn storage_capacity_for(required: u64) -> u64 {
    required.max(16).next_power_of_two()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_source_buffers_grow_geometrically() {
        assert_eq!(storage_capacity_for(0), 16);
        assert_eq!(storage_capacity_for(16), 16);
        assert_eq!(storage_capacity_for(17), 32);
        assert_eq!(storage_capacity_for(4_097), 8_192);
    }
}
