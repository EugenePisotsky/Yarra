pub mod atmosphere;
pub mod clouds;
pub mod codec;
pub mod fog;
pub mod terrain_hierarchy;
pub mod weather;

mod far_objects;
mod page;
mod space;
mod terrain;

pub use far_objects::*;
pub use page::*;
pub use space::*;
pub use terrain::*;

pub use codec::DecodeError;
pub use terrain_hierarchy::*;

mod terrain_heightfield_codec;
mod terrain_preview;

pub use terrain_preview::{TerrainPreviewLeaf, TerrainPreviewProducts};

mod view_bookmark;

pub use view_bookmark::WorldViewBookmark;

mod gameplay_area;

pub use gameplay_area::*;

pub mod road_noise;

mod terrain_wear;

pub use terrain_wear::*;

mod terrain_control;

pub use terrain_control::*;

mod soft_layer;

pub use soft_layer::*;

/// Default authoring source relative to the repository root.
pub const DEFAULT_PROJECT_DATABASE: &str = "content/world.project.sqlite";

/// Default cooked world relative to the asset root (also used in iOS bundles).
pub const DEFAULT_RUNTIME_DATABASE: &str = "generated/world.runtime.sqlite";

pub const DEFAULT_CELL_SIZE: f32 = 32.0;
pub const MAX_DECODED_PAGE_BYTES: u64 = 64 * 1024 * 1024;

/// Versions of everything a project or runtime database stores, its blobs included
/// ([`codec`]): any change to a stored type bumps one.
pub const PROJECT_SCHEMA_VERSION: i64 = 35;

pub const RUNTIME_SCHEMA_VERSION: i64 = 36;

#[cfg(test)]
mod tests;
