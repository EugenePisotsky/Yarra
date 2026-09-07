#import "shaders/vegetation_blade.wgsl"::{
    ProceduralInstance, Species, Camera, DebugConfig, PreparedArena, SPECIES_INDEX_MASK, prepare_blade,
}

struct DrawArgs {
    index_count: u32, instance_count: u32, first_index: u32, base_vertex: u32, first_instance: u32,
}
struct DispatchArgs { x: u32, y: u32, z: u32 }
@group(0) @binding(0) var<storage, read> instances: array<ProceduralInstance>;
@group(0) @binding(1) var<storage, read> species: array<Species>;
@group(0) @binding(2) var<uniform> camera: Camera;
@group(0) @binding(3) var<uniform> debug_config: DebugConfig;
@group(0) @binding(4) var<storage, read> draw_args: array<DrawArgs, 4>;
@group(0) @binding(5) var<storage, read_write> arena: PreparedArena;
@group(0) @binding(6) var<storage, read_write> dispatch_args: DispatchArgs;
@group(0) @binding(7) var<storage, read_write> telemetry: array<u32, 16>;

@compute @workgroup_size(1)
fn prepare_dispatch() {
    var total = 0u;
    var blades = 0u;
    var prepared = 0u;
    let capacity = arrayLength(&arena.blades);
    for (var bin = 0u; bin < 4u; bin += 1u) {
        let count = draw_args[bin].instance_count;
        let per_instance = select(1u, 2u, bin >= 2u);
        total += count;
        prepared += min(count, (capacity - min(blades, capacity)) / per_instance) * per_instance;
        blades += count * per_instance;
    }
    dispatch_args = DispatchArgs((total + 63u) / 64u, 1u, 1u);
    telemetry[11] = select(0u, prepared, debug_config.workload.y != 0u);
    telemetry[12] = select(0u, blades - prepared, debug_config.workload.y != 0u);
}

@compute @workgroup_size(64)
fn prepare(@builtin(global_invocation_id) id: vec3<u32>) {
    var local = id.x;
    var base = 0u;
    var bin = 0u;
    for (; bin < 4u; bin += 1u) {
        let count = draw_args[bin].instance_count;
        if (local < count) { break; }
        local -= count;
        base += count * select(1u, 2u, bin >= 2u);
    }
    if (bin >= 4u) { return; }
    let per_instance = select(1u, 2u, bin >= 2u);
    let first = base + local * per_instance;
    let index = draw_args[bin].first_instance + local;
    if (first + per_instance > arrayLength(&arena.blades)) {
        arena.indices[index] = 0u;
        return;
    }
    let instance = instances[index];
    let profile = species[instance.geometry.y & SPECIES_INDEX_MASK];
    arena.blades[first] = prepare_blade(instance, profile, camera, debug_config, 0u);
    if (per_instance == 2u) {
        arena.blades[first + 1u] = prepare_blade(instance, profile, camera, debug_config, 1u);
    }
    arena.indices[index] = first + 1u;
}
