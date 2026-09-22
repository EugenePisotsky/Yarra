//! Public transaction and IO budgets shared by the source and runtime APIs.
pub const MAX_OBJECT_WRITES_PER_TRANSACTION: usize = 256;
pub const MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION: usize = 64;
pub const MAX_CELL_DESCRIPTOR_QUERY: usize = 4096;
pub const COOK_KEY_BATCH: usize = 128;
pub const MAX_COOK_MANUAL_OBJECTS_PER_CELL: usize = 8192;
pub const MAX_COOK_CATALOG_ROWS: usize = 32768;
pub const MAX_COOK_CATALOG_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_COOK_CELL_BYTES: u64 = 32 * 1024 * 1024;
