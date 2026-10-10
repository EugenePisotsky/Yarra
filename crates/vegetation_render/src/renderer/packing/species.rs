//! One species' shape, material, LOD thresholds and conservative reach, as the grass shaders
//! read them.
use crate::{
    VegetationWind,
    renderer::{
        gpu_types::SpeciesGpu,
        topology::{MAX_LOW_RENDER_SECTIONS, MAX_RENDER_SECTIONS},
    },
};
use vegetation::{TopologyFamily, TopologyProfile};

pub(super) fn effective_horizontal_reach(species: &vegetation::VegetationSpecies) -> f32 {
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

fn topology_code(family: TopologyFamily) -> f32 {
    match family {
        TopologyFamily::Ribbon => 0.0,
        TopologyFamily::RibbonTuft => 1.0,
        TopologyFamily::BroadLeafCluster => 2.0,
    }
}

pub(super) fn fallback_topology_bin(topology: TopologyProfile) -> u32 {
    match topology {
        TopologyProfile::Ribbon(_) => 0,
        TopologyProfile::BroadLeafCluster(_) => 1,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ProceduralLodProfile {
    pub(super) high_minimum_pixels: f32,
    pub(super) low_minimum_pixels: f32,
    pub(super) far_minimum_pixels: f32,
    pub(super) low_density_fraction: f32,
    pub(super) far_density_fraction: f32,
}

pub(super) fn procedural_lod_profile(
    species: &vegetation::VegetationSpecies,
) -> ProceduralLodProfile {
    let procedural_levels = species.representations.iter().collect::<Vec<_>>();
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

pub(in crate::renderer) fn pack_species(
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
                    0.0,
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
                    0.0,
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
        // The pair threshold is placement's alone (`SpeciesChoiceGpu::packing`); it bins each root
        // and the instance carries the result.
        height_packing: [
            species.height.distribution_bias,
            0.0,
            high_detail_radii[0],
            high_detail_radii[1],
        ],
    }
}

pub(super) fn pack_ribbon_curve(profile: vegetation::RibbonCurveProfile) -> [f32; 4] {
    [
        profile.root_tangent_radians.sin() * profile.root_handle_length,
        profile.root_tangent_radians.cos() * profile.root_handle_length,
        profile.tip_tangent_radians.sin() * profile.tip_handle_length,
        profile.tip_tangent_radians.cos() * profile.tip_handle_length,
    ]
}
