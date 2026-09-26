//! Optional CPU material pass over finalized terrain, inside unpublished staging.
mod evaluate;
mod filter;
mod inputs;
pub(crate) mod preview;
use anyhow::{Context, Result, bail};
use filter::Core;
pub use inputs::TerrainBakeLibrary;
use std::path::Path;
use world::*;
use world_db::{
    RuntimeManifest, TerrainMaterialCookStore, TerrainRenderResources, WorldSpaceRecord,
};

#[derive(Debug, Clone, Default)]
pub struct TerrainMaterialBakeStats {
    pub tiles: u64,
    /// Logical simultaneously held filtering cores, excluding codecs/SQLite scratch.
    pub peak_filter_cores: usize,
    pub peak_core_pixels: usize,
    pub tile_gpu_bytes: u64,
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
            if level == 0 {
                let mut cursor = None;
                loop {
                    let keys = store.keys(space.id, 0, cursor)?;
                    let Some((last, _)) = keys.last() else {
                        break;
                    };
                    cursor = Some(CellCoord {
                        x: last.0.x,
                        z: last.0.z,
                    });
                    for batch in keys.chunks(LEAF_BATCH) {
                        let inputs = batch
                            .iter()
                            .map(|&(key, _)| leaf_inputs(&store, key))
                            .collect::<Result<Vec<_>>>()?;
                        let cores = evaluate_leaves(space.cell_size, library, &inputs)?;
                        record(&mut stats, cores.len());
                        for ((key, _, _), core) in inputs.iter().zip(cores) {
                            store.put_core(*key, &core.encode()?)?;
                            count += 1;
                        }
                    }
                }
            } else {
                visit(&store, space.id, level, |key, _| {
                    let mut children: [Option<Core>; 4] = std::array::from_fn(|_| None);
                    for (i, k) in key.0.children()?.unwrap().into_iter().enumerate() {
                        children[i] = load(&store, TerrainMaterialKey(k))?;
                    }
                    record(&mut stats, children.iter().flatten().count() + 1);
                    store.put_core(key, &filter::parent(&children).encode()?)?;
                    count += 1;
                    Ok(())
                })?;
            }
            if count == 0 {
                break;
            }
            visit(&store, space.id, level, |key, drawable| {
                // Finer levels still supply filtering cores to their parents above.
                if !drawable || (level < minimum_level && !store.is_root(key)?) {
                    return Ok(());
                }
                let mut neighbors: [Option<Core>; 9] = std::array::from_fn(|_| None);
                for (i, slot) in neighbors.iter_mut().enumerate() {
                    if let (Some(x), Some(z)) = (
                        key.0.x.checked_add(i as i32 % 3 - 1),
                        key.0.z.checked_add(i as i32 / 3 - 1),
                    ) {
                        *slot = load(&store, TerrainMaterialKey(TerrainNodeKey { x, z, ..key.0 }))?;
                    }
                }
                record(&mut stats, neighbors.iter().flatten().count());
                let tile = filter::finish(key, &neighbors)?;
                store.insert(&tile)?;
                stats.tiles += 1;
                Ok(())
            })?;
        }
    }
    Ok((store.finish()?, stats))
}
/// Leaves are independent. Each batch is read here, evaluated on every core and stored in key
/// order, so SQLite access stays on this thread and the output matches a sequential pass.
/// The batch size is fixed, not per machine, and bounds the leaf cores held at once.
const LEAF_BATCH: usize = 32;

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

fn evaluate_leaves(
    size: f32,
    library: &TerrainBakeLibrary,
    inputs: &[LeafInputs],
) -> Result<Vec<Core>> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, inputs.len().max(1));
    std::thread::scope(|scope| {
        let workers: Vec<_> = inputs
            .chunks(inputs.len().div_ceil(threads).max(1))
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|(key, page, resources)| {
                            evaluate::leaf(
                                *key,
                                size,
                                page,
                                resources,
                                library.get(&resources.texture_set)?,
                            )
                        })
                        .collect::<Result<Vec<_>>>()
                })
            })
            .collect();
        let mut cores = Vec::with_capacity(inputs.len());
        for worker in workers {
            cores.extend(worker.join().expect("leaf evaluation panicked")?);
        }
        Ok(cores)
    })
}

fn record(stats: &mut TerrainMaterialBakeStats, count: usize) {
    stats.peak_filter_cores = stats.peak_filter_cores.max(count);
    stats.peak_core_pixels = stats.peak_core_pixels.max(count * filter::N * filter::N);
}
fn load(store: &TerrainMaterialCookStore, key: TerrainMaterialKey) -> Result<Option<Core>> {
    store.core(key)?.map(|b| Core::decode(&b)).transpose()
}
fn visit(
    store: &TerrainMaterialCookStore,
    space: WorldSpaceId,
    level: u8,
    mut f: impl FnMut(TerrainMaterialKey, bool) -> Result<()>,
) -> Result<()> {
    let mut cursor = None;
    loop {
        let keys = store.keys(space, level, cursor)?;
        if keys.is_empty() {
            return Ok(());
        }
        for &(key, drawable) in &keys {
            f(key, drawable)?;
        }
        cursor = keys.last().map(|(k, _)| CellCoord { x: k.0.x, z: k.0.z });
    }
}
#[cfg(test)]
mod tests;
