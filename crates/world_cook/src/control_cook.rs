//! Terrain v2's control tiles (`world::terrain_control`) above the cells: every node up to
//! [`CONTROL_TOP_LEVEL`] filtered from its children. Each cell's leaf tile was written with the
//! cell. A full build reads and filters each group of cells under one top-level node on its own
//! core with its own reader, then writes every tile; an incremental cook rebuilds only the
//! ancestors of the cells it cooked again.
use crate::parallel;
use crate::terrain_cook::ChangedCells;
use anyhow::{Context, Result};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use world::*;
use world_db::{RuntimeReader, TerrainControlStore, WorldSpaceRecord, encode_terrain_control};

pub(crate) fn cook(
    path: &Path,
    spaces: &[WorldSpaceRecord],
    changed: Option<&ChangedCells>,
) -> Result<()> {
    match changed {
        Some(changed) => update(path, changed),
        None => build(path, spaces),
    }
}

fn build(path: &Path, spaces: &[WorldSpaceRecord]) -> Result<()> {
    let mut written: Vec<(TerrainNodeKey, Vec<u8>)> = Vec::new();

    for space in spaces {
        let cells = RuntimeReader::open_immutable(path)?.terrain_cells(space.id)?;
        let mut groups: HashMap<(i32, i32), Vec<CellCoord>> = HashMap::new();

        for cell in cells {
            groups
                .entry((cell.x >> CONTROL_TOP_LEVEL, cell.z >> CONTROL_TOP_LEVEL))
                .or_default()
                .push(cell);
        }

        let mut groups: Vec<_> = groups.into_values().collect();
        groups.sort_by_key(|g| (g[0].z, g[0].x));

        for tiles in parallel::map(&groups, |cells| ancestors(path, space.id, cells)) {
            written.extend(tiles?);
        }
    }

    let store = TerrainControlStore::open_staging(path)?;

    for (key, payload) in &written {
        store.write(*key, payload)?;
    }

    Ok(store.finish()?)
}

/// Every tile above one group's cells, encoded.
fn ancestors(
    path: &Path,
    space: WorldSpaceId,
    cells: &[CellCoord],
) -> Result<Vec<(TerrainNodeKey, Vec<u8>)>> {
    let reader = RuntimeReader::open_immutable(path)?;
    let keys: Vec<_> = cells
        .iter()
        .map(|&cell| TerrainNodeKey::leaf(space, cell))
        .collect();
    let mut level = HashMap::new();

    for (key, payload) in keys.iter().zip(reader.read_terrain_controls(&keys)?) {
        let payload = payload.context("a cell has no control tile")?;
        level.insert(*key, world_db::decode_terrain_control(&payload)?);
    }

    let mut out = Vec::new();

    for _ in 0..CONTROL_TOP_LEVEL {
        let mut parents = HashMap::new();

        for key in level.keys() {
            let parent = key.parent()?.context("a node below the top has a parent")?;

            if parents.contains_key(&parent) {
                continue;
            }

            let children = parent.children()?.context("a parent has children")?;
            let samples = filter_control_parent(children.map(|c| level.get(&c).map(Vec::as_slice)));
            out.push((parent, encode_terrain_control(&samples)?));
            parents.insert(parent, samples);
        }

        level = parents;
    }

    Ok(out)
}

/// Rebuilds the ancestors of `changed`, level by level, from their stored children.
fn update(path: &Path, changed: &ChangedCells) -> Result<()> {
    let store = TerrainControlStore::open_staging(path)?;

    for (&space, cells) in changed {
        let mut level: BTreeSet<_> = cells
            .iter()
            .map(|&cell| TerrainNodeKey::leaf(space, cell))
            .collect();

        for _ in 0..CONTROL_TOP_LEVEL {
            let parents = level
                .iter()
                .map(|key| key.parent()?.context("a node below the top has a parent"))
                .collect::<Result<BTreeSet<_>>>()?;

            for &parent in &parents {
                let children = parent.children()?.context("a parent has children")?;
                let tiles = store.read(&children)?;
                let samples = filter_control_parent(std::array::from_fn(|i| tiles[i].as_deref()));
                store.write(parent, &encode_terrain_control(&samples)?)?;
            }

            level = parents;
        }
    }

    Ok(store.finish()?)
}
