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

/// A published cell's ground page and control tile; `None` outside the published world.
pub fn published_terrain_leaf(
    reader: &RuntimeReader,
    space: WorldSpaceId,
    cell: CellCoord,
) -> Result<Option<TerrainPreviewLeaf>> {
    let key = PageKey {
        space,
        cell,
        domain: PageDomain::Terrain,
    };
    let Some(page) = reader.read_page(key)? else {
        return Ok(None);
    };
    let PagePayload::TerrainHeightfield(page) = page.decode()?.payload else {
        bail!("unexpected terrain leaf payload");
    };
    let control = published_terrain_control(reader, TerrainNodeKey::leaf(space, cell))?
        .context("a published cell has no control tile")?;
    Ok(Some(TerrainPreviewLeaf { page, control }))
}

/// The texture layer of each of a world's ground surfaces, to lay out edited cells' control
/// tiles as the cook does.
pub struct GroundLayers {
    layers: HashMap<TerrainSurfaceId, u16>,
    cell_size: f32,
}

impl GroundLayers {
    pub fn read(reader: &RuntimeReader, space: WorldSpaceId) -> Result<Self> {
        let resources = reader.read_space_terrain_resources(space)?;
        let cell_size = reader
            .manifest()
            .world_space(space)
            .context("missing terrain space")?
            .cell_size;
        Ok(Self {
            layers: resources
                .surfaces
                .iter()
                .map(|s| (s.surface.id, s.layer))
                .collect(),
            cell_size,
        })
    }

    /// An edited cell as a cook would publish it.
    pub fn leaf(
        &self,
        cell: CellCoord,
        heightfield: TerrainHeightfield,
        ground: &environment_compile::CompiledGround,
        wear: Option<TerrainWear>,
    ) -> Result<TerrainPreviewLeaf> {
        let slots: Vec<u16> = ground
            .surfaces
            .iter()
            .map(|id| {
                self.layers
                    .get(id)
                    .copied()
                    .context("unknown terrain surface")
            })
            .collect::<Result<_>>()?;
        let control = leaf_control(
            &slots,
            &ground.weight_pages,
            wear.as_ref(),
            cell,
            self.cell_size,
        )
        .map_err(anyhow::Error::msg)?;
        Ok(TerrainPreviewLeaf {
            page: TerrainHeightfieldPage {
                heightfield,
                surfaces: ground.surfaces.clone(),
                wear,
            },
            control,
        })
    }
}

/// The preview of `leaves` (edited pages that differ from the publication): their nodes and
/// ancestors where heights changed, and the control tiles of every edited leaf and its
/// ancestors up to [`CONTROL_TOP_LEVEL`]. Unchanged siblings come from the publication.
pub fn preview_terrain(
    reader: &RuntimeReader,
    space: WorldSpaceId,
    leaves: BTreeMap<CellCoord, Arc<TerrainPreviewLeaf>>,
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
    })
}

/// Changed leaves' nodes and every ancestor up to the roots. Borders are checked even across
/// separate coarse roots, where parent validation alone would never see a mismatch; a missing
/// relief halo fails closed.
fn preview_nodes(
    reader: &RuntimeReader,
    space: WorldSpaceId,
    leaves: &BTreeMap<CellCoord, Arc<TerrainPreviewLeaf>>,
    cancelled: &impl Fn() -> bool,
) -> Result<BTreeMap<TerrainNodeKey, Arc<TerrainNode>>> {
    let roots: BTreeSet<_> = reader
        .read_terrain_roots(space)?
        .into_iter()
        .map(|d| d.key)
        .collect();
    let mut nodes = BTreeMap::new();
    let mut changed = BTreeSet::new();

    for (&cell, leaf) in leaves {
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
            &leaf.page.heightfield,
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
    leaves: &BTreeMap<CellCoord, Arc<TerrainPreviewLeaf>>,
    cancelled: &impl Fn() -> bool,
) -> Result<BTreeMap<TerrainNodeKey, Arc<Vec<u32>>>> {
    let mut controls = BTreeMap::new();
    let mut changed = BTreeSet::new();

    for (&cell, leaf) in leaves {
        let key = TerrainNodeKey::leaf(space, cell);
        controls.insert(key, Arc::new(leaf.control.clone()));
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
