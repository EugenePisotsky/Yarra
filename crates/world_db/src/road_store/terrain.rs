use super::*;
use environment_compile::{TerrainSource, TerrainSourceCell};

pub struct RoadTerrainSnapshot {
    pub source: RoadEnvironmentSnapshot,
    pub terrain: TerrainSource,
}
impl ProjectReader {
    /// Small aggregate for style-control limits; no distant height blobs are decoded.
    pub fn read_terrain_height_step(&self, space: WorldSpaceId) -> Result<f32, WorldDbError> {
        Ok(self.connection.query_row("SELECT w.cell_size / (MIN(COALESCE(h.resolution,33))-1) FROM world_spaces w LEFT JOIN source_cells s ON s.world_space_id=w.id LEFT JOIN terrain_cell_heightfields h ON h.world_space_id=s.world_space_id AND h.cell_x=s.cell_x AND h.cell_z=s.cell_z WHERE w.id=?1 GROUP BY w.id", [space.0], |r|r.get(0))?)
    }
    /// Heights, masks and road metadata from one read transaction; no runtime pages are inputs.
    pub fn read_road_terrain_source(
        &self,
        space: WorldSpaceId,
        cells: &[CellCoord],
        bounds: RoadCellBounds,
    ) -> Result<RoadTerrainSnapshot, WorldDbError> {
        if cells.len() > 9 {
            return Err(invalid("terrain source query exceeds nine cells"));
        }
        let tx = self.connection.unchecked_transaction()?;
        let environment = crate::environment_store::read_snapshot(&tx, space, cells)?;
        let roads = query::read_snapshot(&tx, space, bounds)?;
        let (cell_size, minimum_height, maximum_height) = tx.query_row(
            "SELECT cell_size,minimum_y,maximum_y FROM world_spaces WHERE id=?1",
            [space.0],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let mut terrain = TerrainSource {
            space,
            cell_size,
            minimum_height,
            maximum_height,
            loaded_cells: cells.to_vec(),
            cells: vec![],
        };
        let mut q = tx.prepare_cached("SELECT s.height,s.source_revision,h.resolution,h.heights,h.source_revision FROM source_cells s LEFT JOIN terrain_cell_heightfields h USING(world_space_id,cell_x,cell_z) WHERE s.world_space_id=?1 AND s.cell_x=?2 AND s.cell_z=?3")?;
        for &cell in cells {
            let page = q
                .query_row(params![space.0, cell.x, cell.z], |r| {
                    let resolution = r.get::<_, Option<u16>>(2)?.unwrap_or(33);
                    if !(2..=257).contains(&resolution) {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    let heights = if r.get::<_, Option<u16>>(2)?.is_some() {
                        let bytes = r.get_ref(3)?.as_blob()?;
                        if bytes.len() != usize::from(resolution).pow(2) * 4 {
                            return Err(rusqlite::Error::InvalidQuery);
                        }
                        crate::decode_f32_blob(bytes, "terrain heights")?
                    } else {
                        vec![r.get::<_, f32>(0)?; usize::from(resolution).pow(2)]
                    };
                    Ok(TerrainSourceCell {
                        flat: r.get::<_, Option<u16>>(2)?.is_none(),
                        cell,
                        resolution,
                        heights,
                        revision: r.get::<_, Option<i64>>(4)?.unwrap_or(r.get(1)?) as u64,
                    })
                })
                .optional()?;
            if let Some(page) = page {
                terrain.cells.push(page);
            }
        }
        Ok(RoadTerrainSnapshot {
            source: RoadEnvironmentSnapshot { environment, roads },
            terrain,
        })
    }
}
