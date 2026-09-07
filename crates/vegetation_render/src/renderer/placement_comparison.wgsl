// Appended to the actual production compute shader by the native GPU regression test.
@group(0) @binding(14) var<storage, read_write> comparison: array<u32>;

@compute @workgroup_size(64)
fn compare_candidates(@builtin(global_invocation_id) id: vec3<u32>) {
    let item = work_items[id.y];
    if (id.x >= item.candidate_layout.y) { return; }
    let a = evaluate_candidate(item, id.x, false);
    let b = evaluate_candidate(item, id.x, true);
    var result = 1u + a.eligible * 2u;
    if (a.eligible != b.eligible) { result |= 4u; }
    if (a.eligible != 0u && b.eligible != 0u) {
        if (a.bin != b.bin || a.species_index != b.species_index
            || a.lod_morph != b.lod_morph || a.population_density != b.population_density
            || a.occupancy != b.occupancy || a.outcome != b.outcome
            || any(a.candidate.root != b.candidate.root)
            || any(a.candidate.direction != b.candidate.direction)
            || any(a.candidate.group_center != b.candidate.group_center)
            || a.candidate.stable_rank != b.candidate.stable_rank
            || a.candidate.lod_rank != b.candidate.lod_rank
            || a.candidate.clump_variant != b.candidate.clump_variant
            || a.candidate.group_distance != b.candidate.group_distance
            || a.candidate.group_influence != b.candidate.group_influence
            || a.candidate.group_density != b.candidate.group_density
            || a.candidate.seed != b.candidate.seed
            || a.surface.height != b.surface.height
            || any(a.surface.normal != b.surface.normal)
            || a.surface.validity != b.surface.validity) { result |= 8u; }
    }
    comparison[id.y * debug_config.workload.w + id.x] = result;
}
