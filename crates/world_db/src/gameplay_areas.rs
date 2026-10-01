//! The project's named gameplay areas: one revisioned record, read and written whole.
//!
//! Names are unique across the project and the set is small, so there is nothing to query
//! by place. The runtime carries the same payload outside its content hash.
use crate::{ProjectReader, ProjectWriter, WorldDbError};
use rusqlite::{Connection, params};
use std::sync::Arc;
use world::GameplayArea;

pub const MAX_GAMEPLAY_AREA_BYTES: usize = 4 * 1024 * 1024;

/// A runtime's generation: its content hash with the gameplay areas published beside it.
/// Areas stay out of the content hash so repainting one recooks nothing, but a running
/// consumer still has to see that the publication changed.
pub(crate) fn generation_id(
    connection: &Connection,
    content_hash: &[u8],
) -> Result<String, WorldDbError> {
    let areas: Vec<u8> = connection.query_row(
        "SELECT gameplay_areas FROM runtime_metadata WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"runtime-generation-v1");
    hash.update(content_hash);
    hash.update(&areas);
    Ok(hash.finalize().to_hex()[..16].to_owned())
}

#[derive(Debug, Clone, PartialEq)]
pub struct GameplayAreasRecord {
    pub revision: i64,
    pub areas: Arc<[GameplayArea]>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GameplayAreasWriteResult {
    Committed(GameplayAreasRecord),
    /// Someone else saved first; nothing was written.
    Conflict(GameplayAreasRecord),
}

fn invalid(message: impl std::fmt::Display) -> WorldDbError {
    WorldDbError::Cook(format!("gameplay areas: {message}"))
}

pub(crate) fn encode(areas: &[GameplayArea]) -> Result<Vec<u8>, WorldDbError> {
    world::validate_gameplay_areas(areas).map_err(invalid)?;
    let bytes = bincode::serde::encode_to_vec(areas, bincode::config::standard())?;
    if bytes.len() > MAX_GAMEPLAY_AREA_BYTES {
        return Err(invalid("the encoded set is too large"));
    }
    Ok(bytes)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Arc<[GameplayArea]>, WorldDbError> {
    if bytes.len() > MAX_GAMEPLAY_AREA_BYTES {
        return Err(invalid("the encoded set is too large"));
    }
    let (areas, used): (Vec<GameplayArea>, usize) =
        bincode::serde::decode_from_slice(bytes, bincode::config::standard())?;
    if used != bytes.len() {
        return Err(invalid("trailing bytes"));
    }
    world::validate_gameplay_areas(&areas).map_err(invalid)?;
    Ok(areas.into())
}

pub(crate) fn read(connection: &Connection) -> Result<GameplayAreasRecord, WorldDbError> {
    let (revision, payload): (i64, Vec<u8>) = connection.query_row(
        "SELECT revision, payload FROM gameplay_areas WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(GameplayAreasRecord {
        revision,
        areas: decode(&payload)?,
    })
}

/// Replaces the set in an open transaction; every area must lie in a world of the project.
pub(crate) fn store(
    connection: &Connection,
    revision: i64,
    areas: &[GameplayArea],
) -> Result<(), WorldDbError> {
    let payload = encode(areas)?;
    for area in areas {
        let known: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM world_spaces WHERE id=?1)",
            [area.space.0],
            |r| r.get(0),
        )?;
        if !known {
            return Err(invalid(format!("{} is in an unknown world", area.name)));
        }
    }
    connection.execute(
        "UPDATE gameplay_areas SET revision=?1, payload=?2 WHERE singleton=1",
        params![revision, payload],
    )?;
    Ok(())
}

impl ProjectReader {
    pub fn read_gameplay_areas(&self) -> Result<GameplayAreasRecord, WorldDbError> {
        read(&self.connection)
    }
}

impl ProjectWriter {
    /// Replaces the whole set when the stored revision is still the one the caller read.
    pub fn write_gameplay_areas(
        &mut self,
        expected_revision: i64,
        areas: &[GameplayArea],
    ) -> Result<GameplayAreasWriteResult, WorldDbError> {
        if expected_revision <= 0 || expected_revision == i64::MAX {
            return Err(invalid("invalid revision"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let stored = read(&tx)?;
        if stored.revision != expected_revision {
            return Ok(GameplayAreasWriteResult::Conflict(stored));
        }
        store(&tx, expected_revision + 1, areas)?;
        tx.commit()?;
        Ok(GameplayAreasWriteResult::Committed(GameplayAreasRecord {
            revision: expected_revision + 1,
            areas: areas.into(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use world::WorldSpaceId;

    fn area(name: &str, space: i64) -> GameplayArea {
        GameplayArea {
            name: name.into(),
            space: WorldSpaceId(space),
            points: vec![[0., 0.], [8., 0.], [8., 8.]],
            height: Some([-2., 6.]),
        }
    }

    fn writer() -> ProjectWriter {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(schema::PROJECT_SCHEMA).unwrap();
        connection
            .execute(
                "INSERT INTO world_spaces VALUES (1,'main',32.0,-10.0,100.0,?1,1,NULL)",
                [crate::atmosphere::encode(&Default::default()).unwrap()],
            )
            .unwrap();
        ProjectWriter { connection }
    }

    #[test]
    fn a_new_project_has_no_areas_and_a_save_replaces_the_set() {
        let mut writer = writer();
        let empty = read(&writer.connection).unwrap();
        assert_eq!((empty.revision, empty.areas.len()), (1, 0));
        let areas = [area("guard/approach", 1), area("guard/gate_post", 1)];
        let GameplayAreasWriteResult::Committed(saved) =
            writer.write_gameplay_areas(1, &areas).unwrap()
        else {
            panic!("the first save conflicted")
        };
        assert_eq!((saved.revision, &*saved.areas), (2, &areas[..]));
        assert_eq!(read(&writer.connection).unwrap(), saved);
    }

    #[test]
    fn a_stale_or_invalid_save_changes_nothing() {
        let mut writer = writer();
        writer
            .write_gameplay_areas(1, &[area("guard/approach", 1)])
            .unwrap();
        let before = read(&writer.connection).unwrap();
        assert_eq!(
            writer.write_gameplay_areas(1, &[]).unwrap(),
            GameplayAreasWriteResult::Conflict(before.clone())
        );
        for broken in [
            vec![area("guard/approach", 7)],
            vec![area("Guard", 1)],
            vec![area("twice", 1), area("twice", 1)],
        ] {
            assert!(writer.write_gameplay_areas(2, &broken).is_err());
            assert_eq!(read(&writer.connection).unwrap(), before);
        }
        let mut bytes = encode(&before.areas).unwrap();
        bytes.push(0);
        assert!(decode(&bytes).is_err());
    }
}
