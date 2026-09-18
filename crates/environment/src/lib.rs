//! Environment authoring source. No editor, database or renderer dependencies.
//!
//! Compositions describe content; layers apply them through spatial coverage. The
//! compiler resolves this source to ordinary terrain weights and vegetation fields.

pub mod brush;
pub mod fixtures;
mod presets;
pub mod roads;
pub use presets::*;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use vegetation::{VegetationAssemblageId, VegetationCatalog};
use world::{CellCoord, TerrainSurfaceId, WorldSpaceId};

macro_rules! id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub struct $name(pub [u8; 16]);
    };
}
id!(PresetId);
id!(PresetUseId);
id!(LayerId);
id!(OutputId);
id!(ChannelId);

/// Small definition catalog for one world space, independent of its spatial tiles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentDefinition {
    pub space: WorldSpaceId,
    pub revision: u64,
    pub cell_size: f32,
    /// Endpoint-inclusive source grid. 65 samples in a 32 m cell means 0.5 m spacing.
    pub mask_resolution: u16,
    /// IDs available to this world's terrain pack. Texture array slots are not source IDs.
    pub surfaces: Vec<TerrainSurfaceId>,
    pub base_surface: TerrainSurfaceId,
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundTreatment {
    pub id: OutputId,
    pub strength: f32,
    /// Positive relative weights; normalized by the compiler in stable surface-ID order.
    pub surfaces: Vec<SurfaceWeight>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SurfaceWeight {
    pub surface: TerrainSurfaceId,
    pub weight: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VegetationBlend {
    Replace,
    Add,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationTreatment {
    pub id: OutputId,
    pub channel: ChannelId,
    pub assemblage: VegetationAssemblageId,
    /// Stable thinning below the authored maximum, independent of replacement influence.
    pub density: f32,
    pub seed: u32,
    pub blend: VegetationBlend,
    pub strength: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exclusion {
    pub id: OutputId,
    pub channel: ChannelId,
    pub strength: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub revision: u64,
    pub name: String,
    pub preset: PresetId,
    pub overrides: Vec<PresetOverride>,
    pub order: i32,
    pub seed: u32,
    pub enabled: bool,
    /// Multiplies coverage for all outputs, including replacement and exclusion.
    pub opacity: f32,
}

/// An explicitly loaded cell. An absent layer tile here means zero coverage.
/// An absent cell in a snapshot means *unloaded*, never zero coverage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageCell {
    pub cell: CellCoord,
    pub revision: u64,
    pub tiles: Vec<CoverageTile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageTile {
    pub layer: LayerId,
    /// Row-major R8 samples, including both endpoints on each axis.
    pub samples: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageSnapshot {
    pub space: WorldSpaceId,
    pub cells: Vec<CoverageCell>,
}

impl EnvironmentDefinition {
    /// Checks source references and semantics. Spatial data and target budgets are checked by
    /// the compiler, which never assumes deserialization implies validity.
    pub fn validate(
        &self,
        plants: &VegetationCatalog,
        library: &PresetLibrary,
    ) -> Result<(), ValidationError> {
        library.validate(plants)?;
        self.validate_layers(library)
    }
    /// Used after validating the shared library once in a project snapshot.
    pub fn validate_layers(&self, library: &PresetLibrary) -> Result<(), ValidationError> {
        if !self.cell_size.is_finite()
            || !(0.01..=65_536.0).contains(&self.cell_size)
            || !(2..=257).contains(&self.mask_resolution)
            || self.layers.len() > 128
            || self.surfaces.len() > 64
        {
            return Err(ValidationError::Invalid("world grid or definition budget"));
        }
        let surfaces: BTreeSet<_> = self.surfaces.iter().copied().collect();
        if surfaces.len() != self.surfaces.len() || !surfaces.contains(&self.base_surface) {
            return Err(ValidationError::Invalid("surface catalog or base surface"));
        }
        let mut ids = BTreeSet::new();
        for layer in &self.layers {
            if !ids.insert(layer.id)
                || layer.name.trim().is_empty()
                || layer.name.len() > 256
                || !unit(layer.opacity)
            {
                return Err(ValidationError::Invalid("layer"));
            }
            let resolved = library.resolve(layer.preset, &layer.overrides)?;
            if let Some(ground) = resolved.ground {
                for item in ground.surfaces {
                    if !surfaces.contains(&item.surface) {
                        return Err(ValidationError::MissingSurface(item.surface));
                    }
                }
            }
        }
        Ok(())
    }
}

fn unit(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ValidationError {
    #[error("invalid environment {0}")]
    Invalid(&'static str),
    #[error("missing terrain surface {0:?}")]
    MissingSurface(TerrainSurfaceId),
    #[error("missing vegetation assemblage {0:?}")]
    MissingAssemblage(VegetationAssemblageId),
    #[error("invalid preset: {0}")]
    Preset(String),
    #[error("invalid road: {0}")]
    Road(&'static str),
    #[error(transparent)]
    Vegetation(#[from] vegetation::CatalogValidationError),
}
