use super::*;
use crate::editing::EditorObjectWorkingSet;
use crate::environment_paint::preview::compile_source_cells_with_roads;
use std::collections::BTreeSet;
use world_db::*;
struct Fixture {
    path: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("yarra-road-editor-{}.sqlite", uuid::Uuid::new_v4()));
        world_cook::create_road_demo_project(&path).unwrap();
        Self { path }
    }
    fn reader(&self) -> ProjectReader {
        ProjectReader::open_read_only(&self.path).unwrap()
    }
    fn load(&self) -> DenseDomainWorkingSets {
        let mut dense = DenseDomainWorkingSets::default();
        dense.roads.reconcile(
            self.reader()
                .read_road_authoring_snapshot(
                    WorldSpaceId(1),
                    RoadCellBounds {
                        minimum: CellCoord { x: -3, z: -3 },
                        maximum: CellCoord { x: 3, z: 3 },
                    },
                    &[],
                )
                .unwrap(),
            &BTreeSet::new(),
        );
        dense
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
fn keys(d: &DenseDomainWorkingSets) -> (RoadSpanId, RoadKnotId) {
    let s = d
        .roads
        .entries
        .keys()
        .find_map(|k| {
            if let RoadRecordKey::Span(id) = k {
                Some(*id)
            } else {
                None
            }
        })
        .unwrap();
    (s, commands::span(&d.roads, s).unwrap().end.id)
}
fn checkpoint(f: &Fixture, d: &mut DenseDomainWorkingSets) {
    let mut writer = ProjectWriter::open(&f.path).unwrap();
    let result = writer
        .apply_environment_and_roads_transaction(
            1,
            None,
            &[],
            &[],
            &d.roads.writes(),
            &d.roads.dependencies(),
        )
        .unwrap();
    let EnvironmentSourceWriteResult::Committed(commit) = result else {
        panic!("save conflict")
    };
    d.roads.saving = Some(1);
    d.roads.finish_save(
        1,
        &crate::project_store::DenseSaveOutcome::Committed(commit),
    );
}
#[test]
fn split_undo_redo_across_save_keeps_shared_knots_and_new_revisions() {
    let f = Fixture::new();
    let mut dense = f.load();
    let mut history = EditorHistory::default();
    let mut state = RoadToolState::default();
    let mut objects = EditorObjectWorkingSet::default();
    let (span, _) = keys(&dense);
    let before = commands::span(&dense.roads, span).unwrap();
    let changes = commands::split(&dense.roads, span, 8.0);
    assert!(state.commit(&mut dense, &mut history, changes));
    assert_eq!(
        dense
            .roads
            .entries
            .values()
            .filter(|e| matches!(e.current, Some(RoadSourceRecord::Span(_))))
            .count(),
        2
    );
    checkpoint(&f, &mut dense);
    assert_eq!(dense.roads.dirty_count(), 0);
    assert!(history.undo(&mut objects, &mut dense));
    let undone = commands::span(&dense.roads, span).unwrap();
    assert_eq!(undone.start.outgoing, before.start.outgoing);
    assert_eq!(undone.end.incoming, before.end.incoming);
    assert!(undone.revision > before.revision);
    checkpoint(&f, &mut dense);
    assert!(history.redo(&mut objects, &mut dense));
    checkpoint(&f, &mut dense);
    assert_eq!(
        f.reader()
            .read_roads_in_cells(
                WorldSpaceId(1),
                RoadCellBounds {
                    minimum: CellCoord::ZERO,
                    maximum: CellCoord::ZERO
                }
            )
            .unwrap()
            .roads
            .spans
            .len(),
        2
    );
}
#[test]
fn recovery_retains_reference_dependencies_and_detects_a_changed_database() {
    let f = Fixture::new();
    let mut dense = f.load();
    let (_, knot) = keys(&dense);
    let mut record = dense.roads.get(RoadRecordKey::Knot(knot)).unwrap().clone();
    if let RoadSourceRecord::Knot(k) = &mut record {
        k.knot.width += 0.5;
    }
    dense
        .roads
        .apply(&[RoadChange {
            key: record.key(),
            record: Some(record),
        }])
        .unwrap();
    let bytes = ron::to_string(&dense.roads.journal()).unwrap();
    let entries = ron::from_str(&bytes).unwrap();
    let mut restored = DenseDomainWorkingSets::default();
    assert_eq!(restored.roads.restore(entries), 1);
    assert_eq!(restored.roads.writes(), dense.roads.writes());
    assert_eq!(restored.roads.dependencies(), dense.roads.dependencies());
    checkpoint(&f, &mut dense);
    let pins = restored.roads.pins(&BTreeSet::new());
    let snapshot = f
        .reader()
        .read_road_authoring_snapshot(
            WorldSpaceId(1),
            RoadCellBounds {
                minimum: CellCoord::ZERO,
                maximum: CellCoord::ZERO,
            },
            &pins.iter().copied().collect::<Vec<_>>(),
        )
        .unwrap();
    restored.roads.reconcile(snapshot, &pins);
    assert!(restored.roads.conflict);
    assert_eq!(restored.roads.dirty_count(), 1);
}
#[test]
fn saved_and_unsaved_roads_preview_identically_and_removal_restores_the_painted_grass() {
    let f = Fixture::new();
    let mut dense = f.load();
    let reader = f.reader();
    let (span, _) = keys(&dense);
    let source = reader
        .read_environment_snapshot(
            WorldSpaceId(1),
            &environment_dependency_cells(&[CellCoord::ZERO]).unwrap(),
        )
        .unwrap();
    let plan = environment_compile::CompilePlan::new(
        &source.definition,
        &source.vegetation_catalog,
        &source.presets,
        Default::default(),
    )
    .unwrap();
    let compile = |changes: &[RoadChange]| {
        compile_source_cells_with_roads(
            &reader,
            &plan,
            WorldSpaceId(1),
            1,
            1,
            &[CellCoord::ZERO],
            &[],
            changes,
        )
        .unwrap()
    };
    let saved = compile(&[]);
    let base = plan
        .compile_cells(&[CellCoord::ZERO], &source.coverage)
        .unwrap();
    assert_ne!(saved[0].vegetation, base[0].vegetation);
    dense
        .roads
        .apply(&commands::delete_span(&dense.roads, span).unwrap())
        .unwrap();
    let removed = compile(&dense.roads.overrides());
    assert_eq!(removed[0].vegetation, base[0].vegetation);
    assert_eq!(removed[0].ground, base[0].ground);
    let mut writer = ProjectWriter::open(&f.path).unwrap();
    writer
        .apply_road_source_transaction(1, &dense.roads.writes(), &dense.roads.dependencies())
        .unwrap();
    assert_eq!(compile(&[]), removed);
}

#[test]
fn road_relief_preview_save_cook_and_undo_share_heights_without_modifying_base_terrain() {
    let f = Fixture::new();
    let mut dense = f.load();
    let reader = f.reader();
    let base = read_project_database(&f.path)
        .unwrap()
        .terrain_cell_heightfields;
    let source = reader
        .read_environment_snapshot(
            WorldSpaceId(1),
            &environment_dependency_cells(&[CellCoord::ZERO]).unwrap(),
        )
        .unwrap();
    let plan = environment_compile::CompilePlan::new(
        &source.definition,
        &source.vegetation_catalog,
        &source.presets,
        Default::default(),
    )
    .unwrap();
    let compile = |changes: &[RoadChange]| {
        compile_source_cells_with_roads(
            &reader,
            &plan,
            WorldSpaceId(1),
            1,
            1,
            &[CellCoord::ZERO],
            &[],
            changes,
        )
        .unwrap()
        .remove(0)
    };
    let original = compile(&[]);
    let mut profile = dense
        .roads
        .entries
        .values()
        .find_map(|e| match &e.current {
            Some(RoadSourceRecord::Profile(p)) => Some(p.clone()),
            _ => None,
        })
        .unwrap();
    profile.relief.road_depth = 0.15;
    profile.relief.track_depth = 0.08;
    let mut state = RoadToolState::default();
    let mut history = EditorHistory::default();
    assert!(state.commit(
        &mut dense,
        &mut history,
        Ok(vec![RoadChange {
            key: RoadRecordKey::Profile(profile.id),
            record: Some(RoadSourceRecord::Profile(profile))
        }])
    ));
    let shaped = compile(&dense.roads.overrides());
    assert_ne!(shaped.terrain, original.terrain);
    assert_eq!(shaped.ground, original.ground);
    assert_eq!(shaped.vegetation, original.vegetation);
    checkpoint(&f, &mut dense);
    let saved = compile(&[]);
    assert_eq!(saved.terrain, shaped.terrain);
    let document = read_project_database(&f.path).unwrap();
    assert_eq!(document.terrain_cell_heightfields, base);
    let cooked = world_cook::build_runtime(document).unwrap();
    let page = cooked
        .pages
        .iter()
        .find(|p| {
            p.key.space == WorldSpaceId(1)
                && p.key.cell == CellCoord::ZERO
                && p.key.domain == world::PageDomain::TerrainRender
        })
        .unwrap()
        .clone()
        .decode()
        .unwrap();
    let world::PagePayload::TerrainHeightfield(page) = page.payload else {
        panic!()
    };
    assert_eq!(Some(page.heightfield), saved.terrain);
    let mut objects = EditorObjectWorkingSet::default();
    assert!(history.undo(&mut objects, &mut dense));
    assert_eq!(compile(&dense.roads.overrides()).terrain, original.terrain);
    assert!(history.redo(&mut objects, &mut dense));
    assert_eq!(compile(&dense.roads.overrides()).terrain, shaped.terrain);
}
#[test]
fn extension_is_tangent_continuous_and_undo_removes_all_new_records() {
    let f = Fixture::new();
    let mut dense = f.load();
    let mut state = RoadToolState::default();
    let mut history = EditorHistory::default();
    let mut objects = EditorObjectWorkingSet::default();
    let (_, knot) = keys(&dense);
    let endpoint = RoadPoint::from_relative(CellCoord::ZERO, [32.0, 6.0], 8.0).unwrap();
    let (_, changes) = commands::extend(&dense.roads, knot, 8.0, endpoint).unwrap();
    assert!(state.commit(&mut dense, &mut history, Ok(changes)));
    let RoadSourceRecord::Knot(k) = dense.roads.get(RoadRecordKey::Knot(knot)).unwrap() else {
        panic!()
    };
    assert!(k.knot.incoming[0] * k.knot.outgoing[0] < 0.0);
    assert_eq!(k.knot.outgoing[1], 0.0);
    assert!(history.undo(&mut objects, &mut dense));
    assert_eq!(dense.roads.dirty_count(), 0);
}
#[test]
fn creation_undo_is_clean_and_record_budget_does_not_partially_apply() {
    let mut dense = DenseDomainWorkingSets::default();
    let mut state = RoadToolState::default();
    let mut history = EditorHistory::default();
    let mut objects = EditorObjectWorkingSet::default();
    let style = commands::default_profile(
        environment::fixtures::DRY_GROUND,
        environment::ChannelId([1; 16]),
    );
    let style_change = RoadChange {
        key: RoadRecordKey::Profile(style.id),
        record: Some(RoadSourceRecord::Profile(style.clone())),
    };
    let start = RoadPoint::from_relative(CellCoord::ZERO, [1.0, 1.0], 8.0).unwrap();
    let end = RoadPoint::from_relative(CellCoord::ZERO, [12.0, 1.0], 8.0).unwrap();
    dense
        .roads
        .apply(std::slice::from_ref(&style_change))
        .unwrap();
    let (_, _, mut changes) =
        commands::create(&dense.roads, WorldSpaceId(1), 8.0, style.id, start, end).unwrap();
    dense.roads.discard();
    changes.insert(0, style_change);
    assert!(state.commit(&mut dense, &mut history, Ok(changes)));
    assert_eq!(dense.roads.dirty_count(), 5);
    assert!(history.undo(&mut objects, &mut dense));
    assert_eq!(dense.roads.dirty_count(), 0);
    assert!(history.redo(&mut objects, &mut dense));
    for i in 0..30 {
        let (_, _, changes) =
            commands::create(&dense.roads, WorldSpaceId(1), 8.0, style.id, start, end).unwrap();
        let before = dense.roads.writes();
        if dense.roads.apply(&changes).is_err() {
            assert_eq!(dense.roads.writes(), before);
            assert!(i > 5);
            return;
        }
    }
    panic!("expected bounded editing budget");
}

#[test]
fn deleting_saving_and_leaving_the_area_keeps_references_needed_by_undo() {
    let f = Fixture::new();
    let mut dense = f.load();
    let (span, _) = keys(&dense);
    let mut state = RoadToolState::default();
    let mut history = EditorHistory::default();
    let mut objects = EditorObjectWorkingSet::default();
    let changes = commands::delete_span(&dense.roads, span);
    assert!(state.commit(&mut dense, &mut history, changes));
    checkpoint(&f, &mut dense);
    let pins = dense.roads.pins(&history.road_keys());
    let far = CellCoord { x: 6, z: 6 };
    let snapshot = f
        .reader()
        .read_road_authoring_snapshot(
            WorldSpaceId(1),
            RoadCellBounds {
                minimum: far,
                maximum: far,
            },
            &pins.iter().copied().collect::<Vec<_>>(),
        )
        .unwrap();
    dense.roads.reconcile(snapshot, &pins);
    assert!(history.undo(&mut objects, &mut dense));
    assert!(commands::span(&dense.roads, span).is_some());
    checkpoint(&f, &mut dense);
}

#[test]
fn road_creation_uses_the_chosen_style_even_when_material_and_channel_match() {
    let mut w = working::RoadWorkingSet::default();
    let a = commands::default_profile(
        environment::fixtures::DRY_GROUND,
        environment::ChannelId([1; 16]),
    );
    let mut b = a.clone();
    b.id = RoadProfileId([200; 16]);
    b.breakup = 0.1;
    for p in [&a, &b] {
        w.apply(&[RoadChange {
            key: RoadRecordKey::Profile(p.id),
            record: Some(RoadSourceRecord::Profile(p.clone())),
        }])
        .unwrap();
    }
    let point = |x| RoadPoint::from_relative(CellCoord::ZERO, [x, 1.0], 8.0).unwrap();
    for p in [&a, &b] {
        let (_, _, changes) =
            commands::create(&w, WorldSpaceId(1), 8.0, p.id, point(0.0), point(12.0)).unwrap();
        assert_eq!(changes.len(), 4);
        assert!(
            changes.iter().any(
                |c| matches!(&c.record,Some(RoadSourceRecord::Road(r)) if r.road.profile==p.id)
            )
        );
    }
    w.route_minimum_widths.insert(RoadId([1; 16]), Some(2.0));
    assert!(validate_route_style(&w, RoadId([1; 16]), &a).is_err());
}

#[test]
fn junction_branch_move_save_recovery_cook_and_delete_are_atomic() {
    let f = Fixture::new();
    let mut dense = f.load();
    let mut state = RoadToolState::default();
    let mut history = EditorHistory::default();
    let mut objects = EditorObjectWorkingSet::default();
    let (span, _) = keys(&dense);
    let changes = commands::split(&dense.roads, span, 8.0).unwrap();
    let center = changes
        .iter()
        .find_map(|c| match &c.record {
            Some(RoadSourceRecord::Knot(k))
                if k.knot.position
                    == RoadPoint::from_relative(CellCoord::ZERO, [0.0, 0.0], 8.0).unwrap() =>
            {
                Some(k.knot.id)
            }
            _ => None,
        })
        .unwrap();
    assert!(state.commit(&mut dense, &mut history, Ok(changes)));
    let endpoint = RoadPoint::from_relative(CellCoord::ZERO, [0.0, 16.0], 8.0).unwrap();
    let (branch, _, changes) = junctions::branch(&dense.roads, center, 8.0, endpoint).unwrap();
    assert!(
        state.commit(&mut dense, &mut history, Ok(changes)),
        "{:?}",
        state.status
    );
    let j = junctions::at(&dense.roads, center).unwrap().clone();
    let branch_knot = *j.junction.knots.iter().find(|k| **k != center).unwrap();
    let branch_span = dense
        .roads
        .entries
        .values()
        .find_map(|e| match &e.current {
            Some(RoadSourceRecord::Span(s)) if s.road == branch => Some(s.id),
            _ => None,
        })
        .unwrap();
    let mut profile = match dense
        .roads
        .get(RoadRecordKey::Profile(j.junction.profile))
        .unwrap()
    {
        RoadSourceRecord::Profile(p) => p.clone(),
        _ => unreachable!(),
    };
    profile.relief.road_depth = 0.15;
    profile.relief.track_depth = 0.08;
    assert!(state.commit(
        &mut dense,
        &mut history,
        Ok(vec![RoadChange {
            key: RoadRecordKey::Profile(profile.id),
            record: Some(RoadSourceRecord::Profile(profile))
        }])
    ));
    let journal = ron::to_string(&dense.roads.journal()).unwrap();
    let mut recovered = working::RoadWorkingSet::default();
    assert!(recovered.restore(ron::from_str(&journal).unwrap()) > 0);
    assert_eq!(recovered.writes(), dense.roads.writes());
    let live_reader = f.reader();
    let live_source = live_reader
        .read_environment_snapshot(
            WorldSpaceId(1),
            &environment_dependency_cells(&[CellCoord::ZERO]).unwrap(),
        )
        .unwrap();
    let live_plan = environment_compile::CompilePlan::new(
        &live_source.definition,
        &live_source.vegetation_catalog,
        &live_source.presets,
        Default::default(),
    )
    .unwrap();
    let preview = |overrides: &[RoadChange]| {
        compile_source_cells_with_roads(
            &live_reader,
            &live_plan,
            WorldSpaceId(1),
            1,
            1,
            &[CellCoord::ZERO],
            &[],
            overrides,
        )
        .unwrap()
        .remove(0)
    };
    let draft = preview(&dense.roads.overrides());
    checkpoint(&f, &mut dense);
    let saved = preview(&[]);
    assert_eq!(draft.terrain, saved.terrain);
    assert_eq!(draft.ground, saved.ground);
    assert_eq!(draft.vegetation, saved.vegetation);
    let mut loaded = f.load();
    assert_eq!(
        junctions::at(&loaded.roads, center).unwrap().junction,
        j.junction
    );
    let before = loaded
        .roads
        .get(RoadRecordKey::Knot(center))
        .unwrap()
        .clone();
    let mut moved = before.clone();
    let RoadSourceRecord::Knot(k) = &mut moved else {
        unreachable!()
    };
    k.knot.position = RoadPoint::from_relative(CellCoord::ZERO, [0.5, 0.25], 8.0).unwrap();
    let changes = junctions::move_point(
        &loaded.roads,
        RoadChange {
            key: moved.key(),
            record: Some(moved),
        },
    )
    .unwrap();
    let mut move_history = EditorHistory::default();
    assert!(state.commit(&mut loaded, &mut move_history, Ok(changes)));
    assert_eq!(
        junctions::at(&loaded.roads, center)
            .unwrap()
            .junction
            .position,
        match loaded.roads.get(RoadRecordKey::Knot(branch_knot)).unwrap() {
            RoadSourceRecord::Knot(k) => k.knot.position,
            _ => unreachable!(),
        }
    );
    checkpoint(&f, &mut loaded);
    assert!(move_history.undo(&mut objects, &mut loaded));
    checkpoint(&f, &mut loaded);
    assert!(move_history.redo(&mut objects, &mut loaded));
    checkpoint(&f, &mut loaded);
    let reader = f.reader();
    let source = reader
        .read_environment_snapshot(
            WorldSpaceId(1),
            &environment_dependency_cells(&[CellCoord::ZERO]).unwrap(),
        )
        .unwrap();
    let plan = environment_compile::CompilePlan::new(
        &source.definition,
        &source.vegetation_catalog,
        &source.presets,
        Default::default(),
    )
    .unwrap();
    let preview = compile_source_cells_with_roads(
        &reader,
        &plan,
        WorldSpaceId(1),
        1,
        1,
        &[CellCoord::ZERO],
        &[],
        &[],
    )
    .unwrap()
    .remove(0);
    let document = read_project_database(&f.path).unwrap();
    let index = world_db::RoadDocumentIndex::new(&document.roads, &document.environments).unwrap();
    let bounds = RoadCellBounds {
        minimum: CellCoord { x: -1, z: -1 },
        maximum: CellCoord { x: 1, z: 1 },
    };
    assert_eq!(
        reader
            .read_roads_in_cells(WorldSpaceId(1), bounds)
            .unwrap()
            .roads,
        index.snapshot(WorldSpaceId(1), bounds, 8.0).unwrap()
    );
    let cooked = world_cook::build_runtime(document).unwrap();
    let page = cooked
        .pages
        .iter()
        .find(|p| {
            p.key.space == WorldSpaceId(1)
                && p.key.cell == CellCoord::ZERO
                && p.key.domain == world::PageDomain::TerrainRender
        })
        .unwrap()
        .clone()
        .decode()
        .unwrap();
    let world::PagePayload::TerrainHeightfield(page) = page.payload else {
        panic!()
    };
    assert_eq!(Some(page.heightfield), preview.terrain);
    let changes = commands::delete_span(&loaded.roads, branch_span).unwrap();
    assert!(state.commit(&mut loaded, &mut move_history, Ok(changes)));
    assert!(junctions::at(&loaded.roads, center).is_none());
    checkpoint(&f, &mut loaded);
    assert!(move_history.undo(&mut objects, &mut loaded));
    checkpoint(&f, &mut loaded);
    assert!(junctions::at(&f.load().roads, center).is_some());
}

#[test]
fn junction_connections_reject_self_links_style_mismatches_and_excess_arms() {
    let f = Fixture::new();
    let mut d = f.load();
    let (span, endpoint) = keys(&d);
    let changes = commands::split(&d.roads, span, 8.0).unwrap();
    d.roads.apply(&changes).unwrap();
    let center = commands::span(&d.roads, span).unwrap().end.id;
    let point = |x, z| RoadPoint::from_relative(CellCoord::ZERO, [x, z], 8.0).unwrap();
    let (_, _, changes) = junctions::branch(&d.roads, center, 8.0, point(0.0, 16.0)).unwrap();
    d.roads.apply(&changes).unwrap();
    assert!(junctions::join(&d.roads, endpoint, center, 8.0).is_err());
    let (_, _, changes) = junctions::branch(&d.roads, center, 8.0, point(0.0, -16.0)).unwrap();
    d.roads.apply(&changes).unwrap();
    assert!(junctions::branch(&d.roads, center, 8.0, point(16.0, 16.0)).is_err());
    // Detaching is explicit, preserves geometry, and does not delete the remaining junction.
    let branch = junctions::at(&d.roads, center)
        .unwrap()
        .junction
        .knots
        .iter()
        .copied()
        .find(|k| *k != center)
        .unwrap();
    let changes = junctions::detach(&d.roads, branch).unwrap();
    d.roads.apply(&changes).unwrap();
    assert!(junctions::at(&d.roads, branch).is_none());
    assert!(junctions::at(&d.roads, center).is_some());
}

#[test]
fn connecting_two_interior_points_creates_a_four_arm_crossing() {
    let f = Fixture::new();
    let mut d = f.load();
    let (span, _) = keys(&d);
    let changes = commands::split(&d.roads, span, 8.0).unwrap();
    d.roads.apply(&changes).unwrap();
    let center = commands::span(&d.roads, span).unwrap().end.id;
    let profile = match d
        .roads
        .get(RoadRecordKey::Road(
            commands::span(&d.roads, span).unwrap().road,
        ))
        .unwrap()
    {
        RoadSourceRecord::Road(r) => r.road.profile,
        _ => unreachable!(),
    };
    let point = |x, z| RoadPoint::from_relative(CellCoord::ZERO, [x, z], 8.0).unwrap();
    let (road, _, changes) = commands::create(
        &d.roads,
        WorldSpaceId(1),
        8.0,
        profile,
        point(0.0, -16.0),
        point(0.0, 16.0),
    )
    .unwrap();
    let vertical = changes
        .iter()
        .find_map(|c| match &c.record {
            Some(RoadSourceRecord::Span(s)) => Some(s.id),
            _ => None,
        })
        .unwrap();
    d.roads.apply(&changes).unwrap();
    let changes = commands::split(&d.roads, vertical, 8.0).unwrap();
    d.roads.apply(&changes).unwrap();
    let crossing = commands::span(&d.roads, vertical).unwrap().end.id;
    let changes = junctions::join(&d.roads, crossing, center, 8.0).unwrap();
    d.roads.apply(&changes).unwrap();
    checkpoint(&f, &mut d);
    let snapshot = f
        .reader()
        .read_roads_in_cells(
            WorldSpaceId(1),
            RoadCellBounds {
                minimum: CellCoord::ZERO,
                maximum: CellCoord::ZERO,
            },
        )
        .unwrap();
    assert_eq!(snapshot.roads.junctions.len(), 1);
    assert_eq!(
        snapshot
            .roads
            .spans
            .iter()
            .filter(|s| s.road == road)
            .count(),
        2
    );
    world_cook::build_runtime(read_project_database(&f.path).unwrap()).unwrap();
}
