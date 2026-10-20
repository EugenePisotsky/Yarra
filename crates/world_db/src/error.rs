//! Errors shared by project authoring, cooking and runtime reads.
use std::path::PathBuf;
use world::{CellCoord, PageDomain, PageKey, WorldSpaceId};

#[derive(Debug, thiserror::Error)]
pub enum WorldDbError {
    #[error("terrain hierarchy: {0}")]
    TerrainHierarchy(String),
    #[error("world cook: {0}")]
    Cook(String),
    #[error("environment source: {0}")]
    Environment(String),
    #[error("environment coverage: {0}")]
    Coverage(#[from] environment::CoverageError),
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("stored data does not decode: {0}")]
    Decode(#[from] world::DecodeError),
    #[error("encoding failed: {0}")]
    Encode(#[from] bincode::error::EncodeError),
    #[error("vegetation catalog is invalid: {0}")]
    VegetationCatalog(#[from] vegetation::CatalogValidationError),
    #[error("database already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("{database} schema version is {actual}, expected {expected}")]
    SchemaVersion {
        database: &'static str,
        expected: i64,
        actual: i64,
    },
    #[error("page {0:?} failed its BLAKE3 checksum")]
    ChecksumMismatch(PageKey),
    #[error("decoded page size is {actual} bytes, maximum is {maximum}")]
    PageTooLarge { actual: u64, maximum: u64 },
    #[error("decoded page size is {actual} bytes, expected {expected}")]
    DecodedSizeMismatch { expected: u64, actual: u64 },
    #[error("page domain is {actual:?}, expected {expected:?}")]
    DomainMismatch {
        expected: PageDomain,
        actual: PageDomain,
    },
    #[error("integer does not fit the SQLite representation")]
    IntegerOverflow,
    #[error("default world space {0:?} is not present in the world-space catalog")]
    UnknownDefaultWorldSpace(WorldSpaceId),
    #[error("invalid spatial query bounds: minimum {minimum:?}, maximum {maximum:?}")]
    InvalidSpatialQueryBounds {
        minimum: CellCoord,
        maximum: CellCoord,
    },
    #[error("spatial query record limit must be greater than zero")]
    InvalidQueryLimit,
    #[error("object transform values must be finite and scale must be greater than zero")]
    InvalidObjectTransform,
    #[error("object transaction must contain 1 to 256 unique object writes")]
    InvalidObjectTransaction,
    #[error("environment cell transaction must contain 1 to 64 unique cell writes")]
    InvalidEnvironmentCellTransaction,
    #[error("environment cell has invalid coverage or revision")]
    InvalidEnvironmentCellRecord,
}
