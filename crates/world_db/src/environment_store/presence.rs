//! Metadata-only coverage discovery. No mask blobs or world-sized layer lists are decoded.
use super::*;
use environment::LayerId;

pub const MAX_ENVIRONMENT_PRESENCE_ROWS: usize = 4096;
#[derive(Debug, Clone)]
pub struct EnvironmentLayerPresence {
    pub definition_revision: u64,
    pub entries: Vec<(CellCoord, LayerId)>,
    pub truncated: bool,
}
impl ProjectReader {
    pub fn read_environment_layer_presence(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
        limit: usize,
    ) -> Result<EnvironmentLayerPresence, WorldDbError> {
        if limit == 0
            || limit > MAX_ENVIRONMENT_PRESENCE_ROWS
            || minimum.x > maximum.x
            || minimum.z > maximum.z
        {
            return Err(invalid("layer presence query budget"));
        }
        let width = i64::from(maximum.x) - i64::from(minimum.x) + 1;
        let height = i64::from(maximum.z) - i64::from(minimum.z) + 1;
        if width
            .checked_mul(height)
            .is_none_or(|n| n > MAX_ENVIRONMENT_READ_CELLS as i64)
        {
            return Err(invalid("layer presence area budget"));
        }
        let tx = self.connection.unchecked_transaction()?;
        let revision: i64 = tx.query_row(
            "SELECT revision FROM environment_definitions WHERE world_space_id=?1",
            [space.0],
            |r| r.get(0),
        )?;
        let mut q=tx.prepare("SELECT cell_x,cell_z,layer_id FROM environment_coverage WHERE world_space_id=?1 AND cell_x BETWEEN ?2 AND ?3 AND cell_z BETWEEN ?4 AND ?5 ORDER BY cell_x,cell_z,layer_id LIMIT ?6")?;
        let mut entries = q
            .query_map(
                params![
                    space.0,
                    minimum.x,
                    maximum.x,
                    minimum.z,
                    maximum.z,
                    limit as i64 + 1
                ],
                |r| {
                    let raw: Vec<u8> = r.get(2)?;
                    let id = raw.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?;
                    Ok((
                        CellCoord {
                            x: r.get(0)?,
                            z: r.get(1)?,
                        },
                        LayerId(id),
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = entries.len() > limit;
        entries.truncate(limit);
        Ok(EnvironmentLayerPresence {
            definition_revision: revision as u64,
            entries,
            truncated,
        })
    }
}
