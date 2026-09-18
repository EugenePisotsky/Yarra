//! Deterministic environment compilation shared by future editor preview and world cooking.
//! Spatial jobs are bounded; no Bevy, SQLite, filesystem, or GPU work happens here.

mod coverage;
mod plan;
mod raster;
mod roads;
mod scatter;
pub use roads::{RoadCompileProfile, RoadDetailLimits, TerrainSource, TerrainSourceCell};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use vegetation::{VegetationFieldPageData, VegetationPopulationId};
use world::{CellCoord, TerrainSurfaceId, TerrainWeightPage, WorldSpaceId};

pub use plan::{BindingKey, CompilePlan, PopulationBinding, merge_runtime_catalogs};

/// Explicit compiler budgets, not measured hardware performance or runtime draw limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompileProfile {
    pub terrain_resolution: u16,
    pub vegetation_resolution: u16,
    pub max_surfaces_per_cell: usize,
    pub max_layers: usize,
    pub max_population_bindings: usize,
    pub max_fields_per_cell: usize,
    pub max_cells_per_batch: usize,
    pub max_mask_samples: usize,
    /// Conservative allowance for result buffers and raster scratch combined.
    pub max_output_bytes: usize,
    /// Conservative scalar blend-work estimate, checked before rasterization.
    pub max_sample_work: usize,
}

impl Default for CompileProfile {
    fn default() -> Self {
        Self {
            terrain_resolution: 65,
            vegetation_resolution: 64,
            max_surfaces_per_cell: 2,
            max_layers: 128,
            max_population_bindings: 256,
            max_fields_per_cell: 64,
            max_cells_per_batch: 64,
            max_mask_samples: 4 * 1024 * 1024,
            max_output_bytes: 64 * 1024 * 1024,
            max_sample_work: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledGround {
    /// Sorted stable IDs; these are the palette referenced by RGBA weight pages.
    pub surfaces: Vec<TerrainSurfaceId>,
    /// Empty for a single surface, matching the existing runtime terrain contract.
    pub weight_pages: Vec<TerrainWeightPage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledCell {
    pub terrain: Option<world::TerrainHeightfield>,
    pub objects: Vec<world::StaticObjectInstance>,
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub ground: CompiledGround,
    pub vegetation: VegetationFieldPageData,
    /// Includes the plan and this cell's coverage dependency halo, not unrelated loaded cells.
    pub input_fingerprint: [u8; 32],
}

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("invalid road geometry: {0}")]
    Road(String),
    #[error("road query does not certify complete coverage for cell {0:?}")]
    RoadWindow(CellCoord),
    #[error(
        "road spans {first:?} and {second:?} intersect; explicit junction handling is required"
    )]
    RoadJunctionRequired {
        first: environment::roads::RoadSpanId,
        second: environment::roads::RoadSpanId,
    },
    #[error(transparent)]
    Source(#[from] environment::ValidationError),
    #[error("invalid compiler profile")]
    InvalidProfile,
    #[error("compiler budget exceeded: {0}")]
    Budget(&'static str),
    #[error("coverage snapshot belongs to another world space")]
    WrongSpace,
    #[error("duplicate world in generation catalog: {0:?}")]
    DuplicateWorld(WorldSpaceId),
    #[error("world plans disagree about shared plant species {0:?}")]
    ConflictingSpecies(vegetation::VegetationSpeciesId),
    #[error("duplicate requested or loaded cell {0:?}")]
    DuplicateCell(CellCoord),
    #[error("coverage cell {0:?} is not loaded")]
    UnloadedCell(CellCoord),
    #[error("cell {0:?} has an invalid, duplicate or unknown coverage tile")]
    InvalidTile(CellCoord),
    #[error("coverage border mismatch between {cell:?} and {neighbour:?}, layer {layer:?}")]
    BorderMismatch {
        cell: CellCoord,
        neighbour: CellCoord,
        layer: environment::LayerId,
    },
    #[error("cell coordinate has no representable dependency halo")]
    CoordinateRange,
    #[error("cell {cell:?} needs {required} surfaces; target supports {maximum}")]
    SurfaceLimit {
        cell: CellCoord,
        required: usize,
        maximum: usize,
    },
    #[error("derived population identity collision: {0:?}")]
    BindingCollision(VegetationPopulationId),
    #[error("invalid derived vegetation catalog: {0}")]
    Catalog(#[from] vegetation::CatalogValidationError),
    #[error("invalid derived vegetation page: {0}")]
    Page(#[from] vegetation::PageValidationError),
    #[error("cannot fingerprint compiler input: {0}")]
    Encoding(#[from] bincode::error::EncodeError),
}
