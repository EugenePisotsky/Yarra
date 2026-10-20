//! Resident GPU allocation, bind groups and geometric storage growth.
use super::{
    candidate_cache,
    generation::{GenerationInputs, GenerationKey},
    gpu_types::{
        CameraGpu, ConfigGpu, DRAW_ARGS_SIZE, GPU_TELEMETRY_SIZE, INSTANCE_CAPACITY, InstanceGpu,
        WorkItemGpu,
    },
    page_slots,
    pipelines::VegetationPipelines,
    topology::build_topology_indices,
    trample::TrampleMap,
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
    pub(super) coverage: Buffer,
    pub(super) coverage_capacity: u64,
    pub(super) surfaces: Buffer,
    pub(super) surfaces_capacity: u64,
    /// Which ranges of `surfaces` and `coverage` hold each resident page.
    pub(super) page_slots: page_slots::PageSlots,
    pub(super) types: Buffer,
    pub(super) types_capacity: u64,
    pub(super) instances: Buffer,
    pub(super) topology_indices: Buffer,
    pub(super) args: Buffer,
    pub(super) gpu_telemetry: Buffer,
    pub(super) camera: Buffer,
    pub(super) config: Buffer,
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
    pub(super) camera_values: Option<CameraGpu>,
    pub(super) work_item_count: u32,
    pub(super) maximum_candidate_count: u32,
    pub(super) high_detail_radius: f32,
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
        let work_items = dummy_storage(render_device, "vegetation empty work items");
        let visible_work_items =
            dummy_storage(render_device, "vegetation empty visible work queue");
        let candidate_dispatch_args = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation candidate dispatch args"),
            size: 12,
            usage: BufferUsages::STORAGE | BufferUsages::INDIRECT | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let coverage = dummy_storage(render_device, "vegetation empty coverage");
        let surfaces = dummy_storage(render_device, "vegetation empty surfaces");
        let types = dummy_storage(render_device, "vegetation empty grass types");
        let instances = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation blade instances"),
            size: u64::from(INSTANCE_CAPACITY) * size_of::<InstanceGpu>() as u64,
            usage: BufferUsages::STORAGE
                | if cfg!(test) {
                    BufferUsages::COPY_SRC | BufferUsages::COPY_DST
                } else {
                    BufferUsages::empty()
                },
            mapped_at_creation: false,
        });
        let topology_indices = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("vegetation blade topology indices"),
            contents: bytemuck::cast_slice(&build_topology_indices()),
            usage: BufferUsages::INDEX,
        });
        let args = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation indirect args"),
            size: DRAW_ARGS_SIZE,
            usage: BufferUsages::STORAGE
                | BufferUsages::INDIRECT
                | BufferUsages::COPY_DST
                | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let gpu_telemetry = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation GPU telemetry"),
            size: GPU_TELEMETRY_SIZE,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let camera = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation camera"),
            size: size_of::<CameraGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let config = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation configuration"),
            size: size_of::<ConfigGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let schedule_bind_group = create_schedule_bind_group(
            render_device,
            &schedule_layout,
            [
                &work_items,
                &visible_work_items,
                &candidate_dispatch_args,
                &camera,
                &gpu_telemetry,
                &config,
                &types,
            ],
        );
        let compute_bind_group = create_compute_bind_group(
            render_device,
            &compute_layout,
            [
                &work_items,
                &types,
                &coverage,
                &surfaces,
                &instances,
                &args,
                &visible_work_items,
                &config,
                &camera,
                &gpu_telemetry,
                &candidate_cache.entries,
                &candidate_cache.acceptance_bits,
                &candidate_cache.build_items,
            ],
        );
        let draw_bind_group = create_draw_bind_group(
            render_device,
            &draw_layout,
            &instances,
            &types,
            &camera,
            world.resource::<TrampleMap>(),
        );
        Self {
            work_items,
            work_items_capacity: 16,
            visible_work_items,
            visible_work_items_capacity: 16,
            candidate_dispatch_args,
            coverage,
            coverage_capacity: 16,
            surfaces,
            surfaces_capacity: 16,
            page_slots: default(),
            types,
            types_capacity: 16,
            instances,
            topology_indices,
            args,
            gpu_telemetry,
            camera,
            config,
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
            camera_values: None,
            work_item_count: 0,
            maximum_candidate_count: 0,
            high_detail_radius: 0.0,
            active: false,
        }
    }
}

pub(super) fn create_schedule_bind_group(
    render_device: &RenderDevice,
    layout: &BindGroupLayout,
    buffers: [&Buffer; 7],
) -> BindGroup {
    render_device.create_bind_group(
        Some("vegetation visible work scheduling"),
        layout,
        &BindGroupEntries::sequential((
            buffers[0].as_entire_binding(),
            buffers[1].as_entire_binding(),
            buffers[2].as_entire_binding(),
            buffers[3].as_entire_binding(),
            buffers[4].as_entire_binding(),
            buffers[5].as_entire_binding(),
            buffers[6].as_entire_binding(),
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
    buffers: [&Buffer; 13],
) -> BindGroup {
    render_device.create_bind_group(
        Some("vegetation placement"),
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
        )),
    )
}

pub(super) fn create_draw_bind_group(
    render_device: &RenderDevice,
    layout: &BindGroupLayout,
    instances: &Buffer,
    types: &Buffer,
    camera: &Buffer,
    trample: &TrampleMap,
) -> BindGroup {
    render_device.create_bind_group(
        Some("vegetation draw"),
        layout,
        &BindGroupEntries::sequential((
            instances.as_entire_binding(),
            types.as_entire_binding(),
            camera.as_entire_binding(),
            &trample.current_view,
            &trample.previous_view,
            &trample.sampler,
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

/// An empty CPU-written storage buffer for at least `required` bytes, and its capacity.
pub(super) fn writable_storage(
    render_device: &RenderDevice,
    label: &'static str,
    required: u64,
) -> (Buffer, u64) {
    let capacity = storage_capacity_for(required);
    let buffer = render_device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: capacity,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (buffer, capacity)
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
