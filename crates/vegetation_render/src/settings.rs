//! Rendering controls: production quality, preparation capacity and explicit diagnostic
//! overrides.
use bevy::{prelude::*, render::extract_resource::ExtractResource};

/// Startup-only capacity for the prepared-blade arena. Applications own configuration.
#[derive(Resource, Clone, Copy)]
pub struct VegetationPreparationCapacity(pub u64);
impl Default for VegetationPreparationCapacity {
    fn default() -> Self {
        Self(131_072)
    }
}

/// Prepare shared curve and wind values once per blade, with a bounded GPU cache.
#[derive(Resource, ExtractResource, Clone, Copy, Debug)]
#[extract_app(bevy::render::RenderApp)]
pub struct VegetationBladePreparation {
    pub enabled: bool,
}
impl Default for VegetationBladePreparation {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Production geometry, or what a GPU placement diagnostic visualizes instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum VegetationDiagnosticMode {
    /// Procedural curved blades emitted into single- and split-blade indirect bins.
    #[default]
    ProceduralGeometry = 0,
    /// Accepted roots, colored by their selected species.
    AcceptedSpecies = 1,
    /// Accepted parent/child relationships, drawn from the shared parent to each child root.
    ParentLinks = 2,
    /// All page-owned candidates, colored by their acceptance or rejection reason.
    CandidateOutcomes = 3,
    /// Accepted roots linked to their common parent or analytic Voronoi centre.
    GroupStructure = 4,
}

impl VegetationDiagnosticMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::ProceduralGeometry => "procedural geometry",
            Self::AcceptedSpecies => "accepted species",
            Self::ParentLinks => "parent links",
            Self::CandidateOutcomes => "candidate outcomes",
            Self::GroupStructure => "group structure",
        }
    }
}

/// Selects an isolated workload for target-hardware measurements.
///
/// `DrawFrozen` intentionally retains the instances and indirect arguments produced by the last
/// full/compute frame. It is meaningful only with a fixed camera and stable residency.
/// `Full` automatically reuses placement when its inputs are unchanged; `ComputeOnly` and
/// `ScheduleOnly` always execute so their isolated workload remains measurable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum VegetationProfileMode {
    #[default]
    Full = 0,
    DrawFrozen = 1,
    ComputeOnly = 2,
    ScheduleOnly = 3,
    /// Skip all vegetation GPU work, including scheduling and draws.
    Disabled = 4,
}

impl VegetationProfileMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::DrawFrozen => "draw frozen (fixed camera)",
            Self::ComputeOnly => "compute only",
            Self::ScheduleOnly => "schedule only",
            Self::Disabled => "disabled",
        }
    }
}

/// Selects the population-density policy independently from procedural geometry LOD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum VegetationDensityMode {
    /// Use the density fractions and projected thresholds authored on each representation.
    Authored = 0,
    /// Production policy: preserve density while roots are clearly visible, then taper near subpixel spacing.
    #[default]
    Balanced = 1,
    /// Keep every otherwise eligible candidate as a diagnostic visual and cost ceiling.
    FullReference = 2,
}

impl VegetationDensityMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Authored => "authored thinning",
            Self::Balanced => "balanced production",
            Self::FullReference => "100% reference",
        }
    }
}

/// Selects the foliage-lighting response without changing authored material data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum VegetationLightingMode {
    /// Exposure-aware rounded blade highlights blended into a stable clump response.
    #[default]
    RoundedGloss = 0,
    /// Minimal fragment path used to separate geometry/coverage cost from foliage lighting.
    UnlitDiagnostic = 2,
    /// Keeps all vertex invocations but skips procedural instance reads and deformation.
    VertexOnlyDiagnostic = 3,
}

impl VegetationLightingMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::RoundedGloss => "rounded + clump gloss",
            Self::UnlitDiagnostic => "unlit diagnostic",
            Self::VertexOnlyDiagnostic => "minimal vertex diagnostic",
        }
    }
}

/// Shared rendering controls, including production quality and explicit diagnostic overrides.
/// Applications own the UI: the game uses F1 and the editor uses workspace controls.
#[derive(Resource, ExtractResource, Debug, Clone, Copy)]
#[extract_app(bevy::render::RenderApp)]
pub struct VegetationSettings {
    pub diagnostic_mode: VegetationDiagnosticMode,
    pub profile_mode: VegetationProfileMode,
    pub density_mode: VegetationDensityMode,
    pub lighting_mode: VegetationLightingMode,
    pub far_width_compensation: bool,
    /// Optional GPU statistics atomics, independent of placement's required draw counters.
    pub gpu_counters_enabled: bool,
    /// Skip production-only LOD work once a candidate is known to be rejected.
    pub early_rejection: bool,
    /// Cache stable candidate acceptance across camera movement, with a bounded reference fallback.
    pub candidate_cache_enabled: bool,
}

impl Default for VegetationSettings {
    fn default() -> Self {
        Self {
            diagnostic_mode: default(),
            profile_mode: default(),
            density_mode: default(),
            lighting_mode: default(),
            far_width_compensation: true,
            gpu_counters_enabled: false,
            early_rejection: true,
            candidate_cache_enabled: true,
        }
    }
}
