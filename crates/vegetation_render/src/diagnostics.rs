//! Low-frequency diagnostics shared by the main and render worlds.
use bevy::prelude::*;
use std::sync::{Arc, RwLock};

/// A low-frequency snapshot of source lifetime and GPU placement work.
///
/// GPU values are copied into a tiny staging buffer without waiting for the device, so they are
/// normally one or more frames behind the scene counters. This is intentional: diagnostics must
/// not perturb the timings they are meant to explain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VegetationDiagnosticsSnapshot {
    pub blade_preparation_enabled: bool,
    pub blade_preparation_bytes: u64,
    pub blade_preparation_dispatches: u64,
    pub blade_preparation_reuses: u64,
    pub prepared_blades: u32,
    pub preparation_fallback_blades: u32,
    pub scene_revision: u64,
    /// Cumulative completed placement dispatches and frames that reused their results.
    pub generation_dispatches: u64,
    pub generation_reuses: u64,
    pub candidate_cache_enabled: bool,
    pub candidate_cache_bytes: u64,
    pub candidate_cache_builds: u64,
    pub candidate_cache_ready_items: u32,
    pub candidate_cache_planned_items: u32,
    pub source_repacks: u64,
    /// Cumulative backing-buffer growth events. This should stop after warm-up traversal.
    pub source_buffer_reallocations: u64,
    /// Bytes uploaded for the most recent source revision.
    pub source_uploaded_bytes: u64,
    /// Grow-only capacity retained by resident source and scheduling buffers.
    pub source_buffer_capacity_bytes: u64,
    pub source_pages: u32,
    pub source_work_items: u32,
    pub maximum_candidates_per_item: u32,
    pub scheduled_work_items: u32,
    /// Padded compute lanes in the sampled indirect dispatch.
    pub dispatched_candidate_lanes: u32,
    pub candidate_evaluations: u32,
    pub eligible_instances: [u32; 4],
    pub emitted_instances: [u32; 4],
    pub capacity_dropped_instances: [u32; 4],
    /// Indices submitted by the four procedural draws for the sampled frame.
    pub submitted_indices: u64,
    /// Maximum distinct topology inputs per instance before the GPU's post-transform cache.
    pub topology_vertex_inputs: u64,
    /// Fixed number of compact procedural instance records in the active device profile.
    pub procedural_instance_capacity: u32,
    /// Current scene-adaptive capacities for single-high/single-low/split-high/split-low.
    pub topology_instance_capacities: [u32; 4],
    pub procedural_instance_bytes: u64,
    pub gpu_samples: u64,
    /// Scene revision associated with the last completed GPU readback (not CPU preparation).
    pub gpu_scene_revision: u64,
}

/// Thread-safe bridge used by the main and render worlds for vegetation diagnostics.
#[derive(Resource, Clone, Debug, Default)]
pub struct VegetationDiagnostics {
    snapshot: Arc<RwLock<VegetationDiagnosticsSnapshot>>,
}

impl VegetationDiagnostics {
    pub fn snapshot(&self) -> VegetationDiagnosticsSnapshot {
        *self
            .snapshot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn update(&self, update: impl FnOnce(&mut VegetationDiagnosticsSnapshot)) {
        let mut snapshot = self
            .snapshot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update(&mut snapshot);
    }
}
