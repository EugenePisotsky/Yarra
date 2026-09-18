use super::*;
use crate::{
    editing::{EditorHistory, EditorObjectWorkingSet},
    environment_paint::preview::compile_source_cells_with_roads,
};
use environment::*;
use world::{CellCoord, PageDomain, PagePayload};
use world_db::*;

#[test]
fn collection_preview_save_recovery_undo_and_cook_preserve_manual_objects() {
    let path =
        std::env::temp_dir().join(format!("yarra-collections-{}.sqlite", uuid::Uuid::new_v4()));
    world_cook::create_road_demo_project(&path).unwrap();
    let reader = ProjectReader::open_read_only(&path).unwrap();
    let project = read_project_database(&path).unwrap();
    let space = project.default_world_space;
    let definition = project
        .environments
        .iter()
        .find(|d| d.space == space)
        .unwrap();
    let old_root = definition.layers[0].preset;
    let original = project.presets.clone();
    let mut library = original.clone();
    let tree = PresetId([177; 16]);
    library.presets.push(Preset {
        id: tree,
        revision: 1,
        name: "Tree collection".into(),
        kind: PresetKind::AssetCollection(AssetCollection {
            id: OutputId([178; 16]),
            channel: ASSET_CHANNEL,
            assets: vec![CollectionAsset {
                asset: project.assets[0].id,
                weight: 1.0,
                scale_min: 0.8,
                scale_max: 1.2,
            }],
            density: 1.0,
            spacing: 1.5,
            seed: 1,
            max_slope_degrees: 60.0,
            road_clearance: 0.5,
        }),
    });
    // Existing meadow becomes a woodland composition; masks and manual objects stay intact.
    let root = library
        .presets
        .iter_mut()
        .find(|p| p.id == old_root)
        .unwrap();
    let PresetKind::Composition(children) = &mut root.kind else {
        panic!()
    };
    children.push(PresetUse {
        id: PresetUseId([179; 16]),
        name: "Trees".into(),
        preset: tree,
        overrides: vec![],
    });
    let mut dense = DenseDomainWorkingSets::default();
    dense.initialize_presets(&original);
    dense.plants = project.vegetation_catalog.clone();
    let mut history = EditorHistory::default();
    let mut manual = EditorObjectWorkingSet::default();
    history.edit_presets(&mut dense, &library).unwrap();
    assert!(history.undo(&mut manual, &mut dense));
    assert_eq!(dense.presets(), Some(&original));
    assert!(history.redo(&mut manual, &mut dense));
    let snapshot = dense.dirty_preset_snapshot().unwrap();
    let encoded = ron::to_string(&snapshot).unwrap();
    let mut recovered = DenseDomainWorkingSets::default();
    assert!(recovered.restore_preset_snapshot(
        ron::from_str(&encoded).unwrap(),
        project.vegetation_catalog.as_ref().unwrap()
    ));
    assert_eq!(recovered.presets(), dense.presets());
    let cells = [CellCoord::ZERO, CellCoord { x: 1, z: 0 }];
    let compile = |presets: &PresetLibrary, revision| {
        let plan = environment_compile::CompilePlan::new(
            definition,
            project.vegetation_catalog.as_ref().unwrap(),
            presets,
            Default::default(),
        )
        .unwrap();
        compile_source_cells_with_roads(
            &reader,
            &plan,
            space,
            definition.revision,
            revision,
            &cells,
            &[],
            &[],
        )
        .unwrap()
    };
    let before = compile(dense.presets().unwrap(), original.revision);
    assert!(before.iter().any(|c| !c.objects.is_empty()));
    let views = reader
        .read_collection_assets(&[project.assets[0].id])
        .unwrap();
    assert!(!views[0].variants.is_empty());
    let mut writer = ProjectWriter::open(&path).unwrap();
    let EnvironmentSourceWriteResult::Committed(commit) = writer
        .apply_environment_and_roads_transaction(
            original.revision,
            Some(dense.presets().unwrap()),
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap()
    else {
        panic!()
    };
    dense.finish_presets(
        1,
        &crate::project_store::DenseSaveOutcome::Committed(commit),
    );
    let saved = read_project_database(&path).unwrap();
    assert_eq!(saved.objects, project.objects);
    assert_eq!(
        saved.environment_cells.len(),
        project.environment_cells.len()
    );
    let after = compile(&saved.presets, saved.presets.revision);
    for (a, b) in before.iter().zip(&after) {
        assert_eq!(a.objects, b.objects);
    }
    let build = world_cook::build_runtime(saved.clone()).unwrap();
    for cell in &after {
        let page = build
            .pages
            .iter()
            .find(|p| {
                p.key.space == space
                    && p.key.cell == cell.cell
                    && p.key.domain == PageDomain::StaticObjects
            })
            .unwrap()
            .clone()
            .decode()
            .unwrap();
        let PagePayload::StaticObjects(page) = page.payload else {
            panic!()
        };
        assert_eq!(
            page.instances
                .iter()
                .filter(|o| o.generated)
                .cloned()
                .collect::<Vec<_>>(),
            cell.objects
        );
        for o in page.instances.iter().filter(|o| !o.generated) {
            assert!(project.objects.iter().any(|m| m.id == o.id));
        }
        assert!(build.dependencies.iter().any(|d| d.page.space == space
            && d.page.cell == cell.cell
            && d.asset == project.assets[0].id));
        let cell_bounds = build
            .cells
            .iter()
            .find(|c| c.space == space && c.cell == cell.cell)
            .unwrap();
        for o in &cell.objects {
            assert!(
                cell_bounds.maximum_y
                    >= o.translation[1] + views[0].variants[0].bounds[1] * o.scale
            );
        }
    }
    assert!(history.undo(&mut manual, &mut dense));
    assert!(
        compile(dense.presets().unwrap(), saved.presets.revision)
            .iter()
            .all(|c| c.objects.is_empty())
    );
    assert!(history.redo(&mut manual, &mut dense));
    assert_eq!(
        compile(dense.presets().unwrap(), saved.presets.revision)[0].objects,
        after[0].objects
    );
    let mut invalid = saved.presets.clone();
    let PresetKind::AssetCollection(c) = &mut invalid
        .presets
        .iter_mut()
        .find(|p| p.id == tree)
        .unwrap()
        .kind
    else {
        panic!()
    };
    c.assets[0].asset = world::AssetId([255; 32]);
    assert!(
        writer
            .apply_environment_and_roads_transaction(
                saved.presets.revision,
                Some(&invalid),
                &[],
                &[],
                &[],
                &[]
            )
            .is_err()
    );
    assert_eq!(reader.read_environment_presets().unwrap(), saved.presets);
    assert!(
        reader
            .read_collection_assets(&vec![project.assets[0].id; 257])
            .is_err()
    );
    drop(writer);
    drop(reader);
    let _ = std::fs::remove_file(path);
}
