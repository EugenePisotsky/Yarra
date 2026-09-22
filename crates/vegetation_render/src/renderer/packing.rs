//! Pure CPU scene packing, LOD budgets and conservative visibility bounds.
use super::{
    gpu_types::{
        LOW_DETAIL_CAPACITY, LOW_DETAIL_MINIMUM_PARTITION, SINGLE_HIGH_CAPACITY,
        SPLIT_HIGH_CAPACITY, SpeciesChoiceGpu, SpeciesGpu, SurfaceSampleGpu, WorkItemGpu,
    },
    topology::{MAX_LOW_RENDER_SECTIONS, MAX_RENDER_SECTIONS},
};
use crate::VegetationWind;
use bevy::prelude::*;
use std::collections::HashMap;
use vegetation::{
    GrowthPattern, RepresentationKind, TopologyFamily, TopologyProfile, VegetationGroupingProfile,
    candidate_density_retention, candidate_domain_for_extent, decode_octahedral_normal,
};

// Keep the expensive high-topology population below the arena's hard guard even for a top-down
// view of a fully covered field. The remaining headroom absorbs stochastic placement variance,
// clump attraction, page boundaries, and the smooth high/low transition annulus.
const HIGH_DETAIL_BUDGET_UTILIZATION: f32 = 0.5;
const MAX_HIGH_DETAIL_RADIUS: f32 = 96.0;

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

fn effective_horizontal_reach(species: &vegetation::VegetationSpecies) -> f32 {
    let height = species.bounds.maximum_height;
    let width = species.bounds.maximum_half_width * 1.24;
    // A cubic Bezier stays inside the convex hull of its control points. Bound the same p1/p2/p3
    // construction used by the vertex shader in its orthonormal blade frame, then include the
    // root offset, grazing-angle width expansion, and maximum wind displacement. This prevents an
    // artist-entered reach that is too small from making whole pages disappear at view edges.
    let (p1_radius, p2_radius, root_offset) = match species.topology {
        TopologyProfile::Ribbon(profile) => {
            let maximum_root_handle = profile
                .curve_variant_a
                .root_handle_length
                .max(profile.curve_variant_b.root_handle_length);
            let maximum_tip_handle = profile
                .curve_variant_a
                .tip_handle_length
                .max(profile.curve_variant_b.tip_handle_length);
            // P2 is the tip endpoint minus its tangent handle plus lateral camber. The triangle
            // inequality is deliberately conservative for every independent tilt/curve sample.
            let p2_radius =
                height * (1.0 + maximum_tip_handle + profile.maximum_lateral_curve * 0.22);
            (
                height * maximum_root_handle,
                p2_radius,
                if profile.blades_per_render_unit > 1 {
                    species.bounds.maximum_half_width * 0.75
                } else {
                    0.0
                },
            )
        }
        TopologyProfile::BroadLeafCluster(profile) => {
            let maximum_tilt = 0.52 + profile.maximum_droop * 0.45;
            let maximum_bend = profile.maximum_droop;
            let p1_radius = height * (0.34_f32.powi(2) + (maximum_bend * 0.05).powi(2)).sqrt();
            let p2_normal = maximum_tilt.cos() * 0.68 + maximum_bend * 0.16;
            let p2_forward = maximum_tilt.sin() * 0.68 + maximum_bend * 0.22;
            let p2_side = profile.maximum_camber * 0.22;
            let p2_radius =
                height * (p2_normal.powi(2) + p2_forward.powi(2) + p2_side.powi(2)).sqrt();
            (p1_radius, p2_radius, profile.crown_radius * 0.35)
        }
    };
    species.bounds.maximum_horizontal_reach.max(
        height.max(p1_radius).max(p2_radius)
            + root_offset
            + width
            + species.wind.maximum_tip_displacement,
    )
}

/// Conservative CPU counterpart of the GPU root range, including blade/wind reach.
pub fn terrain_contact_radius(
    catalog: &vegetation::VegetationCatalog,
    wind: &VegetationWind,
) -> f32 {
    let strength = if wind.enabled {
        wind.strength.max(0.)
    } else {
        0.
    };
    crate::PROCEDURAL_DISTANCE_METERS
        + catalog
            .species
            .iter()
            .map(|s| effective_horizontal_reach(s) + s.bounds.maximum_height * strength * 1.65)
            .fold(0., f32::max)
}

#[cfg(test)]
pub(super) fn pack_scene(scene: &vegetation::VegetationScene) -> PackedScene {
    pack_scene_with_gate(scene, &default(), [0.; 2])
}

pub(super) fn pack_scene_with_gate(
    scene: &vegetation::VegetationScene,
    gate: &crate::VegetationTerrainGate,
    origin: [f64; 2],
) -> PackedScene {
    let high_detail_radii = high_detail_radii(scene);
    let low_detail_capacities = low_detail_capacities(scene);
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
    let mut coverage = Vec::new();
    let mut surfaces = Vec::new();
    let mut maximum_candidate_count = 0;
    for page in &scene.pages {
        let page_work_start = work_items.len() as u32;
        let page_work_count = page.fields.len() as u32;
        let surface_offset = surfaces.len() as u32;
        let (minimum_surface_height, maximum_surface_height) = page.surface.heights.iter().fold(
            (f32::INFINITY, f32::NEG_INFINITY),
            |(minimum, maximum), height| (minimum.min(*height), maximum.max(*height)),
        );
        surfaces.extend(
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
                }),
        );
        for field in &page.fields {
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
            let coverage_offset = coverage.len() as u32;
            coverage.extend(field.coverage.iter().map(|value| f32::from(*value) / 255.0));
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
                    surface_offset,
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
    PackedScene {
        work_items,
        choices,
        coverage,
        surfaces,
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

fn topology_code(family: TopologyFamily) -> f32 {
    match family {
        TopologyFamily::Ribbon => 0.0,
        TopologyFamily::RibbonTuft => 1.0,
        TopologyFamily::BroadLeafCluster => 2.0,
        TopologyFamily::StemAndHead => 3.0,
        TopologyFamily::CardImpostor => 4.0,
        TopologyFamily::AuthoredMesh => 5.0,
    }
}

fn fallback_topology_bin(topology: TopologyProfile) -> u32 {
    match topology {
        TopologyProfile::Ribbon(_) => 0,
        TopologyProfile::BroadLeafCluster(_) => 1,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ProceduralLodProfile {
    high_minimum_pixels: f32,
    low_minimum_pixels: f32,
    far_minimum_pixels: f32,
    low_density_fraction: f32,
    far_density_fraction: f32,
}

fn procedural_lod_profile(species: &vegetation::VegetationSpecies) -> ProceduralLodProfile {
    let procedural_levels = species
        .representations
        .iter()
        .filter(|level| matches!(level.kind, RepresentationKind::Procedural(_)))
        .collect::<Vec<_>>();
    let high = procedural_levels
        .first()
        .copied()
        .expect("validated species has a procedural representation");
    let low = procedural_levels.get(1).copied().unwrap_or(high);
    let far = procedural_levels.last().copied().unwrap_or(low);
    ProceduralLodProfile {
        high_minimum_pixels: high.minimum_projected_size,
        low_minimum_pixels: low.minimum_projected_size,
        far_minimum_pixels: far.minimum_projected_size,
        low_density_fraction: low.density_fraction,
        far_density_fraction: far.density_fraction,
    }
}

pub(super) fn pack_species(
    species: &vegetation::VegetationSpecies,
    high_detail_radii: [f32; 2],
) -> SpeciesGpu {
    let lod = procedural_lod_profile(species);
    let horizontal_reach = effective_horizontal_reach(species);
    let (topology, shape, shape_secondary, curve_variant_a, curve_variant_b) =
        match species.topology {
            TopologyProfile::Ribbon(profile) => (
                [
                    f32::from(profile.high_section_count.min(MAX_RENDER_SECTIONS)),
                    f32::from(profile.low_section_count.min(MAX_LOW_RENDER_SECTIONS)),
                    f32::from(profile.blades_per_render_unit.min(2)),
                    profile.longitudinal_power,
                ],
                [
                    profile.curve_variant_a.tip_tilt_radians,
                    profile.curve_variant_b.tip_tilt_radians,
                    0.0,
                    0.0,
                ],
                [
                    profile.maximum_lateral_curve,
                    profile.pair_spread_radians,
                    profile.maximum_view_opening_radians.tan(),
                    horizontal_reach,
                ],
                pack_ribbon_curve(profile.curve_variant_a),
                pack_ribbon_curve(profile.curve_variant_b),
            ),
            TopologyProfile::BroadLeafCluster(profile) => (
                [
                    f32::from(profile.high_section_count.min(MAX_RENDER_SECTIONS)),
                    f32::from(profile.low_section_count.min(MAX_LOW_RENDER_SECTIONS)),
                    2.0,
                    0.72,
                ],
                [
                    0.14 + profile.minimum_droop * 0.2,
                    0.52 + profile.maximum_droop * 0.45,
                    profile.minimum_droop,
                    profile.maximum_droop,
                ],
                [
                    profile.maximum_camber,
                    2.2,
                    profile.crown_radius,
                    horizontal_reach,
                ],
                [0.0; 4],
                [0.0; 4],
            ),
        };
    SpeciesGpu {
        root_color: [
            species.material.root_color[0],
            species.material.root_color[1],
            species.material.root_color[2],
            topology_code(species.topology.family()),
        ],
        tip_color_height: [
            species.material.tip_color[0],
            species.material.tip_color[1],
            species.material.tip_color[2],
            species.bounds.maximum_height,
        ],
        bounds: [
            species.bounds.minimum_height,
            species.bounds.maximum_height,
            species.bounds.minimum_half_width,
            species.bounds.maximum_half_width,
        ],
        topology,
        shape,
        shape_secondary,
        curve_variant_a,
        curve_variant_b,
        material: [
            species.material.clump_color_variation,
            species.material.perceptual_roughness,
            species.material.transmission,
            species.material.normal_rounding,
        ],
        shading: [
            species.material.root_ao,
            species.material.tip_ao,
            lod.high_minimum_pixels,
            0.0,
        ],
        group_response: [
            species.group_response.height_coherence,
            species.group_response.silhouette_coherence,
            species.group_response.lateral_curve_coherence,
            0.0,
        ],
        height_packing: [
            species.height.distribution_bias,
            species.height.pair_below_height,
            high_detail_radii[0],
            high_detail_radii[1],
        ],
    }
}

fn pack_ribbon_curve(profile: vegetation::RibbonCurveProfile) -> [f32; 4] {
    [
        profile.root_tangent_radians.sin() * profile.root_handle_length,
        profile.root_tangent_radians.cos() * profile.root_handle_length,
        profile.tip_tangent_radians.sin() * profile.tip_handle_length,
        profile.tip_tangent_radians.cos() * profile.tip_handle_length,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

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
