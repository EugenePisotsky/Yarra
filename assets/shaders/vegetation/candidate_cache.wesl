// Stable candidate acceptance, cached per resident work item across camera movement.
import package::shaders::vegetation::random::random01;
import package::shaders::vegetation::types::WorkItem;
import package::shaders::vegetation::placement::bindings::{
    work_items, candidate_cache_entries, candidate_acceptance_bits, cache_build_items,
};
import package::shaders::vegetation::placement::sampling::{
    owns, sample_surface, sample_candidate, local_occupancy,
};

// Deliberately excludes all camera, wind, projected size and population LOD inputs.
fn stable_candidate_is_present(item: WorkItem, index: u32) -> bool {
    let candidate = sample_candidate(item, index);
    if (!owns(item, candidate.root) || candidate.stable_rank >= item.growth.w) { return false; }
    let surface = sample_surface(item, candidate.root);
    if (surface.validity < 0.5) { return false; }
    let occupancy = local_occupancy(item, candidate.root) * candidate.group_density;
    return random01(candidate.seed ^ 0x4cf5ad43u) < occupancy;
}

var<workgroup> acceptance: array<u32, 64>;

@compute @workgroup_size(64)
fn build_candidate_cache(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_index) lane: u32,
) {
    let index = cache_build_items[id.y];
    var present = false;
    if (id.x < candidate_cache_entries[index].capacity) {
        present = stable_candidate_is_present(work_items[index], id.x);
    }
    acceptance[lane] = select(0u, 1u, present);
    workgroupBarrier();
    if ((lane & 31u) == 0u && id.x < candidate_cache_entries[index].capacity) {
        var mask = 0u;
        for (var bit = 0u; bit < 32u; bit += 1u) {
            mask |= acceptance[lane + bit] << bit;
        }
        candidate_acceptance_bits[candidate_cache_entries[index].base + id.x / 32u] = mask;
    }
}

@compute @workgroup_size(4)
fn finish_candidate_cache(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = cache_build_items[id.x];
    if (index != 0xffffffffu) { atomicStore(&candidate_cache_entries[index].ready, 1u); }
}
