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

/// The coarsest level with control tiles: terrain v2's top clipmap level
/// (`engine::world_streaming::terrain_clipmap`).
const CONTROL_TOP: u8 = 5;

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
    let count = page.surfaces.len();
    let slots: Vec<u16> = page
        .surfaces
        .iter()
        .map(|id| layers.get(id).copied().context("unknown terrain surface"))
        .collect::<Result<_>>()?;
    if count == 0 || count > MAX_TERRAIN_SURFACES_PER_CELL {
        bail!("terrain pages have one to {MAX_TERRAIN_SURFACES_PER_CELL} surfaces");
    }
    if count > 1 && page.weight_pages.len() != count.div_ceil(4) {
        bail!("terrain page weights do not match its surfaces");
    }
    let mut samples = Vec::with_capacity(CONTROL_SAMPLES * CONTROL_SAMPLES);
    let mut ground = vec![0.0_f64; count];
    for j in 0..CONTROL_SAMPLES {
        for i in 0..CONTROL_SAMPLES {
            let uv = [
                i as f64 / CONTROL_INTERVALS as f64,
                j as f64 / CONTROL_INTERVALS as f64,
            ];
            if count == 1 {
                ground[0] = 1.0;
            } else {
                weights(&page.weight_pages, uv, &mut ground);
                if let Some(wear) = &page.wear {
                    let world = [
                        (f64::from(cell.x) + uv[0]) * f64::from(size),
                        (f64::from(cell.z) + uv[1]) * f64::from(size),
                    ];
                    let w = wear.sample(uv).wear;
                    wear.lay(&mut ground, w, bare_share(w, world));
                }
            }
            let mut mix = ControlMix::default();
            for (&layer, &weight) in slots.iter().zip(&ground) {
                mix.add(layer, weight as f32);
            }
            samples.push(mix.pack());
        }
    }
    Ok(samples)
}

/// Bilinear ground weights of the page's surfaces at `uv`, four per RGBA page, normalised.
fn weights(pages: &[TerrainWeightPage], uv: [f64; 2], result: &mut [f64]) {
    result.fill(0.0);
    let n = pages[0].resolution as usize;
    let p = uv.map(|x| x * (n - 1) as f64);
    let a = p.map(|x| (x.floor() as usize).min(n - 1));
    let f = [p[0] - a[0] as f64, p[1] - a[1] as f64];
    for y in 0..2 {
        for x in 0..2 {
            let w =
                (if x == 0 { 1. - f[0] } else { f[0] }) * (if y == 0 { 1. - f[1] } else { f[1] });
            if w <= 0.0 {
                continue;
            }
            let i = ((a[1] + y).min(n - 1) * n + (a[0] + x).min(n - 1)) * 4;
            for (surface, value) in result.iter_mut().enumerate() {
                *value += f64::from(pages[surface / 4].rgba[i + surface % 4]) / 255. * w;
            }
        }
    }
    let sum = result.iter().sum::<f64>().max(1e-9);
    for value in result {
        *value /= sum;
    }
}
