use super::*;
use crate::{WorldSpaceRecord, write_project_database};
use environment::{
    ChannelId, Layer, LayerId, OutputId, Preset, PresetId, PresetKind, VegetationBlend,
    VegetationTreatment,
};
use world::TerrainSurface;

struct Fixture {
    directory: std::path::PathBuf,
    path: std::path::PathBuf,
    definition: EnvironmentDefinition,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "yarra-environment-db-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("project.sqlite");
        let surface = world::TerrainSurfaceId([1; 16]);
        let definition = EnvironmentDefinition {
            space: WorldSpaceId(1),
            revision: 1,
            cell_size: 32.0,
            mask_resolution: 3,
            surfaces: vec![surface],
            base_surface: surface,
            layers: vec![Layer {
                id: LayerId([1; 16]),
                revision: 1,
                name: "Meadow".into(),
                preset: PresetId([1; 16]),
                order: 0,
                seed: 42,
                enabled: true,
                opacity: 1.0,
                overrides: vec![],
            }],
        };
        let presets = PresetLibrary {
            revision: 1,
            presets: vec![Preset {
                id: PresetId([1; 16]),
                revision: 1,
                name: "Meadow".into(),
                kind: PresetKind::Foliage(VegetationTreatment {
                    id: OutputId([1; 16]),
                    channel: ChannelId([1; 16]),
                    assemblage: vegetation::fixtures::DRY_FIELD_ASSEMBLAGE_ID,
                    blend: VegetationBlend::Replace,
                    strength: 1.0,
                    density: 1.0,
                    seed: 0,
                }),
            }],
        };
        let document = ProjectDocument {
            presets,
            default_world_space: definition.space,
            world_spaces: vec![WorldSpaceRecord {
                id: definition.space,
                name: "test".into(),
                cell_size: 32.0,
                minimum_y: 0.0,
                maximum_y: 0.0,
            }],
            vegetation_catalog: Some(vegetation::fixtures::reference_catalog()),
            cells: Vec::new(),
            terrain_surfaces: vec![TerrainSurface {
                id: surface,
                key: "soil".into(),
                display_name: "Soil".into(),
                tile_size: 1.0,
                anti_tiling: false,
                normal_y_sign: 1.0,
                normal_strength: 1.0,
                roughness_min: 0.0,
                roughness_max: 1.0,
            }],
            terrain_texture_sets: Vec::new(),
            terrain_texture_layers: Vec::new(),
            terrain_profiles: Vec::new(),
            terrain_cell_heightfields: Vec::new(),
            environments: vec![definition.clone()],
            environment_cells: Vec::new(),
            roads: Default::default(),
            assets: Vec::new(),
            asset_variants: Vec::new(),
            definitions: Vec::new(),
            objects: Vec::new(),
        };
        write_project_database(&path, &document).unwrap();
        Self {
            directory,
            path,
            definition,
        }
    }
    fn reader(&self) -> ProjectReader {
        ProjectReader::open_read_only(&self.path).unwrap()
    }
    fn writer(&self) -> ProjectWriter {
        ProjectWriter::open(&self.path).unwrap()
    }
    fn cell(&self, x: i32) -> SourceEnvironmentCellRecord {
        SourceEnvironmentCellRecord {
            space: self.definition.space,
            cell: CellCoord { x, z: 0 },
            source_revision: 0,
            definition_revision: 1,
            tiles: vec![CoverageTile {
                layer: self.definition.layers[0].id,
                samples: vec![0, 0, 0, 0, 255, 0, 0, 0, 0],
            }],
        }
    }
    fn snapshot(&self, cells: &[CellCoord]) -> EnvironmentReadSnapshot {
        self.reader()
            .read_environment_snapshot(self.definition.space, cells)
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}
fn write(record: SourceEnvironmentCellRecord, revision: Option<i64>) -> DenseSourceWrite {
    DenseSourceWrite::EnvironmentCoverage {
        expected_source_revision: revision,
        record,
    }
}
fn committed(result: DenseSourceWriteTransactionResult) -> Vec<SourceEnvironmentCellRecord> {
    let DenseSourceWriteTransactionResult::Committed(records) = result else {
        panic!("expected a commit")
    };
    records
        .into_iter()
        .map(|record| {
            let DenseSourceRecord::EnvironmentCoverage(record) = record;
            record
        })
        .collect()
}

#[test]
fn creating_painting_and_removing_a_layer_are_atomic_source_commits() {
    let f = Fixture::new();
    let mut writer = f.writer();
    let mut added = f.definition.clone();
    let mut layer = added.layers[0].clone();
    layer.id = LayerId([2; 16]);
    layer.order = 1;
    added.layers.push(layer.clone());
    let mut paint = f.cell(0);
    paint.tiles[0].layer = layer.id;
    let edit = EnvironmentDefinitionWrite {
        expected_revision: Some(1),
        definition: added.clone(),
    };
    // A shared-edge error after both writes have been staged rolls everything back.
    let mut invalid = paint.clone();
    invalid.tiles[0].samples[5] = 255;
    assert!(
        writer
            .apply_environment_source_transaction(
                1,
                None,
                std::slice::from_ref(&edit),
                &[write(invalid, None)]
            )
            .is_err()
    );
    assert_eq!(f.snapshot(&[paint.cell]).definition, f.definition);
    assert_eq!(f.snapshot(&[paint.cell]).coverage.cells[0].revision, 0);
    let EnvironmentSourceWriteResult::Committed(commit) = writer
        .apply_environment_source_transaction(1, None, &[edit], &[write(paint.clone(), None)])
        .unwrap()
    else {
        panic!("expected commit")
    };
    assert_eq!(commit.definitions[0].revision, 2);
    let DenseSourceRecord::EnvironmentCoverage(mut saved) = commit.coverage[0].clone();
    assert_eq!(saved.definition_revision, 2);
    assert_eq!(saved.source_revision, 1);
    // A stale mask CAS must also roll back an otherwise valid definition edit.
    added.layers[1].name = "Renamed".into();
    let edit = EnvironmentDefinitionWrite {
        expected_revision: Some(2),
        definition: added,
    };
    assert!(matches!(
        writer
            .apply_environment_source_transaction(1, None, &[edit], &[write(saved.clone(), None)])
            .unwrap(),
        EnvironmentSourceWriteResult::CoverageConflict { .. }
    ));
    assert_eq!(f.snapshot(&[paint.cell]).definition, commit.definitions[0]);
    // Undo painting and layer creation together, after an earlier successful save.
    saved.tiles.clear();
    let edit = EnvironmentDefinitionWrite {
        expected_revision: Some(2),
        definition: f.definition.clone(),
    };
    let EnvironmentSourceWriteResult::Committed(removed) = writer
        .apply_environment_source_transaction(1, None, &[edit], &[write(saved, Some(1))])
        .unwrap()
    else {
        panic!("expected commit")
    };
    assert_eq!(removed.definitions[0].layers, f.definition.layers);
    let snapshot = f.snapshot(&[paint.cell]);
    assert!(snapshot.coverage.cells[0].tiles.is_empty());
    assert_eq!(snapshot.coverage.cells[0].revision, 2);
    assert_eq!(snapshot.definition.revision, 3);
    // A stale definition fails before touching any masks.
    let edit = EnvironmentDefinitionWrite {
        expected_revision: Some(1),
        definition: f.definition.clone(),
    };
    assert!(matches!(
        writer
            .apply_environment_source_transaction(1, None, &[edit], &[write(paint, None)])
            .unwrap(),
        EnvironmentSourceWriteResult::DefinitionConflict { .. }
    ));
    assert_eq!(f.snapshot(&[CellCoord::ZERO]).coverage.cells[0].revision, 2);
}

#[test]
fn bounded_snapshot_distinguishes_loaded_empty_from_unrequested_cells() {
    let f = Fixture::new();
    let halo = environment_dependency_cells(&[CellCoord::ZERO]).unwrap();
    let snapshot = f.snapshot(&halo);
    assert_eq!(snapshot.coverage.cells.len(), 9);
    assert!(
        snapshot
            .coverage
            .cells
            .iter()
            .all(|c| c.revision == 0 && c.tiles.is_empty())
    );
    let plan = CompilePlan::new(
        &snapshot.definition,
        &snapshot.vegetation_catalog,
        &snapshot.presets,
        Default::default(),
    )
    .unwrap();
    assert!(
        plan.compile_cells(&[CellCoord::ZERO], &snapshot.coverage)
            .unwrap()[0]
            .vegetation
            .fields
            .is_empty()
    );
    let reader = f.reader();
    assert!(
        reader
            .read_environment_snapshot(f.definition.space, &[CellCoord::ZERO, CellCoord::ZERO])
            .is_err()
    );
    let excessive: Vec<_> = (0..577).map(|x| CellCoord { x, z: 0 }).collect();
    assert!(
        reader
            .read_environment_snapshot(f.definition.space, &excessive)
            .is_err()
    );
    assert!(
        reader
            .read_environment_cells_in_cells(
                f.definition.space,
                CellCoord::ZERO,
                CellCoord::ZERO,
                577
            )
            .is_err()
    );
}

#[test]
fn shared_edge_edits_require_one_atomic_transaction_and_rollback_on_failure() {
    let f = Fixture::new();
    let mut writer = f.writer();
    let mut a = f.cell(0);
    a.tiles[0].samples[5] = 128;
    let mut b = f.cell(1);
    b.tiles[0].samples[3] = 128;
    assert!(
        writer
            .apply_dense_source_transaction(&[write(a.clone(), None)])
            .is_err()
    );
    assert_eq!(f.snapshot(&[a.cell]).coverage.cells[0].revision, 0);
    let records = committed(
        writer
            .apply_dense_source_transaction(&[write(a, None), write(b, None)])
            .unwrap(),
    );
    assert!(records.iter().all(|r| r.source_revision == 1));
    let mut broken = records[0].clone();
    broken.tiles[0].samples[5] = 255;
    assert!(
        writer
            .apply_dense_source_transaction(&[write(broken, Some(1))])
            .is_err()
    );
    let snapshot = f.snapshot(&[CellCoord::ZERO, CellCoord { x: 1, z: 0 }]);
    assert_eq!(snapshot.coverage.cells[0].tiles[0].samples[5], 128);
    assert!(snapshot.coverage.cells.iter().all(|c| c.revision == 1));
}

#[test]
fn competing_writer_conflict_rolls_back_the_entire_gesture() {
    let f = Fixture::new();
    let mut first = f.writer();
    let mut second = f.writer();
    let records = committed(
        first
            .apply_dense_source_transaction(&[write(f.cell(0), None), write(f.cell(1), None)])
            .unwrap(),
    );
    let mut changed = records[1].clone();
    changed.tiles[0].samples[4] = 100;
    committed(
        second
            .apply_dense_source_transaction(&[write(changed, Some(1))])
            .unwrap(),
    );
    let mut a = records[0].clone();
    a.tiles[0].samples[4] = 30;
    assert!(matches!(
        first
            .apply_dense_source_transaction(&[
                write(a, Some(1)),
                write(records[1].clone(), Some(1))
            ])
            .unwrap(),
        DenseSourceWriteTransactionResult::Conflict { .. }
    ));
    let snapshot = f.snapshot(&[CellCoord::ZERO, CellCoord { x: 1, z: 0 }]);
    assert_eq!(snapshot.coverage.cells[0].tiles[0].samples[4], 255);
    assert_eq!(snapshot.coverage.cells[0].revision, 1);
    assert_eq!(snapshot.coverage.cells[1].revision, 2);
}

#[test]
fn erase_keeps_revision_tombstones_and_rejects_stale_recreation() {
    let f = Fixture::new();
    let mut writer = f.writer();
    let mut record = committed(
        writer
            .apply_dense_source_transaction(&[write(f.cell(0), None)])
            .unwrap(),
    )
    .remove(0);
    record.tiles[0].samples.fill(0);
    let erased = committed(
        writer
            .apply_dense_source_transaction(&[write(record, Some(1))])
            .unwrap(),
    )
    .remove(0);
    assert_eq!(erased.source_revision, 2);
    assert!(erased.tiles.is_empty());
    assert!(matches!(
        writer
            .apply_dense_source_transaction(&[write(f.cell(0), None)])
            .unwrap(),
        DenseSourceWriteTransactionResult::Conflict { .. }
    ));
    assert_eq!(f.snapshot(&[CellCoord::ZERO]).coverage.cells[0].revision, 2);
}

#[test]
fn definition_edits_stamp_children_and_invalidate_older_mask_drafts() {
    let f = Fixture::new();
    let mut writer = f.writer();
    let record = committed(
        writer
            .apply_dense_source_transaction(&[write(f.cell(0), None)])
            .unwrap(),
    )
    .remove(0);
    let mut replacement = f.definition.clone();
    replacement.layers[0].opacity = 0.5;
    let EnvironmentDefinitionWriteResult::Committed(changed) = writer
        .replace_environment_definition_if_revision(Some(1), &replacement)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(changed.revision, 2);
    assert_eq!(changed.layers[0].revision, 2);
    assert_eq!(
        f.reader().read_environment_presets().unwrap().presets[0].revision,
        1
    );
    assert!(matches!(
        writer
            .replace_environment_definition_if_revision(Some(1), &replacement)
            .unwrap(),
        EnvironmentDefinitionWriteResult::Conflict { .. }
    ));
    assert!(matches!(
        writer
            .apply_dense_source_transaction(&[write(record, Some(1))])
            .unwrap(),
        DenseSourceWriteTransactionResult::Conflict { .. }
    ));
    assert_eq!(
        f.snapshot(&[CellCoord::ZERO]).definition.layers[0].opacity,
        0.5
    );
}

#[test]
fn removing_referenced_layers_grids_or_plant_assemblages_is_rejected() {
    let f = Fixture::new();
    let mut writer = f.writer();
    committed(
        writer
            .apply_dense_source_transaction(&[write(f.cell(0), None)])
            .unwrap(),
    );
    let mut missing_layer = f.definition.clone();
    missing_layer.layers.clear();
    assert!(
        writer
            .replace_environment_definition_if_revision(Some(1), &missing_layer)
            .is_err()
    );
    let mut grid = f.definition.clone();
    grid.mask_resolution = 5;
    assert!(
        writer
            .replace_environment_definition_if_revision(Some(1), &grid)
            .is_err()
    );
    let mut plants = vegetation::fixtures::reference_catalog();
    plants.assemblages.clear();
    assert!(writer.replace_vegetation_catalog(&plants).is_err());
    assert!(
        writer
            .replace_vegetation_catalog_if_matches(
                Some(&vegetation::fixtures::reference_catalog()),
                &plants
            )
            .is_err()
    );
    assert_eq!(
        f.reader().read_environment_definitions().unwrap(),
        vec![f.definition.clone()]
    );
}

#[test]
fn spatial_queries_are_truncated_explicitly_and_roundtrip_only_source_masks() {
    let f = Fixture::new();
    let mut writer = f.writer();
    committed(
        writer
            .apply_dense_source_transaction(&[write(f.cell(0), None), write(f.cell(1), None)])
            .unwrap(),
    );
    let query = f
        .reader()
        .read_environment_cells_in_cells(
            f.definition.space,
            CellCoord::ZERO,
            CellCoord { x: 10, z: 10 },
            1,
        )
        .unwrap();
    assert!(query.truncated);
    assert_eq!(query.records.len(), 1);
    let reader = f.reader();
    let first = reader
        .read_environment_layer_cells(f.definition.space, f.definition.layers[0].id, None, 1)
        .unwrap();
    let second = reader
        .read_environment_layer_cells(
            f.definition.space,
            f.definition.layers[0].id,
            first.next_cursor,
            1,
        )
        .unwrap();
    assert_eq!(first.definition_revision, 1);
    assert_eq!(first.cells, vec![CellCoord::ZERO]);
    assert_eq!(second.cells, vec![CellCoord { x: 1, z: 0 }]);
    assert_eq!(second.next_cursor, None);
    let document = crate::read_project_database(&f.path).unwrap();
    assert_eq!(document.environments, vec![f.definition.clone()]);
    assert_eq!(document.environment_cells.len(), 2);
    let tables: i64 = writer.connection.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('terrain_cell_surface_slots', 'terrain_cell_weight_pages', 'vegetation_field_pages')", [], |r| r.get(0)).unwrap();
    assert_eq!(tables, 0);
}

#[test]
fn malformed_writes_and_old_schemas_fail_without_mutation() {
    let f = Fixture::new();
    let mut writer = f.writer();
    let mut malformed = f.cell(0);
    malformed.tiles[0].samples.pop();
    assert!(
        writer
            .apply_dense_source_transaction(&[write(malformed, None)])
            .is_err()
    );
    assert!(
        writer
            .apply_dense_source_transaction(&[write(f.cell(0), None), write(f.cell(0), None)])
            .is_err()
    );
    assert_eq!(f.snapshot(&[CellCoord::ZERO]).coverage.cells[0].revision, 0);
    writer
        .connection
        .pragma_update(None, "user_version", 17)
        .unwrap();
    assert!(matches!(
        ProjectReader::open_read_only(&f.path),
        Err(WorldDbError::SchemaVersion { actual: 17, .. })
    ));
    assert!(ProjectWriter::open(&f.path).is_err());
}

#[test]
fn byte_budget_applies_before_writing_and_while_loading_a_multi_cell_snapshot() {
    let f = Fixture::new();
    let mut writer = f.writer();
    let mut definition = f.definition.clone();
    definition.mask_resolution = 257;
    let EnvironmentDefinitionWriteResult::Committed(definition) = writer
        .replace_environment_definition_if_revision(Some(1), &definition)
        .unwrap()
    else {
        panic!()
    };
    let writes: Vec<_> = (0..64)
        .map(|x| {
            let mut record = f.cell(x);
            record.definition_revision = definition.revision;
            record.tiles[0].samples = vec![0; 257 * 257];
            record.tiles[0].samples[257 * 128 + 128] = 255;
            write(record, None)
        })
        .collect();
    assert!(writer.apply_dense_source_transaction(&writes).is_err());
    assert_eq!(f.snapshot(&[CellCoord::ZERO]).coverage.cells[0].revision, 0);
    for chunk in writes.chunks(32) {
        committed(writer.apply_dense_source_transaction(chunk).unwrap());
    }
    let cells: Vec<_> = (0..64).map(|x| CellCoord { x, z: 0 }).collect();
    assert!(
        f.reader()
            .read_environment_snapshot(f.definition.space, &cells)
            .is_err()
    );
    assert!(
        f.reader()
            .read_environment_cells_in_cells(
                f.definition.space,
                CellCoord::ZERO,
                CellCoord { x: 63, z: 0 },
                64
            )
            .is_err()
    );
    assert!(
        f.reader()
            .read_environment_cells_in_cells(
                f.definition.space,
                CellCoord::ZERO,
                CellCoord { x: 10000, z: 10000 },
                1
            )
            .is_err()
    );
}

#[test]
fn preset_layer_and_paint_commit_and_rollback_as_one_checkpoint() {
    let f = Fixture::new();
    let mut writer = f.writer();
    let original = f.reader().read_environment_presets().unwrap();
    let mut library = original.clone();
    let mut copy = library.presets[0].clone();
    copy.id = PresetId([90; 16]);
    copy.name = "Local meadow".into();
    library.presets.push(copy.clone());
    let mut definition = f.definition.clone();
    definition.layers[0].preset = copy.id;
    let edit = EnvironmentDefinitionWrite {
        expected_revision: Some(1),
        definition,
    };
    let mut bad = f.cell(0);
    bad.tiles[0].samples[5] = 255;
    assert!(
        writer
            .apply_environment_source_transaction(
                1,
                Some(&library),
                std::slice::from_ref(&edit),
                &[write(bad, None)]
            )
            .is_err()
    );
    assert_eq!(f.reader().read_environment_presets().unwrap(), original);
    assert_eq!(f.snapshot(&[CellCoord::ZERO]).definition, f.definition);
    let EnvironmentSourceWriteResult::Committed(saved) = writer
        .apply_environment_source_transaction(1, Some(&library), &[edit], &[write(f.cell(0), None)])
        .unwrap()
    else {
        panic!()
    };
    let library = saved.presets.unwrap();
    assert_eq!(library.revision, 2);
    assert_eq!(library.get(copy.id).unwrap().revision, 1);
    let snapshot = f.snapshot(&[CellCoord::ZERO]);
    assert_eq!(snapshot.presets, library);
    assert_eq!(snapshot.definition.layers[0].preset, copy.id);
    assert_eq!(snapshot.coverage.cells[0].revision, 1);
    let mut renamed = library.clone();
    renamed.presets[0].name = "Changed".into();
    assert!(matches!(
        writer
            .apply_environment_source_transaction(1, Some(&renamed), &[], &[])
            .unwrap(),
        EnvironmentSourceWriteResult::LibraryConflict { .. }
    ));
    assert!(matches!(
        writer
            .apply_environment_source_transaction(1, None, &[], &[write(f.cell(1), None)])
            .unwrap(),
        EnvironmentSourceWriteResult::LibraryConflict { .. }
    ));
    assert_eq!(f.reader().read_environment_presets().unwrap(), library);
    let EnvironmentSourceWriteResult::Committed(updated) = writer
        .apply_environment_source_transaction(2, Some(&renamed), &[], &[])
        .unwrap()
    else {
        panic!()
    };
    let updated = updated.presets.unwrap();
    assert_eq!(updated.revision, 3);
    assert_eq!(updated.get(renamed.presets[0].id).unwrap().revision, 2);
    assert_eq!(updated.get(copy.id).unwrap().revision, 1);
}

#[test]
fn dependency_pages_follow_nested_repeated_uses_across_worlds_and_update_atomically() {
    use environment::{PresetOverride, PresetUse, PresetUseId, QuickValue};
    let mut f = Fixture::new();
    let mut document = crate::read_project_database(&f.path).unwrap();
    let leaf = document.presets.presets[0].id;
    if let PresetKind::Foliage(v) = &mut document.presets.presets[0].kind {
        v.blend = VegetationBlend::Add;
    }
    let inner = PresetId([50; 16]);
    let outer = PresetId([51; 16]);
    let use_leaf = |id| PresetUse {
        id: PresetUseId([id; 16]),
        name: "Grass".into(),
        preset: leaf,
        overrides: vec![],
    };
    document.presets.presets.push(Preset {
        id: inner,
        revision: 1,
        name: "Understory".into(),
        kind: PresetKind::Composition(vec![use_leaf(10), use_leaf(11)]),
    });
    document.presets.presets.push(Preset {
        id: outer,
        revision: 1,
        name: "Forest".into(),
        kind: PresetKind::Composition(vec![PresetUse {
            id: PresetUseId([20; 16]),
            name: "Floor".into(),
            preset: inner,
            overrides: vec![],
        }]),
    });
    document.environments[0].layers[0].preset = outer;
    let mut space = document.world_spaces[0].clone();
    space.id = WorldSpaceId(2);
    space.name = "Second world".into();
    document.world_spaces.push(space);
    let mut second = document.environments[0].clone();
    second.space = WorldSpaceId(2);
    second.layers[0].overrides = vec![PresetOverride {
        path: vec![PresetUseId([20; 16]), PresetUseId([10; 16])],
        value: QuickValue::FoliageDensity(0.25),
    }];
    document.environments.push(second);
    f.path = f.directory.join("two-worlds.sqlite");
    write_project_database(&f.path, &document).unwrap();
    let reader = f.reader();
    let page = reader
        .read_environment_preset_layers(leaf, None, 1)
        .unwrap();
    assert_eq!(
        page.layers,
        vec![(WorldSpaceId(1), f.definition.layers[0].id)]
    );
    let next = reader
        .read_environment_preset_layers(leaf, page.next_cursor, 1)
        .unwrap();
    assert_eq!(
        next.layers,
        vec![(WorldSpaceId(2), f.definition.layers[0].id)]
    );
    assert!(next.next_cursor.is_none());
    // Removing a nested child would orphan another world's override: reject the shared edit.
    let mut broken = document.presets.clone();
    let PresetKind::Composition(children) = &mut broken
        .presets
        .iter_mut()
        .find(|p| p.id == inner)
        .unwrap()
        .kind
    else {
        panic!()
    };
    children.remove(0);
    let mut writer = f.writer();
    assert!(
        writer
            .apply_environment_source_transaction(1, Some(&broken), &[], &[])
            .is_err()
    );
    assert_eq!(reader.read_environment_presets().unwrap(), document.presets);
    // Root changes update the reverse dependency index in the same commit.
    let mut detached = document.environments[1].clone();
    detached.layers[0].preset = leaf;
    detached.layers[0].overrides.clear();
    writer
        .apply_environment_source_transaction(
            1,
            None,
            &[EnvironmentDefinitionWrite {
                expected_revision: Some(1),
                definition: detached,
            }],
            &[],
        )
        .unwrap();
    assert_eq!(
        reader
            .read_environment_preset_layers(outer, None, 10)
            .unwrap()
            .layers
            .len(),
        1
    );
    assert_eq!(
        reader
            .read_environment_preset_layers(leaf, None, 10)
            .unwrap()
            .layers
            .len(),
        2
    );
}

#[test]
fn referenced_preset_deletion_and_incompatible_type_changes_do_not_corrupt_source() {
    use environment::{PresetOverride, QuickValue};
    let f = Fixture::new();
    let mut writer = f.writer();
    let original = f.reader().read_environment_presets().unwrap();
    assert!(
        writer
            .apply_environment_source_transaction(1, Some(&PresetLibrary::default()), &[], &[])
            .is_err()
    );
    let mut definition = f.definition.clone();
    definition.layers[0].overrides.push(PresetOverride {
        path: vec![],
        value: QuickValue::FoliageDensity(0.5),
    });
    writer
        .apply_environment_source_transaction(
            1,
            None,
            &[EnvironmentDefinitionWrite {
                expected_revision: Some(1),
                definition,
            }],
            &[],
        )
        .unwrap();
    let mut changed = original.clone();
    changed.presets[0].kind = PresetKind::Exclusion(environment::Exclusion {
        id: OutputId([3; 16]),
        channel: ChannelId([1; 16]),
        strength: 1.0,
    });
    assert!(
        writer
            .apply_environment_source_transaction(1, Some(&changed), &[], &[])
            .is_err()
    );
    assert_eq!(f.reader().read_environment_presets().unwrap(), original);
}

#[test]
fn presence_is_bounded_metadata_only_and_reports_partial_results() {
    let f = Fixture::new();
    let mut writer = f.writer();
    writer
        .apply_environment_source_transaction(
            1,
            None,
            &[],
            &[write(f.cell(0), None), write(f.cell(1), None)],
        )
        .unwrap();
    let reader = f.reader();
    let min = CellCoord { x: -2, z: -2 };
    let max = CellCoord { x: 2, z: 2 };
    let found = reader
        .read_environment_layer_presence(f.definition.space, min, max, 1)
        .unwrap();
    assert!(found.truncated);
    assert_eq!(found.entries.len(), 1);
    assert_eq!(found.definition_revision, 1);
    let found = reader
        .read_environment_layer_presence(f.definition.space, min, max, 4)
        .unwrap();
    assert!(!found.truncated);
    assert_eq!(found.entries.len(), 2);
    assert!(
        reader
            .read_environment_layer_presence(f.definition.space, min, max, 0)
            .is_err()
    );
    assert!(
        reader
            .read_environment_layer_presence(
                f.definition.space,
                min,
                CellCoord { x: 999, z: 999 },
                4
            )
            .is_err()
    );
    // Corrupt payloads deliberately: discovery must not decode or load either blob.
    let connection = rusqlite::Connection::open(&f.path).unwrap();
    connection
        .execute("UPDATE environment_coverage SET samples=x'01010101'", [])
        .unwrap();
    let found = reader
        .read_environment_layer_presence(f.definition.space, min, max, 4)
        .unwrap();
    assert_eq!(found.entries.len(), 2);
    assert!(
        reader
            .read_environment_snapshot(f.definition.space, &[CellCoord::ZERO])
            .is_err()
    );
}
