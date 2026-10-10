//! Optional CPU material pass over finalized terrain, inside unpublished staging.
mod evaluate;
mod filter;
mod inputs;
pub(crate) mod preview;
use crate::parallel;
use crate::terrain_cook::ChangedCells;
use anyhow::{Context, Result, bail};
use evaluate::LeafPlan;
use filter::Core;
pub use inputs::TerrainBakeLibrary;
use std::collections::{BTreeMap, BTreeSet};
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
    /// Spaces that kept their published composites and re-finished only changed tiles.
    pub updated_spaces: u64,
}

pub(super) fn cook(
    path: &Path,
    spaces: &[WorldSpaceRecord],
    library: &TerrainBakeLibrary,
    cache: Option<&Path>,
    changed: Option<&ChangedCells>,
) -> Result<(RuntimeManifest, TerrainMaterialBakeStats)> {
    let store = match changed {
        Some(_) => TerrainMaterialCookStore::open_incremental(path)?,
        None => TerrainMaterialCookStore::open_staging(path)?,
    };
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
        let bake = Bake {
            store: &store,
            cache: cache.as_ref(),
            library,
            size: space.cell_size,
            first,
            minimum_level,
        };
        // A copied publication baked from the same library keeps every core and tile that no
        // changed cell reaches. A clean core missing from the cache restarts the space.
        if let Some(changed) = changed
            && cache.is_some()
            && store.library_fingerprint(space.id)? == Some(library.fingerprint())
        {
            let cells = changed.get(&space.id).cloned().unwrap_or_default();
            match bake.update(space.id, &cells, &mut stats) {
                Ok(()) => {
                    stats.updated_spaces += 1;
                    continue;
                }
                Err(error) if error.chain().any(|e| e.is::<MissingCore>()) => {}
                Err(error) => return Err(error),
            }
        }
        store.clear_space(space.id)?;
        bake.all(space.id, &mut stats)?;
    }
    Ok((store.finish(&library.fingerprint())?, stats))
}

/// A clean core the cook cache no longer holds.
#[derive(Debug)]
struct MissingCore(TerrainMaterialKey);
impl std::fmt::Display for MissingCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "composite core {:?} is not in the cook cache", self.0)
    }
}
impl std::error::Error for MissingCore {}

struct Bake<'a> {
    store: &'a TerrainMaterialCookStore,
    cache: Option<&'a CoreCache>,
    library: &'a TerrainBakeLibrary,
    size: f32,
    /// The level whose cores are evaluated from leaves; finer cores are never staged.
    first: u8,
    minimum_level: u8,
}
impl Bake<'_> {
    /// Every core and tile of a space, level by level.
    fn all(&self, space: WorldSpaceId, stats: &mut TerrainMaterialBakeStats) -> Result<()> {
        for level in self.first..=MAX_TERRAIN_NODE_LEVEL {
            let mut count = 0;
            let start = std::time::Instant::now();
            visit(self.store, space, level, |keys| {
                let keys: Vec<_> = keys.iter().map(|&(key, _)| key).collect();
                for batch in keys.chunks(self.chunk(level)) {
                    for (&key, (core, fingerprint)) in
                        batch.iter().zip(self.cores(level, batch, stats)?)
                    {
                        self.put(key, &core, &fingerprint)?;
                        count += 1;
                    }
                }
                Ok(())
            })?;
            self.time(level, start, stats);
            if count == 0 {
                break;
            }
            let start = std::time::Instant::now();
            visit(self.store, space, level, |keys| {
                // Finer levels still supply filtering cores to their parents above.
                let mut published = Vec::with_capacity(keys.len());
                for &(key, drawable) in keys {
                    if drawable && (level >= self.minimum_level || self.store.is_root(key)?) {
                        published.push(key);
                    }
                }
                self.tiles(&published, stats)
            })?;
            stats.tile_seconds += start.elapsed().as_secs_f64();
        }
        Ok(())
    }

    /// Only the cores with a changed cell beneath, and the published tiles whose
    /// neighbourhood holds a core that changed. The tree's shape is unchanged.
    fn update(
        &self,
        space: WorldSpaceId,
        cells: &BTreeSet<CellCoord>,
        stats: &mut TerrainMaterialBakeStats,
    ) -> Result<()> {
        let mut dirty = cells
            .iter()
            .map(|&cell| ancestor(TerrainNodeKey::leaf(space, cell), self.first))
            .collect::<Result<BTreeSet<_>>>()?;
        let mut level = self.first;
        while !dirty.is_empty() {
            let start = std::time::Instant::now();
            let keys: Vec<_> = dirty.iter().copied().collect();
            let mut changed = BTreeSet::new();
            for batch in keys.chunks(self.chunk(level)) {
                for (&key, (core, fingerprint)) in
                    batch.iter().zip(self.cores(level, batch, stats)?)
                {
                    if self.store.core_fingerprint(key)? != Some(fingerprint) {
                        changed.insert(key);
                    }
                    self.put(key, &core, &fingerprint)?;
                }
            }
            self.time(level, start, stats);
            let start = std::time::Instant::now();
            let mut tiles = BTreeSet::new();
            for &key in &changed {
                for neighbour in neighbours(key).into_iter().flatten() {
                    if self.store.has_composite(neighbour)? {
                        tiles.insert(neighbour);
                    }
                }
            }
            self.tiles(&tiles.into_iter().collect::<Vec<_>>(), stats)?;
            stats.tile_seconds += start.elapsed().as_secs_f64();
            dirty = BTreeSet::new();
            for key in changed {
                if let Some(parent) = key.0.parent()?.map(TerrainMaterialKey)
                    && self.store.core_fingerprint(parent)?.is_some()
                {
                    dirty.insert(parent);
                }
            }
            level += 1;
        }
        Ok(())
    }

    /// Groups at the first level hold 4^level leaves; keep leaves per batch bounded.
    fn chunk(&self, level: u8) -> usize {
        if level == self.first {
            (BATCH >> (2 * u32::from(level).min(8))).max(1)
        } else {
            BATCH
        }
    }

    fn time(&self, level: u8, start: std::time::Instant, stats: &mut TerrainMaterialBakeStats) {
        *if level == self.first {
            &mut stats.leaf_seconds
        } else {
            &mut stats.parent_seconds
        } += start.elapsed().as_secs_f64();
    }

    /// New cores for `batch`, all at `level`, with their fingerprints.
    fn cores(
        &self,
        level: u8,
        batch: &[TerrainMaterialKey],
        stats: &mut TerrainMaterialBakeStats,
    ) -> Result<Vec<(StagedCore, [u8; 32])>> {
        if level == self.first {
            return group_cores(
                self.store,
                self.size,
                self.library,
                self.cache,
                batch,
                stats,
            );
        }
        record(stats, 5 * batch.len());
        let children = batch
            .iter()
            .map(|&key| {
                let keys = key.0.children()?.context("parent without children")?;
                let mut payloads: [Option<StagedCore>; 4] = Default::default();
                for (payload, child) in payloads.iter_mut().zip(keys) {
                    *payload = self.core(TerrainMaterialKey(child))?;
                }
                Ok(payloads)
            })
            .collect::<Result<Vec<_>>>()?;
        parallel::map(&children, |payloads| {
            let mut cores: [Option<Core>; 4] = Default::default();
            for (core, payload) in cores.iter_mut().zip(payloads) {
                *core = payload.as_ref().map(decode_core).transpose()?;
            }
            let core = filter::parent(&cores);
            Ok((StagedCore::compress(&core.encode()?)?, core.fingerprint))
        })
        .into_iter()
        .collect()
    }

    /// The core behind `key`: staged by this cook, or else the published one from the cache.
    fn core(&self, key: TerrainMaterialKey) -> Result<Option<StagedCore>> {
        if let Some(core) = self.store.core_payload(key)? {
            return Ok(Some(core));
        }
        let Some(fingerprint) = self.store.core_fingerprint(key)? else {
            return Ok(None);
        };
        let cached = match self.cache {
            Some(cache) => cache.get(key, &fingerprint)?,
            None => None,
        };
        Ok(Some(cached.ok_or(MissingCore(key))?))
    }

    fn put(
        &self,
        key: TerrainMaterialKey,
        core: &StagedCore,
        fingerprint: &[u8; 32],
    ) -> Result<()> {
        self.store.put_core_payload(key, core)?;
        self.store.put_core_fingerprint(key, fingerprint)?;
        // Every level is cached: a later cook reads clean siblings and neighbours from it.
        if let Some(cache) = self.cache
            && !cache.contains(key, fingerprint)?
        {
            cache.put(key, fingerprint, core)?;
        }
        Ok(())
    }

    /// Finishes and stores the tiles of `keys`, all at one level, from their neighbourhoods.
    fn tiles(
        &self,
        keys: &[TerrainMaterialKey],
        stats: &mut TerrainMaterialBakeStats,
    ) -> Result<()> {
        for batch in keys.chunks(BATCH) {
            let neighbourhoods = self.neighbourhood_cores(batch)?;
            record(stats, neighbourhoods.len());
            let tiles = parallel::map(batch, |&key| -> Result<PreparedComposite> {
                let neighbors = neighbours(key).map(|k| k.and_then(|k| neighbourhoods.get(&k)));
                Ok(PreparedComposite::new(&filter::finish(key, &neighbors)?)?)
            });
            for tile in tiles {
                self.store.insert_prepared(&tile?)?;
                stats.tiles += 1;
            }
        }
        Ok(())
    }

    /// Every core the batch's neighbourhoods read, each loaded and decoded once.
    fn neighbourhood_cores(
        &self,
        batch: &[TerrainMaterialKey],
    ) -> Result<BTreeMap<TerrainMaterialKey, Core>> {
        let keys: BTreeSet<_> = batch
            .iter()
            .flat_map(|&k| neighbours(k))
            .flatten()
            .collect();
        let mut payloads = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(payload) = self.core(key)? {
                payloads.push((key, payload));
            }
        }
        parallel::map(&payloads, |(key, payload)| {
            Ok((*key, decode_core(payload)?))
        })
        .into_iter()
        .collect()
    }
}

/// The ancestor of `key` at `level`.
fn ancestor(mut key: TerrainNodeKey, level: u8) -> Result<TerrainMaterialKey> {
    while key.level < level {
        key = key.parent()?.context("terrain node level limit")?;
    }
    Ok(TerrainMaterialKey(key))
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
        domain: PageDomain::Terrain,
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
    batch: &[TerrainMaterialKey],
    stats: &mut TerrainMaterialBakeStats,
) -> Result<Vec<(StagedCore, [u8; 32])>> {
    let groups = batch
        .iter()
        .map(|&key| {
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
    Ok(cores
        .into_iter()
        .map(Option::unwrap)
        .zip(fingerprints)
        .collect())
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
