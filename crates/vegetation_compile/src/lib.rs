//! Deterministic, renderer-independent reference placement for vegetation.
//!
//! The runtime renderer performs equivalent placement on the GPU. This CPU reference gives
//! tests and diagnostics a single definition of page ownership, population competition and
//! stable thinning.

use thiserror::Error;
use vegetation::{
    CandidateSample, SceneValidationError, VegetationCatalog, VegetationFieldPage,
    VegetationPopulationField, VegetationPopulationId, VegetationSpeciesId,
    candidate_density_retention, candidate_domain, choose_species, random01, sample_candidate,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugPlacement {
    pub root: [f32; 3],
    pub surface_normal: [f32; 3],
    pub group_center_xz: [f32; 2],
    pub group_key: u32,
    pub group_distance: f32,
    pub group_influence: f32,
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

            let occupancy =
                local_occupancy(catalog, page, field, &sample) * sample.group.density_retention;
            if random01(sample.seed ^ 0x4cf5_ad43) >= occupancy {
                continue;
            }

            placements.push(DebugPlacement {
                root: [sample.root_xz[0], surface.height, sample.root_xz[1]],
                surface_normal: surface.normal,
                group_center_xz: sample.group.center_xz,
                group_key: sample.group.key,
                group_distance: sample.group.normalized_distance,
                group_influence: sample.group.boundary_influence,
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use vegetation::{VegetationScene, fixtures};

    use super::*;

    #[test]
    fn split_pages_match_one_combined_world_lattice() {
        let catalog = fixtures::reference_catalog();
        for population in [
            fixtures::DRY_TUFT_POPULATION_ID,
            fixtures::SHORT_FILL_POPULATION_ID,
        ] {
            let left = fixtures::full_coverage_page([0.0, 0.0], 16.0, population);
            let right = fixtures::full_coverage_page([16.0, 0.0], 16.0, population);
            let combined = fixtures::full_coverage_page([0.0, 0.0], 32.0, population);

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

            assert_eq!(split_seeds, combined_seeds, "population {population:?}");
        }
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
