//! Build terrain products from finalized leaves in an unpublished runtime generation.
//! Nodes are read and written here in key order and prepared on every core; batches bound
//! the sample data held at once. The whole-document path is retained for reference fixtures.
use crate::parallel;
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use world::{
    CellCoord, ControlNeighbourhood, MAX_TERRAIN_NODE_LEVEL, PagePayload, SoftGround,
    TerrainHeightfield, TerrainNode, TerrainNodeKey, WorldSpaceId, control_neighbourhood,
    soft_layer_neighbours,
};

use world_db::{
    EncodedPage, EncodedTerrainNode, MAX_TERRAIN_NODE_QUERY, PreparedTerrainNode, RuntimeManifest,
    TerrainCookStore, WorldSpaceRecord,
};

/// Cells per world: cooked again, or whose leaf is to be built again.
pub(super) type ChangedCells = BTreeMap<WorldSpaceId, BTreeSet<CellCoord>>;

/// A cell's leaf node: its ground page's heights on the hierarchy's grid, under the soft layer
/// the control tiles around it lay.
pub(crate) fn leaf_node(
    key: TerrainNodeKey,
    field: &TerrainHeightfield,
    resolution: u16,
    tiles: &ControlNeighbourhood,
    soft: &SoftGround,
) -> Result<TerrainNode> {
    let node = TerrainNode::leaf(key, field, resolution)?;
    Ok(
        match soft.layer(tiles, resolution).map_err(anyhow::Error::msg)? {
            Some(layer) => node.with_soft_layer(layer)?,
            None => node,
        },
    )
}

/// Builds the hierarchy into empty staging, or with `changed` updates the copied hierarchy
/// of a publication with the same terrain cells: only changed leaves and their ancestors.
pub(super) fn cook_hierarchy(
    staging_path: &Path,
    spaces: &[WorldSpaceRecord],
    changed: Option<&ChangedCells>,
) -> Result<RuntimeManifest> {
    let store = match changed {
        Some(_) => TerrainCookStore::open_incremental(staging_path)?,
        None => TerrainCookStore::open_staging(staging_path)?,
    };

    for space in spaces {
        // A world uses one hierarchy grid. In particular, the compiler's 2x2 flat
        // optimization must not create incompatible topology next to detailed relief.
        let resolution = store.grid_resolution(space.id)?;
        let leaves = Leaves {
            store: &store,
            space: space.id,
            resolution,
            soft: SoftGround {
                layer_depths: store.layer_soft_depths(space.id)?,
                cell_size: space.cell_size,
            },
        };
        let Some(changed) = changed else {
            build(&leaves)?;
            continue;
        };
        let cells = if store
            .stored_resolution(space.id)?
            .is_some_and(|stored| stored != resolution)
        {
            // Every leaf is resampled to the new grid.
            leaf_cells(&store, space.id)?
        } else {
            changed.get(&space.id).cloned().unwrap_or_default()
        };
        update(&leaves, &cells)?;
    }

    Ok(store.finish(spaces)?)
}

/// What a world's leaves are built from.
struct Leaves<'a> {
    store: &'a TerrainCookStore,
    space: WorldSpaceId,
    resolution: u16,
    soft: SoftGround,
}

impl Leaves<'_> {
    /// The leaves of `pages`, built on every core.
    fn prepare(&self, pages: &[EncodedPage]) -> Result<Vec<Result<PreparedTerrainNode>>> {
        // The control tiles their soft layer reads: theirs and their neighbours'.
        let mut tiles = HashMap::new();

        if self.soft.has_soft_ground() {
            let cells: BTreeSet<_> = pages
                .iter()
                .flat_map(|p| soft_layer_neighbours(p.key.cell))
                .collect();
            let keys: Vec<_> = cells
                .iter()
                .map(|&c| TerrainNodeKey::leaf(self.space, c))
                .collect();

            for (cell, tile) in cells.into_iter().zip(self.store.controls(&keys)?) {
                if let Some(tile) = tile {
                    tiles.insert(cell, tile);
                }
            }
        }

        let (resolution, soft) = (self.resolution, &self.soft);
        Ok(parallel::map(pages, |page| {
            let key = TerrainNodeKey::leaf(page.key.space, page.key.cell);
            let PagePayload::TerrainHeightfield(terrain) = page.clone().decode()?.payload else {
                bail!("terrain hierarchy requires finalized heightfield pages at {key:?}");
            };
            let around = control_neighbourhood(page.key.cell, |c| tiles.get(&c).map(Vec::as_slice));
            let node = leaf_node(key, &terrain.heightfield, resolution, &around, soft)?;
            Ok(PreparedTerrainNode::new(&node)?)
        }))
    }
}

fn build(leaves: &Leaves) -> Result<()> {
    let (store, space) = (leaves.store, leaves.space);
    let mut cursor = None;

    loop {
        let pages = store.leaf_pages(space, cursor, MAX_TERRAIN_NODE_QUERY)?;
        let Some(last) = pages.last() else {
            break;
        };
        cursor = Some(last.key.cell);

        for node in leaves.prepare(&pages)? {
            store.write(&node?, false)?;
        }
    }

    for level in 0..MAX_TERRAIN_NODE_LEVEL {
        let mut cursor = None;
        let mut count = 0_u64;
        let mut at_origin = true;

        // Check whether another level can merge anything. Four quadrants around
        // zero never share a parent; one remaining node cannot merge either.
        loop {
            let keys = store.keys(space, level, cursor)?;

            if keys.is_empty() {
                break;
            }

            count += keys.len() as u64;
            at_origin &= keys
                .iter()
                .all(|k| (-1..=0).contains(&k.x) && (-1..=0).contains(&k.z));
            cursor = keys.last().map(|k| CellCoord { x: k.x, z: k.z });
        }

        if count <= 1 || at_origin {
            break;
        }

        cursor = None;

        loop {
            let keys = store.keys(space, level, cursor)?;

            if keys.is_empty() {
                break;
            }

            let mut parents = BTreeSet::new();

            for key in &keys {
                let parent = key.parent()?.context("terrain hierarchy level limit")?;
                // The keyset page can split siblings. A DB existence check keeps
                // this bounded without retaining a world-sized set of parents.
                if !store.contains(parent)? {
                    parents.insert(parent);
                }
            }

            write_parents(store, &parents, false)?;
            cursor = keys.last().map(|k| CellCoord { x: k.x, z: k.z });
        }
    }

    Ok(())
}

/// Replaces the leaves of `cells`, then each level of their ancestors. The leaf set is
/// unchanged, so the tree keeps its shape and only existing ancestors are rebuilt.
fn update(leaves: &Leaves, cells: &BTreeSet<CellCoord>) -> Result<()> {
    let (store, space) = (leaves.store, leaves.space);
    let cells: Vec<_> = cells.iter().copied().collect();

    for batch in cells.chunks(MAX_TERRAIN_NODE_QUERY) {
        let pages = batch
            .iter()
            .map(|&cell| {
                store
                    .leaf_page(space, cell)?
                    .with_context(|| format!("changed terrain cell {cell:?} has no ground page"))
            })
            .collect::<Result<Vec<_>>>()?;
        for node in leaves.prepare(&pages)? {
            store.write(&node?, true)?;
        }
    }

    let mut level: BTreeSet<_> = cells
        .iter()
        .map(|&cell| TerrainNodeKey::leaf(space, cell))
        .collect();

    while !level.is_empty() {
        let mut parents = BTreeSet::new();

        for key in &level {
            if let Some(parent) = key.parent()?
                && store.contains(parent)?
            {
                parents.insert(parent);
            }
        }

        write_parents(store, &parents, true)?;
        level = parents;
    }

    Ok(())
}

/// Builds `parents` from their stored children, which are loaded per batch.
fn write_parents(
    store: &TerrainCookStore,
    parents: &BTreeSet<TerrainNodeKey>,
    replace: bool,
) -> Result<()> {
    let parents: Vec<_> = parents.iter().copied().collect();

    for batch in parents.chunks(MAX_TERRAIN_NODE_QUERY) {
        let children = batch
            .iter()
            .map(|&key| Ok((key, store.children(key)?)))
            .collect::<Result<Vec<_>>>()?;
        let nodes = parallel::map(&children, |(key, children)| -> Result<_> {
            let children = children
                .iter()
                .map(|child| child.clone().map(EncodedTerrainNode::decode).transpose())
                .collect::<Result<Vec<_>, _>>()?;
            let node = TerrainNode::parent(*key, std::array::from_fn(|i| children[i].as_ref()))
                .with_context(|| format!("could not build terrain parent {key:?}"))?;
            Ok(PreparedTerrainNode::new(&node)?)
        });

        for node in nodes {
            store.write(&node?, replace)?;
        }
    }

    Ok(())
}

fn leaf_cells(store: &TerrainCookStore, space: WorldSpaceId) -> Result<BTreeSet<CellCoord>> {
    let mut cells = BTreeSet::new();
    let mut cursor = None;

    loop {
        let keys = store.keys(space, 0, cursor)?;
        let Some(last) = keys.last() else {
            return Ok(cells);
        };
        cursor = Some(CellCoord {
            x: last.x,
            z: last.z,
        });
        cells.extend(keys.iter().map(|k| CellCoord { x: k.x, z: k.z }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TestDirectory, cook_document, decoded_pages, demo_project_document, road_demo};
    use world_db::{RoadSourceRecord, RuntimeReader};

    #[test]
    fn flat_world_hierarchy_keeps_midpoints_required_by_stitching() {
        let directory = TestDirectory::new("flat-hierarchy");
        let reader = directory.cook(&demo_project_document());
        let roots = reader.read_terrain_roots(WorldSpaceId(2)).unwrap();
        assert!(!roots.is_empty());

        for root in roots {
            assert_eq!(root.resolution, Some(3));
        }
    }

    #[test]
    fn mountain_fixture_has_deterministic_cover_and_nested_bounds() {
        let mut project = demo_project_document();
        let space = project.default_world_space;
        project
            .world_spaces
            .iter_mut()
            .find(|s| s.id == space)
            .unwrap()
            .maximum_y = 1600.0;

        // Existing 512m/256-cell fixture, with no increase in source working set.
        // Evaluate shared coordinates identically, including the negative quadrants.
        for cell in &mut project.terrain_cell_heightfields {
            let n = usize::from(cell.resolution);
            for z in 0..n {
                for x in 0..n {
                    let wx = cell.cell.x as f32 * 32.0 + x as f32;
                    let wz = cell.cell.z as f32 * 32.0 + z as f32;
                    cell.heights[z * n + x] =
                        100.0 + 1400.0 * (-(wx / 55.0).powi(2) - (wz / 180.0).powi(2)).exp();
                }
            }
        }

        let directory = TestDirectory::new("mountain-hierarchy");
        let reader = directory.cook(&project);
        let roots = reader.read_terrain_roots(space).unwrap();
        assert_eq!(roots.len(), 4);
        assert!(
            roots
                .iter()
                .all(|r| r.key.level == 3 && r.resolution == Some(33))
        );
        assert_eq!(
            roots
                .iter()
                .map(|r| 4_u64.pow(u32::from(r.key.level)))
                .sum::<u64>(),
            256
        );
        assert_eq!(
            roots
                .iter()
                .map(|r| r.height_bounds[1])
                .fold(f32::NEG_INFINITY, f32::max),
            1500.0
        );

        // Every child's height bounds lie within its parent's, down to the leaves.
        fn check(reader: &RuntimeReader, key: TerrainNodeKey) {
            let node = reader
                .read_terrain_node(key)
                .unwrap()
                .unwrap()
                .decode()
                .unwrap();
            if let Some(keys) = key.children().unwrap() {
                for key in keys {
                    let child = reader
                        .read_terrain_node(key)
                        .unwrap()
                        .unwrap()
                        .decode()
                        .unwrap();
                    assert!(
                        child.height_bounds[0] >= node.height_bounds[0]
                            && child.height_bounds[1] <= node.height_bounds[1]
                    );
                    check(reader, key);
                }
            }
        }
        for root in &roots {
            check(&reader, root.key);
        }

        assert_eq!(
            directory.cook(&project).manifest().content_hash,
            reader.manifest().content_hash
        );
    }

    #[test]
    fn hierarchy_preserves_final_road_relief_and_failed_publication_keeps_old_generation() {
        let mut project = road_demo::document();

        for space in &mut project.world_spaces {
            space.minimum_y = -500.0;
            space.maximum_y = 2500.0;
        }

        for cell in &mut project.terrain_cell_heightfields {
            cell.heights.fill(1500.0);
        }

        for record in &mut project.roads.records {
            if let RoadSourceRecord::Profile(profile) = record {
                profile.relief.road_depth = 0.01;
                profile.relief.track_depth = 0.005;
            }
        }

        let directory = TestDirectory::new("road-hierarchy");
        let runtime = cook_document(&project, &directory.0).unwrap();
        let reader = RuntimeReader::open_immutable(&runtime).unwrap();
        let manifest = reader.manifest().clone();
        let mut deformed = 0;

        for leaf in decoded_pages(&reader, manifest.default_world_space) {
            let PagePayload::TerrainHeightfield(page) = leaf.payload else {
                continue;
            };

            if page
                .heightfield
                .heights
                .iter()
                .any(|&y| y < 1500.0 && y > 1499.9)
            {
                deformed += 1;
            }

            let node = reader
                .read_terrain_node(TerrainNodeKey::leaf(leaf.key.space, leaf.key.cell))
                .unwrap()
                .unwrap()
                .decode()
                .unwrap();
            let field = node.heightfield.unwrap();
            let ratio = usize::from((field.resolution - 1) / (page.heightfield.resolution - 1));

            for z in 0..usize::from(page.heightfield.resolution) {
                for x in 0..usize::from(page.heightfield.resolution) {
                    assert_eq!(
                        field.height_at(x * ratio, z * ratio).to_bits(),
                        page.heightfield.height_at(x, z).to_bits()
                    );
                }
            }
        }

        assert!(
            deformed > 0,
            "fixture must actually deform the mountain-height road"
        );
        drop(reader);

        // Corrupt a terrain page of a staged copy whose hierarchy is to be built again,
        // after a good generation already exists.
        let staging = directory.0.join("staging.sqlite");
        std::fs::copy(&runtime, &staging).unwrap();
        rusqlite::Connection::open(&staging)
            .unwrap()
            .execute_batch(
                "DELETE FROM terrain_roots; DELETE FROM terrain_nodes; \
                 DELETE FROM terrain_hierarchy_spaces; \
                 UPDATE cell_pages SET payload = substr(payload, 2) \
                 WHERE (world_space_id, cell_x, cell_z, domain) = \
                     (SELECT world_space_id, cell_x, cell_z, domain FROM cell_pages \
                      WHERE domain = 1 LIMIT 1);",
            )
            .unwrap();
        assert!(
            crate::publish_staged_runtime(&runtime, &staging, &manifest.world_spaces, None)
                .is_err()
        );
        let reader = RuntimeReader::open_immutable(&runtime).unwrap();
        assert_eq!(reader.manifest().content_hash, manifest.content_hash);
        assert!(
            !reader
                .read_terrain_roots(manifest.default_world_space)
                .unwrap()
                .is_empty()
        );
    }
}
