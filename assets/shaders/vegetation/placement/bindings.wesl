// The placement bind group, shared by generation and the stable candidate cache.
import package::shaders::vegetation::types::{
    WorkItem, ProceduralInstance, DiagnosticInstance, Camera, Config, Telemetry,
};

struct SpeciesChoice {
    metadata: vec4<u32>,
    threshold: vec4<f32>,
    density: vec4<f32>,
    // xy: minimum/maximum height, z: short/tall bias, w: group height coherence
    height: vec4<f32>,
    // x: pair-below height (zero disables), yz: single/split high-topology radii
    packing: vec4<f32>,
}

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

// One source-stable acceptance bit per original candidate, preserving lattice dispatch order.
// Ready is published only after every word is written. Zero capacity means reference fallback.
struct CandidateCacheEntry {
    base: u32,
    capacity: u32,
    reserved: u32,
    ready: atomic<u32>,
}

@group(0) @binding(0) var<storage, read> work_items: array<WorkItem>;
@group(0) @binding(1) var<storage, read> choices: array<SpeciesChoice>;
@group(0) @binding(2) var<storage, read> coverage_values: array<f32>;
@group(0) @binding(3) var<storage, read> surface_samples: array<SurfaceSample>;
@group(0) @binding(4) var<storage, read_write> procedural_instances: array<ProceduralInstance>;
@group(0) @binding(5) var<storage, read_write> diagnostic_instances: array<DiagnosticInstance>;
@group(0) @binding(6) var<storage, read_write> draw_args: array<DrawIndexedIndirectArgs, 4>;
@group(0) @binding(7) var<storage, read> visible_work_items: array<u32>;
@group(0) @binding(8) var<uniform> config: Config;
@group(0) @binding(9) var<uniform> camera: Camera;
@group(0) @binding(10) var<storage, read_write> telemetry: Telemetry;
@group(0) @binding(11) var<storage, read_write> candidate_cache_entries: array<CandidateCacheEntry>;
@group(0) @binding(12) var<storage, read_write> candidate_acceptance_bits: array<u32>;
@group(0) @binding(13) var<storage, read> cache_build_items: array<u32>;
