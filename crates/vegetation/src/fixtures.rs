//! Small, deterministic fixtures shared by contract, compiler, and renderer tests.
//!
//! These are deliberately code-authored. They exercise the V2 model before an editor or database
//! schema is allowed to depend on it.

use crate::{
    BroadLeafTopologyProfile, GrowthPattern, RepresentationKind, RepresentationLevel,
    RibbonTopologyProfile, TopologyFamily, TopologyProfile, VegetationAssemblage,
    VegetationAssemblageId, VegetationBounds, VegetationCatalog, VegetationFieldPage,
    VegetationMaterialProfile, VegetationPopulation, VegetationPopulationField,
    VegetationPopulationId, VegetationScene, VegetationSpecies, VegetationSpeciesId,
    VegetationSurfaceField, VegetationWindProfile,
};

pub const DRY_FINE_SPECIES_ID: VegetationSpeciesId = VegetationSpeciesId([1; 16]);
pub const GREEN_FINE_SPECIES_ID: VegetationSpeciesId = VegetationSpeciesId([2; 16]);
pub const BROAD_LEAF_SPECIES_ID: VegetationSpeciesId = VegetationSpeciesId([3; 16]);
pub const SHORT_FILL_SPECIES_ID: VegetationSpeciesId = VegetationSpeciesId([4; 16]);

pub const DRY_TUFT_POPULATION_ID: VegetationPopulationId = VegetationPopulationId([11; 16]);
pub const GREEN_FINE_POPULATION_ID: VegetationPopulationId = VegetationPopulationId([12; 16]);
pub const BROAD_LEAF_POPULATION_ID: VegetationPopulationId = VegetationPopulationId([13; 16]);
pub const SHORT_FILL_POPULATION_ID: VegetationPopulationId = VegetationPopulationId([14; 16]);

pub const DRY_FIELD_ASSEMBLAGE_ID: VegetationAssemblageId = VegetationAssemblageId([21; 16]);
pub const MIXED_GREEN_ASSEMBLAGE_ID: VegetationAssemblageId = VegetationAssemblageId([22; 16]);

pub const REFERENCE_PAGE_SIZE: f32 = 16.0;
pub const REFERENCE_FIELD_RESOLUTION: u16 = 16;

pub fn reference_catalog() -> VegetationCatalog {
    VegetationCatalog {
        species: vec![
            dry_fine_species(),
            green_fine_species(),
            broad_leaf_species(),
            short_fill_species(),
        ],
        populations: vec![
            VegetationPopulation {
                id: DRY_TUFT_POPULATION_ID,
                key: "dry_tuft".into(),
                species: vec![crate::SpeciesChoice {
                    species: DRY_FINE_SPECIES_ID,
                    weight: 1.0,
                }],
                density_per_square_meter: 6.0,
                seed: 0x843a_2f19,
                growth: GrowthPattern::ParentChild {
                    parent_spacing: 3.0,
                    children_per_parent: 64,
                    radius: 2.35,
                    parent_jitter: 0.72,
                    radial_weight: 0.2,
                    tangential_weight: 1.0,
                    random_weight: 0.25,
                    flow_weight: 0.3,
                },
                competition_group: None,
            },
            VegetationPopulation {
                id: GREEN_FINE_POPULATION_ID,
                key: "green_fine".into(),
                species: vec![crate::SpeciesChoice {
                    species: GREEN_FINE_SPECIES_ID,
                    weight: 1.0,
                }],
                density_per_square_meter: 5.0,
                seed: 0x19ad_7483,
                growth: GrowthPattern::ParentChild {
                    parent_spacing: 2.0,
                    children_per_parent: 24,
                    radius: 1.45,
                    parent_jitter: 0.6,
                    radial_weight: 0.3,
                    tangential_weight: 0.55,
                    random_weight: 0.35,
                    flow_weight: 0.75,
                },
                competition_group: Some(1),
            },
            VegetationPopulation {
                id: BROAD_LEAF_POPULATION_ID,
                key: "broad_leaf".into(),
                species: vec![crate::SpeciesChoice {
                    species: BROAD_LEAF_SPECIES_ID,
                    weight: 1.0,
                }],
                density_per_square_meter: 1.0,
                seed: 0x713e_f25b,
                growth: GrowthPattern::ParentChild {
                    parent_spacing: 1.25,
                    children_per_parent: 2,
                    radius: 0.65,
                    parent_jitter: 0.8,
                    radial_weight: 0.7,
                    tangential_weight: -0.15,
                    random_weight: 0.45,
                    flow_weight: 0.15,
                },
                competition_group: Some(1),
            },
            VegetationPopulation {
                id: SHORT_FILL_POPULATION_ID,
                key: "short_split_fill".into(),
                species: vec![crate::SpeciesChoice {
                    species: SHORT_FILL_SPECIES_ID,
                    weight: 1.0,
                }],
                density_per_square_meter: 18.0,
                seed: 0x5a37_c19d,
                growth: GrowthPattern::Uniform { jitter: 0.92 },
                competition_group: None,
            },
        ],
        assemblages: vec![
            VegetationAssemblage {
                id: DRY_FIELD_ASSEMBLAGE_ID,
                key: "dry_field".into(),
                populations: vec![DRY_TUFT_POPULATION_ID, SHORT_FILL_POPULATION_ID],
            },
            VegetationAssemblage {
                id: MIXED_GREEN_ASSEMBLAGE_ID,
                key: "mixed_green".into(),
                populations: vec![
                    GREEN_FINE_POPULATION_ID,
                    BROAD_LEAF_POPULATION_ID,
                    SHORT_FILL_POPULATION_ID,
                ],
            },
        ],
    }
}

pub fn reference_scene() -> VegetationScene {
    VegetationScene {
        catalog: reference_catalog(),
        pages: vec![reference_page([0.0, 0.0]), reference_page([16.0, 0.0])],
    }
}

/// An adjacent-page fixture containing dry tufts and a competing fine/broad-leaf mixture.
pub fn reference_page(origin_xz: [f32; 2]) -> VegetationFieldPage {
    let resolution = usize::from(REFERENCE_FIELD_RESOLUTION);
    let mut dry_coverage = Vec::with_capacity(resolution * resolution);
    let mut fine_coverage = Vec::with_capacity(resolution * resolution);
    let mut broad_coverage = Vec::with_capacity(resolution * resolution);
    let mut short_coverage = Vec::with_capacity(resolution * resolution);

    for z in 0..resolution {
        for x in 0..resolution {
            let world_x = origin_xz[0] + (x as f32 + 0.5) * REFERENCE_PAGE_SIZE / resolution as f32;
            let world_z = origin_xz[1] + (z as f32 + 0.5) * REFERENCE_PAGE_SIZE / resolution as f32;
            let broad_patch = smoothstep(0.2, 0.8, hash_noise(world_x * 0.17, world_z * 0.17));
            let dry_band = smoothstep(2.0, 6.0, world_z) * (1.0 - smoothstep(10.0, 15.0, world_z));
            dry_coverage.push(to_unorm8(0.18 + 0.72 * dry_band));
            fine_coverage.push(to_unorm8(0.9 - 0.45 * broad_patch));
            broad_coverage.push(to_unorm8(0.15 + 0.8 * broad_patch));
            short_coverage.push(to_unorm8(0.82 + 0.16 * (1.0 - broad_patch)));
        }
    }

    VegetationFieldPage {
        origin_xz,
        size: REFERENCE_PAGE_SIZE,
        surface: VegetationSurfaceField::flat(2, 0.0, [0.0, 1.0, 0.0]),
        fields: vec![
            VegetationPopulationField {
                population: DRY_TUFT_POPULATION_ID,
                resolution: REFERENCE_FIELD_RESOLUTION,
                coverage: dry_coverage,
                flow_direction: [0.9, 0.3],
            },
            VegetationPopulationField {
                population: GREEN_FINE_POPULATION_ID,
                resolution: REFERENCE_FIELD_RESOLUTION,
                coverage: fine_coverage,
                flow_direction: [0.2, 1.0],
            },
            VegetationPopulationField {
                population: BROAD_LEAF_POPULATION_ID,
                resolution: REFERENCE_FIELD_RESOLUTION,
                coverage: broad_coverage,
                flow_direction: [0.2, 1.0],
            },
            VegetationPopulationField {
                population: SHORT_FILL_POPULATION_ID,
                resolution: REFERENCE_FIELD_RESOLUTION,
                coverage: short_coverage,
                flow_direction: [0.55, 0.84],
            },
        ],
    }
}

pub fn full_coverage_page(
    origin_xz: [f32; 2],
    size: f32,
    population: VegetationPopulationId,
) -> VegetationFieldPage {
    VegetationFieldPage {
        origin_xz,
        size,
        surface: VegetationSurfaceField::flat(2, 0.0, [0.0, 1.0, 0.0]),
        fields: vec![VegetationPopulationField {
            population,
            resolution: 1,
            coverage: vec![u8::MAX],
            flow_direction: [0.0, 1.0],
        }],
    }
}

fn dry_fine_species() -> VegetationSpecies {
    let mut topology = ribbon_topology(7, 3, 2, 0.72);
    topology.minimum_tilt_radians = 0.42;
    topology.maximum_tilt_radians = 1.32;
    topology.minimum_bend = 0.28;
    topology.maximum_bend = 1.2;
    topology.maximum_lateral_curve = 0.38;
    topology.pair_spread_radians = 0.62;
    VegetationSpecies {
        id: DRY_FINE_SPECIES_ID,
        key: "dry_fine_ribbon".into(),
        topology: TopologyProfile::Ribbon(topology),
        material: material([0.11, 0.035, 0.01], [0.72, 0.28, 0.04], 0.18, 0.72),
        wind: wind(0.28, 0.95, 0.42),
        bounds: bounds(0.48, 1.25, 0.006, 0.022, 0.8),
        representations: ribbon_representations(TopologyFamily::Ribbon),
    }
}

fn green_fine_species() -> VegetationSpecies {
    let mut topology = ribbon_topology(8, 3, 1, 0.68);
    topology.minimum_tilt_radians = 0.24;
    topology.maximum_tilt_radians = 1.16;
    topology.minimum_bend = 0.18;
    topology.maximum_bend = 1.0;
    topology.maximum_lateral_curve = 0.32;
    VegetationSpecies {
        id: GREEN_FINE_SPECIES_ID,
        key: "green_fine_ribbon".into(),
        topology: TopologyProfile::Ribbon(topology),
        material: material([0.015, 0.06, 0.01], [0.12, 0.52, 0.08], 0.1, 0.78),
        wind: wind(0.38, 0.8, 0.34),
        bounds: bounds(0.42, 1.05, 0.008, 0.026, 0.62),
        representations: ribbon_representations(TopologyFamily::Ribbon),
    }
}

fn short_fill_species() -> VegetationSpecies {
    let mut topology = ribbon_topology(5, 2, 2, 0.78);
    topology.minimum_tilt_radians = 0.52;
    topology.maximum_tilt_radians = 1.34;
    topology.minimum_bend = 0.18;
    topology.maximum_bend = 0.78;
    topology.maximum_lateral_curve = 0.22;
    topology.pair_spread_radians = 1.18;
    VegetationSpecies {
        id: SHORT_FILL_SPECIES_ID,
        key: "short_split_fill_ribbon".into(),
        topology: TopologyProfile::Ribbon(topology),
        material: material([0.018, 0.035, 0.008], [0.24, 0.34, 0.07], 0.12, 0.88),
        wind: wind(0.72, 1.15, 0.12),
        bounds: bounds(0.14, 0.38, 0.014, 0.042, 0.4),
        representations: ribbon_representations(TopologyFamily::Ribbon),
    }
}

fn broad_leaf_species() -> VegetationSpecies {
    VegetationSpecies {
        id: BROAD_LEAF_SPECIES_ID,
        key: "green_broad_leaf".into(),
        topology: TopologyProfile::BroadLeafCluster(BroadLeafTopologyProfile {
            high_section_count: 6,
            low_section_count: 2,
            minimum_leaf_count: 2,
            maximum_leaf_count: 2,
            crown_radius: 0.42,
            minimum_droop: 0.08,
            maximum_droop: 0.7,
            maximum_camber: 0.32,
        }),
        material: material([0.008, 0.045, 0.006], [0.17, 0.58, 0.09], 0.07, 0.82),
        wind: wind(0.62, 1.4, 0.2),
        bounds: bounds(0.16, 0.52, 0.025, 0.09, 0.5),
        representations: vec![
            RepresentationLevel {
                minimum_projected_size: 36.0,
                density_fraction: 1.0,
                kind: RepresentationKind::Procedural(TopologyFamily::BroadLeafCluster),
            },
            RepresentationLevel {
                minimum_projected_size: 5.0,
                density_fraction: 0.32,
                kind: RepresentationKind::Procedural(TopologyFamily::BroadLeafCluster),
            },
            RepresentationLevel {
                minimum_projected_size: 0.75,
                density_fraction: 0.32,
                kind: RepresentationKind::Procedural(TopologyFamily::BroadLeafCluster),
            },
        ],
    }
}

fn ribbon_topology(
    high_section_count: u8,
    low_section_count: u8,
    blades_per_render_unit: u8,
    longitudinal_power: f32,
) -> RibbonTopologyProfile {
    RibbonTopologyProfile {
        high_section_count,
        low_section_count,
        blades_per_render_unit,
        longitudinal_power,
        minimum_tilt_radians: 0.08,
        maximum_tilt_radians: 1.1,
        minimum_bend: 0.08,
        maximum_bend: 0.85,
        maximum_lateral_curve: 0.26,
        pair_spread_radians: 0.46,
    }
}

fn material(
    root_color: [f32; 3],
    tip_color: [f32; 3],
    variation: f32,
    roughness: f32,
) -> VegetationMaterialProfile {
    VegetationMaterialProfile {
        root_color,
        tip_color,
        clump_color_variation: variation,
        perceptual_roughness: roughness,
        transmission: 0.18,
        root_ao: 0.42,
        tip_ao: 0.92,
        normal_rounding: 0.32,
    }
}

fn wind(stiffness: f32, drag: f32, maximum_tip_displacement: f32) -> VegetationWindProfile {
    VegetationWindProfile {
        stiffness,
        drag,
        phase_spread_radians: std::f32::consts::TAU,
        vertical_response: 0.16,
        maximum_tip_displacement,
    }
}

fn bounds(
    minimum_height: f32,
    maximum_height: f32,
    minimum_half_width: f32,
    maximum_half_width: f32,
    maximum_horizontal_reach: f32,
) -> VegetationBounds {
    VegetationBounds {
        minimum_height,
        maximum_height,
        minimum_half_width,
        maximum_half_width,
        maximum_horizontal_reach,
    }
}

fn ribbon_representations(family: TopologyFamily) -> Vec<RepresentationLevel> {
    vec![
        RepresentationLevel {
            minimum_projected_size: 48.0,
            density_fraction: 1.0,
            kind: RepresentationKind::Procedural(family),
        },
        RepresentationLevel {
            minimum_projected_size: 7.0,
            density_fraction: 0.25,
            kind: RepresentationKind::Procedural(family),
        },
        RepresentationLevel {
            minimum_projected_size: 0.75,
            density_fraction: 0.25,
            kind: RepresentationKind::Procedural(family),
        },
    ]
}

fn hash_noise(x: f32, z: f32) -> f32 {
    let value = (x.mul_add(12.9898, z * 78.233)).sin() * 43_758.547;
    value.fract().abs()
}

fn smoothstep(minimum: f32, maximum: f32, value: f32) -> f32 {
    let t = ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn to_unorm8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
