use super::*;
use world::atmosphere::AtmosphereProfile;

#[derive(Debug, Clone)]
pub struct AtmosphereWrite {
    pub space: WorldSpaceId,
    pub expected_revision: i64,
    pub profile: AtmosphereProfile,
}

#[derive(Debug, Clone)]
pub enum AtmosphereWriteResult {
    Committed(Vec<(WorldSpaceId, i64, AtmosphereProfile)>),
    Conflict {
        space: WorldSpaceId,
        revision: i64,
        actual: AtmosphereProfile,
    },
}

pub(super) fn encode(profile: &AtmosphereProfile) -> Result<Vec<u8>, WorldDbError> {
    profile
        .validate()
        .map_err(|e| WorldDbError::Cook(e.into()))?;
    Ok(bincode::serde::encode_to_vec(
        profile,
        bincode::config::standard(),
    )?)
}
pub(super) fn decode(bytes: &[u8]) -> Result<AtmosphereProfile, WorldDbError> {
    if bytes.len() > 4096 {
        return Err(WorldDbError::Cook("atmosphere exceeds 4096 bytes".into()));
    }
    let (p, consumed): (AtmosphereProfile, _) =
        bincode::serde::decode_from_slice(bytes, bincode::config::standard().with_limit::<4096>())?;
    if consumed != bytes.len() {
        return Err(WorldDbError::Cook("trailing atmosphere bytes".into()));
    }
    p.validate().map_err(|e| WorldDbError::Cook(e.into()))?;
    Ok(p)
}
impl ProjectWriter {
    /// All worlds in the bounded batch commit together, or no source record changes.
    pub fn write_atmospheres(
        &mut self,
        writes: &[AtmosphereWrite],
    ) -> Result<AtmosphereWriteResult, WorldDbError> {
        if writes.is_empty() || writes.len() > 32 {
            return Err(WorldDbError::Cook(
                "invalid atmosphere transaction size".into(),
            ));
        }
        let mut ids = HashSet::new();
        let payloads = writes
            .iter()
            .map(|w| {
                if !ids.insert(w.space)
                    || w.expected_revision <= 0
                    || w.expected_revision == i64::MAX
                {
                    return Err(WorldDbError::Cook(
                        "invalid atmosphere revision or duplicate world".into(),
                    ));
                }
                encode(&w.profile)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for w in writes {
            let (revision, bytes): (i64, Vec<u8>) = tx.query_row(
                "SELECT atmosphere_revision, atmosphere FROM world_spaces WHERE id=?1",
                [w.space.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if revision != w.expected_revision {
                return Ok(AtmosphereWriteResult::Conflict {
                    space: w.space,
                    revision,
                    actual: decode(&bytes)?,
                });
            }
        }
        for (w, payload) in writes.iter().zip(payloads) {
            tx.execute(
                "UPDATE world_spaces SET atmosphere=?1, atmosphere_revision=?2 WHERE id=?3",
                params![payload, w.expected_revision + 1, w.space.0],
            )?;
        }
        tx.commit()?;
        Ok(AtmosphereWriteResult::Committed(
            writes
                .iter()
                .map(|w| (w.space, w.expected_revision + 1, w.profile.clone()))
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conflict_rolls_back_the_entire_batch_and_round_trips_colors() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(schema::PROJECT_SCHEMA).unwrap();
        let p = AtmosphereProfile::default();
        for id in [1, 2] {
            connection
                .execute(
                    "INSERT INTO world_spaces VALUES (?1,?2,32,0,10,?3,1)",
                    params![id, format!("world{id}"), encode(&p).unwrap()],
                )
                .unwrap();
        }
        let mut writer = ProjectWriter { connection };
        let mut changed = p.clone();
        changed.phases[1].sun_srgb = [0.8, 0.5, 0.2];
        let writes = [
            AtmosphereWrite {
                space: WorldSpaceId(1),
                expected_revision: 1,
                profile: changed.clone(),
            },
            AtmosphereWrite {
                space: WorldSpaceId(2),
                expected_revision: 2,
                profile: changed.clone(),
            },
        ];
        assert!(matches!(
            writer.write_atmospheres(&writes).unwrap(),
            AtmosphereWriteResult::Conflict { .. }
        ));
        assert_eq!(
            query_world_spaces(&writer.connection).unwrap()[0].atmosphere,
            p
        );
        assert!(matches!(
            writer.write_atmospheres(&writes[..1]).unwrap(),
            AtmosphereWriteResult::Committed(_)
        ));
        assert_eq!(
            query_world_spaces(&writer.connection).unwrap()[0].atmosphere,
            changed
        );
        let mut bytes = encode(&p).unwrap();
        bytes.push(0);
        assert!(decode(&bytes).is_err());
    }
}
