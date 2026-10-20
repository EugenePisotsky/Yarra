//! Live terrain previews for the editor: a complete sparse override of the publication for
//! edited cells (their pages, the hierarchy nodes their heights change and the control tiles
//! their ground changes), which terrain v2 draws in place of the published ones. Never modifies
//! the source or runtime DB.
use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};
use world::*;
use world_db::RuntimeReader;

pub const MAX_PREVIEW_LEAVES: usize = 256;

pub fn published_terrain_leaf(
    reader: &RuntimeReader,
    space: WorldSpaceId,
    cell: CellCoord,
) -> Result<Option<TerrainHeightfieldPage>> {
    let key = PageKey {
        space,
        cell,
        domain: PageDomain::Terrain,
        lod: 0,
    };
    let Some(page) = reader.read_page(key)? else {
        return Ok(None);
    };
    match page.decode()?.payload {
        PagePayload::TerrainHeightfield(page) => Ok(Some(page)),
        _ => bail!("unexpected terrain leaf payload"),
    }
}

/// The preview of `leaves` (edited pages that differ from the publication): their nodes and
/// ancestors where heights changed, and the control tiles of every edited leaf and its
/// ancestors up to [`CONTROL_TOP_LEVEL`]. Unchanged siblings come from the publication.
pub fn preview_terrain(
    reader: &RuntimeReader,
    space: WorldSpaceId,
    leaves: BTreeMap<CellCoord, Arc<TerrainHeightfieldPage>>,
    cancelled: impl Fn() -> bool,
) -> Result<TerrainPreviewProducts> {
    if leaves.len() > MAX_PREVIEW_LEAVES {
        bail!("Live terrain exceeds 256 changed cells; Save & Publish to continue.")
    }
    let nodes = preview_nodes(reader, space, &leaves, &cancelled)?;
    let controls = preview_controls(reader, space, &leaves, &cancelled)?;
    Ok(TerrainPreviewProducts {
        nodes,
        controls,
        leaves,
        ..Default::default()
    })
}

/// Changed leaves' nodes and every ancestor up to the roots. Borders are checked even across
/// separate coarse roots, where parent validation alone would never see a mismatch; a missing
/// relief halo fails closed.
fn preview_nodes(
    reader: &RuntimeReader,
    space: WorldSpaceId,
    leaves: &BTreeMap<CellCoord, Arc<TerrainHeightfieldPage>>,
    cancelled: &impl Fn() -> bool,
) -> Result<BTreeMap<TerrainNodeKey, Arc<TerrainNode>>> {
    let roots: BTreeSet<_> = reader
        .read_terrain_roots(space)?
        .into_iter()
        .map(|d| d.key)
        .collect();
    let mut nodes = BTreeMap::new();
    let mut changed = BTreeSet::new();
    for (&cell, page) in leaves {
        if cancelled() {
            bail!("superseded terrain preview")
        }
        let key = TerrainNodeKey::leaf(space, cell);
        let old = reader
            .read_terrain_node(key)?
            .context("preview cannot add terrain outside the published domain")?
            .decode()?;
        let node = TerrainNode::leaf(
            key,
            &page.heightfield,
            old.heightfield
                .as_ref()
                .context("missing leaf samples")?
                .resolution,
        )?;
        if node != old {
            changed.insert(key);
            nodes.insert(key, Arc::new(node));
        }
    }
    for &key in &changed {
        let h = nodes[&key].heightfield.as_ref().unwrap();
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let other = TerrainNodeKey {
                x: key.x + dx,
                z: key.z + dz,
                ..key
            };
            let node = match nodes.get(&other) {
                Some(n) => Some(n.clone()),
                None => reader
                    .read_terrain_node(other)?
                    .map(|e| e.decode().map(Arc::new))
                    .transpose()?,
            };
            if let Some(other) = node {
                let b = other
                    .heightfield
                    .as_ref()
                    .context("missing neighbor samples")?;
                let n = usize::from(h.resolution);
                for i in 0..n {
                    let (a_i, b_i) = match (dx, dz) {
                        (-1, 0) => (i * n, i * n + n - 1),
                        (1, 0) => (i * n + n - 1, i * n),
                        (0, -1) => (i, (n - 1) * n + i),
                        _ => ((n - 1) * n + i, i),
                    };
                    if h.resolution != b.resolution
                        || h.heights[a_i].to_bits() != b.heights[b_i].to_bits()
                        || h.normals_oct[a_i] != b.normals_oct[b_i]
                    {
                        bail!(
                            "Live terrain needs its neighboring relief/normal cells; previous preview retained"
                        )
                    }
                }
            }
        }
    }
    while !changed.is_empty() {
        let parents: BTreeSet<_> = changed
            .iter()
            .filter(|k| !roots.contains(k))
            .filter_map(|k| k.parent().ok().flatten())
            .collect();
        for &key in &parents {
            if cancelled() {
                bail!("superseded terrain preview")
            }
            let mut children: [Option<Arc<TerrainNode>>; 4] = Default::default();
            for (i, child) in key
                .children()?
                .context("missing children")?
                .into_iter()
                .enumerate()
            {
                children[i] = match nodes.get(&child) {
                    Some(n) => Some(n.clone()),
                    None => reader
                        .read_terrain_node(child)?
                        .map(|e| e.decode().map(Arc::new))
                        .transpose()?,
                };
            }
            let node = TerrainNode::parent(key, std::array::from_fn(|i| children[i].as_deref()))?;
            nodes.insert(key, Arc::new(node));
        }
        changed = parents;
    }
    Ok(nodes)
}

/// Every edited leaf's control tile, and each ancestor's filtered from its children (published
/// ones for unedited siblings) up to [`CONTROL_TOP_LEVEL`].
fn preview_controls(
    reader: &RuntimeReader,
    space: WorldSpaceId,
    leaves: &BTreeMap<CellCoord, Arc<TerrainHeightfieldPage>>,
    cancelled: &impl Fn() -> bool,
) -> Result<BTreeMap<TerrainNodeKey, Arc<Vec<u32>>>> {
    let resources = reader.read_space_terrain_resources(space)?;
    let layers: HashMap<TerrainSurfaceId, u16> = resources
        .surfaces
        .iter()
        .map(|s| (s.surface.id, s.layer))
        .collect();
    let size = reader
        .manifest()
        .world_spaces
        .iter()
        .find(|s| s.id == space)
        .context("missing terrain space")?
        .cell_size;
    let mut controls = BTreeMap::new();
    let mut changed = BTreeSet::new();
    for (&cell, page) in leaves {
        if cancelled() {
            bail!("superseded terrain preview")
        }
        let slots: Vec<u16> = page
            .surfaces
            .iter()
            .map(|id| layers.get(id).copied().context("unknown terrain surface"))
            .collect::<Result<_>>()?;
        let key = TerrainNodeKey::leaf(space, cell);
        let samples = leaf_control(page, &slots, cell, size).map_err(anyhow::Error::msg)?;
        controls.insert(key, Arc::new(samples));
        changed.insert(key);
    }
    for _ in 0..CONTROL_TOP_LEVEL {
        let parents: BTreeSet<_> = changed
            .iter()
            .filter_map(|k| k.parent().ok().flatten())
            .collect();
        for &key in &parents {
            if cancelled() {
                bail!("superseded terrain preview")
            }
            let children = key.children()?.context("missing children")?;
            let published = reader.read_terrain_controls(&children)?;
            let tiles: Vec<Option<Arc<Vec<u32>>>> = children
                .iter()
                .zip(published)
                .map(|(child, payload)| match controls.get(child) {
                    Some(tile) => Ok(Some(Arc::clone(tile))),
                    None => payload
                        .map(|p| world_db::decode_terrain_control(&p).map(Arc::new))
                        .transpose()
                        .map_err(anyhow::Error::from),
                })
                .collect::<Result<_>>()?;
            let tile = filter_control_parent(std::array::from_fn(|i| {
                tiles[i].as_deref().map(Vec::as_slice)
            }));
            controls.insert(key, Arc::new(tile));
        }
        changed = parents;
    }
    Ok(controls)
}

/// The published control tile of `key`, to restore it where an undone edit leaves it.
pub fn published_terrain_control(
    reader: &RuntimeReader,
    key: TerrainNodeKey,
) -> Result<Option<Vec<u32>>> {
    reader
        .read_terrain_controls(&[key])?
        .pop()
        .flatten()
        .map(|p| world_db::decode_terrain_control(&p))
        .transpose()
        .map_err(anyhow::Error::from)
}
