//! SQLite version/path checks and checked binary column conversion.
use crate::WorldDbError;
use rusqlite::{Connection, OpenFlags};
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

/// Opens an existing project database, read-only or for writing, with foreign keys on and
/// waiting up to a second for another writer; its schema must be this build's.
pub(crate) fn open_project(path: &Path, write: bool) -> Result<Connection, WorldDbError> {
    let flags = if write {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let connection = Connection::open_with_flags(path, flags)?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 1000;")?;

    if !write {
        connection.execute_batch("PRAGMA query_only = ON;")?;
    }

    ensure_schema_version(&connection, world::PROJECT_SCHEMA_VERSION, "project")?;
    Ok(connection)
}

/// Opens an unpublished runtime database for a cook to write: foreign keys on, a larger page
/// cache and temporary data on disk; its schema must be this build's.
pub(crate) fn open_staging_runtime(path: &Path) -> Result<Connection, WorldDbError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON; PRAGMA cache_size = -8192; PRAGMA temp_store = FILE;",
    )?;
    ensure_schema_version(&connection, world::RUNTIME_SCHEMA_VERSION, "runtime")?;
    Ok(connection)
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

/// The start view column's size limit.
const MAX_START_VIEW_BYTES: usize = 65536;

pub(crate) fn encode_start_view(
    view: Option<&world::WorldViewBookmark>,
) -> Result<Option<Vec<u8>>, WorldDbError> {
    view.map(|view| {
        view.validate()
            .map_err(|e| WorldDbError::Cook(format!("start view: {e}")))?;
        Ok(world::codec::encode(view)?)
    })
    .transpose()
}

pub(crate) fn decode_start_view(
    bytes: Option<Vec<u8>>,
) -> Result<Option<world::WorldViewBookmark>, WorldDbError> {
    bytes
        .map(|bytes| {
            let view: world::WorldViewBookmark =
                world::codec::decode::<_, MAX_START_VIEW_BYTES>(&bytes)?;
            view.validate()
                .map_err(|e| WorldDbError::Cook(format!("start view: {e}")))?;
            Ok(view)
        })
        .transpose()
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
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect())
}
