//! Terrain v2's control tiles (`world::terrain_control`): every cell's ground weights, worn where
//! roads wear them as near detail wears them, as control samples by texture layer, and every
//! node above them up to [`CONTROL_TOP`] filtered from its children. Each group of cells under
//! one top-level node is read and computed on its own core with its own reader, then all tiles
//! are written; every cook rebuilds them in full.
use crate::parallel;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::Path;
use world::*;
use world_db::{RuntimeReader, TerrainControlStore, WorldSpaceRecord};

const CONTROL_TOP: u8 = CONTROL_TOP_LEVEL;

pub(crate) fn cook(path: &Path, spaces: &[WorldSpaceRecord]) -> Result<()> {
    let start = std::time::Instant::now();
    let mut written: Vec<(TerrainNodeKey, Vec<u8>)> = Vec::new();

    for space in spaces {
        let reader = RuntimeReader::open_immutable(path)?;
        let cells = reader.terrain_cells(space.id)?;

        if cells.is_empty() {
            continue;
        }

        let resources = reader.read_space_terrain_resources(space.id)?;
        let layers: HashMap<TerrainSurfaceId, u16> = resources
            .surfaces
            .iter()
            .map(|s| (s.surface.id, s.layer))
            .collect();

        if layers.values().any(|&l| l >= MAX_CONTROL_LAYERS) {
            bail!("terrain controls name at most {MAX_CONTROL_LAYERS} texture layers");
        }

        drop(reader);
        let mut groups: HashMap<(i32, i32), Vec<CellCoord>> = HashMap::new();

        for cell in cells {
            groups
                .entry((cell.x >> CONTROL_TOP, cell.z >> CONTROL_TOP))
                .or_default()
                .push(cell);
        }

        let mut groups: Vec<_> = groups.into_values().collect();
        groups.sort_by_key(|g| (g[0].z, g[0].x));
        let size = space.cell_size;
        let tiles = parallel::map(&groups, |cells| group(path, space.id, size, &layers, cells));

        for tiles in tiles {
            written.extend(tiles?);
        }
    }

    let store = TerrainControlStore::open_staging(path)?;
    let bytes: usize = written.iter().map(|(_, p)| p.len()).sum();

    for (key, payload) in &written {
        store.write(*key, payload)?;
    }

    store.finish()?;
    eprintln!(
        "terrain controls: {} tiles, {:.1} MB, {:.1} s",
        written.len(),
        bytes as f64 / 1e6,
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

/// The tiles of one top-level node: its cells' and every level's above them.
fn group(
    path: &Path,
    space: WorldSpaceId,
    size: f32,
    layers: &HashMap<TerrainSurfaceId, u16>,
    cells: &[CellCoord],
) -> Result<Vec<(TerrainNodeKey, Vec<u8>)>> {
    let reader = RuntimeReader::open_immutable(path)?;
    let mut level: HashMap<TerrainNodeKey, Vec<u32>> = HashMap::new();

    for &cell in cells {
        let key = PageKey {
            space,
            cell,
            domain: PageDomain::Terrain,
            lod: 0,
        };
        let PagePayload::TerrainHeightfield(page) = reader
            .read_page(key)?
            .context("missing terrain page")?
            .decode()?
            .payload
        else {
            bail!("terrain controls need heightfield pages");
        };
        let node = TerrainNodeKey {
            space,
            level: 0,
            x: cell.x,
            z: cell.z,
        };
        level.insert(node, leaf(&page, layers, cell, size)?);
    }

    let mut out = Vec::new();

    for depth in 0..CONTROL_TOP {
        let mut parents: HashMap<TerrainNodeKey, Vec<u32>> = HashMap::new();
        let keys: Vec<_> = level.keys().map(|k| parent(*k)).collect();

        for key in keys {
            if parents.contains_key(&key) {
                continue;
            }

            let children = key.children()?.context("a parent has children")?;
            let samples = filter_control_parent(children.map(|c| level.get(&c).map(Vec::as_slice)));
            parents.insert(key, samples);
        }

        debug_assert!(parents.keys().all(|k| k.level == depth + 1));

        for (key, samples) in level.drain() {
            out.push((key, world_db::encode_terrain_control(&samples)?));
        }

        level = parents;
    }

    for (key, samples) in level {
        out.push((key, world_db::encode_terrain_control(&samples)?));
    }

    Ok(out)
}

fn parent(key: TerrainNodeKey) -> TerrainNodeKey {
    TerrainNodeKey {
        level: key.level + 1,
        x: key.x >> 1,
        z: key.z >> 1,
        ..key
    }
}

/// A cell's samples from its ground weights, worn where roads wear it.
fn leaf(
    page: &TerrainHeightfieldPage,
    layers: &HashMap<TerrainSurfaceId, u16>,
    cell: CellCoord,
    size: f32,
) -> Result<Vec<u32>> {
    let slots: Vec<u16> = page
        .surfaces
        .iter()
        .map(|id| layers.get(id).copied().context("unknown terrain surface"))
        .collect::<Result<_>>()?;
    leaf_control(page, &slots, cell, size).map_err(anyhow::Error::msg)
}
