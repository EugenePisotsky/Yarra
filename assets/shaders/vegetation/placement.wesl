// Placement: projected-size LOD and bounded classify-once emission.
//
// Every candidate performs placement, terrain, visibility and LOD work exactly once. Accepted
// instances append directly to the high- or low-detail arena. The capacities are emergency
// guards, not population controls: a nonzero capacity-drop counter rejects the content/device
// budget and must be fixed rather than accepted as normal rendering behavior.
import package::shaders::vegetation::types::{
    WorkItem, TYPE_INDEX_MASK, LOD_MORPH_MASK, LOD_MORPH_SHIFT, DENSITY_FADE_BAND, PI,
};
import package::shaders::vegetation::placement::bindings::{
    work_items, types, instances, draws, visible_work_items, config, telemetry,
    candidate_cache_entries, candidate_acceptance_bits, pose_dispatch,
};
import package::shaders::vegetation::placement::sampling::{
    Candidate, SurfaceResult, sample_surface, sample_candidate, candidate_grows, sample_coverage, owns,
};
import package::shaders::vegetation::placement::lod::{
    MINIMUM_EXTENT_PIXELS, candidate_is_visible, projected_blade_extent_pixels,
    projected_spacing_pixels, population_density, detail_weight,
};

struct CandidateEvaluation {
    candidate: Candidate,
    surface: SurfaceResult,
    // 0 high detail, 1 low detail.
    bin: u32,
    lod_morph: f32,
    population_density: f32,
    eligible: bool,
}

const WORKGROUP_SIZE: u32 = constants::WORKGROUP_SIZE;
const HIGH_CAPACITY: u32 = constants::HIGH_CAPACITY;
const LOW_CAPACITY: u32 = constants::LOW_CAPACITY;

fn use_candidate_cache(index: u32) -> bool {
    return config.workload.z != 0u && atomicLoad(&candidate_cache_entries[index].ready) != 0u;
}

fn evaluate_candidate(item: WorkItem, index: u32) -> CandidateEvaluation {
    var result: CandidateEvaluation;
    let grass = types[item.field.w];
    var candidate = sample_candidate(item, grass, index);
    let surface = sample_surface(item, candidate.root);
    result.candidate = candidate;
    result.surface = surface;
    if (!candidate_grows(item, candidate, surface) || !candidate_is_visible(candidate, surface, grass)) {
        return result;
    }
    // Blades at the edge of worn or bare ground (a path, a wheel track) turn to lean out over it,
    // as grass overhangs a road's edges.
    candidate.facing = edge_facing(item, candidate);
    result.candidate = candidate;
    if (projected_blade_extent_pixels(candidate, surface, grass) < MINIMUM_EXTENT_PIXELS) {
        return result;
    }
    let density = population_density(projected_spacing_pixels(grass, candidate, surface));
    // A narrow, stable rank band past the target stays alive so the draw shader can narrow
    // retiring blades away. Centring the band on the target keeps the integrated width.
    if (density < 0.999 && candidate.lod_rank >= min(density + DENSITY_FADE_BAND * 0.5, 1.0)) {
        return result;
    }
    let weight = detail_weight(candidate);
    result.bin = select(1u, 0u, weight > 0.0);
    result.lod_morph = weight;
    result.population_density = density;
    result.eligible = true;
    return result;
}

// Where coverage falls off within a texel or two, the facing turns toward the barer side, the
// more the steeper the fall. Samples beyond the page count as the root's own coverage.
fn edge_facing(item: WorkItem, candidate: Candidate) -> vec2<f32> {
    let step = item.page.z / f32(item.field.z);
    let root = candidate.root;
    let here = sample_coverage(item, root);
    var toward = vec2(0.0);
    for (var i = 0u; i < 4u; i += 1u) {
        let d = select(vec2(0.0, select(-1.0, 1.0, i == 3u)), vec2(select(-1.0, 1.0, i == 1u), 0.0), i < 2u);
        let p = root + d * step;
        let c = select(here, sample_coverage(item, p), owns(item, p));
        toward += d * (here - c);
    }
    let edge = clamp(length(toward) * 1.2, 0.0, 0.85);
    if (edge < 0.05) {
        return candidate.facing;
    }
    let mixed = mix(candidate.facing, normalize(toward), edge);
    let length_mixed = length(mixed);
    return select(candidate.facing, mixed / length_mixed, length_mixed > 1e-4);
}

fn bin_capacity(bin: u32) -> u32 {
    return select(LOW_CAPACITY, HIGH_CAPACITY, bin == 0u);
}

fn bin_offset(bin: u32) -> u32 {
    return select(HIGH_CAPACITY, 0u, bin == 0u);
}

@compute @workgroup_size(WORKGROUP_SIZE, 1, 1)
fn generate(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let item_index = visible_work_items[invocation.y];
    let item = work_items[item_index];
    if (invocation.x >= item.field.x) { return; }
    if (use_candidate_cache(item_index)) {
        let mask = candidate_acceptance_bits[candidate_cache_entries[item_index].base + invocation.x / 32u];
        if ((mask & (1u << (invocation.x & 31u))) == 0u) { return; }
    }
    if (config.workload.y != 0u) {
        atomicAdd(&telemetry.values[1], 1u);
    }
    let evaluation = evaluate_candidate(item, invocation.x);
    if (!evaluation.eligible) {
        return;
    }
    let bin = evaluation.bin;
    if (config.workload.y != 0u) {
        atomicAdd(&telemetry.values[2u + bin], 1u);
    }
    let local_slot = atomicAdd(&draws.blades[bin], 1u);
    if (local_slot >= bin_capacity(bin)) {
        if (config.workload.y != 0u) {
            atomicAdd(&telemetry.values[6u + bin], 1u);
        }
        return;
    }
    let candidate = evaluation.candidate;
    let slot = bin_offset(bin) + local_slot;
    let facing_angle = fract(atan2(candidate.facing.y, candidate.facing.x) / (2.0 * PI) + 1.0);
    instances[slot].root_clump = vec4<f32>(
        candidate.root.x,
        evaluation.surface.height,
        candidate.root.y,
        bitcast<f32>(pack2x16unorm(vec2<f32>(
            f32(candidate.clump.key & 0xffffu) / 65535.0,
            candidate.lod_rank,
        ))),
    );
    instances[slot].geometry = vec4<u32>(
        pack2x16unorm(vec2<f32>(facing_angle, candidate.clump.distance)),
        (item.field.w & TYPE_INDEX_MASK)
            | (u32(round(clamp(evaluation.lod_morph, 0.0, 1.0) * f32(LOD_MORPH_MASK))) << LOD_MORPH_SHIFT)
            | (bin << 31u),
        pack2x16snorm(evaluation.surface.normal.xz),
        (candidate.seed & 0x00ffffffu)
            | (u32(round(clamp(evaluation.population_density, 0.0, 1.0) * 255.0)) << 24u),
    );
}

const BLADES_PER_BATCH: u32 = constants::BLADES_PER_BATCH;

@compute @workgroup_size(1, 1, 1)
fn finalize() {
    var widest = 0u;
    for (var bin = 0u; bin < 2u; bin += 1u) {
        let blades = min(atomicLoad(&draws.blades[bin]), bin_capacity(bin));
        let batches = (blades + BLADES_PER_BATCH - 1u) / BLADES_PER_BATCH;
        atomicStore(&draws.blades[bin], blades);
        atomicStore(&draws.args[bin].instance_count, batches);
        atomicStore(&draws.args[bin].base_vertex, 0u);
        atomicStore(&draws.args[bin].first_instance, bin_offset(bin) / BLADES_PER_BATCH);
        widest = max(widest, batches * BLADES_PER_BATCH);
    }
    // The pose pass takes a workgroup row per bin, as wide as the fuller bin's batches.
    pose_dispatch[0] = (widest + WORKGROUP_SIZE - 1u) / WORKGROUP_SIZE;
    pose_dispatch[1] = 2u;
    pose_dispatch[2] = 1u;
    atomicStore(&draws.args[0].index_count, constants::HIGH_BATCH_INDEX_COUNT);
    atomicStore(&draws.args[0].first_index, constants::HIGH_FIRST_INDEX);
    atomicStore(&draws.args[1].index_count, constants::LOW_BATCH_INDEX_COUNT);
    atomicStore(&draws.args[1].first_index, constants::LOW_FIRST_INDEX);
}
