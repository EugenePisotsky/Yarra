//! Deterministic, renderer-independent compilation and reference placement for vegetation V2.
//!
//! The runtime renderer will perform equivalent placement on the GPU. Keeping a CPU reference
//! here gives cooking tools, tests, and diagnostics a single definition of page ownership,
//! population competition, and stable thinning.

use std::collections::BTreeMap;

use thiserror::Error;
use vegetation::{
    CandidateSample, SceneValidationError, VegetationCatalog, VegetationFieldPage,
    VegetationFieldPageData, VegetationPopulationField, VegetationPopulationId,
    VegetationSpeciesId, candidate_density_retention, candidate_domain, choose_species, random01,
    sample_candidate,
};

#[derive(Debug, Clone, PartialEq)]
pub struct SourcePopulationMask {
    pub population: VegetationPopulationId,
    pub resolution: u16,
    pub coverage: Vec<u8>,
    pub flow_direction: [f32; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceFieldPage {
    /// Repeated population masks are combined with a maximum operation.
    pub masks: Vec<SourcePopulationMask>,
}

/// Compiles authoring masks into the renderer-neutral page contract.
///
/// Maximum blending makes repeated strokes idempotent. Flow directions are accumulated and
/// normalized, which prevents source layer order from changing the cooked result.
pub fn compile_page(
    catalog: &VegetationCatalog,
    source: SourceFieldPage,
) -> Result<VegetationFieldPageData, CompileError> {
    catalog.validate()?;
    let mut merged = BTreeMap::<VegetationPopulationId, MergedMask>::new();
    for mask in source.masks {
        if catalog.population(mask.population).is_none() {
            return Err(CompileError::MissingPopulation(mask.population));
        }
        let resolution = usize::from(mask.resolution);
        if resolution == 0
            || resolution > 256
            || mask.coverage.len() != resolution * resolution
            || !mask.flow_direction.into_iter().all(f32::is_finite)
        {
            return Err(CompileError::InvalidMask(mask.population));
        }

        match merged.entry(mask.population) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(MergedMask {
                    resolution: mask.resolution,
                    coverage: mask.coverage,
                    flow_directions: vec![mask.flow_direction],
                });
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let merged = entry.get_mut();
                if merged.resolution != mask.resolution {
                    return Err(CompileError::ResolutionMismatch(mask.population));
                }
                for (destination, source) in merged.coverage.iter_mut().zip(mask.coverage) {
                    *destination = (*destination).max(source);
                }
                merged.flow_directions.push(mask.flow_direction);
            }
        }
    }

    let page = VegetationFieldPageData {
        fields: merged
            .into_iter()
            .map(|(population, mask)| VegetationPopulationField {
                population,
                resolution: mask.resolution,
                coverage: mask.coverage,
                flow_direction: stable_average_direction(mask.flow_directions),
            })
            .collect(),
    };
    page.validate(catalog).map_err(CompileError::InvalidPage)?;
    Ok(page)
}

#[derive(Debug, Clone, PartialEq)]
struct MergedMask {
    resolution: u16,
    coverage: Vec<u8>,
    flow_directions: Vec<[f32; 2]>,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum CompileError {
    #[error(transparent)]
    InvalidCatalog(#[from] vegetation::CatalogValidationError),
    #[error("source mask references missing vegetation population {0:?}")]
    MissingPopulation(VegetationPopulationId),
    #[error("source mask for vegetation population {0:?} is invalid")]
    InvalidMask(VegetationPopulationId),
    #[error("source masks for vegetation population {0:?} use different resolutions")]
    ResolutionMismatch(VegetationPopulationId),
    #[error(transparent)]
    InvalidPage(#[from] vegetation::PageValidationError),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugPlacement {
    pub root: [f32; 3],
    pub surface_normal: [f32; 3],
    pub parent_xz: [f32; 2],
    pub rest_direction: [f32; 2],
    pub species: VegetationSpeciesId,
    pub population: VegetationPopulationId,
    pub competition_group: Option<u16>,
    pub clump_variant: f32,
    pub seed: u32,
}

pub fn generate_scene_debug_placements(
    scene: &vegetation::VegetationScene,
) -> Result<Vec<DebugPlacement>, PlacementError> {
    scene.validate()?;
    let mut placements = Vec::new();
    for page in &scene.pages {
        placements.extend(generate_page_debug_placements(&scene.catalog, page)?);
    }
    Ok(placements)
}

pub fn generate_page_debug_placements(
    catalog: &VegetationCatalog,
    page: &VegetationFieldPage,
) -> Result<Vec<DebugPlacement>, PlacementError> {
    catalog.validate()?;
    page.validate(catalog)?;

    let mut placements = Vec::new();
    for field in &page.fields {
        let population = catalog
            .population(field.population)
            .expect("validated page population");
        let domain = candidate_domain(page, population);
        let density_retention = candidate_density_retention(population);

        for candidate_index in 0..domain.candidate_count() {
            let sample =
                sample_candidate(population, domain, candidate_index, field.flow_direction)
                    .expect("candidate index came from the domain");
            if !page.owns(sample.root_xz) || sample.stable_rank >= density_retention {
                continue;
            }

            let surface = page
                .surface
                .sample(page.origin_xz, page.size, sample.root_xz);
            if surface.validity < 0.5 {
                continue;
            }

            let occupancy = local_occupancy(catalog, page, field, &sample);
            if random01(sample.seed ^ 0x4cf5_ad43) >= occupancy {
                continue;
            }

            placements.push(DebugPlacement {
                root: [sample.root_xz[0], surface.height, sample.root_xz[1]],
                surface_normal: surface.normal,
                parent_xz: sample.parent_xz,
                rest_direction: sample.rest_direction,
                species: choose_species(population, random01(sample.seed ^ 0xd1b5_4a35)),
                population: population.id,
                competition_group: population.competition_group,
                clump_variant: sample.clump_variant,
                seed: sample.seed,
            });
        }
    }
    Ok(placements)
}

/// Returns a probability that spends one local density budget across competing populations.
///
/// Without competition, coverage directly gates a population. For a competition group, each
/// population receives a share of the strongest requested local density rather than every painted
/// density accumulating. This supports mixed fields without multiplying overdraw in overlaps.
fn local_occupancy(
    catalog: &VegetationCatalog,
    page: &VegetationFieldPage,
    field: &VegetationPopulationField,
    sample: &CandidateSample,
) -> f32 {
    let population = catalog
        .population(field.population)
        .expect("validated population field");
    let own_coverage = field.sample_coverage(page, sample.root_xz);
    let Some(group) = population.competition_group else {
        return own_coverage;
    };

    let mut total_requested_density = 0.0_f32;
    let mut strongest_requested_density = 0.0_f32;
    for peer_field in &page.fields {
        let peer_population = catalog
            .population(peer_field.population)
            .expect("validated population field");
        if peer_population.competition_group == Some(group) {
            let requested_density = peer_population.density_per_square_meter
                * peer_field.sample_coverage(page, sample.root_xz);
            total_requested_density += requested_density;
            strongest_requested_density = strongest_requested_density.max(requested_density);
        }
    }
    if total_requested_density <= f32::EPSILON {
        return 0.0;
    }
    (strongest_requested_density * own_coverage / total_requested_density).clamp(0.0, 1.0)
}

#[derive(Debug, Error)]
pub enum PlacementError {
    #[error(transparent)]
    Scene(#[from] SceneValidationError),
    #[error(transparent)]
    Catalog(#[from] vegetation::CatalogValidationError),
    #[error(transparent)]
    Page(#[from] vegetation::PageValidationError),
}

fn stable_average_direction(mut directions: Vec<[f32; 2]>) -> [f32; 2] {
    directions.sort_by(|left, right| {
        left[0]
            .total_cmp(&right[0])
            .then_with(|| left[1].total_cmp(&right[1]))
    });
    let sum = directions.into_iter().fold([0.0_f64; 2], |sum, value| {
        [sum[0] + f64::from(value[0]), sum[1] + f64::from(value[1])]
    });
    let length_squared = sum[0] * sum[0] + sum[1] * sum[1];
    if length_squared <= 1e-10 {
        return [0.0, 0.0];
    }
    let inverse_length = length_squared.sqrt().recip();
    [
        (sum[0] * inverse_length) as f32,
        (sum[1] * inverse_length) as f32,
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use vegetation::{VegetationScene, fixtures};

    use super::*;

    #[test]
    fn repeated_masks_are_max_blended_and_flow_is_order_independent() {
        let catalog = fixtures::reference_catalog();
        let first = SourcePopulationMask {
            population: fixtures::DRY_TUFT_POPULATION_ID,
            resolution: 2,
            coverage: vec![10, 240, 30, 40],
            flow_direction: [1.0, 0.0],
        };
        let second = SourcePopulationMask {
            population: fixtures::DRY_TUFT_POPULATION_ID,
            resolution: 2,
            coverage: vec![20, 30, 220, 5],
            flow_direction: [0.0, 1.0],
        };
        let source = |masks| SourceFieldPage { masks };

        let forward = compile_page(&catalog, source(vec![first.clone(), second.clone()])).unwrap();
        let reverse = compile_page(&catalog, source(vec![second, first])).unwrap();

        assert_eq!(forward, reverse);
        assert_eq!(forward.fields[0].coverage, vec![20, 240, 220, 40]);
        assert!(
            (forward.fields[0].flow_direction[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6
        );
    }

    #[test]
    fn split_pages_match_one_combined_world_lattice() {
        let catalog = fixtures::reference_catalog();
        let left = fixtures::full_coverage_page([0.0, 0.0], 16.0, fixtures::DRY_TUFT_POPULATION_ID);
        let right =
            fixtures::full_coverage_page([16.0, 0.0], 16.0, fixtures::DRY_TUFT_POPULATION_ID);
        let combined =
            fixtures::full_coverage_page([0.0, 0.0], 32.0, fixtures::DRY_TUFT_POPULATION_ID);

        let split_seeds = generate_page_debug_placements(&catalog, &left)
            .unwrap()
            .into_iter()
            .chain(generate_page_debug_placements(&catalog, &right).unwrap())
            .map(|placement| placement.seed)
            .collect::<HashSet<_>>();
        let combined_seeds = generate_page_debug_placements(&catalog, &combined)
            .unwrap()
            .into_iter()
            .filter(|placement| placement.root[2] < 16.0)
            .map(|placement| placement.seed)
            .collect::<HashSet<_>>();

        assert_eq!(split_seeds, combined_seeds);
    }

    #[test]
    fn competition_spends_a_shared_density_budget() {
        let mut scene = fixtures::reference_scene();
        scene.pages[0]
            .fields
            .retain(|field| field.population != fixtures::DRY_TUFT_POPULATION_ID);
        scene.pages.truncate(1);
        let competing_count = generate_scene_debug_placements(&scene).unwrap().len();

        for population in &mut scene.catalog.populations {
            population.competition_group = None;
        }
        let independent_count = generate_scene_debug_placements(&scene).unwrap().len();

        assert!(competing_count > 0);
        assert!(competing_count < independent_count);
    }

    #[test]
    fn reference_scene_is_deterministic() {
        let scene: VegetationScene = fixtures::reference_scene();
        let first = generate_scene_debug_placements(&scene).unwrap();
        let second = generate_scene_debug_placements(&scene).unwrap();
        assert_eq!(first, second);
    }
}
