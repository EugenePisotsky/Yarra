use super::*;
use environment_compile::{CompilePlan, CompileProfile};
use world_db::{
    DenseSourceRecord, DenseSourceWrite, DenseSourceWriteTransactionResult, ProjectReader,
    ProjectWriter,
};

/// Exercises the actual brush command -> undo -> save -> database/compiler path on a disposable
/// demo. No renderer, machine-specific working database, or long GPU run is required.
#[test]
fn stroke_save_reload_compile_and_cancel_preserve_borders() {
    let directory = std::env::temp_dir().join(format!(
        "yarra-paint-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("project.sqlite");
    world_cook::create_demo_project(&path).unwrap();
    let reader = ProjectReader::open_read_only(&path).unwrap();
    let definition = reader
        .read_environment_definitions()
        .unwrap()
        .into_iter()
        .find(|d| !d.layers.is_empty())
        .unwrap();
    let layer = definition
        .layers
        .iter()
        .max_by_key(|layer| layer.order)
        .unwrap()
        .id;
    let brush = CoverageBrush {
        radius: 7.0,
        falloff: 0.6,
        strength: 0.85,
        operation: BrushOperation::Paint,
    };
    let from = [63.0, 63.0];
    let to = [66.0, 68.0];
    let cells = brush.cells(definition.cell_size, from, to).unwrap();
    assert!(cells.len() > 1);
    let halo = world_db::environment_dependency_cells(&cells).unwrap();
    let original = reader
        .read_environment_snapshot(definition.space, &halo)
        .unwrap();
    let records = original
        .coverage
        .cells
        .iter()
        .map(|cell| SourceEnvironmentCellRecord {
            space: definition.space,
            cell: cell.cell,
            source_revision: cell.revision as i64,
            definition_revision: definition.revision,
            tiles: cell.tiles.clone(),
        })
        .collect::<Vec<_>>();
    let resources = reader
        .read_environment_terrain_resources(definition.space)
        .unwrap();
    let plan = CompilePlan::new(
        &definition,
        &original.vegetation_catalog,
        &original.presets,
        CompileProfile {
            terrain_resolution: resources.profile.weight_resolution,
            ..Default::default()
        },
    )
    .unwrap();
    let baseline = plan.compile_cells(&cells, &original.coverage).unwrap();
    let mut dense = DenseDomainWorkingSets::from_environment_records(&records);
    let mut stroke = Stroke {
        space: definition.space,
        layer,
        brush,
        before: BTreeMap::new(),
        last_point: None,
    };
    apply_segment(&mut stroke, &definition, &cells, from, &mut dense).unwrap();
    apply_segment(&mut stroke, &definition, &cells, to, &mut dense).unwrap();
    assert!(!stroke.before.is_empty());
    let mut snapshot = original.coverage.clone();
    for cell in &mut snapshot.cells {
        *cell = dense
            .environment_record(definition.space, cell.cell)
            .unwrap()
            .coverage();
    }
    let painted = plan.compile_cells(&cells, &snapshot).unwrap(); // Also validates every shared edge.
    assert!(
        baseline
            .iter()
            .zip(&painted)
            .any(|(a, b)| a.ground != b.ground || a.vegetation != b.vegetation)
    );
    let mut history = EditorHistory::default();
    let mut objects = EditorObjectWorkingSet::default();
    let mut paint = EnvironmentPaintState {
        stroke: Some(stroke),
        ..Default::default()
    };
    paint.finish(&mut dense, &mut history, false);
    assert_eq!(history.undo_len(), 1);
    assert!(history.undo(&mut objects, &mut dense));
    assert_eq!(dense.dirty_count(), 0);
    assert!(history.redo(&mut objects, &mut dense));
    assert!(dense.dirty_count() > 0);
    let writes = dense
        .dirty_snapshots()
        .into_iter()
        .map(|dirty| {
            let DenseSourceRecord::EnvironmentCoverage(record) = dirty.current;
            let expected_source_revision = dirty.base.map(|base| {
                let DenseSourceRecord::EnvironmentCoverage(base) = base;
                base.source_revision
            });
            DenseSourceWrite::EnvironmentCoverage {
                expected_source_revision,
                record,
            }
        })
        .collect::<Vec<_>>();
    let mut writer = ProjectWriter::open(&path).unwrap();
    assert!(matches!(
        writer.apply_dense_source_transaction(&writes).unwrap(),
        DenseSourceWriteTransactionResult::Committed(_)
    ));
    let reloaded = reader
        .read_environment_snapshot(definition.space, &halo)
        .unwrap();
    let compiled = plan.compile_cells(&cells, &reloaded.coverage).unwrap();
    for (live, saved) in painted.iter().zip(&compiled) {
        assert_eq!(live.ground, saved.ground);
        assert_eq!(live.vegetation, saved.vegetation);
    }
    // An unsaved undo must override the newer database even when it equals the runtime baseline.
    let undo_preview = super::preview::compile_source_cells(
        &reader,
        &plan,
        definition.space,
        definition.revision,
        original.presets.revision,
        &cells,
        &records,
    )
    .unwrap();
    for (before, undone) in baseline.iter().zip(&undo_preview) {
        assert_eq!(before.ground, undone.ground);
        assert_eq!(before.vegetation, undone.vegetation);
    }
    // Escape restores just the active gesture and creates no new history entry.
    let before_cancel = dense.current_records();
    let mut stroke = Stroke {
        space: definition.space,
        layer,
        brush: CoverageBrush {
            operation: BrushOperation::Erase,
            ..brush
        },
        before: BTreeMap::new(),
        last_point: None,
    };
    apply_segment(&mut stroke, &definition, &cells, to, &mut dense).unwrap();
    paint.stroke = Some(stroke);
    paint.finish(&mut dense, &mut history, true);
    let mut after_cancel = dense.current_records();
    let mut before_cancel = before_cancel;
    let sort = |record: &DenseSourceRecord| {
        let DenseSourceRecord::EnvironmentCoverage(record) = record;
        record.cell
    };
    after_cancel.sort_by_key(sort);
    before_cancel.sort_by_key(sort);
    assert_eq!(after_cancel, before_cancel);
    assert_eq!(history.undo_len(), 1);
    drop(writer);
    drop(reader);
    std::fs::remove_dir_all(directory).unwrap();
}
