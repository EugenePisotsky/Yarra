use super::*;
use environment::{LayerId, Preset, PresetId, PresetKind};
const MAX_LIBRARY_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn read_library(connection: &Connection) -> Result<PresetLibrary, WorldDbError> {
    let revision: i64 = connection.query_row(
        "SELECT revision FROM environment_preset_state WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let mut statement = connection.prepare(
        "SELECT preset_id,revision,payload FROM environment_presets ORDER BY preset_id LIMIT 513",
    )?;
    let mut rows = statement.query([])?;
    let mut presets = Vec::new();
    let mut bytes = 0;
    while let Some(row) = rows.next()? {
        let raw = row.get_ref(2)?.as_blob().map_err(rusqlite::Error::from)?;
        bytes += raw.len();
        if bytes > MAX_LIBRARY_BYTES
            || presets.len() >= environment::MAX_PRESETS
            || raw.len() > MAX_DEFINITION_BYTES
        {
            return Err(invalid("preset library byte/count budget"));
        }
        let (preset, consumed): (Preset, usize) = bincode::serde::decode_from_slice(
            raw,
            bincode::config::standard().with_limit::<1048576>(),
        )?;
        let id: Vec<u8> = row.get(0)?;
        let stored: i64 = row.get(1)?;
        if consumed != raw.len()
            || id != preset.id.0
            || stored <= 0
            || preset.revision != stored as u64
        {
            return Err(invalid("preset row identity/revision mismatch"));
        }
        presets.push(preset);
    }
    if revision <= 0 {
        return Err(invalid("preset library revision"));
    }
    Ok(PresetLibrary {
        revision: revision as u64,
        presets,
    })
}
pub(super) fn validate_library_references(
    connection: &Connection,
    library: &PresetLibrary,
    plants: &VegetationCatalog,
) -> Result<(), WorldDbError> {
    library
        .validate(plants)
        .map_err(|e| invalid(e.to_string()))?;
    for preset in &library.presets {
        if let PresetKind::AssetCollection(c) = &preset.kind {
            for asset in &c.assets {
                crate::collection_assets::read_asset(connection, asset.asset)?;
            }
        }
        if let PresetKind::Ground(g) = &preset.kind {
            for weight in &g.surfaces {
                let found: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM terrain_surfaces WHERE surface_id=?1)",
                    [weight.surface.0.as_slice()],
                    |r| r.get(0),
                )?;
                if !found {
                    return Err(invalid("preset references an unknown terrain surface"));
                }
            }
        }
    }
    Ok(())
}
pub(super) fn store_library(
    connection: &Connection,
    library: &PresetLibrary,
) -> Result<(), WorldDbError> {
    if library.presets.len() > environment::MAX_PRESETS {
        return Err(invalid("preset count budget"));
    }
    let mut encoded = Vec::new();
    let mut bytes = 0;
    for preset in &library.presets {
        let payload = bincode::serde::encode_to_vec(preset, bincode::config::standard())?;
        bytes += payload.len();
        if payload.len() > MAX_DEFINITION_BYTES || bytes > MAX_LIBRARY_BYTES {
            return Err(invalid("preset byte budget"));
        }
        encoded.push((preset, payload));
    }
    connection.execute("DELETE FROM environment_preset_dependencies", [])?;
    connection.execute("DELETE FROM environment_presets", [])?;
    for (preset, payload) in encoded {
        connection.execute(
            "INSERT INTO environment_presets(preset_id,revision,payload) VALUES (?1,?2,?3)",
            params![
                preset.id.0.as_slice(),
                i64::try_from(preset.revision).map_err(|_| WorldDbError::IntegerOverflow)?,
                payload
            ],
        )?;
    }
    for preset in &library.presets {
        if let PresetKind::Composition(children) = &preset.kind {
            for id in children.iter().map(|c| c.preset).collect::<BTreeSet<_>>() {
                connection.execute("INSERT INTO environment_preset_dependencies(parent_id,child_id) VALUES (?1,?2)",params![preset.id.0.as_slice(),id.0.as_slice()])?;
            }
        }
    }
    connection.execute("INSERT INTO environment_preset_state(singleton,revision) VALUES (1,?1) ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision",[i64::try_from(library.revision).map_err(|_|WorldDbError::IntegerOverflow)?])?;
    Ok(())
}
pub(super) fn revisioned_library(
    base: &PresetLibrary,
    replacement: &PresetLibrary,
) -> Result<PresetLibrary, WorldDbError> {
    let mut result = replacement.clone();
    result.revision = next_revision(base.revision)?;
    for preset in &mut result.presets {
        preset.revision = if let Some(old) = base.get(preset.id) {
            preset.revision = old.revision;
            if old == preset {
                old.revision
            } else {
                next_revision(old.revision)?
            }
        } else {
            1
        };
    }
    result.presets.sort_by_key(|p| p.id);
    Ok(result)
}
#[derive(Debug, Clone)]
pub struct EnvironmentPresetLayers {
    pub library_revision: u64,
    pub layers: Vec<(WorldSpaceId, LayerId)>,
    pub next_cursor: Option<(WorldSpaceId, LayerId)>,
}
impl ProjectReader {
    pub fn read_environment_presets(&self) -> Result<PresetLibrary, WorldDbError> {
        let tx = self.connection.unchecked_transaction()?;
        read_library(&tx)
    }
    /// Reverse transitive dependencies, including repeated/nested uses and uses in other worlds.
    pub fn read_environment_preset_layers(
        &self,
        preset: PresetId,
        after: Option<(WorldSpaceId, LayerId)>,
        limit: usize,
    ) -> Result<EnvironmentPresetLayers, WorldDbError> {
        if limit == 0 || limit > MAX_ENVIRONMENT_READ_CELLS {
            return Err(invalid("preset dependency query budget"));
        }
        let tx = self.connection.unchecked_transaction()?;
        let library = read_library(&tx)?;
        if library.get(preset).is_none() {
            return Err(invalid("unknown preset"));
        }
        let mut q=tx.prepare("WITH RECURSIVE users(id) AS (SELECT ?1 UNION SELECT d.parent_id FROM environment_preset_dependencies d JOIN users u ON d.child_id=u.id) SELECT l.world_space_id,l.layer_id FROM environment_layer_presets l JOIN users u ON l.preset_id=u.id WHERE ?2=0 OR (l.world_space_id,l.layer_id)>(?3,?4) ORDER BY l.world_space_id,l.layer_id LIMIT ?5")?;
        let cursor = after.unwrap_or((WorldSpaceId(0), LayerId([0; 16])));
        let mut layers = q
            .query_map(
                params![
                    preset.0.as_slice(),
                    i32::from(after.is_some()),
                    cursor.0.0,
                    cursor.1.0.as_slice(),
                    limit as i64 + 1
                ],
                |r| {
                    let id: Vec<u8> = r.get(1)?;
                    let id: [u8; 16] = id.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?;
                    Ok((WorldSpaceId(r.get(0)?), LayerId(id)))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let more = layers.len() > limit;
        layers.truncate(limit);
        let next_cursor = if more { layers.last().copied() } else { None };
        Ok(EnvironmentPresetLayers {
            library_revision: library.revision,
            layers,
            next_cursor,
        })
    }
}
