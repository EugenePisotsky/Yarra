//! Optional CPU material pass over finalized terrain, inside unpublished staging.
mod evaluate;
mod filter;
mod inputs;
pub(crate) mod preview;
use crate::parallel;
use anyhow::{Context, Result, bail};
use evaluate::LeafPlan;
use filter::Core;
pub use inputs::TerrainBakeLibrary;
use std::path::Path;
use world::*;
use world_db::{
    CoreCache, PreparedComposite, RuntimeManifest, StagedCore, TerrainMaterialCookStore,
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
    /// Cores at the first baked level: reused from the cook cache, or evaluated from leaves.
    pub cached_cores: u64,
    pub evaluated_cores: u64,
    pub evaluated_leaves: u64,
}

pub(super) fn cook(
    path: &Path,
    spaces: &[WorldSpaceRecord],
    library: &TerrainBakeLibrary,
    cache: Option<&Path>,
) -> Result<(RuntimeManifest, TerrainMaterialBakeStats)> {
    let store = TerrainMaterialCookStore::open_staging(path)?;
    let cache = cache.map(CoreCache::open).transpose()?;
    let mut stats = TerrainMaterialBakeStats {
        tile_gpu_bytes: TerrainComposite::gpu_bytes() as u64,
        ..Default::default()
    };
    for space in spaces {
        let minimum_level = store.composite_minimum_level(space.id)?;
        // Leaves below the finest published level are evaluated in memory and filtered
        // straight to it, so finer cores are never staged. A root below that level needs its
        // same-level neighbours' cores for its tile, so such spaces start at the leaves.
        let first = if store
            .lowest_root_level(space.id)?
            .is_none_or(|lowest| lowest >= minimum_level)
        {
            minimum_level
        } else {
            0
        };
        for level in first..=MAX_TERRAIN_NODE_LEVEL {
            let mut count = 0;
            let start = std::time::Instant::now();
            visit(&store, space.id, level, |keys| {
                // Groups at the first level hold 4^level leaves; keep leaves per batch bounded.
                let chunk = if level == first {
                    (BATCH >> (2 * u32::from(level).min(8))).max(1)
                } else {
                    BATCH
                };
                for batch in keys.chunks(chunk) {
                    let cores = if level == first {
                        group_cores(
                            &store,
                            space.cell_size,
                            library,
                            cache.as_ref(),
                            batch,
                            &mut stats,
                        )?
                    } else {
                        record(&mut stats, 5 * batch.len());
                        parent_cores(&store, batch)?
                    };
                    for (&(key, _), core) in batch.iter().zip(cores) {
                        store.put_core_payload(key, &core?)?;
                        count += 1;
                    }
                }
                Ok(())
            })?;
            *if level == first {
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

/// Cores for nodes at the first baked level, each from all leaves beneath it. Leaves are
/// fingerprinted first; a group whose folded fingerprint is cached skips evaluation.
fn group_cores(
    store: &TerrainMaterialCookStore,
    size: f32,
    library: &TerrainBakeLibrary,
    cache: Option<&CoreCache>,
    batch: &[(TerrainMaterialKey, bool)],
    stats: &mut TerrainMaterialBakeStats,
) -> Result<Vec<Result<StagedCore>>> {
    let groups = batch
        .iter()
        .map(|&(key, _)| {
            let leaves = if key.0.level == 0 {
                vec![key]
            } else {
                store.leaves_within(key)?
            };
            let inputs = leaves
                .into_iter()
                .map(|leaf| leaf_inputs(store, leaf))
                .collect::<Result<Vec<_>>>()?;
            Ok((key, inputs))
        })
        .collect::<Result<Vec<_>>>()?;
    let leaves: Vec<_> = groups.iter().flat_map(|(_, inputs)| inputs).collect();
    let leaf_fingerprints = parallel::map(&leaves, |(key, page, resources)| {
        let inputs = library.get(&resources.texture_set)?;
        Ok(LeafPlan::new(*key, size, page, resources, inputs)?.fingerprint)
    })
    .into_iter()
    .collect::<Result<Vec<_>>>()?;
    let mut next = leaf_fingerprints.into_iter();
    let fingerprints = groups
        .iter()
        .map(|(key, inputs)| {
            let leaves: std::collections::BTreeMap<_, _> = inputs
                .iter()
                .map(|(leaf, _, _)| (leaf.0, next.next().unwrap()))
                .collect();
            fold_fingerprint(key.0, &leaves).context("composite group without leaves")
        })
        .collect::<Result<Vec<_>>>()?;
    let mut cores = Vec::with_capacity(groups.len());
    for ((key, _), fingerprint) in groups.iter().zip(&fingerprints) {
        cores.push(match cache {
            Some(cache) => cache.get(*key, fingerprint)?,
            None => None,
        });
    }
    let missing: Vec<_> = groups
        .iter()
        .enumerate()
        .filter(|(i, _)| cores[*i].is_none())
        .flat_map(|(i, (_, inputs))| inputs.iter().map(move |leaf| (i, leaf)))
        .collect();
    record(stats, missing.len());
    let evaluated = parallel::map(&missing, |(_, (key, page, resources))| {
        evaluate::leaf(
            *key,
            size,
            page,
            resources,
            library.get(&resources.texture_set)?,
        )
    });
    let mut missed = std::collections::BTreeMap::<usize, std::collections::BTreeMap<_, _>>::new();
    for ((i, (key, _, _)), core) in missing.iter().zip(evaluated) {
        missed.entry(*i).or_default().insert(key.0, core?);
    }
    stats.cached_cores += (groups.len() - missed.len()) as u64;
    stats.evaluated_cores += missed.len() as u64;
    stats.evaluated_leaves += missing.len() as u64;
    for (i, mut leaves) in missed {
        let (key, _) = &groups[i];
        let core = fold_core(key.0, &mut leaves).context("composite group without leaves")?;
        if core.fingerprint != fingerprints[i] {
            bail!("composite core fingerprint disagrees with its inputs at {key:?}");
        }
        let staged = StagedCore::compress(&core.encode()?)?;
        if let Some(cache) = cache {
            cache.put(*key, &fingerprints[i], &staged)?;
        }
        cores[i] = Some(staged);
    }
    Ok(cores.into_iter().map(|core| Ok(core.unwrap())).collect())
}

/// The fingerprint `fold_core` would give `node`: parents hash their children's, as
/// `filter::parent` does. `None` where no leaf lies beneath.
fn fold_fingerprint(
    node: TerrainNodeKey,
    leaves: &std::collections::BTreeMap<TerrainNodeKey, [u8; 32]>,
) -> Option<[u8; 32]> {
    let Some(children) = node.children().ok().flatten() else {
        return leaves.get(&node).copied();
    };
    let children = children.map(|child| fold_fingerprint(child, leaves));
    children
        .iter()
        .any(Option::is_some)
        .then(|| filter::parent_fingerprint(children))
}

/// Filters leaf cores up to `node`, as staging them level by level would.
fn fold_core(
    node: TerrainNodeKey,
    leaves: &mut std::collections::BTreeMap<TerrainNodeKey, Core>,
) -> Option<Core> {
    let Some(children) = node.children().ok().flatten() else {
        return leaves.remove(&node);
    };
    let children = children.map(|child| fold_core(child, leaves));
    children
        .iter()
        .any(Option::is_some)
        .then(|| filter::parent(&children))
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
