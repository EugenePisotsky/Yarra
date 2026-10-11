//! Rendering controls: which grass work runs, for measurements, and optional GPU counters.
use bevy::{prelude::*, render::extract_resource::ExtractResource};

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

/// Shared rendering controls. Applications own the UI: the game uses F1 and the editor uses
/// workspace controls.
#[derive(Resource, ExtractResource, Debug, Clone, Copy)]
#[extract_app(bevy::render::RenderApp)]
pub struct VegetationSettings {
    pub profile_mode: VegetationProfileMode,
    /// Optional GPU statistics atomics, independent of placement's required draw counters.
    pub gpu_counters_enabled: bool,
    /// Cache stable candidate acceptance across camera movement, with a bounded reference fallback.
    pub candidate_cache_enabled: bool,
}

impl Default for VegetationSettings {
    fn default() -> Self {
        Self {
            profile_mode: default(),
            gpu_counters_enabled: false,
            candidate_cache_enabled: true,
        }
    }
}
