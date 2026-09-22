//! Cache only stable production acceptance, not view-dependent visibility or LOD.
use super::{
    buffers::{VegetationBuffers, dummy_storage, grow_storage, update_storage},
    gpu_types::WorkItemGpu,
    pipelines::VegetationPipelines,
};
use crate::{VegetationDebugSettings, VegetationDiagnostics, VegetationProfileMode};
use bevy::{
    prelude::*,
    render::{
        render_resource::{Buffer, ComputePassDescriptor, ComputePipelineId, PipelineCache},
        renderer::{RenderContext, RenderDevice, RenderQueue},
    },
};

const MAX_MASK_BYTES: u64 = 1024 * 1024;
const MAX_BUILD_LANES: u32 = 262_144;
const MAX_BUILD_ITEMS: usize = 4;

#[derive(Resource)]
pub(super) struct CandidateCache {
    pub entries: Buffer,
    pub acceptance_bits: Buffer,
    pub build_items: Buffer,
    // Base word, candidate count, reserved, GPU ready. Every candidate has its own bit.
    descriptors: Vec<[u32; 4]>,
    pending: Vec<u32>,
    completed: usize,
    pipelines: Option<[ComputePipelineId; 2]>,
    pub serial: u64,
}

impl FromWorld for CandidateCache {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>();
        Self {
            entries: dummy_storage(device, "vegetation candidate cache entries"),
            acceptance_bits: dummy_storage(device, "vegetation candidate acceptance bits"),
            build_items: dummy_storage(device, "vegetation candidate cache build queue"),
            descriptors: Vec::new(),
            pending: Vec::new(),
            completed: 0,
            pipelines: None,
            serial: 0,
        }
    }
}

fn plan(items: &[WorkItemGpu], position: Vec3, byte_budget: u64) -> (Vec<[u32; 4]>, Vec<u32>, u64) {
    let mut order: Vec<_> = (0..items.len() as u32).collect();
    order.sort_by(|&a, &b| {
        let distance = |i: u32| {
            let page = items[i as usize].page;
            let center = Vec2::new(page[0], page[1]) + Vec2::splat(page[2] * 0.5);
            center.distance_squared(Vec2::new(position.x, position.z))
        };
        distance(a).total_cmp(&distance(b))
    });
    let mut descriptors = vec![[0; 4]; items.len()];
    let mut pending = Vec::new();
    let mut indices = 0u64;
    for index in order {
        let count = items[index as usize].layout[1];
        if count == 0
            || count > MAX_BUILD_LANES
            || (indices + u64::from(count.div_ceil(32))) * 4 > byte_budget
        {
            continue;
        }
        descriptors[index as usize] = [indices as u32, count, 0, 0];
        pending.push(index);
        indices += u64::from(count.div_ceil(32));
    }
    (descriptors, pending, indices * 4)
}

impl CandidateCache {
    /// Source revision changes invalidate all cached acceptance, including peer competition.
    /// Camera motion never changes this plan. Uncached fields remain on the reference path.
    pub fn prepare(
        &mut self,
        device: &RenderDevice,
        queue: &RenderQueue,
        items: &[WorkItemGpu],
        position: Vec3,
    ) -> bool {
        let (descriptors, pending, bytes) = plan(items, position, MAX_MASK_BYTES);
        self.descriptors = descriptors;
        self.pending = pending;
        self.completed = 0;
        self.serial = self.serial.wrapping_add(1);
        let mut replaced = false;
        if let Some((buffer, _)) = update_storage(
            device,
            queue,
            &self.entries,
            self.entries.size(),
            "vegetation candidate cache entries",
            bytemuck::cast_slice(&self.descriptors),
        ) {
            self.entries = buffer;
            replaced = true;
        }
        if let Some((buffer, _)) = grow_storage(
            device,
            self.acceptance_bits.size(),
            "vegetation candidate acceptance bits",
            bytes,
        ) {
            self.acceptance_bits = buffer;
            replaced = true;
        }
        debug_assert!(self.acceptance_bits.size() <= MAX_MASK_BYTES);
        replaced
    }
}

pub(super) fn build(
    mut context: RenderContext,
    mut cache: ResMut<CandidateCache>,
    buffers: Res<VegetationBuffers>,
    pipelines: Res<VegetationPipelines>,
    pipeline_cache: Res<PipelineCache>,
    queue: Res<RenderQueue>,
    settings: Res<VegetationDebugSettings>,
    diagnostics: Res<VegetationDiagnostics>,
) {
    let enabled = settings.candidate_cache_enabled
        && settings.mode as u32 == 0
        && matches!(
            settings.profile_mode,
            VegetationProfileMode::Full | VegetationProfileMode::ComputeOnly
        );
    diagnostics.update(|s| {
        s.candidate_cache_enabled = enabled;
        s.candidate_cache_bytes =
            cache.entries.size() + cache.acceptance_bits.size() + cache.build_items.size();
        s.candidate_cache_ready_items = cache.completed as u32;
        s.candidate_cache_planned_items = cache.pending.len() as u32;
    });
    if !enabled || !buffers.active {
        return;
    }
    let (Some(build), Some(finish)) = (
        pipeline_cache.get_compute_pipeline(pipelines.cache_build),
        pipeline_cache.get_compute_pipeline(pipelines.cache_finish),
    ) else {
        return;
    };
    let ids = [build.id(), finish.id()];
    if cache.pipelines != Some(ids) {
        // Shader reload may change stable acceptance even if source buffers are unchanged.
        for (index, entry) in cache.descriptors.iter().enumerate() {
            if entry[1] != 0 {
                context.command_encoder().clear_buffer(
                    &cache.entries,
                    index as u64 * 16 + 8,
                    Some(8),
                );
            }
        }
        cache.completed = 0;
        cache.serial = cache.serial.wrapping_add(1);
        cache.pipelines = Some(ids);
    }
    let mut selected = [u32::MAX; MAX_BUILD_ITEMS];
    let mut count = 0;
    let mut maximum = 0;
    for &index in &cache.pending[cache.completed..] {
        let next_maximum = maximum.max(cache.descriptors[index as usize][1].div_ceil(64) * 64);
        if count == MAX_BUILD_ITEMS || next_maximum * (count as u32 + 1) > MAX_BUILD_LANES {
            break;
        }
        maximum = next_maximum;
        selected[count] = index;
        count += 1;
    }
    if count == 0 {
        return;
    }
    queue.write_buffer(&cache.build_items, 0, bytemuck::cast_slice(&selected));
    {
        let mut pass = context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("vegetation stable candidate cache build"),
                ..default()
            });
        pass.set_bind_group(0, &buffers.compute_bind_group, &[]);
        pass.set_pipeline(build);
        pass.dispatch_workgroups(maximum.div_ceil(64), count as u32, 1);
    }
    {
        // Separate pass: make acceptance bits visible before publishing ready to generation.
        let mut pass = context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("vegetation candidate cache publish"),
                ..default()
            });
        pass.set_bind_group(0, &buffers.compute_bind_group, &[]);
        pass.set_pipeline(finish);
        pass.dispatch_workgroups(1, 1, 1);
    }
    cache.completed += count;
    cache.serial = cache.serial.wrapping_add(1);
    diagnostics.update(|s| {
        s.candidate_cache_builds += count as u64;
        s.candidate_cache_ready_items = cache.completed as u32;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::packing::pack_scene;
    #[test]
    fn planning_has_bounded_nonoverlapping_slots_and_reference_fallback() {
        let mut items = pack_scene(&vegetation::fixtures::reference_scene()).work_items;
        for (i, item) in items.iter_mut().enumerate() {
            item.layout[1] = 100 + i as u32;
        }
        items[0].layout[1] = MAX_BUILD_LANES + 1;
        let (entries, pending, bytes) = plan(&items, Vec3::ZERO, 1200);
        assert!(bytes <= 1200 && pending.len() > 1);
        assert_eq!(entries[0], [0; 4]);
        let mut end = 0;
        for i in pending {
            let e = entries[i as usize];
            assert_eq!(e[0], end);
            assert_eq!(e[1], items[i as usize].layout[1]);
            assert_eq!(&e[2..], &[0, 0]);
            end += e[1].div_ceil(32);
        }
        assert_eq!(u64::from(end) * 4, bytes);
    }
}
