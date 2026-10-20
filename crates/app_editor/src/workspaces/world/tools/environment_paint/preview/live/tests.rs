use super::*;
use std::path::PathBuf;

/// A disposable road world, not a GPU or the user's world.
#[test]
fn live_road_edit_move_save_undo_and_cancel() {
    let folder = std::env::temp_dir().join(format!("yarra-editor-live-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(folder.clone());
    let source = folder.join("source.sqlite");
    let runtime = folder.join("runtime.sqlite");
    world_cook::create_road_demo_project(&source).unwrap();
    let mut document = world_db::read_project_database(&source).unwrap();
    let manifest = world_cook::cook_project(&source, &runtime).unwrap();
    let space = document.default_world_space;
    let definition = document
        .environments
        .iter()
        .find(|d| d.space == space)
        .unwrap();
    let plan = CompilePlan::new(
        definition,
        document.vegetation_catalog.as_ref().unwrap(),
        &document.presets,
        Default::default(),
    )
    .unwrap();
    let reader = RuntimeReader::open_immutable(&runtime).unwrap();
    let catalog = plan.catalog().clone();
    let mut job = Input {
        revision: 1,
        context: (space, 1, 1, 1, manifest.generation_id.clone()),
        plan: Arc::new(plan),
        catalog,
        definition_revision: definition.revision,
        library_revision: document.presets.revision,
        records: vec![],
        roads: vec![],
        regions: Some(vec![]),
        resident: (-3..=3)
            .flat_map(|x| (-3..=3).map(move |z| CellCoord { x, z }))
            .collect(),
        global: false,
        base: None,
    };
    let token = AtomicU64::new(1);
    let baseline = compile_inner(&source, &runtime, &job, &token).unwrap();
    assert!(
        baseline.products.leaves.is_empty(),
        "published and compiled source must agree initially"
    );
    let original = document
        .roads
        .records
        .iter()
        .find_map(|r| {
            if let world_db::RoadSourceRecord::Profile(p) = r {
                Some(p.clone())
            } else {
                None
            }
        })
        .unwrap();
    let mut profile = original.clone();
    profile.relief.road_depth = 0.25;
    profile.relief.track_depth = 0.1;
    let change = |p: environment::roads::CartTrackProfile| {
        crate::workspaces::world::tools::road_authoring::working::RoadChange {
            key: world_db::RoadRecordKey::Profile(p.id),
            record: Some(world_db::RoadSourceRecord::Profile(p)),
        }
    };
    job.base = Some(baseline);
    job.roads = vec![change(profile.clone())];
    job.regions = None;
    let edited = compile_inner(&source, &runtime, &job, &token).unwrap();
    assert!(!edited.products.nodes.is_empty());
    assert!(!edited.products.controls.is_empty());
    assert!(!edited.products.leaves.is_empty());
    let edited_leaves = edited.products.leaves.clone();

    // A preview of the published pages reproduces the published control tiles, roads included.
    let cell = *edited_leaves.keys().next().unwrap();
    let page = world_cook::published_terrain_leaf(&reader, space, cell)
        .unwrap()
        .unwrap();
    let same = world_cook::preview_terrain(
        &reader,
        space,
        BTreeMap::from([(cell, Arc::new(page))]),
        || false,
    )
    .unwrap();
    assert!(same.nodes.is_empty());
    assert_eq!(
        same.controls.len(),
        usize::from(world::CONTROL_TOP_LEVEL) + 1
    );

    for (key, tile) in &same.controls {
        assert_eq!(
            Some(tile.to_vec()),
            world_cook::published_terrain_control(&reader, *key).unwrap()
        );
    }

    // Leaving the road source neighborhood retains the same hierarchy products.
    job.base = Some(edited.clone());
    job.resident = vec![CellCoord { x: 6, z: 6 }];
    let moved = compile_inner(&source, &runtime, &job, &token).unwrap();
    assert!(Arc::ptr_eq(&edited.products, &moved.products));

    // Save changes the source baseline, not the runtime publication or visible edit.
    for r in &mut document.roads.records {
        if matches!(r,world_db::RoadSourceRecord::Profile(p) if p.id==profile.id) {
            *r = world_db::RoadSourceRecord::Profile(profile.clone());
        }
    }

    std::fs::remove_file(&source).unwrap();
    world_db::write_project_database(&source, &document).unwrap();
    job.base = Some(moved);
    job.roads.clear();
    job.regions = Some(vec![]);
    job.context.3 += 1;
    let saved = compile_inner(&source, &runtime, &job, &token).unwrap();
    assert_eq!(saved.products.leaves, edited_leaves);
    assert_eq!(reader.manifest().generation_id, manifest.generation_id);

    // Unsaved undo must override the newly saved source as well as the live products.
    job.base = Some(saved);
    job.roads = vec![change(original)];
    job.regions = None;
    let undone = compile_inner(&source, &runtime, &job, &token).unwrap();
    assert!(undone.products.leaves.is_empty());

    for (key, node) in &undone.products.nodes {
        assert_eq!(
            **node,
            reader
                .read_terrain_node(*key)
                .unwrap()
                .unwrap()
                .decode()
                .unwrap()
        );
    }

    assert!(!undone.products.controls.is_empty());

    for (key, tile) in &undone.products.controls {
        assert_eq!(
            Some(tile.to_vec()),
            world_cook::published_terrain_control(&reader, *key).unwrap()
        );
    }

    token.store(2, Ordering::Relaxed);
    assert!(
        compile_inner(&source, &runtime, &job, &token)
            .err()
            .unwrap()
            .to_string()
            .contains("superseded")
    );
}
