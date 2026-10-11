//! Low-frequency diagnostics shared by the main and render worlds.
use bevy::prelude::*;
use std::sync::{Arc, RwLock};

/// A low-frequency snapshot of source lifetime and GPU placement work.
///
/// GPU values are copied into a tiny staging buffer without waiting for the device, so they are
/// normally one or more frames behind the scene counters. This is intentional: diagnostics must
/// not perturb the timings they are meant to explain.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VegetationDiagnosticsSnapshot {
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
    /// High- and low-detail blades.
    pub eligible_instances: [u32; 2],
    pub emitted_instances: [u32; 2],
    pub capacity_dropped_instances: [u32; 2],
    /// Indices submitted by the two blade draws for the sampled frame.
    pub submitted_indices: u64,
    /// Blade vertices shaded for the sampled frame.
    pub blade_vertices: u64,
    /// Fixed number of blade instance records in the active device profile.
    pub instance_capacity: u32,
    pub bin_capacities: [u32; 2],
    pub instance_bytes: u64,
    /// Metres around the detail centre within which blades take the high-detail mesh.
    pub high_detail_radius: f32,
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
