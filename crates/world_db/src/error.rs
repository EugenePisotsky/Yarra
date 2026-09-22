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
    #[error(transparent)]
    EnvironmentCompile(#[from] environment_compile::CompileError),
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("page payload is invalid: {0}")]
    Payload(#[from] world::PagePayloadDecodeError),
    #[error("vegetation catalog is invalid: {0}")]
    VegetationCatalog(#[from] vegetation::CatalogValidationError),
    #[error("vegetation data encoding failed: {0}")]
    VegetationEncode(#[from] bincode::error::EncodeError),
    #[error("vegetation data decoding failed: {0}")]
    VegetationDecode(#[from] bincode::error::DecodeError),
    #[error("vegetation catalog contains trailing bytes: decoded {consumed} of {total}")]
    VegetationCatalogTrailingBytes { consumed: usize, total: usize },
    #[error("vegetation catalog format version is {actual}, expected {expected}")]
    VegetationCatalogFormatVersion { expected: i64, actual: i64 },
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
    #[error("dense source transaction must contain 1 to 64 unique environment cell writes")]
    InvalidDenseSourceTransaction,
    #[error("environment cell has invalid coverage or revision")]
    InvalidDenseSourceRecord,
}
