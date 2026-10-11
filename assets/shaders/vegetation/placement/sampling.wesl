// Candidates on a grass type's lattice and the page data under them: coverage, terrain surface
// and Voronoi clumps. Mirrors crates/vegetation/src/placement.rs, which tests compare against.
import package::shaders::vegetation::types::{WorkItem, GrassType};
import package::shaders::vegetation::random::{hash32, hash_cell, random01};
import package::shaders::vegetation::placement::bindings::{
    coverage_values, surface_samples, camera,
};

struct SurfaceResult {
    height: f32,
    normal: vec3<f32>,
    validity: f32,
}

struct Clump {
    key: u32,
    // Canonical world XZ.
    center: vec2<f32>,
    radial: vec2<f32>,
    // Distance from the centre, as a share of the largest possible.
    distance: f32,
    // 1 inside the clump, falling to 0 at the boundary with its neighbour.
    influence: f32,
    // Share of candidates the clump keeps here.
    retention: f32,
}

struct Candidate {
    // Render-space XZ.
    root: vec2<f32>,
    clump: Clump,
    facing: vec2<f32>,
    rank: f32,
    lod_rank: f32,
    seed: u32,
}

const PI_2: f32 = 6.283185307179586;
// vegetation::placement constants.
const CLUMP_JITTER: f32 = 0.9;
const CLUMP_SOFTNESS: f32 = 0.25;
const CLUMP_DENSITY_VARIATION: f32 = 0.2;
const FACING_JITTER: f32 = 0.35;

fn normalize_or(value: vec2<f32>, fallback: vec2<f32>) -> vec2<f32> {
    let length_squared = dot(value, value);
    if (length_squared <= 1e-10) {
        return fallback;
    }
    return value * inverseSqrt(length_squared);
}

fn type_seed(grass: GrassType) -> u32 {
    return bitcast<u32>(grass.placement.z);
}

fn owns(item: WorkItem, world_xz: vec2<f32>) -> bool {
    return all(world_xz >= item.page.xy) && all(world_xz < item.page.xy + vec2<f32>(item.page.z));
}

fn sample_coverage(item: WorkItem, world_xz: vec2<f32>) -> f32 {
    if (!owns(item, world_xz)) {
        return 0.0;
    }
    let resolution = item.field.z;
    let local = clamp((world_xz - item.page.xy) / item.page.z, vec2<f32>(0.0), vec2<f32>(0.999999));
    let texel = vec2<u32>(local * f32(resolution));
    return coverage_values[item.field.y + texel.y * resolution + texel.x];
}

fn sample_surface(item: WorkItem, world_xz: vec2<f32>) -> SurfaceResult {
    let resolution = item.surface.y;
    let local = clamp((world_xz - item.page.xy) / item.page.z, vec2<f32>(0.0), vec2<f32>(1.0));
    let grid = local * f32(resolution - 1u);
    let minimum = vec2<u32>(floor(grid));
    let maximum = min(minimum + vec2<u32>(1u), vec2<u32>(resolution - 1u));
    let blend = fract(grid);
    let indices = array<u32, 4>(
        item.surface.x + minimum.y * resolution + minimum.x,
        item.surface.x + minimum.y * resolution + maximum.x,
        item.surface.x + maximum.y * resolution + minimum.x,
        item.surface.x + maximum.y * resolution + maximum.x,
    );
    // Same 00--11 triangle diagonal as the ground mesh and CPU surface_triangle_weights.
    var weights = array<f32, 4>(1.0 - blend.x, blend.x - blend.y, 0.0, blend.y);
    if (blend.y >= blend.x) {
        weights = array<f32, 4>(1.0 - blend.y, 0.0, blend.y - blend.x, blend.x);
    }
    var height = 0.0;
    var normal = vec3<f32>(0.0);
    var validity = 0.0;
    for (var corner = 0u; corner < 4u; corner += 1u) {
        let sample = surface_samples[indices[corner]];
        height += sample.height_validity.x * weights[corner];
        validity += sample.height_validity.y * weights[corner];
        normal += sample.normal.xyz * weights[corner];
    }
    if (dot(normal, normal) <= 1e-10) {
        normal = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        normal = normalize(normal);
    }
    return SurfaceResult(height, normal, validity);
}

// The nearest of the 3×3 clump feature points around `root`, and how clearly it is nearest.
fn sample_clump(grass: GrassType, seed: u32, root: vec2<f32>) -> Clump {
    let size = grass.clumps.x;
    let base = vec2<i32>(floor(root / size));
    var nearest = 1e30;
    var second = 1e30;
    var center = root;
    var key = seed;
    for (var dz = -1; dz <= 1; dz += 1) {
        for (var dx = -1; dx <= 1; dx += 1) {
            let cell = base + vec2<i32>(dx, dz);
            let cell_key = hash_cell(seed, cell, 0x4f1bcdc9u);
            let point = (vec2<f32>(cell) + 0.5 + (vec2(
                random01(cell_key ^ 0x9e3779b9u),
                random01(cell_key ^ 0x85ebca6bu),
            ) - 0.5) * CLUMP_JITTER) * size;
            let delta = root - point;
            let d = dot(delta, delta);
            if (d < nearest) {
                second = nearest;
                nearest = d;
                center = point;
                key = cell_key;
            } else if (d < second) {
                second = d;
            }
        }
    }
    let distance = sqrt(nearest);
    let influence = smoothstep(0.0, 1.0, (sqrt(second) - distance) / (size * CLUMP_SOFTNESS));
    let normalized = clamp(distance / (size * 1.41421356237), 0.0, 1.0);
    // Tufted types thin out between clumps; even ones barely do.
    let edge = 1.0 - 0.6 * grass.clumps.y;
    let spatial = 1.0 + (edge - 1.0) * pow(normalized, 1.5);
    let variation = 1.0 - CLUMP_DENSITY_VARIATION * random01(key ^ 0xd1b54a35u);
    return Clump(
        key,
        center,
        normalize_or(root - center, vec2<f32>(1.0, 0.0)),
        normalized,
        influence,
        clamp(spatial * variation, 0.0, 1.0),
    );
}

fn angle_direction(value: u32) -> vec2<f32> {
    let angle = random01(value) * PI_2;
    return vec2<f32>(cos(angle), sin(angle));
}

// `patches` of the way from the blade's rank to its clump's, mapped through the blend's
// distribution so it stays uniform and coverage keeps the same share of blades.
fn patch_rank(blade: f32, clump: f32, patches: f32) -> f32 {
    let value = clump * patches + blade * (1.0 - patches);
    let wide = max(patches, 1.0 - patches);
    let narrow = 1.0 - wide;
    if (narrow < 1e-4) {
        return value;
    }
    if (value < narrow) {
        return value * value / (2.0 * wide * narrow);
    }
    if (value < wide) {
        return (value - narrow * 0.5) / wide;
    }
    return 1.0 - (1.0 - value) * (1.0 - value) / (2.0 * wide * narrow);
}

fn sample_candidate(item: WorkItem, grass: GrassType, index: u32) -> Candidate {
    let seed_of_type = type_seed(grass);
    let spacing = grass.placement.y;
    let count_x = u32(item.domain.z);
    let cell = vec2<i32>(
        item.domain.x + i32(index % count_x),
        item.domain.y + i32(index / count_x),
    );
    let seed = hash_cell(seed_of_type, cell, 0x6d2b79f5u);
    let jittered = (vec2<f32>(cell) + vec2(
        random01(seed ^ 0xa511e9b3u),
        random01(seed ^ 0x63d83595u),
    )) * spacing;
    let clump = sample_clump(grass, seed_of_type, jittered);
    let root = jittered + (clump.center - jittered) * (grass.clumps.y * clump.influence);
    // Four neighbouring cells hold one of each LOD lane, rotated per block, so keeping the
    // lowest quarter of ranks keeps one root in every 2×2 block.
    let block = vec2<i32>(floor(vec2<f32>(cell) * 0.5));
    let quadrant = u32(cell.x - block.x * 2) + 2u * u32(cell.y - block.y * 2);
    let lane = (quadrant + (hash_cell(seed_of_type, block, 0xa24baed5u) & 3u)) & 3u;

    let own = angle_direction(seed ^ 0x165667b1u);
    let clump_facing = angle_direction(clump.key ^ 0x68e31da4u);
    let grouped = grass.clumps.w * clump.influence;
    let splay = grass.placement.x * clump.influence;
    let mixed = clump_facing * grouped + clump.radial * splay + own * (1.0 - grouped);
    let base_facing = normalize_or(mixed, own);
    let turn = (random01(seed ^ 0x7f4a7c15u) * 2.0 - 1.0) * FACING_JITTER;
    let facing = vec2<f32>(
        base_facing.x * cos(turn) - base_facing.y * sin(turn),
        base_facing.x * sin(turn) + base_facing.y * cos(turn),
    );
    var render_clump = clump;
    render_clump.center = clump.center - camera.render_origin.xy;
    return Candidate(
        root - camera.render_origin.xy,
        render_clump,
        facing,
        patch_rank(random01(seed ^ 0x4cf5ad43u), random01(clump.key ^ 0x2545f491u), grass.dry_tip.w),
        (f32(lane) + random01(seed ^ 0x91e10da5u)) * 0.25,
        seed,
    );
}

// Camera-independent acceptance: the page owns the root, its ground is valid, and coverage,
// thinned by the clump, exceeds the candidate's rank (shared by the clump as the type patches).
fn candidate_grows(item: WorkItem, candidate: Candidate, surface: SurfaceResult) -> bool {
    return owns(item, candidate.root)
        && surface.validity >= 0.5
        && candidate.rank < sample_coverage(item, candidate.root) * candidate.clump.retention;
}
