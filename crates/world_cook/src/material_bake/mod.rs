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
use world_db::{RuntimeManifest, TerrainMaterialCookStore, WorldSpaceRecord};

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
        for level in 0..=MAX_TERRAIN_NODE_LEVEL {
            let mut count = 0;
            visit(&store, space.id, level, |key, _| {
                let core = if level == 0 {
                    let page_key = PageKey {
                        space: space.id,
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
                    evaluate::leaf(
                        key,
                        space.cell_size,
                        &page,
                        &resources,
                        library.get(&resources.texture_set)?,
                    )?
                } else {
                    let mut children: [Option<Core>; 4] = std::array::from_fn(|_| None);
                    for (i, k) in key.0.children()?.unwrap().into_iter().enumerate() {
                        children[i] = load(&store, TerrainMaterialKey(k))?;
                    }
                    record(&mut stats, children.iter().flatten().count() + 1);
                    filter::parent(&children)
                };
                record(&mut stats, 1);
                store.put_core(key, &core.encode()?)?;
                count += 1;
                Ok(())
            })?;
            if count == 0 {
                break;
            }
            visit(&store, space.id, level, |key, drawable| {
                if !drawable {
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
