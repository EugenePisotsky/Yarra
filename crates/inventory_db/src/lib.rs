//! Relational item/category authoring. Live state and checkpoints belong to `save`.
mod catalog;
mod catalog_storage;
mod storage;
pub use catalog::CatalogStore;
pub use catalog_storage::{CATALOG_SCHEMA, read_catalog, write_catalog};
pub use storage::{InventoryDbError, Result, SCHEMA_VERSION};
