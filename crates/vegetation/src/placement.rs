//! Deterministic blade placement: one candidate per cell of a world-aligned lattice at the type's
//! spacing, jittered, gathered toward Voronoi clump centres, and thinned by coverage. The GPU
//! (`shaders/vegetation/placement/sampling.wesl`) reproduces this; tests compare the two.
use crate::{GrassClumps, VegetationFieldPage};

/// What placement needs of a type that grows on a lattice: grass or ground details.
pub trait Lattice {
    /// Every type grows on its own world-aligned lattice.
    fn seed(&self) -> u32;
    /// Metres between lattice candidates.
    fn spacing(&self) -> f32;
    fn clumps(&self) -> GrassClumps;
}

/// Clump feature points sit this far from their cell centres, as a share of the cell.
pub const CLUMP_JITTER: f32 = 0.9;
/// Width of the fade between neighbouring clumps, as a share of the clump size. Roots near a
/// boundary keep their own place and facing.
pub const CLUMP_SOFTNESS: f32 = 0.25;
/// Share of clumps' density that varies from clump to clump.
pub const CLUMP_DENSITY_VARIATION: f32 = 0.2;
/// Radians of random turn added to each blade's facing.
pub const FACING_JITTER: f32 = 0.35;

/// The lattice cells one page's candidates come from, including those whose roots move into the
/// page toward a clump centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidateDomain {
    pub cell_min: [i32; 2],
    pub cell_count: [u32; 2],
    pub spacing: f32,
}

impl CandidateDomain {
    pub fn candidate_count(self) -> u32 {
        self.cell_count[0].saturating_mul(self.cell_count[1])
    }
}

/// The clump a root belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clump {
    pub key: u32,
    pub center_xz: [f32; 2],
    /// Unit direction from the centre to the root.
    pub radial: [f32; 2],
    /// Distance from the centre, as a share of the largest possible.
    pub distance: f32,
    /// 1 inside the clump, falling to 0 at the boundary with its neighbour.
    pub influence: f32,
    /// Share of candidates the clump keeps here: fewer toward its edge and in sparser clumps.
    pub retention: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub root_xz: [f32; 2],
    pub clump: Clump,
    /// Unit direction the blade faces and leans toward.
    pub facing: [f32; 2],
    /// Stable, uniform 0..1 rank deciding acceptance against coverage: the blade's own, or as
    /// the type's `patches` say, its clump's.
    pub rank: f32,
    /// Nested four-way rank: distance thinning keeps the lowest ranks.
    pub lod_rank: f32,
    pub seed: u32,
}

pub fn candidate_domain(origin_xz: [f32; 2], size: f32, grass: &impl Lattice) -> CandidateDomain {
    let spacing = grass.spacing();
    let clumps = grass.clumps();
    let radius = clumps.size * std::f32::consts::SQRT_2 * clumps.pull;
    let minimum = [
        ((origin_xz[0] - radius) / spacing).floor() as i32,
        ((origin_xz[1] - radius) / spacing).floor() as i32,
    ];
    let maximum = [
        ((origin_xz[0] + size + radius) / spacing).ceil() as i32,
        ((origin_xz[1] + size + radius) / spacing).ceil() as i32,
    ];
    CandidateDomain {
        cell_min: minimum,
        cell_count: [
            maximum[0].saturating_sub(minimum[0]) as u32,
            maximum[1].saturating_sub(minimum[1]) as u32,
        ],
        spacing,
    }
}

pub fn sample_candidate(
    grass: &impl Lattice,
    domain: CandidateDomain,
    index: u32,
) -> Option<Candidate> {
    if index >= domain.candidate_count() {
        return None;
    }
    let type_seed = grass.seed();
    let cell_x = domain.cell_min[0] + (index % domain.cell_count[0]) as i32;
    let cell_z = domain.cell_min[1] + (index / domain.cell_count[0]) as i32;
    let seed = hash_cell(type_seed, cell_x, cell_z, 0x6d2b_79f5);
    let jittered = [
        (cell_x as f32 + random01(seed ^ 0xa511_e9b3)) * domain.spacing,
        (cell_z as f32 + random01(seed ^ 0x63d8_3595)) * domain.spacing,
    ];
    let clump = sample_clump(grass, type_seed, jittered);
    let pull = grass.clumps().pull * clump.influence;
    let root_xz = [
        jittered[0] + (clump.center_xz[0] - jittered[0]) * pull,
        jittered[1] + (clump.center_xz[1] - jittered[1]) * pull,
    ];
    // Four neighbouring cells hold one of each LOD lane, rotated per block, so keeping the
    // lowest quarter of ranks keeps one root in every 2×2 block.
    let block = [cell_x.div_euclid(2), cell_z.div_euclid(2)];
    let quadrant = cell_x.rem_euclid(2) as u32 + 2 * cell_z.rem_euclid(2) as u32;
    let lane = (quadrant + (hash_cell(type_seed, block[0], block[1], 0xa24b_aed5) & 3)) & 3;
    Some(Candidate {
        root_xz,
        clump,
        facing: facing(grass, clump, seed),
        rank: patch_rank(
            random01(seed ^ 0x4cf5_ad43),
            random01(clump.key ^ 0x2545_f491),
            grass.clumps().patches,
        ),
        lod_rank: (lane as f32 + random01(seed ^ 0x91e1_0da5)) * 0.25,
        seed,
    })
}

/// Whether a candidate grows on `page`: the page owns its root and its coverage, thinned by the
/// clump, exceeds its rank.
pub fn accepted(candidate: &Candidate, page: &VegetationFieldPage, coverage: f32) -> bool {
    page.owns(candidate.root_xz) && candidate.rank < coverage * candidate.clump.retention
}

/// `patches` of the way from the blade's rank to its clump's, mapped through the blend's
/// distribution so it stays uniform and coverage keeps the same share of blades.
pub fn patch_rank(blade: f32, clump: f32, patches: f32) -> f32 {
    let value = clump * patches + blade * (1.0 - patches);
    let wide = patches.max(1.0 - patches);
    let narrow = 1.0 - wide;
    if narrow < 1e-4 {
        return value;
    }
    if value < narrow {
        value * value / (2.0 * wide * narrow)
    } else if value < wide {
        (value - narrow * 0.5) / wide
    } else {
        1.0 - (1.0 - value) * (1.0 - value) / (2.0 * wide * narrow)
    }
}

fn facing(grass: &impl Lattice, clump: Clump, seed: u32) -> [f32; 2] {
    let angle = |s: u32| {
        let a = random01(s) * std::f32::consts::TAU;
        [a.cos(), a.sin()]
    };
    let own = angle(seed ^ 0x1656_67b1);
    let shared = angle(clump.key ^ 0x68e3_1da4);
    let c = grass.clumps();
    let grouped = c.facing * clump.influence;
    let splay = c.splay * clump.influence;
    let mixed = [
        shared[0] * grouped + clump.radial[0] * splay + own[0] * (1.0 - grouped),
        shared[1] * grouped + clump.radial[1] * splay + own[1] * (1.0 - grouped),
    ];
    let base = normalize_or(mixed, own);
    let turn = (random01(seed ^ 0x7f4a_7c15) * 2.0 - 1.0) * FACING_JITTER;
    let (sin, cos) = turn.sin_cos();
    [base[0] * cos - base[1] * sin, base[0] * sin + base[1] * cos]
}

/// The nearest of the 3×3 clump feature points around `root`, and how clearly it is nearest.
pub fn sample_clump(grass: &impl Lattice, type_seed: u32, root: [f32; 2]) -> Clump {
    let size = grass.clumps().size;
    let base = [
        (root[0] / size).floor() as i32,
        (root[1] / size).floor() as i32,
    ];
    let mut nearest = (f32::INFINITY, root, type_seed);
    let mut second = f32::INFINITY;
    for dz in -1..=1 {
        for dx in -1..=1 {
            let cell = [base[0] + dx, base[1] + dz];
            let key = hash_cell(type_seed, cell[0], cell[1], 0x4f1b_cdc9);
            let center = [
                (cell[0] as f32 + 0.5 + (random01(key ^ 0x9e37_79b9) - 0.5) * CLUMP_JITTER) * size,
                (cell[1] as f32 + 0.5 + (random01(key ^ 0x85eb_ca6b) - 0.5) * CLUMP_JITTER) * size,
            ];
            let d = (root[0] - center[0]).powi(2) + (root[1] - center[1]).powi(2);
            if d < nearest.0 {
                second = nearest.0;
                nearest = (d, center, key);
            } else if d < second {
                second = d;
            }
        }
    }
    let (distance, center, key) = (nearest.0.sqrt(), nearest.1, nearest.2);
    let influence = smoothstep01((second.sqrt() - distance) / (size * CLUMP_SOFTNESS));
    let normalized = (distance / (size * std::f32::consts::SQRT_2)).clamp(0.0, 1.0);
    // Tufted types thin out between clumps; even ones barely do.
    let edge = 1.0 - 0.6 * grass.clumps().pull;
    let spatial = 1.0 + (edge - 1.0) * normalized.powf(1.5);
    let clump = 1.0 - CLUMP_DENSITY_VARIATION * random01(key ^ 0xd1b5_4a35);
    Clump {
        key,
        center_xz: center,
        radial: normalize_or([root[0] - center[0], root[1] - center[1]], [1.0, 0.0]),
        distance: normalized,
        influence,
        retention: (spatial * clump).clamp(0.0, 1.0),
    }
}

pub fn hash32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

pub fn hash_cell(seed: u32, x: i32, z: i32, salt: u32) -> u32 {
    hash32(
        seed ^ (x as u32).wrapping_mul(0x8da6_b343) ^ (z as u32).wrapping_mul(0xd816_3841) ^ salt,
    )
}

pub fn random01(value: u32) -> f32 {
    hash32(value) as f32 * (1.0 / u32::MAX as f32)
}

fn normalize_or(value: [f32; 2], fallback: [f32; 2]) -> [f32; 2] {
    let length_squared = value[0] * value[0] + value[1] * value[1];
    if length_squared <= 1e-10 {
        return fallback;
    }
    let inverse_length = length_squared.sqrt().recip();
    [value[0] * inverse_length, value[1] * inverse_length]
}

fn smoothstep01(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn domains_are_world_aligned_across_page_boundaries() {
        let grass = fixtures::tufts();
        let left = candidate_domain([0.0, 0.0], 16.0, &grass);
        let right = candidate_domain([16.0, 0.0], 16.0, &grass);
        assert_eq!(left.spacing, right.spacing);
        assert!(left.cell_min[0] < right.cell_min[0]);
        // The margin covers roots pulled across the page edge.
        assert!((left.cell_min[0] as f32) * left.spacing < -grass.clumps.size * grass.clumps.pull);
    }

    #[test]
    fn candidates_are_stable_and_face_unit_directions() {
        let grass = fixtures::meadow();
        let domain = candidate_domain([0.0, 0.0], 16.0, &grass);
        let first = sample_candidate(&grass, domain, 17).unwrap();
        assert_eq!(first, sample_candidate(&grass, domain, 17).unwrap());
        assert!((first.facing[0].hypot(first.facing[1]) - 1.0).abs() < 1e-5);
        assert!(sample_candidate(&grass, domain, domain.candidate_count()).is_none());
    }

    #[test]
    fn clumps_are_bounded_and_pull_roots_toward_their_centres() {
        let grass = fixtures::tufts();
        let domain = candidate_domain([0.0, 0.0], 8.0, &grass);
        let mut closer = 0;
        for index in 0..domain.candidate_count() {
            let c = sample_candidate(&grass, domain, index).unwrap();
            let k = c.clump;
            assert!((0.0..=1.0).contains(&k.distance));
            assert!((0.0..=1.0).contains(&k.influence));
            assert!((0.0..=1.0).contains(&k.retention));
            let d = (c.root_xz[0] - k.center_xz[0]).hypot(c.root_xz[1] - k.center_xz[1]);
            closer +=
                usize::from(d <= k.distance * grass.clumps.size * std::f32::consts::SQRT_2 + 1e-4);
        }
        assert_eq!(closer, domain.candidate_count() as usize);
    }

    #[test]
    fn nested_lod_rank_keeps_one_candidate_per_two_by_two_block() {
        let grass = fixtures::meadow();
        let domain = CandidateDomain {
            cell_min: [-2, -2],
            cell_count: [4, 4],
            spacing: grass.spacing(),
        };
        let kept = (0..domain.candidate_count())
            .filter(|&i| sample_candidate(&grass, domain, i).unwrap().lod_rank < 0.25)
            .count();
        assert_eq!(kept, 4);
    }

    #[test]
    fn patch_rank_stays_uniform_for_every_blend() {
        for patches in [0.0, 0.2, 0.5, 0.8, 1.0] {
            let mut buckets = [0u32; 10];
            for i in 0..20_000u32 {
                let rank = patch_rank(random01(i), random01(hash32(i ^ 0x55)), patches);
                assert!((0.0..=1.0).contains(&rank));
                buckets[((rank * 10.0) as usize).min(9)] += 1;
            }
            assert!(
                buckets.iter().all(|&n| (1_700..=2_300).contains(&n)),
                "{patches}: {buckets:?}"
            );
        }
    }

    #[test]
    fn patchy_coverage_keeps_or_drops_whole_clumps() {
        let mut grass = fixtures::tufts();
        grass.clumps.patches = 1.0;
        let domain = candidate_domain([0.0, 0.0], 8.0, &grass);
        let mut ranks = std::collections::HashMap::new();
        for index in 0..domain.candidate_count() {
            let c = sample_candidate(&grass, domain, index).unwrap();
            assert_eq!(*ranks.entry(c.clump.key).or_insert(c.rank), c.rank);
        }
        assert!(ranks.len() > 20);
    }
}
