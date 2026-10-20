//! Original terrain heights around the cells being compiled, as authored; the compiler grades
//! roads into them.
use serde::{Deserialize, Serialize};
use world::{CellCoord, WorldSpaceId};

/// A certified, bounded source-height halo. Absent cells in `loaded_cells` are world edges.
/// Heights must be original authoring data, never previously graded/cooked pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainSource {
    pub space: WorldSpaceId,
    pub cell_size: f32,
    pub minimum_height: f32,
    pub maximum_height: f32,
    pub loaded_cells: Vec<CellCoord>,
    pub cells: Vec<TerrainSourceCell>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainSourceCell {
    /// Original constant-height cells can retain a four-vertex mesh when relief is absent.
    pub flat: bool,
    pub cell: CellCoord,
    pub revision: u64,
    pub resolution: u16,
    pub heights: Vec<f32>,
}
