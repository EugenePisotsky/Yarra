//! Build terrain products from finalized leaves in an unpublished runtime generation.
//! This pass has bounded sample memory, as does the production source/environment
//! pass in streaming_cook. The whole-document path is retained for reference fixtures.
use anyhow::{Context, Result, bail};
use std::path::Path;
use world::{CellCoord, MAX_TERRAIN_NODE_LEVEL, PagePayload, TerrainNodeKey, WorldSpaceId};
use world_db::{RuntimeManifest, TerrainCookStore, TerrainHierarchySummary, WorldSpaceRecord};

pub(super) fn cook_hierarchy(
    staging_path: &Path,
    spaces: &[WorldSpaceRecord],
) -> Result<(RuntimeManifest, Vec<TerrainHierarchySummary>)> {
    let store = TerrainCookStore::open_staging(staging_path)?;
    for space in spaces {
        // A world uses one hierarchy grid. In particular, the compiler's 2x2 flat
        // optimization must not create incompatible topology next to detailed relief.
        let mut resolution = 2;
        visit_leaves(&store, space.id, |_, field| {
            resolution = resolution.max(field.resolution);
            Ok(())
        })?;
        visit_leaves(&store, space.id, |key, field| {
            store.insert_leaf(key, &field, resolution)?;
            Ok(())
        })?;

        for level in 0..MAX_TERRAIN_NODE_LEVEL {
            let mut cursor = None;
            let mut count = 0_u64;
            let mut at_origin = true;
            // Check whether another level can merge anything. Four quadrants around
            // zero never share a parent; one remaining node cannot merge either.
            loop {
                let keys = store.keys(space.id, level, cursor)?;
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
                let keys = store.keys(space.id, level, cursor)?;
                if keys.is_empty() {
                    break;
                }
                for key in &keys {
                    let parent = key.parent()?.context("terrain hierarchy level limit")?;
                    // The keyset page can split siblings. A DB existence check keeps
                    // this bounded without retaining a world-sized set of parents.
                    if !store.contains(parent)? {
                        store.build_parent(parent).with_context(|| {
                            format!("could not build terrain parent {parent:?}")
                        })?;
                    }
                }
                cursor = keys.last().map(|k| CellCoord { x: k.x, z: k.z });
            }
        }
    }
    Ok(store.finish(spaces)?)
}

fn visit_leaves(
    store: &TerrainCookStore,
    space: WorldSpaceId,
    mut visit: impl FnMut(TerrainNodeKey, world::TerrainHeightfield) -> Result<()>,
) -> Result<()> {
    let mut cursor = None;
    loop {
        let pages = store.leaf_pages(space, cursor)?;
        if pages.is_empty() {
            break;
        }
        for page in pages {
            cursor = Some(page.key.cell);
            let key = TerrainNodeKey::leaf(space, page.key.cell);
            let PagePayload::TerrainHeightfield(terrain) = page.decode()?.payload else {
                bail!("terrain hierarchy requires finalized heightfield pages at {key:?}");
            };
            visit(key, terrain.heightfield)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_runtime, demo_project_document, publish_runtime_database, road_demo};
    use world_db::{RoadSourceRecord, RuntimeReader};

    struct Output(std::path::PathBuf);
    impl Output {
        fn new() -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "yarra-terrain-cook-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join("runtime.sqlite")
        }
    }
    impl Drop for Output {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn mountain_fixture_has_deterministic_cover_and_conservative_bounds() {
        let output = Output::new();
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
        let build = build_runtime(project).unwrap();
        let first = publish_runtime_database(&output.path(), &build).unwrap();
        let reader = RuntimeReader::open_immutable(&output.path()).unwrap();
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
        assert!(roots.iter().all(|r| r.geometric_error > 0.0));
        // Descend to every leaf, checking that the advertised parent error bounds
        // differences even where the coarse grid skips fine mountain samples.
        fn check(reader: &RuntimeReader, key: TerrainNodeKey) {
            let node = reader
                .read_terrain_node(key)
                .unwrap()
                .unwrap()
                .decode()
                .unwrap();
            if let Some(keys) = key.children().unwrap() {
                for (i, key) in keys.into_iter().enumerate() {
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
                    let field = child.heightfield.as_ref().unwrap();
                    let n = usize::from(field.resolution);
                    for z in 0..n {
                        for x in 0..n {
                            let p = [
                                (i % 2) as f32 * 0.5 + x as f32 / (n - 1) as f32 * 0.5,
                                (i / 2) as f32 * 0.5 + z as f32 / (n - 1) as f32 * 0.5,
                            ];
                            let delta = (node.heightfield.as_ref().unwrap().sample(p, 1.0).height
                                - field.height_at(x, z))
                            .abs();
                            assert!(delta + child.geometric_error <= node.geometric_error + 0.001);
                        }
                    }
                    check(reader, key);
                }
            }
        }
        for root in &roots {
            check(&reader, root.key);
        }
        let bytes: u64 = roots.iter().map(|r| r.gpu_bytes_estimate).sum();
        eprintln!(
            "512m mountain fixture: 256 leaves -> {} coarse roots, {} coarse triangles, {bytes} estimated geometry bytes",
            roots.len(),
            roots.len() * 32 * 32 * 2
        );
        drop(reader);
        let second = publish_runtime_database(&output.path(), &build).unwrap();
        assert_eq!(first.content_hash, second.content_hash);
    }

    #[test]
    fn hierarchy_preserves_final_road_relief_and_failed_publication_keeps_old_generation() {
        let output = Output::new();
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
        let mut build = build_runtime(project).unwrap();
        let manifest = publish_runtime_database(&output.path(), &build).unwrap();
        let reader = RuntimeReader::open_immutable(&output.path()).unwrap();
        let mut deformed = 0;
        for page in &build.pages {
            if page.key.domain != world::PageDomain::TerrainRender {
                continue;
            }
            let PagePayload::TerrainHeightfield(page) = page.clone().decode().unwrap().payload
            else {
                panic!()
            };
            if page
                .heightfield
                .heights
                .iter()
                .any(|&y| y < 1500.0 && y > 1499.9)
            {
                deformed += 1;
            }
        }
        for leaf in build
            .pages
            .iter()
            .filter(|p| p.key.domain == world::PageDomain::TerrainRender)
        {
            let PagePayload::TerrainHeightfield(page) = leaf.clone().decode().unwrap().payload
            else {
                panic!()
            };
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
        // Corrupt a staged terrain page, after a good generation already exists.
        build
            .pages
            .iter_mut()
            .find(|p| p.key.domain == world::PageDomain::TerrainRender)
            .unwrap()
            .payload
            .pop();
        assert!(publish_runtime_database(&output.path(), &build).is_err());
        let reader = RuntimeReader::open_immutable(&output.path()).unwrap();
        assert_eq!(reader.manifest().content_hash, manifest.content_hash);
        assert!(
            !reader
                .read_terrain_roots(manifest.default_world_space)
                .unwrap()
                .is_empty()
        );
    }
}
