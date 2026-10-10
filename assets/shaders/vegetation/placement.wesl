// Placement: projected-size LOD and bounded classify-once emission.
//
// Every candidate performs expensive placement, terrain, competition, species, visibility, and LOD
// work exactly once. Accepted instances append directly to one of four bounded topology arenas. The
// capacities are emergency guards, not population controls: a nonzero capacity-drop counter rejects
// the content/device budget and must be fixed rather than accepted as normal rendering behavior.
import package::shaders::vegetation::types::{
    WorkItem, QUARTER_LOD_FLAG, SPECIES_INDEX_MASK, LOD_MORPH_MASK, LOD_MORPH_SHIFT,
    DENSITY_MODE_BALANCED, DENSITY_MODE_FULL_REFERENCE, SPLIT_LOW_DENSITY_BUDGET_SCALE,
};
import package::shaders::vegetation::random::random01;
import package::shaders::vegetation::lod::{
    near_field_coverage, quarter_candidate_count, remap_quarter_candidate,
};
import package::shaders::vegetation::placement::bindings::{
    work_items, procedural_instances, diagnostic_instances, draw_args, visible_work_items, config,
    camera, telemetry, candidate_cache_entries, candidate_acceptance_bits,
};
import package::shaders::vegetation::placement::sampling::{
    Candidate, SurfaceResult, owns, sample_surface, sample_candidate, local_occupancy,
    choose_species_choice,
};
import package::shaders::vegetation::placement::lod::{
    candidate_is_visible, blade_extent_limits_pixels, generated_candidate_height,
    candidate_topology_class, projected_population_spacing_pixels, population_lod_density,
    balanced_population_lod_density, population_lod_retention_limit,
};

struct CandidateEvaluation {
    candidate: Candidate,
    surface: SurfaceResult,
    occupancy: f32,
    outcome: u32,
    species_index: u32,
    bin: u32,
    lod_morph: f32,
    population_density: f32,
    eligible: u32,
}

const WORKGROUP_SIZE: u32 = constants::WORKGROUP_SIZE;
const SINGLE_HIGH_CAPACITY: u32 = constants::SINGLE_HIGH_CAPACITY;
const SPLIT_HIGH_CAPACITY: u32 = constants::SPLIT_HIGH_CAPACITY;
const LOW_DETAIL_CAPACITY: u32 = constants::LOW_DETAIL_CAPACITY;
const LOW_DETAIL_MINIMUM_PARTITION: u32 = constants::LOW_DETAIL_MINIMUM_PARTITION;
const SPLIT_HIGH_OFFSET: u32 = SINGLE_HIGH_CAPACITY;
const LOW_DETAIL_OFFSET: u32 = SINGLE_HIGH_CAPACITY + SPLIT_HIGH_CAPACITY;
const MAX_DIAGNOSTIC_INSTANCES: u32 = constants::MAX_DIAGNOSTIC_INSTANCES;
const WORK_ITEM_INDEX_MASK: u32 = 0x7fffffffu;
const DIAGNOSTIC_INDEX_COUNT: u32 = constants::DIAGNOSTIC_INDEX_COUNT;
const SINGLE_HIGH_INDEX_COUNT: u32 = constants::SINGLE_HIGH_INDEX_COUNT;
const SINGLE_LOW_INDEX_COUNT: u32 = constants::SINGLE_LOW_INDEX_COUNT;
const SPLIT_HIGH_INDEX_COUNT: u32 = constants::SPLIT_HIGH_INDEX_COUNT;
const SPLIT_LOW_INDEX_COUNT: u32 = constants::SPLIT_LOW_INDEX_COUNT;
const DIAGNOSTIC_FIRST_INDEX: u32 = constants::DIAGNOSTIC_FIRST_INDEX;
const SINGLE_HIGH_FIRST_INDEX: u32 = constants::SINGLE_HIGH_FIRST_INDEX;
const SINGLE_LOW_FIRST_INDEX: u32 = constants::SINGLE_LOW_FIRST_INDEX;
const SPLIT_HIGH_FIRST_INDEX: u32 = constants::SPLIT_HIGH_FIRST_INDEX;
const SPLIT_LOW_FIRST_INDEX: u32 = constants::SPLIT_LOW_FIRST_INDEX;

fn use_candidate_cache(index: u32, quarter_lod: bool) -> bool {
    return !quarter_lod && config.values.x == 0u && (config.workload.w & 2u) != 0u
        && atomicLoad(&candidate_cache_entries[index].ready) != 0u;
}

fn evaluate_candidate(item: WorkItem, candidate_index: u32, early_rejection: bool) -> CandidateEvaluation {
    // Rejected production candidates never reach emission. Avoid height, projection, and LOD
    // calculations for them; retain the full evaluation for visual diagnostic modes.
    var rejected: CandidateEvaluation;
    let reject_early = early_rejection && config.values.x == 0u;
    let candidate = sample_candidate(item, candidate_index);
    if (reject_early && (!owns(item, candidate.root) || candidate.stable_rank >= item.growth.w)) {
        return rejected;
    }
    let surface = sample_surface(item, candidate.root);
    if (reject_early && surface.validity < 0.5) {
        return rejected;
    }
    let occupancy = local_occupancy(item, candidate.root) * candidate.group_density;
    var outcome = 0u;
    if (candidate.stable_rank >= item.growth.w) {
        outcome = 1u;
    } else if (surface.validity < 0.5) {
        outcome = 2u;
    } else if (random01(candidate.seed ^ 0x4cf5ad43u) >= occupancy) {
        outcome = 3u;
    }
    if (reject_early && outcome != 0u) {
        return rejected;
    }

    let choice = choose_species_choice(item, random01(candidate.seed ^ 0xd1b54a35u));
    if (reject_early && !candidate_is_visible(candidate, surface, choice)) {
        return rejected;
    }
    let generated_height = generated_candidate_height(choice, candidate);
    let topology_class = candidate_topology_class(choice, generated_height);
    let high_radius = select(choice.packing.y, choice.packing.z, topology_class != 0u);
    let extent_limits = blade_extent_limits_pixels(
        candidate,
        surface,
        choice,
        high_radius,
    );
    let projected_extent = min(extent_limits.x, extent_limits.y);
    let projected_spacing = projected_population_spacing_pixels(item, candidate, surface);
    let authored_population_density = population_lod_density(choice, projected_spacing);
    var population_density = authored_population_density;
    if (config.values.y == DENSITY_MODE_BALANCED) {
        population_density = balanced_population_lod_density(projected_spacing);
    } else if (config.values.y == DENSITY_MODE_FULL_REFERENCE) {
        population_density = 1.0;
    }
    let near_coverage = near_field_coverage(candidate.root, camera);
    if (config.values.y == DENSITY_MODE_BALANCED) {
        population_density = max(population_density, near_coverage);
    }
    // Remove the paired-root reduction gradually inside the gameplay focus region.
    // Far grass still uses the existing low-density, three-triangle representation.
    if (topology_class != 0u && config.values.y != DENSITY_MODE_FULL_REFERENCE) {
        let near_scale = select(0.0, near_coverage, config.values.y == DENSITY_MODE_BALANCED);
        population_density *= mix(SPLIT_LOW_DENSITY_BUDGET_SCALE, 1.0, near_scale);
    }
    var lod = 0u;
    if (projected_extent < choice.threshold.y) {
        lod = 1u;
    }
    let lod_morph = select(
        smoothstep(choice.threshold.y, choice.threshold.y * 1.45, projected_extent),
        0.0,
        lod != 0u,
    );
    let bin = select(0u, topology_class * 2u + lod, config.values.x == 0u);
    var eligible = 1u;
    if (!owns(item, candidate.root)) {
        eligible = 0u;
    } else if (!candidate_is_visible(candidate, surface, choice)) {
        eligible = 0u;
    } else if (
        config.values.x == 0u
        && (projected_extent < choice.threshold.w
            || (lod == 1u
                && candidate.lod_rank >= population_lod_retention_limit(population_density, topology_class)))
    ) {
        eligible = 0u;
    } else if (outcome != 0u && config.values.x != 3u) {
        eligible = 0u;
    } else if (config.values.x == 2u && item.peers.w != 1u) {
        eligible = 0u;
    } else if (config.values.x == 4u && item.peers.w == 0u) {
        eligible = 0u;
    }
    return CandidateEvaluation(
        candidate,
        surface,
        occupancy,
        outcome,
        choice.metadata.x,
        bin,
        lod_morph,
        population_density,
        eligible,
    );
}

fn active_capacity(bin: u32) -> u32 {
    if (config.values.x != 0u) {
        return select(0u, MAX_DIAGNOSTIC_INSTANCES, bin == 0u);
    }
    switch bin {
        case 0u: { return SINGLE_HIGH_CAPACITY; }
        case 1u: {
            return clamp(
                config.values.w,
                LOW_DETAIL_MINIMUM_PARTITION,
                LOW_DETAIL_CAPACITY - LOW_DETAIL_MINIMUM_PARTITION,
            );
        }
        case 2u: { return SPLIT_HIGH_CAPACITY; }
        default: {
            return LOW_DETAIL_CAPACITY - clamp(
                config.values.w,
                LOW_DETAIL_MINIMUM_PARTITION,
                LOW_DETAIL_CAPACITY - LOW_DETAIL_MINIMUM_PARTITION,
            );
        }
    }
}

fn instance_offset(bin: u32) -> u32 {
    switch bin {
        case 0u: { return 0u; }
        case 1u: { return LOW_DETAIL_OFFSET; }
        case 2u: { return SPLIT_HIGH_OFFSET; }
        default: {
            return LOW_DETAIL_OFFSET + active_capacity(1u);
        }
    }
}

@compute @workgroup_size(WORKGROUP_SIZE, 1, 1)
fn generate(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let encoded_work_item = visible_work_items[invocation.y];
    let quarter_lod = (encoded_work_item & QUARTER_LOD_FLAG) != 0u;
    let work_item_index = encoded_work_item & WORK_ITEM_INDEX_MASK;
    let item = work_items[work_item_index];
    let cached = use_candidate_cache(work_item_index, quarter_lod);
    var candidate_count = item.candidate_layout.y;
    if (quarter_lod) {
        candidate_count = quarter_candidate_count(item);
    }
    if (invocation.x >= candidate_count) { return; }
    var candidate_index = invocation.x;
    if (quarter_lod) {
        candidate_index = remap_quarter_candidate(item, invocation.x);
    }
    if (candidate_index == 0xffffffffu) {
        return;
    }
    if (cached) {
        let mask = candidate_acceptance_bits[candidate_cache_entries[work_item_index].base + candidate_index / 32u];
        if ((mask & (1u << (candidate_index & 31u))) == 0u) { return; }
    }
    if (config.workload.y != 0u) {
        atomicAdd(&telemetry.values[1], 1u);
    }
    let evaluation = evaluate_candidate(item, candidate_index, (config.workload.w & 1u) != 0u);
    if (evaluation.eligible == 0u) {
        return;
    }
    let bin = evaluation.bin;
    if (config.workload.y != 0u) {
        atomicAdd(&telemetry.values[2u + bin], 1u);
    }

    let local_slot = atomicAdd(&draw_args[bin].instance_count, 1u);
    if (local_slot >= active_capacity(bin)) {
        if (config.workload.y != 0u) {
            atomicAdd(&telemetry.values[6u + bin], 1u);
        }
        return;
    }
    if (config.values.x == 0u) {
        let slot = instance_offset(bin) + local_slot;
        procedural_instances[slot].root_clump = vec4<f32>(
            evaluation.candidate.root.x,
            evaluation.surface.height,
            evaluation.candidate.root.y,
            bitcast<f32>(pack2x16unorm(vec2<f32>(
                evaluation.candidate.clump_variant,
                evaluation.candidate.lod_rank,
            ))),
        );
        procedural_instances[slot].geometry = vec4<u32>(
            pack2x16snorm(evaluation.candidate.direction),
            (evaluation.species_index & SPECIES_INDEX_MASK)
                | (u32(round(clamp(evaluation.lod_morph, 0.0, 1.0) * f32(LOD_MORPH_MASK)))
                    << LOD_MORPH_SHIFT)
                | ((evaluation.bin & 1u) << 31u),
            pack2x16snorm(evaluation.surface.normal.xz),
            (evaluation.candidate.seed & 0x00ffffffu)
                | (u32(round(clamp(evaluation.population_density, 0.0, 1.0) * 255.0)) << 24u),
        );
        return;
    }

    let parent_surface = sample_surface(item, evaluation.candidate.group_center);
    let parent_height = select(
        evaluation.surface.height,
        parent_surface.height,
        parent_surface.validity >= 0.5,
    );
    diagnostic_instances[local_slot].root_direction = vec4<f32>(
        evaluation.candidate.root.x,
        evaluation.surface.height,
        evaluation.candidate.root.y,
        evaluation.candidate.direction.x,
    );
    diagnostic_instances[local_slot].direction_species = vec4<f32>(
        evaluation.candidate.direction.y,
        evaluation.candidate.clump_variant,
        f32(evaluation.species_index),
        bitcast<f32>(pack2x16snorm(evaluation.surface.normal.xz)),
    );
    diagnostic_instances[local_slot].parent_status = vec4<f32>(
        evaluation.candidate.group_center.x,
        parent_height,
        evaluation.candidate.group_center.y,
        f32(evaluation.outcome),
    );
    diagnostic_instances[local_slot].diagnostics = vec4<f32>(
        evaluation.occupancy,
        evaluation.candidate.stable_rank,
        evaluation.candidate.group_distance,
        evaluation.candidate.group_influence,
    );
}

@compute @workgroup_size(1, 1, 1)
fn finalize() {
    let diagnostic_mode = config.values.x != 0u;
    for (var bin = 0u; bin < 4u; bin += 1u) {
        atomicStore(
            &draw_args[bin].instance_count,
            min(atomicLoad(&draw_args[bin].instance_count), active_capacity(bin)),
        );
        atomicStore(&draw_args[bin].base_vertex, 0u);
    }
    atomicStore(
        &draw_args[0].index_count,
        select(SINGLE_HIGH_INDEX_COUNT, DIAGNOSTIC_INDEX_COUNT, diagnostic_mode),
    );
    atomicStore(
        &draw_args[0].first_index,
        select(SINGLE_HIGH_FIRST_INDEX, DIAGNOSTIC_FIRST_INDEX, diagnostic_mode),
    );
    atomicStore(&draw_args[0].first_instance, 0u);

    atomicStore(&draw_args[1].index_count, SINGLE_LOW_INDEX_COUNT);
    atomicStore(&draw_args[1].first_index, SINGLE_LOW_FIRST_INDEX);
    atomicStore(
        &draw_args[1].first_instance,
        LOW_DETAIL_OFFSET,
    );

    atomicStore(&draw_args[2].index_count, SPLIT_HIGH_INDEX_COUNT);
    atomicStore(&draw_args[2].first_index, SPLIT_HIGH_FIRST_INDEX);
    atomicStore(
        &draw_args[2].first_instance,
        SPLIT_HIGH_OFFSET,
    );

    atomicStore(&draw_args[3].index_count, SPLIT_LOW_INDEX_COUNT);
    atomicStore(&draw_args[3].first_index, SPLIT_LOW_FIRST_INDEX);
    atomicStore(
        &draw_args[3].first_instance,
        LOW_DETAIL_OFFSET + active_capacity(1u),
    );
}
