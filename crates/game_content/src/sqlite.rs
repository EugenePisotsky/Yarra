//! The SQLite files content publishes, a bundle and a language pack: each is written to a
//! staging file and linked into place whole, and read in one read-only transaction.
use crate::{ContentError, Result};
use game_types::ContentId;
use rusqlite::{Connection, OpenFlags, Transaction};

use std::{
    fs::{self, OpenOptions},
    path::Path,
};

/// Creates `schema` in a new file, lets `write` fill it, and links it in at `output`. An
/// existing file is never replaced, and a failure leaves nothing behind.
pub(crate) fn publish(
    output: &Path,
    application_id: i64,
    version: i64,
    schema: &str,
    write: impl FnOnce(&Transaction) -> Result<()>,
) -> Result<()> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let stage = parent.join(format!(".yarra-{}.building", ContentId::random().raw()));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)?;

    let result = (|| -> Result<()> {
        let mut connection =
            Connection::open_with_flags(&stage, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.execute_batch("PRAGMA synchronous=FULL;")?;
        let tx = connection.transaction()?;
        tx.execute_batch(schema)?;
        write(&tx)?;
        tx.pragma_update(None, "application_id", application_id)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
        connection.close().map_err(|(_, error)| error)?;

        // A hard link of the closed file: it appears whole, and only where nothing is.
        fs::hard_link(&stage, output)?;
        Ok(())
    })();

    let cleanup = fs::remove_file(&stage);
    result?;
    cleanup?;
    Ok(())
}

/// Opens a published file for reading in one transaction, so every read sees the same file.
/// Returns its version for the caller to judge.
pub(crate) fn open(path: &Path, application_id: i64) -> Result<(Connection, i64)> {
    if !fs::metadata(path)?.is_file() {
        return Err(ContentError::WrongDatabase);
    }

    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; BEGIN;")?;
    let application: i64 = connection.pragma_query_value(None, "application_id", |r| r.get(0))?;

    if application != application_id {
        return Err(ContentError::WrongDatabase);
    }

    let version = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    Ok((connection, version))
}
