//! Pure vegetation domain contracts shared by cooking, rendering, editor tooling, and tests.
//!
//! This crate deliberately has no Bevy, SQLite, or GPU dependency. It defines what vegetation
//! means; integration crates decide how records are stored, streamed, and rendered.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod fixtures;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub struct $name(pub [u8; 16]);
    };
}

id_type!(VegetationSpeciesId);
id_type!(VegetationPopulationId);
id_type!(VegetationAssemblageId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TopologyFamily {
    Ribbon,
    RibbonTuft,
    BroadLeafCluster,
    StemAndHead,
    CardImpostor,
    AuthoredMesh,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RibbonCurveProfile {
    /// Tip position on the normalized blade-height arc, measured from the terrain normal toward
    /// the blade facing direction.
    pub tip_tilt_radians: f32,
    /// Root tangent angle measured from the terrain normal toward the blade facing direction.
    pub root_tangent_radians: f32,
    /// Tip tangent angle measured from the terrain normal toward the blade facing direction.
    /// Values above 90 degrees let the curve arrive at the tip while travelling downward.
    pub tip_tangent_radians: f32,
    /// Root-side cubic handle length as a fraction of the blade height.
    pub root_handle_length: f32,
    /// Tip-side cubic handle length as a fraction of the blade height.
    pub tip_handle_length: f32,
}

impl RibbonCurveProfile {
    fn is_valid(self) -> bool {
        finite_range(self.tip_tilt_radians, 0.0, 1.55)
            && finite_range(self.root_tangent_radians, -1.55, 1.55)
            && finite_range(self.tip_tangent_radians, -1.55, 3.05)
            && finite_range(self.root_handle_length, 0.02, 1.5)
            && finite_range(self.tip_handle_length, 0.02, 1.5)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RibbonTopologyProfile {
    pub high_section_count: u8,
    pub low_section_count: u8,
    pub blades_per_render_unit: u8,
    pub longitudinal_power: f32,
    /// Endpoints of one correlated, per-blade silhouette axis. Each variant owns the tip plus both
    /// cubic handles so the editor and renderer interpolate complete curves.
    pub curve_variant_a: RibbonCurveProfile,
    pub curve_variant_b: RibbonCurveProfile,
    pub maximum_lateral_curve: f32,
    pub pair_spread_radians: f32,
    /// Maximum angle through which the renderer may rotate the ribbon width line toward the
    /// camera-facing width line around the local curve tangent. The response is zero when the
    /// physical ribbon already faces the camera and grows continuously toward edge-on views; zero
    /// disables it.
    pub maximum_view_opening_radians: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BroadLeafTopologyProfile {
    pub high_section_count: u8,
    pub low_section_count: u8,
    pub minimum_leaf_count: u8,
    pub maximum_leaf_count: u8,
    pub crown_radius: f32,
    pub minimum_droop: f32,
    pub maximum_droop: f32,
    pub maximum_camber: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TopologyProfile {
    Ribbon(RibbonTopologyProfile),
    BroadLeafCluster(BroadLeafTopologyProfile),
}

impl TopologyProfile {
    pub const fn family(self) -> TopologyFamily {
        match self {
            Self::Ribbon(profile) if profile.blades_per_render_unit > 2 => {
                TopologyFamily::RibbonTuft
            }
            Self::Ribbon(_) => TopologyFamily::Ribbon,
            Self::BroadLeafCluster(_) => TopologyFamily::BroadLeafCluster,
        }
    }

    fn is_valid(self) -> bool {
        match self {
            Self::Ribbon(profile) => {
                (2..=12).contains(&profile.high_section_count)
                    && (1..profile.high_section_count).contains(&profile.low_section_count)
                    && (1..=2).contains(&profile.blades_per_render_unit)
                    && finite_range(profile.longitudinal_power, 0.2, 4.0)
                    && profile.curve_variant_a.is_valid()
                    && profile.curve_variant_b.is_valid()
                    && profile.curve_variant_a.tip_tilt_radians
                        <= profile.curve_variant_b.tip_tilt_radians
                    && finite_range(profile.maximum_lateral_curve, 0.0, 1.0)
                    && finite_range(profile.pair_spread_radians, 0.0, 3.15)
                    && finite_range(
                        profile.maximum_view_opening_radians,
                        0.0,
                        std::f32::consts::FRAC_PI_4,
                    )
            }
            Self::BroadLeafCluster(profile) => {
                (2..=12).contains(&profile.high_section_count)
                    && (1..profile.high_section_count).contains(&profile.low_section_count)
                    && profile.minimum_leaf_count == 2
                    && profile.maximum_leaf_count == 2
                    && finite_range(profile.crown_radius, 0.0, 2.0)
                    && finite_range(profile.minimum_droop, 0.0, 2.0)
                    && finite_range(profile.maximum_droop, profile.minimum_droop, 2.0)
                    && finite_range(profile.maximum_camber, 0.0, 1.0)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VegetationMaterialProfile {
    pub root_color: [f32; 3],
    pub tip_color: [f32; 3],
    pub clump_color_variation: f32,
    pub perceptual_roughness: f32,
    pub transmission: f32,
    pub root_ao: f32,
    pub tip_ao: f32,
    pub normal_rounding: f32,
}

impl VegetationMaterialProfile {
    fn is_valid(self) -> bool {
        self.root_color.into_iter().all(valid_color_component)
            && self.tip_color.into_iter().all(valid_color_component)
            && finite_range(self.clump_color_variation, 0.0, 1.0)
            && finite_range(self.perceptual_roughness, 0.0, 1.0)
            && finite_range(self.transmission, 0.0, 1.0)
            && finite_range(self.root_ao, 0.0, 1.0)
            && finite_range(self.tip_ao, 0.0, 1.0)
            && finite_range(self.normal_rounding, 0.0, 1.0)
    }
}

/// How strongly a species replaces per-blade shape randomness with a stable group signal.
///
/// A value of zero keeps independent blade variation. A value of one makes every blade in a
/// group use the same random coordinate for that channel. The authored topology ranges remain the
/// hard geometry bounds in both cases.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VegetationGroupResponseProfile {
    pub height_coherence: f32,
    /// Coherence of the complete ribbon silhouette (tip plus both handles), or the complete
    /// broad-leaf droop shape for that topology family.
    pub silhouette_coherence: f32,
    pub lateral_curve_coherence: f32,
}

impl VegetationGroupResponseProfile {
    fn is_valid(self) -> bool {
        finite_range(self.height_coherence, 0.0, 1.0)
            && finite_range(self.silhouette_coherence, 0.0, 1.0)
            && finite_range(self.lateral_curve_coherence, 0.0, 1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VegetationWindProfile {
    pub stiffness: f32,
    pub drag: f32,
    pub phase_spread_radians: f32,
    pub vertical_response: f32,
    pub maximum_tip_displacement: f32,
}

impl VegetationWindProfile {
    fn is_valid(self) -> bool {
        finite_range(self.stiffness, 0.0, 1.0)
            && finite_range(self.drag, 0.0, 4.0)
            && finite_range(self.phase_spread_radians, 0.0, std::f32::consts::TAU)
            && finite_range(self.vertical_response, 0.0, 1.0)
            && finite_range(self.maximum_tip_displacement, 0.0, 8.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VegetationBounds {
    pub minimum_height: f32,
    pub maximum_height: f32,
    pub minimum_half_width: f32,
    pub maximum_half_width: f32,
    pub maximum_horizontal_reach: f32,
}

impl VegetationBounds {
    fn is_valid(self) -> bool {
        finite_range(self.minimum_height, 0.01, 16.0)
            && finite_range(self.maximum_height, self.minimum_height, 16.0)
            && finite_range(self.minimum_half_width, 0.0001, 4.0)
            && finite_range(self.maximum_half_width, self.minimum_half_width, 4.0)
            && finite_range(self.maximum_horizontal_reach, 0.0, 16.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RepresentationKind {
    Procedural(TopologyFamily),
    CardImpostor,
    AuthoredMesh,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RepresentationLevel {
    /// The representation is eligible at or above this projected pixel size.
    pub minimum_projected_size: f32,
    pub density_fraction: f32,
    pub kind: RepresentationKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationSpecies {
    pub id: VegetationSpeciesId,
    pub key: String,
    pub topology: TopologyProfile,
    pub material: VegetationMaterialProfile,
    pub group_response: VegetationGroupResponseProfile,
    pub wind: VegetationWindProfile,
    pub bounds: VegetationBounds,
    /// Ordered from the highest-detail representation to the farthest.
    pub representations: Vec<RepresentationLevel>,
}

impl VegetationSpecies {
    fn is_valid(&self) -> bool {
        if self.key.is_empty()
            || !self.topology.is_valid()
            || !self.material.is_valid()
            || !self.group_response.is_valid()
            || !self.wind.is_valid()
            || !self.bounds.is_valid()
            || self.representations.is_empty()
        {
            return false;
        }

        let topology_family = self.topology.family();
        let mut previous_size = f32::INFINITY;
        let mut previous_density = 1.0;
        for level in &self.representations {
            if !finite_range(level.minimum_projected_size, 0.0, 16_384.0)
                || !finite_range(level.density_fraction, f32::EPSILON, 1.0)
                || level.minimum_projected_size >= previous_size
                || level.density_fraction > previous_density
            {
                return false;
            }
            if let RepresentationKind::Procedural(family) = level.kind
                && family != topology_family
                && !(topology_family == TopologyFamily::RibbonTuft
                    && family == TopologyFamily::Ribbon)
            {
                return false;
            }
            previous_size = level.minimum_projected_size;
            previous_density = level.density_fraction;
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GrowthPattern {
    Uniform {
        jitter: f32,
    },
    ParentChild {
        parent_spacing: f32,
        children_per_parent: u16,
        radius: f32,
        parent_jitter: f32,
    },
}

impl GrowthPattern {
    fn is_valid(self, density_per_square_meter: f32) -> bool {
        match self {
            Self::Uniform { jitter } => finite_range(jitter, 0.0, 1.0),
            Self::ParentChild {
                parent_spacing,
                children_per_parent,
                radius,
                parent_jitter,
            } => {
                if !finite_range(parent_spacing, 0.05, 64.0)
                    || !(1..=256).contains(&children_per_parent)
                    || !finite_range(radius, 0.0, parent_spacing * 2.0)
                    || !finite_range(parent_jitter, 0.0, 1.0)
                {
                    return false;
                }
                let maximum_density = f32::from(children_per_parent) / parent_spacing.powi(2);
                density_per_square_meter <= maximum_density * (1.0 + 1e-5)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VoronoiClumpProfile {
    /// Distance between procedural feature-point cells in world metres.
    pub spacing: f32,
    /// Jitter of each feature point inside its cell, from centred to the full cell.
    pub feature_jitter: f32,
    /// Width of the fade where the nearest two feature points are similarly distant.
    pub boundary_softness: f32,
    /// Fraction of the centre vector applied to candidate roots away from boundaries.
    pub root_attraction: f32,
    /// Candidate retention at the feature point and at the outer clump region.
    pub center_retention: f32,
    pub edge_retention: f32,
    pub retention_falloff: f32,
    /// Bounded group-to-group reduction of the retention profile.
    pub density_variation: f32,
}

impl VoronoiClumpProfile {
    fn is_valid(self) -> bool {
        finite_range(self.spacing, 0.05, 64.0)
            && finite_range(self.feature_jitter, 0.0, 1.0)
            && finite_range(self.boundary_softness, 0.0, 1.0)
            && finite_range(self.root_attraction, 0.0, 1.0)
            && finite_range(self.center_retention, 0.0, 1.0)
            && finite_range(self.edge_retention, 0.0, 1.0)
            && finite_range(self.retention_falloff, 0.1, 8.0)
            && finite_range(self.density_variation, 0.0, 1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum VegetationGroupingProfile {
    None,
    /// Use the explicit parent produced by parent/child root placement.
    Parent,
    /// Assign continuous stratified roots to analytic world-space Voronoi groups.
    Voronoi(VoronoiClumpProfile),
}

impl VegetationGroupingProfile {
    fn is_valid(self, growth: GrowthPattern) -> bool {
        match (self, growth) {
            (Self::None, _) | (Self::Parent, GrowthPattern::ParentChild { .. }) => true,
            (Self::Voronoi(profile), GrowthPattern::Uniform { .. }) => profile.is_valid(),
            (Self::Parent, GrowthPattern::Uniform { .. })
            | (Self::Voronoi(_), GrowthPattern::ParentChild { .. }) => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VegetationOrientationProfile {
    pub shared_group_weight: f32,
    pub radial_weight: f32,
    pub tangential_weight: f32,
    pub random_weight: f32,
    pub flow_weight: f32,
    pub angular_jitter_radians: f32,
}

impl VegetationOrientationProfile {
    fn is_valid(self, grouping: VegetationGroupingProfile) -> bool {
        let grouped_weight = self.shared_group_weight.abs()
            + self.radial_weight.abs()
            + self.tangential_weight.abs();
        finite_range(self.shared_group_weight.abs(), 0.0, 4.0)
            && finite_range(self.radial_weight.abs(), 0.0, 4.0)
            && finite_range(self.tangential_weight.abs(), 0.0, 4.0)
            && finite_range(self.random_weight, 0.0, 4.0)
            && finite_range(self.flow_weight.abs(), 0.0, 4.0)
            && finite_range(self.angular_jitter_radians, 0.0, std::f32::consts::PI)
            && grouped_weight + self.random_weight + self.flow_weight.abs() > f32::EPSILON
            && (!matches!(grouping, VegetationGroupingProfile::None)
                || grouped_weight <= f32::EPSILON)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpeciesChoice {
    pub species: VegetationSpeciesId,
    pub weight: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationPopulation {
    pub id: VegetationPopulationId,
    pub key: String,
    pub species: Vec<SpeciesChoice>,
    pub density_per_square_meter: f32,
    pub seed: u32,
    pub growth: GrowthPattern,
    pub grouping: VegetationGroupingProfile,
    pub orientation: VegetationOrientationProfile,
    /// Populations sharing a nonzero group compete for local occupancy.
    pub competition_group: Option<u16>,
}

impl VegetationPopulation {
    fn is_valid(&self, species_ids: &HashSet<VegetationSpeciesId>) -> bool {
        let unique_choice_count = self
            .species
            .iter()
            .map(|choice| choice.species)
            .collect::<HashSet<_>>()
            .len();
        !self.key.is_empty()
            && finite_range(self.density_per_square_meter, 0.0001, 512.0)
            && self.growth.is_valid(self.density_per_square_meter)
            && self.grouping.is_valid(self.growth)
            && self.orientation.is_valid(self.grouping)
            && !self.species.is_empty()
            && unique_choice_count == self.species.len()
            && self.species.iter().all(|choice| {
                species_ids.contains(&choice.species)
                    && choice.weight.is_finite()
                    && choice.weight > 0.0
            })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationAssemblage {
    pub id: VegetationAssemblageId,
    pub key: String,
    pub populations: Vec<VegetationPopulationId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationCatalog {
    pub species: Vec<VegetationSpecies>,
    pub populations: Vec<VegetationPopulation>,
    pub assemblages: Vec<VegetationAssemblage>,
}

impl VegetationCatalog {
    pub fn validate(&self) -> Result<(), CatalogValidationError> {
        let mut species_ids = HashSet::new();
        let mut species_keys = HashSet::new();
        for species in &self.species {
            if !species_ids.insert(species.id) {
                return Err(CatalogValidationError::DuplicateSpecies(species.id));
            }
            if !species_keys.insert(species.key.as_str()) {
                return Err(CatalogValidationError::DuplicateSpeciesKey(
                    species.key.clone(),
                ));
            }
            if !species.is_valid() {
                return Err(CatalogValidationError::InvalidSpecies(species.id));
            }
        }

        let mut population_ids = HashSet::new();
        let mut population_keys = HashSet::new();
        for population in &self.populations {
            if !population_ids.insert(population.id) {
                return Err(CatalogValidationError::DuplicatePopulation(population.id));
            }
            if !population_keys.insert(population.key.as_str()) {
                return Err(CatalogValidationError::DuplicatePopulationKey(
                    population.key.clone(),
                ));
            }
            if !population.is_valid(&species_ids) {
                return Err(CatalogValidationError::InvalidPopulation(population.id));
            }
        }

        let mut assemblage_ids = HashSet::new();
        let mut assemblage_keys = HashSet::new();
        for assemblage in &self.assemblages {
            if !assemblage_ids.insert(assemblage.id) {
                return Err(CatalogValidationError::DuplicateAssemblage(assemblage.id));
            }
            if !assemblage_keys.insert(assemblage.key.as_str()) {
                return Err(CatalogValidationError::DuplicateAssemblageKey(
                    assemblage.key.clone(),
                ));
            }
            if assemblage.key.is_empty()
                || assemblage.populations.is_empty()
                || assemblage
                    .populations
                    .iter()
                    .any(|population| !population_ids.contains(population))
                || assemblage
                    .populations
                    .iter()
                    .copied()
                    .collect::<HashSet<_>>()
                    .len()
                    != assemblage.populations.len()
            {
                return Err(CatalogValidationError::InvalidAssemblage(assemblage.id));
            }
        }
        Ok(())
    }

    pub fn species(&self, id: VegetationSpeciesId) -> Option<&VegetationSpecies> {
        self.species.iter().find(|species| species.id == id)
    }

    pub fn population(&self, id: VegetationPopulationId) -> Option<&VegetationPopulation> {
        self.populations
            .iter()
            .find(|population| population.id == id)
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum CatalogValidationError {
    #[error("duplicate vegetation species {0:?}")]
    DuplicateSpecies(VegetationSpeciesId),
    #[error("duplicate vegetation species key {0}")]
    DuplicateSpeciesKey(String),
    #[error("invalid vegetation species {0:?}")]
    InvalidSpecies(VegetationSpeciesId),
    #[error("duplicate vegetation population {0:?}")]
    DuplicatePopulation(VegetationPopulationId),
    #[error("duplicate vegetation population key {0}")]
    DuplicatePopulationKey(String),
    #[error("invalid vegetation population {0:?}")]
    InvalidPopulation(VegetationPopulationId),
    #[error("duplicate vegetation assemblage {0:?}")]
    DuplicateAssemblage(VegetationAssemblageId),
    #[error("duplicate vegetation assemblage key {0}")]
    DuplicateAssemblageKey(String),
    #[error("invalid vegetation assemblage {0:?}")]
    InvalidAssemblage(VegetationAssemblageId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationPopulationField {
    pub population: VegetationPopulationId,
    pub resolution: u16,
    /// Row-major R8 weight samples.
    pub coverage: Vec<u8>,
    /// Optional normalized world-space direction. Zero selects the population's fallback flow.
    pub flow_direction: [f32; 2],
}

impl VegetationPopulationField {
    pub fn sample_coverage(&self, page: &VegetationFieldPage, world_xz: [f32; 2]) -> f32 {
        if !page.owns(world_xz) || self.resolution == 0 {
            return 0.0;
        }
        let resolution = usize::from(self.resolution);
        let local_x = ((world_xz[0] - page.origin_xz[0]) / page.size).clamp(0.0, 0.999_999);
        let local_z = ((world_xz[1] - page.origin_xz[1]) / page.size).clamp(0.0, 0.999_999);
        let x = (local_x * resolution as f32) as usize;
        let z = (local_z * resolution as f32) as usize;
        f32::from(self.coverage[z * resolution + x]) / 255.0
    }
}

/// Persistent, terrain-independent vegetation data for one streamed world page.
///
/// The page key and world-space record provide its cell and extent. Relief is deliberately absent:
/// a resident vegetation page joins this data with the matching streamed terrain heightfield. This
/// avoids storing the same surface twice and guarantees placement, grounding, and rendering sample
/// one authoritative terrain page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationFieldPageData {
    pub fields: Vec<VegetationPopulationField>,
}

impl VegetationFieldPageData {
    pub fn validate(&self, catalog: &VegetationCatalog) -> Result<(), PageValidationError> {
        let mut populations = HashSet::new();
        for field in &self.fields {
            if catalog.population(field.population).is_none() {
                return Err(PageValidationError::MissingPopulation(field.population));
            }
            if !populations.insert(field.population) {
                return Err(PageValidationError::DuplicatePopulation(field.population));
            }
            let resolution = usize::from(field.resolution);
            if resolution == 0
                || resolution > 256
                || field.coverage.len() != resolution * resolution
                || !field.flow_direction.into_iter().all(f32::is_finite)
            {
                return Err(PageValidationError::InvalidField(field.population));
            }
        }
        Ok(())
    }
}

/// Resident/debug page assembled from persisted vegetation fields and streamed terrain relief.
///
/// This composite is convenient for CPU reference placement and the current GPU diagnostic, but it
/// is not the format written to vegetation pages in the world database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationFieldPage {
    pub origin_xz: [f32; 2],
    pub size: f32,
    pub surface: VegetationSurfaceField,
    pub fields: Vec<VegetationPopulationField>,
}

impl VegetationFieldPage {
    pub fn owns(&self, world_xz: [f32; 2]) -> bool {
        world_xz[0] >= self.origin_xz[0]
            && world_xz[1] >= self.origin_xz[1]
            && world_xz[0] < self.origin_xz[0] + self.size
            && world_xz[1] < self.origin_xz[1] + self.size
    }

    pub fn validate(&self, catalog: &VegetationCatalog) -> Result<(), PageValidationError> {
        if !self.origin_xz.into_iter().all(f32::is_finite)
            || !finite_range(self.size, 0.01, 65_536.0)
        {
            return Err(PageValidationError::InvalidExtent);
        }
        if !self.surface.is_valid() {
            return Err(PageValidationError::InvalidSurface);
        }
        self.data().validate(catalog)
    }

    pub fn data(&self) -> VegetationFieldPageData {
        VegetationFieldPageData {
            fields: self.fields.clone(),
        }
    }

    pub fn from_data(
        origin_xz: [f32; 2],
        size: f32,
        surface: VegetationSurfaceField,
        data: VegetationFieldPageData,
    ) -> Self {
        Self {
            origin_xz,
            size,
            surface,
            fields: data.fields,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum PageValidationError {
    #[error("invalid vegetation page extent")]
    InvalidExtent,
    #[error("invalid vegetation page surface field")]
    InvalidSurface,
    #[error("vegetation page references missing population {0:?}")]
    MissingPopulation(VegetationPopulationId),
    #[error("vegetation page repeats population {0:?}")]
    DuplicatePopulation(VegetationPopulationId),
    #[error("invalid field for vegetation population {0:?}")]
    InvalidField(VegetationPopulationId),
}

/// Page-local terrain samples shared by every population on the page.
///
/// Samples include both page edges, so adjacent cooked pages can carry identical boundary values.
/// Normals use signed 16-bit octahedral encoding; validity is R8. This keeps the persistent
/// contract compact while exposing ordinary world-space samples to CPU and GPU placement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationSurfaceField {
    pub resolution: u16,
    pub heights: Vec<f32>,
    pub normals_oct: Vec<[i16; 2]>,
    pub validity: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VegetationSurfaceSample {
    pub height: f32,
    pub normal: [f32; 3],
    pub validity: f32,
}

impl VegetationSurfaceField {
    pub fn flat(resolution: u16, height: f32, normal: [f32; 3]) -> Self {
        let sample_count = usize::from(resolution).saturating_mul(usize::from(resolution));
        Self {
            resolution,
            heights: vec![height; sample_count],
            normals_oct: vec![encode_octahedral_normal(normal); sample_count],
            validity: vec![u8::MAX; sample_count],
        }
    }

    pub fn set_flat(&mut self, height: f32, normal: [f32; 3]) {
        self.heights.fill(height);
        self.normals_oct.fill(encode_octahedral_normal(normal));
        self.validity.fill(u8::MAX);
    }

    pub fn sample(
        &self,
        page_origin_xz: [f32; 2],
        page_size: f32,
        world_xz: [f32; 2],
    ) -> VegetationSurfaceSample {
        let resolution = usize::from(self.resolution);
        debug_assert!(resolution >= 2 && self.is_valid());
        let local_x = ((world_xz[0] - page_origin_xz[0]) / page_size).clamp(0.0, 1.0);
        let local_z = ((world_xz[1] - page_origin_xz[1]) / page_size).clamp(0.0, 1.0);
        let grid_x = local_x * (resolution - 1) as f32;
        let grid_z = local_z * (resolution - 1) as f32;
        let x0 = grid_x.floor() as usize;
        let z0 = grid_z.floor() as usize;
        let x1 = (x0 + 1).min(resolution - 1);
        let z1 = (z0 + 1).min(resolution - 1);
        let tx = grid_x - x0 as f32;
        let tz = grid_z - z0 as f32;
        let indices = [
            z0 * resolution + x0,
            z0 * resolution + x1,
            z1 * resolution + x0,
            z1 * resolution + x1,
        ];
        let weights = [
            (1.0 - tx) * (1.0 - tz),
            tx * (1.0 - tz),
            (1.0 - tx) * tz,
            tx * tz,
        ];
        let mut height = 0.0;
        let mut normal = [0.0; 3];
        let mut validity = 0.0;
        for (index, weight) in indices.into_iter().zip(weights) {
            height += self.heights[index] * weight;
            let decoded = decode_octahedral_normal(self.normals_oct[index]);
            normal[0] += decoded[0] * weight;
            normal[1] += decoded[1] * weight;
            normal[2] += decoded[2] * weight;
            validity += f32::from(self.validity[index]) / 255.0 * weight;
        }
        VegetationSurfaceSample {
            height,
            normal: normalize3_or(normal, [0.0, 1.0, 0.0]),
            validity,
        }
    }

    fn is_valid(&self) -> bool {
        let resolution = usize::from(self.resolution);
        let expected = resolution.saturating_mul(resolution);
        (2..=257).contains(&resolution)
            && self.heights.len() == expected
            && self.normals_oct.len() == expected
            && self.validity.len() == expected
            && self.heights.iter().all(|height| height.is_finite())
    }
}

pub fn encode_octahedral_normal(normal: [f32; 3]) -> [i16; 2] {
    let normal = normalize3_or(normal, [0.0, 1.0, 0.0]);
    let inverse_l1 = (normal[0].abs() + normal[1].abs() + normal[2].abs()).recip();
    let mut encoded = [normal[0] * inverse_l1, normal[2] * inverse_l1];
    if normal[1] < 0.0 {
        encoded = [
            (1.0 - encoded[1].abs()) * encoded[0].signum(),
            (1.0 - encoded[0].abs()) * encoded[1].signum(),
        ];
    }
    [
        (encoded[0].clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16,
        (encoded[1].clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16,
    ]
}

pub fn decode_octahedral_normal(encoded: [i16; 2]) -> [f32; 3] {
    let encoded = [
        f32::from(encoded[0]) / f32::from(i16::MAX),
        f32::from(encoded[1]) / f32::from(i16::MAX),
    ];
    let mut normal = [
        encoded[0],
        1.0 - encoded[0].abs() - encoded[1].abs(),
        encoded[1],
    ];
    if normal[1] < 0.0 {
        let original_x = normal[0];
        normal[0] = (1.0 - normal[2].abs()) * original_x.signum();
        normal[2] = (1.0 - original_x.abs()) * normal[2].signum();
    }
    normalize3_or(normal, [0.0, 1.0, 0.0])
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationScene {
    pub catalog: VegetationCatalog,
    pub pages: Vec<VegetationFieldPage>,
}

impl VegetationScene {
    pub fn validate(&self) -> Result<(), SceneValidationError> {
        self.catalog.validate()?;
        for (index, page) in self.pages.iter().enumerate() {
            page.validate(&self.catalog)
                .map_err(|source| SceneValidationError::Page { index, source })?;
        }
        for (first, first_page) in self.pages.iter().enumerate() {
            for (second, second_page) in self.pages.iter().enumerate().skip(first + 1) {
                if pages_overlap(first_page, second_page) {
                    return Err(SceneValidationError::OverlappingPages { first, second });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum SceneValidationError {
    #[error(transparent)]
    Catalog(#[from] CatalogValidationError),
    #[error("invalid vegetation page {index}: {source}")]
    Page {
        index: usize,
        source: PageValidationError,
    },
    #[error("vegetation pages {first} and {second} overlap")]
    OverlappingPages { first: usize, second: usize },
}

fn pages_overlap(first: &VegetationFieldPage, second: &VegetationFieldPage) -> bool {
    first.origin_xz[0] < second.origin_xz[0] + second.size
        && first.origin_xz[0] + first.size > second.origin_xz[0]
        && first.origin_xz[1] < second.origin_xz[1] + second.size
        && first.origin_xz[1] + first.size > second.origin_xz[1]
}

/// World-lattice candidate range for one page/population pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidateDomain {
    pub cell_min: [i32; 2],
    pub cell_count: [u32; 2],
    pub candidates_per_cell: u32,
    pub spacing: f32,
}

impl CandidateDomain {
    pub fn candidate_count(self) -> u32 {
        self.cell_count[0]
            .saturating_mul(self.cell_count[1])
            .saturating_mul(self.candidates_per_cell)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupSample {
    pub key: u32,
    pub center_xz: [f32; 2],
    pub radial_direction: [f32; 2],
    pub normalized_distance: f32,
    pub boundary_influence: f32,
    pub density_retention: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidateSample {
    pub root_xz: [f32; 2],
    pub group: GroupSample,
    pub rest_direction: [f32; 2],
    pub stable_rank: f32,
    /// Nested four-way rank used by representation-density LOD.
    pub lod_rank: f32,
    pub clump_variant: f32,
    pub seed: u32,
}

pub fn candidate_domain(
    page: &VegetationFieldPage,
    population: &VegetationPopulation,
) -> CandidateDomain {
    let (spacing, placement_radius, candidates_per_cell) = match population.growth {
        GrowthPattern::Uniform { .. } => {
            (population.density_per_square_meter.sqrt().recip(), 0.0, 1)
        }
        GrowthPattern::ParentChild {
            parent_spacing,
            children_per_parent,
            radius,
            ..
        } => (parent_spacing, radius, u32::from(children_per_parent)),
    };
    let grouping_radius = match population.grouping {
        VegetationGroupingProfile::Voronoi(profile) => {
            profile.spacing * std::f32::consts::SQRT_2 * profile.root_attraction
        }
        VegetationGroupingProfile::None | VegetationGroupingProfile::Parent => 0.0,
    };
    let radius = placement_radius.max(grouping_radius);
    let minimum = [
        ((page.origin_xz[0] - radius) / spacing).floor() as i32,
        ((page.origin_xz[1] - radius) / spacing).floor() as i32,
    ];
    let maximum = [
        ((page.origin_xz[0] + page.size + radius) / spacing).ceil() as i32,
        ((page.origin_xz[1] + page.size + radius) / spacing).ceil() as i32,
    ];
    CandidateDomain {
        cell_min: minimum,
        cell_count: [
            maximum[0].saturating_sub(minimum[0]) as u32,
            maximum[1].saturating_sub(minimum[1]) as u32,
        ],
        candidates_per_cell,
        spacing,
    }
}

pub fn sample_candidate(
    population: &VegetationPopulation,
    domain: CandidateDomain,
    candidate_index: u32,
    flow_direction: [f32; 2],
) -> Option<CandidateSample> {
    if candidate_index >= domain.candidate_count() || domain.cell_count[0] == 0 {
        return None;
    }
    let cell_index = candidate_index / domain.candidates_per_cell;
    let child_index = candidate_index % domain.candidates_per_cell;
    let cell_x = domain.cell_min[0] + (cell_index % domain.cell_count[0]) as i32;
    let cell_z = domain.cell_min[1] + (cell_index / domain.cell_count[0]) as i32;
    let cell_seed = hash_cell(population.seed, cell_x, cell_z, 0x6d2b_79f5);

    let (parent, initial_root) = match population.growth {
        GrowthPattern::Uniform { jitter } => {
            let offset = [
                0.5 + (random01(cell_seed ^ 0xa511_e9b3) - 0.5) * jitter,
                0.5 + (random01(cell_seed ^ 0x63d8_3595) - 0.5) * jitter,
            ];
            let root = [
                (cell_x as f32 + offset[0]) * domain.spacing,
                (cell_z as f32 + offset[1]) * domain.spacing,
            ];
            (root, root)
        }
        GrowthPattern::ParentChild {
            parent_jitter,
            radius,
            ..
        } => {
            let parent_offset = [
                0.5 + (random01(cell_seed ^ 0xa511_e9b3) - 0.5) * parent_jitter,
                0.5 + (random01(cell_seed ^ 0x63d8_3595) - 0.5) * parent_jitter,
            ];
            let parent = [
                (cell_x as f32 + parent_offset[0]) * domain.spacing,
                (cell_z as f32 + parent_offset[1]) * domain.spacing,
            ];
            let child_seed = hash32(cell_seed ^ child_index.wrapping_mul(0x9e37_79b9));
            let angle = random01(child_seed ^ 0xc2b2_ae35) * std::f32::consts::TAU;
            let distance = random01(child_seed ^ 0x27d4_eb2f).sqrt() * radius;
            let radial = [angle.cos(), angle.sin()];
            let root = [
                parent[0] + radial[0] * distance,
                parent[1] + radial[1] * distance,
            ];
            (parent, root)
        }
    };

    let seed = hash32(cell_seed ^ child_index.wrapping_mul(0x85eb_ca6b));
    let group = sample_group(population, initial_root, parent, cell_seed, seed);
    let mut root = initial_root;
    if let VegetationGroupingProfile::Voronoi(profile) = population.grouping {
        let attraction = profile.root_attraction * group.boundary_influence;
        root = [
            root[0] + (group.center_xz[0] - root[0]) * attraction,
            root[1] + (group.center_xz[1] - root[1]) * attraction,
        ];
    }
    let lod_lane = match population.growth {
        GrowthPattern::Uniform { .. } => {
            let block_x = cell_x.div_euclid(2);
            let block_z = cell_z.div_euclid(2);
            let quadrant = cell_x.rem_euclid(2) as u32 + 2 * cell_z.rem_euclid(2) as u32;
            let rotation = hash_cell(population.seed, block_x, block_z, 0xa24b_aed5) & 3;
            (quadrant + rotation) & 3
        }
        GrowthPattern::ParentChild { .. } => {
            let group = child_index / 4;
            let rotation = hash_cell(
                population.seed,
                cell_x,
                cell_z,
                0xa24b_aed5 ^ group.wrapping_mul(0x9e37_79b9),
            ) & 3;
            (child_index % 4 + rotation) & 3
        }
    };
    let random_angle = random01(seed ^ 0x1656_67b1) * std::f32::consts::TAU;
    let random_direction = [random_angle.cos(), random_angle.sin()];
    let shared_angle = random01(group.key ^ 0x68e3_1da4) * std::f32::consts::TAU;
    let shared_direction = [shared_angle.cos(), shared_angle.sin()];
    let radial = group.radial_direction;
    let tangent = [-radial[1], radial[0]];
    let flow = normalize_or(flow_direction, [1.0, 0.0]);
    let orientation = population.orientation;
    let group_influence = group.boundary_influence;
    let mixed = [
        shared_direction[0] * orientation.shared_group_weight * group_influence
            + radial[0] * orientation.radial_weight * group_influence
            + tangent[0] * orientation.tangential_weight * group_influence
            + random_direction[0] * orientation.random_weight
            + flow[0] * orientation.flow_weight,
        shared_direction[1] * orientation.shared_group_weight * group_influence
            + radial[1] * orientation.radial_weight * group_influence
            + tangent[1] * orientation.tangential_weight * group_influence
            + random_direction[1] * orientation.random_weight
            + flow[1] * orientation.flow_weight,
    ];
    let direction = normalize_or(mixed, random_direction);
    let angular_jitter =
        (random01(seed ^ 0x7f4a_7c15) * 2.0 - 1.0) * orientation.angular_jitter_radians;
    let rest_direction = [
        direction[0] * angular_jitter.cos() - direction[1] * angular_jitter.sin(),
        direction[0] * angular_jitter.sin() + direction[1] * angular_jitter.cos(),
    ];
    Some(CandidateSample {
        root_xz: root,
        group,
        rest_direction,
        stable_rank: random01(seed ^ 0x94d0_49bb),
        lod_rank: (lod_lane as f32 + random01(seed ^ 0x91e1_0da5)) * 0.25,
        clump_variant: random01(group.key ^ 0x3c6e_f372),
        seed,
    })
}

fn sample_group(
    population: &VegetationPopulation,
    root: [f32; 2],
    parent: [f32; 2],
    parent_key: u32,
    candidate_seed: u32,
) -> GroupSample {
    match population.grouping {
        VegetationGroupingProfile::None => GroupSample {
            key: candidate_seed,
            center_xz: root,
            radial_direction: [0.0, 0.0],
            normalized_distance: 0.0,
            boundary_influence: 0.0,
            density_retention: 1.0,
        },
        VegetationGroupingProfile::Parent => {
            let delta = [root[0] - parent[0], root[1] - parent[1]];
            let distance = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
            let radius = match population.growth {
                GrowthPattern::ParentChild { radius, .. } => radius,
                GrowthPattern::Uniform { .. } => 0.0,
            };
            GroupSample {
                key: parent_key,
                center_xz: parent,
                radial_direction: normalize_or(delta, [1.0, 0.0]),
                normalized_distance: if radius <= f32::EPSILON {
                    0.0
                } else {
                    (distance / radius).clamp(0.0, 1.0)
                },
                boundary_influence: 1.0,
                density_retention: 1.0,
            }
        }
        VegetationGroupingProfile::Voronoi(profile) => {
            sample_voronoi_group(population.seed, root, profile)
        }
    }
}

fn sample_voronoi_group(seed: u32, root: [f32; 2], profile: VoronoiClumpProfile) -> GroupSample {
    let base_x = (root[0] / profile.spacing).floor() as i32;
    let base_z = (root[1] / profile.spacing).floor() as i32;
    let mut nearest_distance_squared = f32::INFINITY;
    let mut second_distance_squared = f32::INFINITY;
    let mut nearest_center = root;
    let mut nearest_key = seed;

    for dz in -1..=1 {
        for dx in -1..=1 {
            let cell_x = base_x + dx;
            let cell_z = base_z + dz;
            let key = hash_cell(seed, cell_x, cell_z, 0x4f1b_cdc9);
            let offset = [
                0.5 + (random01(key ^ 0x9e37_79b9) - 0.5) * profile.feature_jitter,
                0.5 + (random01(key ^ 0x85eb_ca6b) - 0.5) * profile.feature_jitter,
            ];
            let center = [
                (cell_x as f32 + offset[0]) * profile.spacing,
                (cell_z as f32 + offset[1]) * profile.spacing,
            ];
            let delta = [root[0] - center[0], root[1] - center[1]];
            let distance_squared = delta[0] * delta[0] + delta[1] * delta[1];
            if distance_squared < nearest_distance_squared {
                second_distance_squared = nearest_distance_squared;
                nearest_distance_squared = distance_squared;
                nearest_center = center;
                nearest_key = key;
            } else if distance_squared < second_distance_squared {
                second_distance_squared = distance_squared;
            }
        }
    }

    let distance = nearest_distance_squared.sqrt();
    let second_distance = second_distance_squared.sqrt();
    let softness_width = profile.spacing * profile.boundary_softness;
    let boundary_influence = if softness_width <= f32::EPSILON {
        1.0
    } else {
        smoothstep01((second_distance - distance) / softness_width)
    };
    let normalized_distance =
        (distance / (profile.spacing * std::f32::consts::SQRT_2)).clamp(0.0, 1.0);
    let distance_profile = normalized_distance.powf(profile.retention_falloff);
    let spatial_retention = profile.center_retention
        + (profile.edge_retention - profile.center_retention) * distance_profile;
    let group_retention = 1.0 - profile.density_variation * random01(nearest_key ^ 0xd1b5_4a35);
    let delta = [root[0] - nearest_center[0], root[1] - nearest_center[1]];

    GroupSample {
        key: nearest_key,
        center_xz: nearest_center,
        radial_direction: normalize_or(delta, [1.0, 0.0]),
        normalized_distance,
        boundary_influence,
        density_retention: (spatial_retention * group_retention).clamp(0.0, 1.0),
    }
}

pub fn candidate_density_retention(population: &VegetationPopulation) -> f32 {
    match population.growth {
        GrowthPattern::Uniform { .. } => 1.0,
        GrowthPattern::ParentChild {
            parent_spacing,
            children_per_parent,
            ..
        } => {
            let maximum_density = f32::from(children_per_parent) / parent_spacing.powi(2);
            (population.density_per_square_meter / maximum_density).clamp(0.0, 1.0)
        }
    }
}

pub fn choose_species(population: &VegetationPopulation, random: f32) -> VegetationSpeciesId {
    let total = population
        .species
        .iter()
        .map(|choice| choice.weight)
        .sum::<f32>();
    let mut target = random * total;
    for choice in &population.species {
        if target <= choice.weight {
            return choice.species;
        }
        target -= choice.weight;
    }
    population
        .species
        .last()
        .expect("validated population")
        .species
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

fn normalize3_or(value: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let length_squared = value[0] * value[0] + value[1] * value[1] + value[2] * value[2];
    if length_squared <= 1e-10 {
        return fallback;
    }
    let inverse_length = length_squared.sqrt().recip();
    [
        value[0] * inverse_length,
        value[1] * inverse_length,
        value[2] * inverse_length,
    ]
}

fn valid_color_component(value: f32) -> bool {
    value.is_finite() && (0.0..=16.0).contains(&value)
}

fn finite_range(value: f32, minimum: f32, maximum: f32) -> bool {
    value.is_finite() && (minimum..=maximum).contains(&value)
}

fn smoothstep01(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_fixture_is_valid() {
        fixtures::reference_scene().validate().unwrap();
    }

    #[test]
    fn candidate_domains_are_world_aligned_across_page_boundaries() {
        let scene = fixtures::reference_scene();
        let population = scene
            .catalog
            .population(fixtures::DRY_TUFT_POPULATION_ID)
            .unwrap();
        let left = candidate_domain(&scene.pages[0], population);
        let right = candidate_domain(&scene.pages[1], population);
        assert_eq!(left.spacing, right.spacing);
        assert!(left.cell_min[0] < right.cell_min[0]);
    }

    #[test]
    fn parent_child_sample_is_stable() {
        let scene = fixtures::reference_scene();
        let population = scene
            .catalog
            .population(fixtures::DRY_TUFT_POPULATION_ID)
            .unwrap();
        let domain = candidate_domain(&scene.pages[0], population);
        let first = sample_candidate(population, domain, 17, [0.0, 1.0]).unwrap();
        let second = sample_candidate(population, domain, 17, [0.0, 1.0]).unwrap();
        assert_eq!(first, second);
        assert!(
            (first.rest_direction[0].powi(2) + first.rest_direction[1].powi(2) - 1.0).abs() < 1e-5
        );
    }

    #[test]
    fn siblings_share_the_explicit_parent_group() {
        let scene = fixtures::reference_scene();
        let population = scene
            .catalog
            .population(fixtures::DRY_TUFT_POPULATION_ID)
            .unwrap();
        let domain = candidate_domain(&scene.pages[0], population);
        let first = sample_candidate(population, domain, 0, [0.0, 1.0]).unwrap();
        let sibling = sample_candidate(population, domain, 1, [0.0, 1.0]).unwrap();
        assert_eq!(first.group.key, sibling.group.key);
        assert_eq!(first.group.center_xz, sibling.group.center_xz);
        assert_eq!(first.clump_variant, sibling.clump_variant);
        assert_eq!(first.group.boundary_influence, 1.0);
    }

    #[test]
    fn voronoi_group_sampling_is_stable_and_bounded() {
        let scene = fixtures::reference_scene();
        let population = scene
            .catalog
            .population(fixtures::SHORT_FILL_POPULATION_ID)
            .unwrap();
        let VegetationGroupingProfile::Voronoi(profile) = population.grouping else {
            panic!("short fill fixture must exercise analytic grouping");
        };
        let first = sample_voronoi_group(population.seed, [7.25, -3.75], profile);
        let second = sample_voronoi_group(population.seed, [7.25, -3.75], profile);
        assert_eq!(first, second);
        assert!((0.0..=1.0).contains(&first.normalized_distance));
        assert!((0.0..=1.0).contains(&first.boundary_influence));
        assert!((0.0..=1.0).contains(&first.density_retention));
    }

    #[test]
    fn grouping_source_must_match_the_root_placement_contract() {
        let mut scene = fixtures::reference_scene();
        let population = scene
            .catalog
            .populations
            .iter_mut()
            .find(|population| population.id == fixtures::SHORT_FILL_POPULATION_ID)
            .unwrap();
        population.grouping = VegetationGroupingProfile::Parent;
        assert!(matches!(
            scene.catalog.validate(),
            Err(CatalogValidationError::InvalidPopulation(_))
        ));
    }

    #[test]
    fn overlapping_pages_are_rejected_before_they_duplicate_roots() {
        let mut scene = fixtures::reference_scene();
        scene.pages[1].origin_xz = [15.99, 0.0];
        assert!(matches!(
            scene.validate(),
            Err(SceneValidationError::OverlappingPages {
                first: 0,
                second: 1
            })
        ));
    }

    #[test]
    fn surface_sampling_interpolates_height_and_decodes_normals() {
        let normal = normalize3_or([0.25, 1.0, -0.4], [0.0, 1.0, 0.0]);
        let surface = VegetationSurfaceField {
            resolution: 2,
            heights: vec![0.0, 2.0, 4.0, 6.0],
            normals_oct: vec![encode_octahedral_normal(normal); 4],
            validity: vec![u8::MAX; 4],
        };
        let sample = surface.sample([0.0, 0.0], 8.0, [4.0, 4.0]);
        assert!((sample.height - 3.0).abs() < 1e-6);
        assert!((sample.normal[0] - normal[0]).abs() < 1e-4);
        assert!((sample.normal[1] - normal[1]).abs() < 1e-4);
        assert!((sample.normal[2] - normal[2]).abs() < 1e-4);
        assert_eq!(sample.validity, 1.0);
    }

    #[test]
    fn catalog_rejects_topology_counts_the_renderer_cannot_honor() {
        let mut scene = fixtures::reference_scene();
        let ribbon = scene
            .catalog
            .species
            .iter_mut()
            .find(|species| matches!(species.topology, TopologyProfile::Ribbon(_)))
            .unwrap();
        let TopologyProfile::Ribbon(profile) = &mut ribbon.topology else {
            unreachable!();
        };
        profile.blades_per_render_unit = 3;
        assert!(matches!(
            scene.catalog.validate(),
            Err(CatalogValidationError::InvalidSpecies(_))
        ));

        let mut scene = fixtures::reference_scene();
        let broad = scene
            .catalog
            .species
            .iter_mut()
            .find(|species| matches!(species.topology, TopologyProfile::BroadLeafCluster(_)))
            .unwrap();
        let TopologyProfile::BroadLeafCluster(profile) = &mut broad.topology else {
            unreachable!();
        };
        profile.maximum_leaf_count = 3;
        assert!(matches!(
            scene.catalog.validate(),
            Err(CatalogValidationError::InvalidSpecies(_))
        ));
    }

    #[test]
    fn catalog_rejects_invalid_ribbon_curve_handles() {
        let mut scene = fixtures::reference_scene();
        let ribbon = scene
            .catalog
            .species
            .iter_mut()
            .find(|species| matches!(species.topology, TopologyProfile::Ribbon(_)))
            .unwrap();
        let TopologyProfile::Ribbon(profile) = &mut ribbon.topology else {
            unreachable!();
        };
        profile.curve_variant_b.tip_handle_length = 1.51;
        assert!(matches!(
            scene.catalog.validate(),
            Err(CatalogValidationError::InvalidSpecies(_))
        ));
    }

    #[test]
    fn catalog_rejects_reversed_ribbon_silhouette_tips() {
        let mut scene = fixtures::reference_scene();
        let ribbon = scene
            .catalog
            .species
            .iter_mut()
            .find(|species| matches!(species.topology, TopologyProfile::Ribbon(_)))
            .unwrap();
        let TopologyProfile::Ribbon(profile) = &mut ribbon.topology else {
            unreachable!();
        };
        profile.curve_variant_a.tip_tilt_radians = profile.curve_variant_b.tip_tilt_radians + 0.01;
        assert!(matches!(
            scene.catalog.validate(),
            Err(CatalogValidationError::InvalidSpecies(_))
        ));
    }

    #[test]
    fn catalog_rejects_unbounded_view_opening() {
        let mut scene = fixtures::reference_scene();
        let ribbon = scene
            .catalog
            .species
            .iter_mut()
            .find(|species| matches!(species.topology, TopologyProfile::Ribbon(_)))
            .unwrap();
        let TopologyProfile::Ribbon(profile) = &mut ribbon.topology else {
            unreachable!();
        };
        profile.maximum_view_opening_radians = std::f32::consts::FRAC_PI_4 + 0.01;
        assert!(matches!(
            scene.catalog.validate(),
            Err(CatalogValidationError::InvalidSpecies(_))
        ));
    }

    #[test]
    fn catalog_rejects_density_that_increases_with_distance() {
        let mut scene = fixtures::reference_scene();
        let species = &mut scene.catalog.species[0];
        species.representations[0].density_fraction = 0.25;
        species.representations[1].density_fraction = 0.5;
        assert!(matches!(
            scene.catalog.validate(),
            Err(CatalogValidationError::InvalidSpecies(_))
        ));
    }

    #[test]
    fn nested_lod_rank_retains_one_candidate_per_four_way_group() {
        let scene = fixtures::reference_scene();
        let uniform = scene
            .catalog
            .population(fixtures::SHORT_FILL_POPULATION_ID)
            .unwrap();
        let uniform_domain = CandidateDomain {
            cell_min: [-2, -2],
            cell_count: [4, 4],
            candidates_per_cell: 1,
            spacing: 1.0,
        };
        let retained_uniform = (0..uniform_domain.candidate_count())
            .filter(|&index| {
                sample_candidate(uniform, uniform_domain, index, [1.0, 0.0])
                    .unwrap()
                    .lod_rank
                    < 0.25
            })
            .count();
        assert_eq!(retained_uniform, 4);

        let parent_child = scene
            .catalog
            .population(fixtures::DRY_TUFT_POPULATION_ID)
            .unwrap();
        let parent_domain = CandidateDomain {
            cell_min: [-1, -1],
            cell_count: [2, 2],
            candidates_per_cell: 8,
            spacing: 1.0,
        };
        let retained_children = (0..parent_domain.candidate_count())
            .filter(|&index| {
                sample_candidate(parent_child, parent_domain, index, [1.0, 0.0])
                    .unwrap()
                    .lod_rank
                    < 0.25
            })
            .count();
        assert_eq!(retained_children, 8);
    }
}
