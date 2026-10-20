//! Deterministic, renderer-independent reference placement for grass.
//!
//! The runtime renderer places the same blades on the GPU. This CPU reference gives tests and
//! diagnostics a single definition of page ownership, coverage acceptance and clumping.

use thiserror::Error;

use crate::{
    GrassTypeId, SceneValidationError, VegetationCatalog, VegetationFieldPage, accepted,
    candidate_domain, sample_candidate,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugPlacement {
    pub root: [f32; 3],
    pub surface_normal: [f32; 3],
    pub grass: GrassTypeId,
    pub clump_center_xz: [f32; 2],
    pub clump_key: u32,
    pub clump_distance: f32,
    pub facing: [f32; 2],
    pub seed: u32,
}

pub fn generate_scene_debug_placements(
    scene: &crate::VegetationScene,
) -> Result<Vec<DebugPlacement>, PlacementError> {
    scene.validate()?;
    let mut placements = Vec::new();

    for page in &scene.pages {
        placements.extend(generate_page_debug_placements(&scene.catalog, page)?);
    }

    Ok(placements)
}

/// Every blade `page` grows: one candidate per lattice cell of each field's type, kept where
/// the page owns its root, its ground is valid and coverage beats its rank.
pub fn generate_page_debug_placements(
    catalog: &VegetationCatalog,
    page: &VegetationFieldPage,
) -> Result<Vec<DebugPlacement>, PlacementError> {
    catalog.validate()?;
    page.validate(catalog)?;
    let mut placements = Vec::new();

    for field in &page.fields {
        let grass = catalog.grass(field.grass).expect("validated page grass");
        let domain = candidate_domain(page.origin_xz, page.size, grass);

        for index in 0..domain.candidate_count() {
            let candidate =
                sample_candidate(grass, domain, index).expect("index came from the domain");

            if !page.owns(candidate.root_xz) {
                continue;
            }

            let surface = page
                .surface
                .sample(page.origin_xz, page.size, candidate.root_xz);

            if surface.validity < 0.5
                || !accepted(
                    &candidate,
                    page,
                    field.sample_coverage(page, candidate.root_xz),
                )
            {
                continue;
            }

            placements.push(DebugPlacement {
                root: [candidate.root_xz[0], surface.height, candidate.root_xz[1]],
                surface_normal: surface.normal,
                grass: field.grass,
                clump_center_xz: candidate.clump.center_xz,
                clump_key: candidate.clump.key,
                clump_distance: candidate.clump.distance,
                facing: candidate.facing,
                seed: candidate.seed,
            });
        }
    }

    Ok(placements)
}

#[derive(Debug, Error)]
pub enum PlacementError {
    #[error(transparent)]
    Scene(#[from] SceneValidationError),
    #[error(transparent)]
    Catalog(#[from] crate::CatalogValidationError),
    #[error(transparent)]
    Page(#[from] crate::PageValidationError),
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::fixtures;

    use super::*;

    #[test]
    fn split_pages_match_one_combined_world_lattice() {
        let catalog = fixtures::reference_catalog();
        for grass in [fixtures::meadow().id(), fixtures::tufts().id()] {
            let left = fixtures::full_coverage_page([0.0, 0.0], 16.0, grass);
            let right = fixtures::full_coverage_page([16.0, 0.0], 16.0, grass);
            let combined = fixtures::full_coverage_page([0.0, 0.0], 32.0, grass);
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
            assert_eq!(split_seeds, combined_seeds, "grass {grass:?}");
        }
    }

    #[test]
    fn full_coverage_grows_about_the_type_density() {
        let catalog = fixtures::reference_catalog();
        let meadow = fixtures::meadow();
        let page = fixtures::full_coverage_page([0.0, 0.0], 16.0, meadow.id());
        let count = generate_page_debug_placements(&catalog, &page)
            .unwrap()
            .len() as f32;
        let full = meadow.density * 16.0 * 16.0;

        // Clumps keep 80–100% of candidates, fewer toward their edges.
        assert!((0.7 * full..=full).contains(&count), "{count} of {full}");
    }

    #[test]
    fn reference_scene_is_deterministic() {
        let scene = fixtures::reference_scene();
        let first = generate_scene_debug_placements(&scene).unwrap();
        let second = generate_scene_debug_placements(&scene).unwrap();
        assert!(!first.is_empty());
        assert_eq!(first, second);
    }
}
