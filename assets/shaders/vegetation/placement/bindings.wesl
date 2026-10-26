// The placement bind group, shared by generation and the stable candidate cache.
import package::shaders::vegetation::types::{
    WorkItem, GrassType, Instance, Camera, Config, Telemetry,
};

struct SurfaceSample {
    height_validity: vec4<f32>,
    normal: vec4<f32>,
}

struct DrawIndexedIndirectArgs {
    index_count: atomic<u32>,
    instance_count: atomic<u32>,
    first_index: atomic<u32>,
    base_vertex: atomic<u32>,
    first_instance: atomic<u32>,
}

// Each detail bin's draw, then the blades each bin holds. A draw instance is a batch of
// BLADES_PER_BATCH blades (vegetation_render topology.rs), the high bin's batches first.
struct Draws {
    args: array<DrawIndexedIndirectArgs, 2>,
    blades: array<atomic<u32>, 2>,
}

// One source-stable acceptance bit per candidate, in lattice dispatch order.
// Ready is published only after every word is written. Zero capacity means reference fallback.
struct CandidateCacheEntry {
    base: u32,
    capacity: u32,
    reserved: u32,
    ready: atomic<u32>,
}

@group(0) @binding(0) var<storage, read> work_items: array<WorkItem>;
@group(0) @binding(1) var<storage, read> types: array<GrassType>;
@group(0) @binding(2) var<storage, read> coverage_values: array<f32>;
@group(0) @binding(3) var<storage, read> surface_samples: array<SurfaceSample>;
@group(0) @binding(4) var<storage, read_write> instances: array<Instance>;
@group(0) @binding(5) var<storage, read_write> draws: Draws;
@group(0) @binding(6) var<storage, read> visible_work_items: array<u32>;
@group(0) @binding(7) var<uniform> config: Config;
@group(0) @binding(8) var<uniform> camera: Camera;
@group(0) @binding(9) var<storage, read_write> telemetry: Telemetry;
@group(0) @binding(10) var<storage, read_write> candidate_cache_entries: array<CandidateCacheEntry>;
@group(0) @binding(11) var<storage, read_write> candidate_acceptance_bits: array<u32>;
@group(0) @binding(12) var<storage, read> cache_build_items: array<u32>;
// The pose pass's workgroup counts (pose.wesl).
@group(0) @binding(13) var<storage, read_write> pose_dispatch: array<u32, 3>;
