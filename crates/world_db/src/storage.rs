//! SQLite version/path checks and checked binary column conversion.
use crate::WorldDbError;
use rusqlite::Connection;
use std::fs;
use std::path::Path;

pub(crate) fn ensure_new_database_path(path: &Path) -> Result<(), WorldDbError> {
    if path.exists() {
        return Err(WorldDbError::AlreadyExists(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub(crate) fn ensure_schema_version(
    connection: &Connection,
    expected: i64,
    database: &'static str,
) -> Result<(), WorldDbError> {
    let actual: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if actual != expected {
        return Err(WorldDbError::SchemaVersion {
            database,
            expected,
            actual,
        });
    }
    Ok(())
}

pub(crate) fn immutable_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let escaped = raw
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('?', "%3F")
        .replace('#', "%23");
    format!("file:{escaped}?immutable=1")
}

pub(crate) fn blob_array<const N: usize>(
    bytes: &[u8],
    field: &'static str,
) -> rusqlite::Result<[u8; N]> {
    bytes.try_into().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Blob,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{field} must contain exactly {N} bytes"),
            )
            .into(),
        )
    })
}

pub(crate) fn encode_f32_blob(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

pub(crate) fn decode_f32_blob(bytes: &[u8], field: &'static str) -> rusqlite::Result<Vec<f32>> {
    if !bytes.len().is_multiple_of(size_of::<f32>()) {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Blob,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{field} byte length must be divisible by four"),
            )
            .into(),
        ));
    }
    Ok(bytes
        .chunks_exact(size_of::<f32>())
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte chunk")))
        .collect())
}
