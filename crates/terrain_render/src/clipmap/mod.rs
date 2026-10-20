//! Terrain v2: GPU height clipmaps with CDLOD patches and per-pixel materials (why: the terrain
//! entries in docs/DECISIONS.md).
mod gpu;
mod patch;
mod select;

pub(crate) use gpu::install;

pub use gpu::{
    COVERED_GROUND, ClipmapFrame, ClipmapGround, ClipmapSurface, ClipmapTerrainMaterial,
    ClipmapTextures, ClipmapUploads, MAX_LEVELS, NodeBlock, Residency, WEAR_SLOTS, WEAR_WINDOW,
    patch_mesh, patch_tag, patch_transform, wear_slot,
};

pub use patch::{ClipmapPatch, PatchAssets};

pub use select::{
    ClipmapLayout, LodRanges, MORPH_SHARE, NodeSource, NodeWindow, Patch, distance, select,
};
