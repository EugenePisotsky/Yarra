//! Terrain v2: GPU height clipmaps with CDLOD patches (`--terrain v2`). See the terrain rendering
//! roadmap in docs/REFACTORING.md.
mod gpu;
mod select;
pub(crate) use gpu::install;
pub use gpu::{
    COVERED_GROUND, ClipmapFrame, ClipmapGround, ClipmapSurface, ClipmapTerrainMaterial,
    ClipmapTextures, ClipmapUploads, MAX_LEVELS, NodeBlock, Residency, patch_mesh,
};
pub use select::{
    ClipmapLayout, LodRanges, MORPH_SHARE, NodeSource, NodeWindow, Patch, distance, select,
};
