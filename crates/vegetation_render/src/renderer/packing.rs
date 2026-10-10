//! Pure CPU scene packing, LOD budgets and conservative visibility bounds.
use super::gpu_types::{
    LOW_DETAIL_CAPACITY, LOW_DETAIL_MINIMUM_PARTITION, SINGLE_HIGH_CAPACITY, SPECIES_INDEX_MASK,
    SPLIT_HIGH_CAPACITY, SpeciesChoiceGpu, SpeciesGpu, SurfaceSampleGpu, WorkItemGpu,
};
use bevy::prelude::*;
use species::{effective_horizontal_reach, fallback_topology_bin, procedural_lod_profile};
use std::collections::HashMap;
use vegetation::{
    GrowthPattern, VegetationGroupingProfile, candidate_density_retention,
    candidate_domain_for_extent, decode_octahedral_normal,
};

mod species;
#[cfg(test)]
use species::pack_ribbon_curve;
pub(super) use species::pack_species;
pub use species::terrain_contact_radius;

// Keep the expensive high-topology population below the arena's hard guard even for a top-down
// view of a fully covered field. The remaining headroom absorbs stochastic placement variance,
// clump attraction, page boundaries, and the smooth high/low transition annulus.
const HIGH_DETAIL_BUDGET_UTILIZATION: f32 = 0.5;
const MAX_HIGH_DETAIL_RADIUS: f32 = 96.0;

/// The reference packing, compared with the incremental page layout in tests.
#[cfg(test)]
pub(super) struct PackedScene {
    pub(super) work_items: Vec<WorkItemGpu>,
    pub(super) choices: Vec<SpeciesChoiceGpu>,
    pub(super) coverage: Vec<f32>,
    pub(super) surfaces: Vec<SurfaceSampleGpu>,
    pub(super) species: Vec<SpeciesGpu>,
    pub(super) maximum_candidate_count: u32,
    pub(super) low_detail_capacities: [u32; 2],
}

fn maximum_topology_densities(scene: &vegetation::VegetationScene) -> [f32; 2] {
    let mut maximum_density = [0.0_f32; 2];
    for page in &scene.pages {
        let mut page_density = [0.0_f32; 2];
        for field in &page.fields {
            let population = scene
                .catalog
                .population(field.population)
                .expect("validated scene population");
            let total_weight = population
                .species
                .iter()
                .map(|choice| choice.weight)
                .sum::<f32>();
            for choice in &population.species {
                let species = scene
                    .catalog
                    .species(choice.species)
                    .expect("validated species choice");
                let species_share = choice.weight / total_weight;
                let split_fraction = species.expected_split_topology_fraction();
                page_density[0] +=
                    population.density_per_square_meter * species_share * (1.0 - split_fraction);
                page_density[1] +=
                    population.density_per_square_meter * species_share * split_fraction;
            }
        }
        for (maximum, density) in maximum_density.iter_mut().zip(page_density) {
            *maximum = maximum.max(density);
        }
    }

    maximum_density
}

pub(super) fn pack_lod_focus(focus: Option<Vec3>, camera: Vec3, forward: Vec3) -> [f32; 4] {
    let Some(focus) = focus.filter(|v| v.is_finite()) else {
        return [camera.x, camera.z, 0.0, 0.0];
    };
    // A vertical view has no preferred ground direction and keeps a circular footprint.
    let horizontal = forward.xz();
    let direction = if horizontal.length_squared() > 1e-4 {
        horizontal.normalize()
    } else {
        Vec2::ZERO
    };
    let center = focus.xz() + direction * 2.0;
    [center.x, center.y, direction.x, direction.y]
}

fn high_detail_radii(scene: &vegetation::VegetationScene) -> [f32; 2] {
    let maximum_density = maximum_topology_densities(scene);
    let radius_for = |capacity: u32, density: f32| {
        if density <= f32::EPSILON {
            return MAX_HIGH_DETAIL_RADIUS;
        }
        ((capacity as f32 * HIGH_DETAIL_BUDGET_UTILIZATION) / (std::f32::consts::PI * density))
            .sqrt()
            .min(MAX_HIGH_DETAIL_RADIUS)
    };
    [
        radius_for(SINGLE_HIGH_CAPACITY, maximum_density[0]),
        radius_for(SPLIT_HIGH_CAPACITY, maximum_density[1]),
    ]
}

fn low_detail_capacities(scene: &vegetation::VegetationScene) -> [u32; 2] {
    const PARTITION_ALIGNMENT: u32 = 256;

    let densities = maximum_topology_densities(scene);
    let total_density = densities[0] + densities[1];
    let single_share = if total_density <= f32::EPSILON {
        0.5
    } else {
        densities[0] / total_density
    };
    let flexible_capacity = LOW_DETAIL_CAPACITY - LOW_DETAIL_MINIMUM_PARTITION * 2;
    let unaligned_single =
        LOW_DETAIL_MINIMUM_PARTITION + (flexible_capacity as f32 * single_share).round() as u32;
    let single = unaligned_single
        .div_ceil(PARTITION_ALIGNMENT)
        .saturating_mul(PARTITION_ALIGNMENT)
        .clamp(
            LOW_DETAIL_MINIMUM_PARTITION,
            LOW_DETAIL_CAPACITY - LOW_DETAIL_MINIMUM_PARTITION,
        );
    [single, LOW_DETAIL_CAPACITY - single]
}

#[cfg(test)]
pub(super) fn pack_scene(scene: &vegetation::VegetationScene) -> PackedScene {
    pack_scene_with_gate(scene, &default(), [0.; 2])
}

/// Where one page's samples live in the surface and coverage buffers: the surface offset and
/// one coverage offset per field, in elements.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PageLayout {
    pub(super) surface: u32,
    pub(super) coverage: Vec<u32>,
}

/// Everything except the page samples. Species budgets and work items depend on the whole
/// resident scene, so these are rebuilt for every revision; they are small.
pub(super) struct PackedItems {
    pub(super) work_items: Vec<WorkItemGpu>,
    pub(super) choices: Vec<SpeciesChoiceGpu>,
    pub(super) species: Vec<SpeciesGpu>,
    pub(super) maximum_candidate_count: u32,
    pub(super) low_detail_capacities: [u32; 2],
}

/// A page's surface samples as uploaded. They do not depend on the render origin.
pub(super) fn pack_surface(
    page: &vegetation::VegetationFieldPage,
) -> impl Iterator<Item = SurfaceSampleGpu> + '_ {
    page.surface
        .heights
        .iter()
        .zip(&page.surface.normals_oct)
        .zip(&page.surface.validity)
        .map(|((height, normal), validity)| {
            let normal = decode_octahedral_normal(*normal);
            SurfaceSampleGpu {
                height_validity: [*height, f32::from(*validity) / 255.0, 0.0, 0.0],
                normal: [normal[0], normal[1], normal[2], 0.0],
            }
        })
}

/// A field's coverage samples as uploaded.
pub(super) fn pack_coverage(
    field: &vegetation::VegetationPopulationField,
) -> impl Iterator<Item = f32> + '_ {
    field.coverage.iter().map(|value| f32::from(*value) / 255.0)
}

/// The reference packing: every page's samples in order, followed by its work items.
#[cfg(test)]
pub(super) fn pack_scene_with_gate(
    scene: &vegetation::VegetationScene,
    gate: &crate::VegetationTerrainGate,
    origin: [f64; 2],
) -> PackedScene {
    let mut coverage = Vec::new();
    let mut surfaces = Vec::new();
    let layouts: Vec<_> = scene
        .pages
        .iter()
        .map(|page| {
            let surface = surfaces.len() as u32;
            surfaces.extend(pack_surface(page));
            let coverage_offsets = page
                .fields
                .iter()
                .map(|field| {
                    let offset = coverage.len() as u32;
                    coverage.extend(pack_coverage(field));
                    offset
                })
                .collect();
            PageLayout {
                surface,
                coverage: coverage_offsets,
            }
        })
        .collect();
    let items = pack_items(scene, gate, origin, &layouts);
    PackedScene {
        work_items: items.work_items,
        choices: items.choices,
        coverage,
        surfaces,
        species: items.species,
        maximum_candidate_count: items.maximum_candidate_count,
        low_detail_capacities: items.low_detail_capacities,
    }
}

/// Work items, species choices and species for `scene`, addressing each page's samples at
/// `layouts[page]`.
pub(super) fn pack_items(
    scene: &vegetation::VegetationScene,
    gate: &crate::VegetationTerrainGate,
    origin: [f64; 2],
    layouts: &[PageLayout],
) -> PackedItems {
    debug_assert_eq!(layouts.len(), scene.pages.len());
    let high_detail_radii = high_detail_radii(scene);
    let low_detail_capacities = low_detail_capacities(scene);
    // Instances carry the species index in SPECIES_INDEX_MASK's bits.
    assert!(
        scene.catalog.species.len() <= SPECIES_INDEX_MASK as usize + 1,
        "a vegetation catalog holds at most {} species",
        SPECIES_INDEX_MASK + 1
    );
    let species_indices = scene
        .catalog
        .species
        .iter()
        .enumerate()
        .map(|(index, species)| (species.id, index as u32))
        .collect::<HashMap<_, _>>();
    let species = scene
        .catalog
        .species
        .iter()
        .map(|species| pack_species(species, high_detail_radii))
        .collect::<Vec<_>>();

    let mut work_items = Vec::new();
    let mut choices = Vec::new();
    let mut maximum_candidate_count = 0;
    for (page, layout) in scene.pages.iter().zip(layouts) {
        debug_assert_eq!(layout.coverage.len(), page.fields.len());
        let page_work_start = work_items.len() as u32;
        let page_work_count = page.fields.len() as u32;
        let (minimum_surface_height, maximum_surface_height) = page.surface.heights.iter().fold(
            (f32::INFINITY, f32::NEG_INFINITY),
            |(minimum, maximum), height| (minimum.min(*height), maximum.max(*height)),
        );
        for (field, &coverage_offset) in page.fields.iter().zip(&layout.coverage) {
            let population = scene
                .catalog
                .population(field.population)
                .expect("validated scene population");
            let domain = candidate_domain_for_extent(
                [
                    (f64::from(page.origin_xz[0]) + origin[0]) as f32,
                    (f64::from(page.origin_xz[1]) + origin[1]) as f32,
                ],
                page.size,
                population,
            );
            maximum_candidate_count = maximum_candidate_count.max(domain.candidate_count());
            let choice_offset = choices.len() as u32;
            let total_weight = population
                .species
                .iter()
                .map(|choice| choice.weight)
                .sum::<f32>();
            let mut cumulative_weight = 0.0;
            let mut maximum_height = 0.0_f32;
            let mut maximum_horizontal_reach = 0.0_f32;
            let mut minimum_high_threshold = f32::INFINITY;
            let mut minimum_low_threshold = f32::INFINITY;
            let mut maximum_low_density = 0.0_f32;
            for choice in &population.species {
                cumulative_weight += choice.weight;
                let species_definition = scene
                    .catalog
                    .species(choice.species)
                    .expect("validated species choice");
                let lod = procedural_lod_profile(species_definition);
                let horizontal_reach = effective_horizontal_reach(species_definition);
                maximum_height = maximum_height.max(species_definition.bounds.maximum_height);
                maximum_horizontal_reach = maximum_horizontal_reach.max(horizontal_reach);
                minimum_high_threshold = minimum_high_threshold.min(lod.high_minimum_pixels);
                minimum_low_threshold = minimum_low_threshold.min(lod.low_minimum_pixels);
                maximum_low_density = maximum_low_density.max(lod.low_density_fraction);
                choices.push(SpeciesChoiceGpu {
                    metadata: [
                        species_indices[&choice.species],
                        fallback_topology_bin(species_definition.topology),
                        horizontal_reach.to_bits(),
                        species_definition.bounds.maximum_height.to_bits(),
                    ],
                    threshold: [
                        cumulative_weight / total_weight,
                        lod.high_minimum_pixels,
                        lod.low_minimum_pixels,
                        lod.far_minimum_pixels,
                    ],
                    density: [1.0, lod.low_density_fraction, lod.far_density_fraction, 0.0],
                    height: [
                        species_definition.bounds.minimum_height,
                        species_definition.bounds.maximum_height,
                        species_definition.height.distribution_bias,
                        species_definition.group_response.height_coherence,
                    ],
                    packing: [
                        species_definition.height.pair_below_height,
                        high_detail_radii[0],
                        high_detail_radii[1],
                        0.0,
                    ],
                });
            }

            let (radius, jitter, pattern) = match population.growth {
                GrowthPattern::Uniform { jitter } => (0.0, jitter, 0),
                GrowthPattern::ParentChild {
                    radius,
                    parent_jitter,
                    ..
                } => (radius, parent_jitter, 1),
            };
            let (grouping, group_density, grouping_source) = match population.grouping {
                VegetationGroupingProfile::None => ([0.0; 4], [1.0, 1.0, 1.0, 0.0], 0),
                VegetationGroupingProfile::Parent => ([0.0; 4], [1.0, 1.0, 1.0, 0.0], 1),
                VegetationGroupingProfile::Voronoi(profile) => (
                    [
                        profile.spacing,
                        profile.feature_jitter,
                        profile.boundary_softness,
                        profile.root_attraction,
                    ],
                    [
                        profile.center_retention,
                        profile.edge_retention,
                        profile.retention_falloff,
                        profile.density_variation,
                    ],
                    2,
                ),
            };
            let orientation = population.orientation;
            work_items.push(WorkItemGpu {
                page: [
                    page.origin_xz[0],
                    page.origin_xz[1],
                    page.size,
                    minimum_low_threshold,
                ],
                domain: [
                    domain.cell_min[0],
                    domain.cell_min[1],
                    domain.cell_count[0] as i32,
                    domain.cell_count[1] as i32,
                ],
                layout: [
                    domain.candidates_per_cell,
                    domain.candidate_count(),
                    coverage_offset,
                    u32::from(field.resolution),
                ],
                population: [
                    choice_offset,
                    population.species.len() as u32,
                    population.seed,
                    population
                        .competition_group
                        .map_or(0, |group| u32::from(group) + 1),
                ],
                growth: [
                    domain.spacing,
                    radius,
                    jitter,
                    candidate_density_retention(population),
                ],
                direction_weights: [
                    orientation.radial_weight,
                    orientation.tangential_weight,
                    orientation.random_weight,
                    orientation.flow_weight,
                ],
                flow_density: [
                    field.flow_direction[0],
                    field.flow_direction[1],
                    population.density_per_square_meter,
                    0.0,
                ],
                grouping,
                group_density,
                orientation: [
                    orientation.shared_group_weight,
                    orientation.angular_jitter_radians,
                    origin[0] as f32,
                    origin[1] as f32,
                ],
                peers: [page_work_start, page_work_count, pattern, grouping_source],
                surface: [
                    layout.surface,
                    u32::from(page.surface.resolution),
                    minimum_surface_height.to_bits(),
                    maximum_surface_height.to_bits(),
                ],
                bounds: [
                    maximum_height,
                    maximum_horizontal_reach,
                    minimum_high_threshold,
                    maximum_low_density,
                ],
            });
        }
    }

    apply_terrain_gate(&mut work_items, gate);
    PackedItems {
        work_items,
        choices,
        species,
        maximum_candidate_count,
        low_detail_capacities,
    }
}

/// A gate is a draw/scheduling input, never a change to the source population.
/// Keeping work-item indices stable also preserves peer ranges and candidate masks.
pub(super) fn apply_terrain_gate(
    items: &mut [WorkItemGpu],
    gate: &crate::VegetationTerrainGate,
) -> bool {
    let mut changed = false;
    for item in items {
        let id = [
            item.page[0].to_bits(),
            item.page[1].to_bits(),
            item.page[2].to_bits(),
        ];
        let blocked = if gate.block_all || gate.blocked_pages.contains(&id) {
            1.
        } else {
            0.
        };
        changed |= item.flow_density[3] != blocked;
        item.flow_density[3] = blocked;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use vegetation::TopologyProfile;

    #[test]
    fn gameplay_detail_centre_tracks_subject_instead_of_orbit_distance() {
        let subject = Vec3::new(32.0, 4.0, -64.0);
        let near = pack_lod_focus(
            Some(subject),
            subject + Vec3::new(0.0, 1.6, 4.0),
            Vec3::new(0.0, -0.2, -1.0),
        );
        let far = pack_lod_focus(
            Some(subject),
            subject + Vec3::new(0.0, 12.0, 18.0),
            Vec3::new(0.0, -0.7, -0.7),
        );
        assert_eq!(near, far);
        assert_eq!(near, [32.0, -66.0, 0.0, -1.0]);
        assert_eq!(
            pack_lod_focus(None, Vec3::new(5.0, 8.0, 9.0), Vec3::NEG_Z),
            [5.0, 9.0, 0.0, 0.0]
        );
        assert_eq!(
            pack_lod_focus(
                Some(subject),
                subject + Vec3::Y * 20.0,
                Vec3::new(0.0, -1.0, -1e-7)
            ),
            [32.0, -64.0, 0.0, 0.0]
        );
    }

    #[test]
    fn streamed_scene_upload_excludes_canopy_raster() {
        let catalog: vegetation::VegetationCatalog = ron::from_str(include_str!(
            "../../../../content/vegetation/field-current.ron"
        ))
        .unwrap();
        let population = catalog
            .populations
            .iter()
            .find(|p| p.key == "short_split_fill")
            .unwrap()
            .id;
        let pages = (-3..=3)
            .flat_map(|z| (-3..=3).map(move |x| (x, z)))
            .map(|(x, z)| vegetation::VegetationFieldPage {
                origin_xz: [x as f32 * 32.0, z as f32 * 32.0],
                size: 32.0,
                surface: vegetation::VegetationSurfaceField::flat(33, 0.0, [0.0, 1.0, 0.0]),
                fields: vec![vegetation::VegetationPopulationField {
                    population,
                    resolution: 16,
                    coverage: vec![255; 256],
                    flow_direction: [0.0, 1.0],
                }],
            })
            .collect();
        let scene = vegetation::VegetationScene { catalog, pages };
        let started = std::time::Instant::now();
        let packed = pack_scene(&scene);
        let pack_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(packed.coverage.len(), 49 * 256);
        assert_eq!(packed.work_items[0].layout[2], 0);
        // Explicit diagnostic for the removed synchronous path; no timing assertion or GPU loop.
        if std::env::var_os("YARRA_TRACE_BOUNDARY_REBUILD").is_some() {
            let started = std::time::Instant::now();
            let boundary =
                crate::canopy_coverage::BoundaryField::for_scene(&scene.catalog, &scene.pages);
            let values = boundary.gpu_values();
            eprintln!(
                "49-page scene: former synchronous canopy bake {:.2} ms / {} bytes; source pack without canopy {:.2} ms / {} coverage bytes",
                started.elapsed().as_secs_f64() * 1000.0,
                values.len() * 4,
                pack_ms,
                packed.coverage.len() * 4
            );
        }
    }

    #[test]
    fn low_arena_partition_tracks_the_authored_height_mix() {
        let mut mostly_single = vegetation::fixtures::reference_scene();
        for species in &mut mostly_single.catalog.species {
            if let TopologyProfile::Ribbon(profile) = &mut species.topology {
                profile.blades_per_render_unit = 1;
                species.height.pair_below_height = 0.0;
            }
        }
        let single_capacities = low_detail_capacities(&mostly_single);

        let mut mostly_split = vegetation::fixtures::reference_scene();
        for species in &mut mostly_split.catalog.species {
            if let TopologyProfile::Ribbon(profile) = &mut species.topology {
                profile.blades_per_render_unit = 2;
                species.height.pair_below_height = species.bounds.maximum_height;
            }
        }
        let split_capacities = low_detail_capacities(&mostly_split);

        assert!(single_capacities[0] > split_capacities[0]);
        assert_eq!(single_capacities.iter().sum::<u32>(), LOW_DETAIL_CAPACITY);
        assert_eq!(split_capacities.iter().sum::<u32>(), LOW_DETAIL_CAPACITY);
    }

    #[test]
    fn reference_fixture_packs_all_fields() {
        let scene = vegetation::fixtures::reference_scene();
        let packed = pack_scene(&scene);
        let high_detail_radii = high_detail_radii(&scene);
        assert_eq!(packed.work_items.len(), 8);
        assert_eq!(packed.species.len(), 4);
        assert!(packed.choices.iter().any(|choice| choice.metadata[1] == 0));
        assert!(packed.choices.iter().any(|choice| choice.metadata[1] == 1));
        assert!(packed.choices.iter().all(|choice| choice.metadata[1] < 2));
        assert!(packed.maximum_candidate_count > 0);
        assert!(packed.coverage.len() > scene.pages.len());
        assert_eq!(packed.surfaces.len(), scene.pages.len() * 4);
        assert_eq!(
            packed.low_detail_capacities.iter().sum::<u32>(),
            LOW_DETAIL_CAPACITY
        );
        assert!(
            packed
                .low_detail_capacities
                .iter()
                .all(|capacity| *capacity >= LOW_DETAIL_MINIMUM_PARTITION)
        );
        assert!(packed.choices.iter().all(|choice| {
            choice.packing[1] == high_detail_radii[0]
                && choice.packing[2] == high_detail_radii[1]
                && choice.height[0] <= choice.height[1]
                && (-1.0..=1.0).contains(&choice.height[2])
        }));
        let maximum_density = maximum_topology_densities(&scene);
        let expected_single_radius = ((SINGLE_HIGH_CAPACITY as f32
            * HIGH_DETAIL_BUDGET_UTILIZATION)
            / (std::f32::consts::PI * maximum_density[0]))
            .sqrt()
            .min(MAX_HIGH_DETAIL_RADIUS);
        let expected_split_radius = ((SPLIT_HIGH_CAPACITY as f32 * HIGH_DETAIL_BUDGET_UTILIZATION)
            / (std::f32::consts::PI * maximum_density[1]))
            .sqrt()
            .min(MAX_HIGH_DETAIL_RADIUS);
        assert!((high_detail_radii[0] - expected_single_radius).abs() < 1e-4);
        assert!((high_detail_radii[1] - expected_split_radius).abs() < 1e-4);
        assert!(
            high_detail_radii[0] >= 22.0,
            "the reference tall-grass high-geometry boundary moved too close: {} m",
            high_detail_radii[0]
        );
        let short_species_index = scene
            .catalog
            .species
            .iter()
            .position(|species| species.key == "short_split_fill_ribbon")
            .unwrap();
        let short_species = &scene.catalog.species[short_species_index];
        let TopologyProfile::Ribbon(short_topology) = short_species.topology else {
            unreachable!();
        };
        assert_eq!(
            packed.species[short_species_index].curve_variant_a,
            pack_ribbon_curve(short_topology.curve_variant_a)
        );
        assert_eq!(
            packed.species[short_species_index].shape[..2],
            [
                short_topology.curve_variant_a.tip_tilt_radians,
                short_topology.curve_variant_b.tip_tilt_radians,
            ]
        );
        assert_eq!(
            packed.species[short_species_index].shape_secondary[2],
            short_topology.maximum_view_opening_radians.tan()
        );
        assert!(effective_horizontal_reach(short_species) >= short_species.bounds.maximum_height);
        assert!(
            effective_horizontal_reach(short_species)
                > short_species.bounds.maximum_horizontal_reach
        );
        assert!(scene.catalog.species.iter().all(|species| {
            let lod = procedural_lod_profile(species);
            lod.low_density_fraction <= 0.32 && lod.far_density_fraction <= 0.32
        }));
    }
}
