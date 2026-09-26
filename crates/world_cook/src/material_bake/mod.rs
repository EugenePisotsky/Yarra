//! Optional CPU material pass over finalized terrain, inside unpublished staging.
mod evaluate;
mod filter;
mod inputs;
pub(crate) mod preview;
use crate::parallel;
use anyhow::{Context, Result, bail};
use filter::Core;
pub use inputs::TerrainBakeLibrary;
use std::path::Path;
use world::*;
use world_db::{
    PreparedComposite, RuntimeManifest, StagedCore, TerrainMaterialCookStore,
    TerrainRenderResources, WorldSpaceRecord,
};

#[derive(Debug, Clone, Default)]
pub struct TerrainMaterialBakeStats {
    pub tiles: u64,
    /// Logical simultaneously held filtering cores, excluding codecs/SQLite scratch.
    pub peak_filter_cores: usize,
    pub peak_core_pixels: usize,
    pub tile_gpu_bytes: u64,
    /// Wall-clock time evaluating leaves, filtering parents and finishing published tiles.
    pub leaf_seconds: f64,
    pub parent_seconds: f64,
    pub tile_seconds: f64,
}

pub(super) fn cook(
    path: &Path,
    spaces: &[WorldSpaceRecord],
    library: &TerrainBakeLibrary,
) -> Result<(RuntimeManifest, TerrainMaterialBakeStats)> {
    let store = TerrainMaterialCookStore::open_staging(path)?;
    let mut stats = TerrainMaterialBakeStats {
        tile_gpu_bytes: TerrainComposite::gpu_bytes() as u64,
        ..Default::default()
    };
    for space in spaces {
        let minimum_level = store.composite_minimum_level(space.id)?;
        for level in 0..=MAX_TERRAIN_NODE_LEVEL {
            let mut count = 0;
            let start = std::time::Instant::now();
            visit(&store, space.id, level, |keys| {
                for batch in keys.chunks(BATCH) {
                    let cores = if level == 0 {
                        leaf_cores(&store, space.cell_size, library, batch)?
                    } else {
                        parent_cores(&store, batch)?
                    };
                    record(&mut stats, if level == 0 { 1 } else { 5 } * batch.len());
                    for (&(key, _), core) in batch.iter().zip(cores) {
                        store.put_core_payload(key, &core?)?;
                        count += 1;
                    }
                }
                Ok(())
            })?;
            *if level == 0 {
                &mut stats.leaf_seconds
            } else {
                &mut stats.parent_seconds
            } += start.elapsed().as_secs_f64();
            if count == 0 {
                break;
            }
            let start = std::time::Instant::now();
            visit(&store, space.id, level, |keys| {
                // Finer levels still supply filtering cores to their parents above.
                let mut published = Vec::with_capacity(keys.len());
                for &(key, drawable) in keys {
                    if drawable && (level >= minimum_level || store.is_root(key)?) {
                        published.push(key);
                    }
                }
                for batch in published.chunks(BATCH) {
                    let neighbourhoods = neighbourhood_cores(&store, batch)?;
                    record(&mut stats, neighbourhoods.len());
                    let tiles = parallel::map(batch, |&key| -> Result<PreparedComposite> {
                        let neighbors =
                            neighbours(key).map(|k| k.and_then(|k| neighbourhoods.get(&k)));
                        Ok(PreparedComposite::new(&filter::finish(key, &neighbors)?)?)
                    });
                    for tile in tiles {
                        store.insert_prepared(&tile?)?;
                        stats.tiles += 1;
                    }
                }
                Ok(())
            })?;
            stats.tile_seconds += start.elapsed().as_secs_f64();
        }
    }
    Ok((store.finish()?, stats))
}
/// Every phase reads its inputs here, works on every core and writes in key order, so SQLite
/// access stays on this thread and the output matches a sequential pass. The batch size is
/// fixed, not per machine, and bounds the cores held at once.
const BATCH: usize = 32;

type LeafInputs = (
    TerrainMaterialKey,
    TerrainHeightfieldPage,
    TerrainRenderResources,
);

fn leaf_inputs(store: &TerrainMaterialCookStore, key: TerrainMaterialKey) -> Result<LeafInputs> {
    let page_key = PageKey {
        space: key.0.space,
        cell: CellCoord {
            x: key.0.x,
            z: key.0.z,
        },
        domain: PageDomain::TerrainRender,
        lod: 0,
    };
    let PagePayload::TerrainHeightfield(page) = store
        .reader()
        .read_page(page_key)?
        .context("missing finalized ground")?
        .decode()?
        .payload
    else {
        bail!("composites require finalized heightfields")
    };
    let resources = store.reader().read_terrain_resources(page_key)?;
    Ok((key, page, resources))
}

fn leaf_cores(
    store: &TerrainMaterialCookStore,
    size: f32,
    library: &TerrainBakeLibrary,
    batch: &[(TerrainMaterialKey, bool)],
) -> Result<Vec<Result<StagedCore>>> {
    let inputs = batch
        .iter()
        .map(|&(key, _)| leaf_inputs(store, key))
        .collect::<Result<Vec<_>>>()?;
    Ok(parallel::map(&inputs, |(key, page, resources)| {
        let core = evaluate::leaf(
            *key,
            size,
            page,
            resources,
            library.get(&resources.texture_set)?,
        )?;
        Ok(StagedCore::compress(&core.encode()?)?)
    }))
}

fn parent_cores(
    store: &TerrainMaterialCookStore,
    batch: &[(TerrainMaterialKey, bool)],
) -> Result<Vec<Result<StagedCore>>> {
    let children = batch
        .iter()
        .map(|&(key, _)| {
            let keys = key.0.children()?.context("parent without children")?;
            let mut payloads: [Option<StagedCore>; 4] = Default::default();
            for (payload, child) in payloads.iter_mut().zip(keys) {
                *payload = store.core_payload(TerrainMaterialKey(child))?;
            }
            Ok(payloads)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(parallel::map(&children, |payloads| {
        let mut cores: [Option<Core>; 4] = Default::default();
        for (core, payload) in cores.iter_mut().zip(payloads) {
            *core = payload.as_ref().map(decode_core).transpose()?;
        }
        Ok(StagedCore::compress(&filter::parent(&cores).encode()?)?)
    }))
}

/// Same-level 3×3 neighbourhood of a tile, row-major; `None` where a key would overflow.
fn neighbours(key: TerrainMaterialKey) -> [Option<TerrainMaterialKey>; 9] {
    std::array::from_fn(|i| {
        Some(TerrainMaterialKey(TerrainNodeKey {
            x: key.0.x.checked_add(i as i32 % 3 - 1)?,
            z: key.0.z.checked_add(i as i32 / 3 - 1)?,
            ..key.0
        }))
    })
}

/// Every core the batch's neighbourhoods read, each loaded and decoded once.
fn neighbourhood_cores(
    store: &TerrainMaterialCookStore,
    batch: &[TerrainMaterialKey],
) -> Result<std::collections::BTreeMap<TerrainMaterialKey, Core>> {
    let keys: std::collections::BTreeSet<_> = batch
        .iter()
        .flat_map(|&k| neighbours(k))
        .flatten()
        .collect();
    let mut payloads = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(payload) = store.core_payload(key)? {
            payloads.push((key, payload));
        }
    }
    parallel::map(&payloads, |(key, payload)| {
        Ok((*key, decode_core(payload)?))
    })
    .into_iter()
    .collect()
}

fn decode_core(payload: &StagedCore) -> Result<Core> {
    Core::decode(&payload.decompress()?)
}

fn record(stats: &mut TerrainMaterialBakeStats, count: usize) {
    stats.peak_filter_cores = stats.peak_filter_cores.max(count);
    stats.peak_core_pixels = stats.peak_core_pixels.max(count * filter::N * filter::N);
}

fn visit(
    store: &TerrainMaterialCookStore,
    space: WorldSpaceId,
    level: u8,
    mut f: impl FnMut(&[(TerrainMaterialKey, bool)]) -> Result<()>,
) -> Result<()> {
    let mut cursor = None;
    loop {
        let keys = store.keys(space, level, cursor)?;
        let Some((last, _)) = keys.last() else {
            return Ok(());
        };
        cursor = Some(CellCoord {
            x: last.0.x,
            z: last.0.z,
        });
        f(&keys)?;
    }
}
#[cfg(test)]
mod tests;
