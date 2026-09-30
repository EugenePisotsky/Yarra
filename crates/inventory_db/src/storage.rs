use inventory::{InventoryError, ItemCatalog};
use rusqlite::{Connection, OpenFlags};
use std::{fs::OpenOptions, path::Path};
use thiserror::Error;

pub const SCHEMA_VERSION: i64 = 4;
pub(crate) const CATALOG_APPLICATION: i64 = 0x59494341; // YICA
pub(crate) use crate::catalog_storage::{read_catalog, validate_catalog};
pub type Result<T> = std::result::Result<T, InventoryDbError>;

#[derive(Debug, Error)]
pub enum InventoryDbError {
    #[error(transparent)]
    Domain(#[from] InventoryError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("unsupported inventory database schema {0}")]
    SchemaVersion(i64),
    #[error("wrong database kind")]
    WrongDatabase,
    #[error("record was changed by another writer")]
    Conflict,
    #[error("invalid stored data: {0}")]
    InvalidData(String),
}

pub(crate) fn create(path: &Path, catalog: &ItemCatalog) -> Result<Connection> {
    validate_catalog(catalog)?;
    OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = (|| {
        let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure(&connection)?;
        let transaction = connection.transaction()?;
        transaction.execute_batch(crate::catalog_storage::CATALOG_SCHEMA)?;
        crate::catalog_storage::write_catalog(&transaction, catalog)?;
        transaction.pragma_update(None, "application_id", CATALOG_APPLICATION)?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        transaction.commit()?;
        Ok(connection)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}
pub(crate) fn open(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let actual: i64 = connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
    if actual != CATALOG_APPLICATION {
        return Err(InventoryDbError::WrongDatabase);
    }
    let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        return Err(InventoryDbError::SchemaVersion(version));
    }
    configure(&connection)?;
    Ok(connection)
}
fn configure(connection: &Connection) -> Result<()> {
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 1000;")?;
    Ok(())
}
pub(crate) fn id(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<[u8; 16]> {
    row.get_ref(index)?
        .as_blob()?
        .try_into()
        .map_err(|_| rusqlite::Error::InvalidQuery)
}

pub(crate) fn integer(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| InventoryError::Overflow.into())
}
pub(crate) fn unsigned(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

pub(crate) fn json<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}
