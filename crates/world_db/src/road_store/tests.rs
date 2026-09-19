use super::*;
use crate::{
    ProjectDocument, SourceCellRecord, SourceEnvironmentCellRecord, WorldSpaceRecord,
    write_project_database,
};
use environment::{ChannelId, CoverageTile, EnvironmentDefinition, Layer, LayerId};
use environment_compile::CompilePlan;
use world::{TerrainSurface, TerrainSurfaceId};
const SPACE: WorldSpaceId = WorldSpaceId(1);
const SOIL: TerrainSurfaceId = TerrainSurfaceId([1; 16]);
const GREEN: TerrainSurfaceId = TerrainSurfaceId([2; 16]);
const ROUTE: RoadId = RoadId([1; 16]);
const PROFILE: RoadProfileId = RoadProfileId([1; 16]);
fn id(n: u128) -> [u8; 16] {
    n.to_be_bytes()
}
fn fixture_records(count: usize) -> Vec<RoadSourceRecord> {
    let mut records = vec![
        RoadSourceRecord::Profile(CartTrackProfile {
            id: PROFILE,
            revision: 1,
            name: "Cart track".into(),
            ground: environment::fixtures::DRY_GROUND,
            vegetation_channel: ChannelId([1; 16]),
            track_spacing: 1.55,
            track_width: 0.65,
            edge_softness: 0.15,
            center_ground: 0.0,
            center_retention: 1.0,
            shoulder_ground: 0.15,
            shoulder_retention: 0.55,
            track_retention: 0.0,
            edge_variation: 0.08,
            edge_patch_size: 2.0,
            breakup: 0.7,
            breakup_patch_size: 1.2,
            relief: Default::default(),
        }),
        RoadSourceRecord::Road(SourceRoad {
            space: SPACE,
            road: Road {
                id: ROUTE,
                revision: 1,
                name: "Country route".into(),
                profile: PROFILE,
                seed: 481,
                enabled: true,
                order: 0,
                direction: TravelDirection::Bidirectional,
                travel_modes: vec![RoadTravelMode::Foot, RoadTravelMode::Cart],
            },
        }),
    ];
    for n in 0..=count {
        records.push(RoadSourceRecord::Knot(SourceRoadKnot {
            road: ROUTE,
            knot: RoadKnot {
                id: RoadKnotId(id(n as u128 + 1)),
                revision: 1,
                position: RoadPoint::from_relative(
                    CellCoord::ZERO,
                    [-2.0 + n as f64 * 20.0, 4.0],
                    8.0,
                )
                .unwrap(),
                incoming: [-20.0 / 3.0, 0.0],
                outgoing: [20.0 / 3.0, 0.0],
                width: 3.5,
            },
        }));
    }
    for n in 0..count {
        records.push(RoadSourceRecord::Span(SourceRoadSpan {
            id: RoadSpanId(id(n as u128 + 1)),
            revision: 1,
            road: ROUTE,
            start: RoadKnotId(id(n as u128 + 1)),
            end: RoadKnotId(id(n as u128 + 2)),
        }));
    }
    records
}
fn project() -> ProjectDocument {
    let presets = environment::fixtures::meadow_library(SOIL, GREEN, ChannelId([1; 16]));
    let definition = EnvironmentDefinition {
        space: SPACE,
        revision: 1,
        cell_size: 8.0,
        mask_resolution: 17,
        surfaces: vec![SOIL, GREEN],
        base_surface: GREEN,
        layers: vec![Layer {
            id: LayerId([1; 16]),
            revision: 1,
            name: "Meadow".into(),
            preset: environment::fixtures::GREEN_MEADOW,
            overrides: vec![],
            order: 0,
            seed: 42,
            enabled: true,
            opacity: 1.0,
        }],
    };
    let samples = (0..17)
        .flat_map(|z| {
            (0..17).map(move |x| {
                if x == 0 || z == 0 || x == 16 || z == 16 {
                    0
                } else {
                    255
                }
            })
        })
        .collect();
    ProjectDocument {
        default_world_space: SPACE,
        world_spaces: vec![WorldSpaceRecord {
            atmosphere: Default::default(),
            atmosphere_revision: 1,
            id: SPACE,
            name: "Test".into(),
            cell_size: 8.0,
            minimum_y: 0.0,
            maximum_y: 0.0,
        }],
        vegetation_catalog: Some(vegetation::fixtures::reference_catalog()),
        cells: vec![SourceCellRecord {
            space: SPACE,
            cell: CellCoord::ZERO,
            height: 0.0,
            source_revision: 1,
        }],
        terrain_surfaces: [SOIL, GREEN]
            .into_iter()
            .enumerate()
            .map(|(i, id)| TerrainSurface {
                id,
                key: format!("ground_{i}"),
                display_name: format!("Ground {i}"),
                tile_size: 1.0,
                anti_tiling: false,
                normal_y_sign: 1.0,
                normal_strength: 1.0,
                roughness_min: 0.0,
                roughness_max: 1.0,
            })
            .collect(),
        terrain_texture_sets: vec![],
        terrain_texture_layers: vec![],
        terrain_profiles: vec![],
        terrain_cell_heightfields: vec![],
        presets,
        environments: vec![definition],
        environment_cells: vec![SourceEnvironmentCellRecord {
            space: SPACE,
            cell: CellCoord::ZERO,
            source_revision: 1,
            definition_revision: 1,
            tiles: vec![CoverageTile {
                layer: LayerId([1; 16]),
                samples,
            }],
        }],
        roads: RoadDocument::default(),
        assets: vec![],
        asset_variants: vec![],
        definitions: vec![],
        objects: vec![],
    }
}
struct Fixture {
    directory: std::path::PathBuf,
    path: std::path::PathBuf,
}
impl Fixture {
    fn new(count: usize) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "yarra-road-db-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("project.sqlite");
        let mut document = project();
        if count > 0 {
            document.roads.records = fixture_records(count);
        }
        write_project_database(&path, &document).unwrap();
        Self { directory, path }
    }
    fn reader(&self) -> ProjectReader {
        ProjectReader::open_read_only(&self.path).unwrap()
    }
    fn writer(&self) -> ProjectWriter {
        ProjectWriter::open(&self.path).unwrap()
    }
    fn snapshot(&self) -> RoadReadSnapshot {
        self.reader()
            .read_roads_in_cells(SPACE, bounds(0, 0))
            .unwrap()
    }
    fn records(&self) -> Vec<RoadSourceRecord> {
        crate::read_project_database(&self.path)
            .unwrap()
            .roads
            .records
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}
fn bounds(x: i32, z: i32) -> RoadCellBounds {
    RoadCellBounds {
        minimum: CellCoord { x, z },
        maximum: CellCoord { x, z },
    }
}
fn write(record: RoadSourceRecord, expected: Option<u64>) -> RoadSourceWrite {
    RoadSourceWrite {
        key: record.key(),
        expected_revision: expected,
        record: Some(record),
    }
}
fn commit(result: RoadSourceWriteResult) -> RoadSourceCommit {
    let RoadSourceWriteResult::Committed(c) = result else {
        panic!("expected commit, got {result:?}")
    };
    c
}
fn compile(s: &RoadEnvironmentSnapshot) -> Vec<environment_compile::CompiledCell> {
    let e = &s.environment;
    let plan = CompilePlan::new(
        &e.definition,
        &e.vegetation_catalog,
        &e.presets,
        Default::default(),
    )
    .unwrap();
    plan.compile_cells_with_roads(
        &[CellCoord::ZERO],
        &e.coverage,
        &s.roads.roads,
        Default::default(),
    )
    .unwrap()
}
#[test]
fn normalized_roundtrip_and_atomic_environment_snapshot_match_offline_compiler() {
    let f = Fixture::new(1);
    let reader = f.reader();
    let s = reader
        .read_environment_with_roads(
            SPACE,
            &crate::environment_dependency_cells(&[CellCoord::ZERO]).unwrap(),
            bounds(0, 0),
        )
        .unwrap();
    assert_eq!(s.roads.roads.spans.len(), 1);
    assert_eq!(s.roads.dependencies().len(), 5);
    assert_ne!(s.roads.roads.spans[0].start.position.cell, CellCoord::ZERO);
    let doc = crate::read_project_database(&f.path).unwrap();
    let index = RoadDocumentIndex::new(&doc.roads, &doc.environments).unwrap();
    assert_eq!(
        index.cell_snapshot(SPACE, CellCoord::ZERO, 8.0).unwrap(),
        s.roads.roads
    );
    let compiled = compile(&s);
    assert!(!compiled[0].vegetation.fields.is_empty());
    let output = f.directory.join("roundtrip.sqlite");
    write_project_database(&output, &doc).unwrap();
    let r = ProjectReader::open_read_only(&output).unwrap();
    assert_eq!(
        r.read_roads_in_cells(SPACE, bounds(0, 0)).unwrap().roads,
        s.roads.roads
    );
    assert_eq!(
        reader.manifest().schema_version,
        world::PROJECT_SCHEMA_VERSION
    );
}
#[test]
fn create_and_stale_multi_record_edit_are_atomic() {
    let f = Fixture::new(0);
    let records = fixture_records(1);
    let writes = records
        .iter()
        .cloned()
        .map(|r| write(r, None))
        .collect::<Vec<_>>();
    let first = commit(
        f.writer()
            .apply_road_source_transaction(1, &writes, &[])
            .unwrap(),
    );
    assert_eq!(first.records.len(), 5);
    assert_eq!(first.revision, 2);
    let snapshot = f.snapshot();
    let mut knot = knot(&f.reader().connection, RoadKnotId(id(1))).unwrap();
    knot.knot.position.local[1] += 0.5;
    let update = write(RoadSourceRecord::Knot(knot), Some(1));
    let saved = commit(
        f.writer()
            .apply_road_source_transaction(
                1,
                std::slice::from_ref(&update),
                &snapshot.dependencies(),
            )
            .unwrap(),
    );
    assert_eq!(saved.records[0].revision, Some(2));
    let before = f.records();
    let mut road = route(&f.reader().connection, ROUTE).unwrap();
    road.road.name = "Must roll back".into();
    let result = f
        .writer()
        .apply_road_source_transaction(
            1,
            &[write(RoadSourceRecord::Road(road), Some(1)), update],
            &snapshot.dependencies(),
        )
        .unwrap();
    assert!(matches!(result, RoadSourceWriteResult::Conflict(_)));
    assert_eq!(before, f.records());
}
#[test]
fn local_knot_move_updates_incident_spans_and_old_new_bounds_without_route_revision() {
    let f = Fixture::new(4);
    let reader = f.reader();
    let before = f.records();
    let mut k = knot(&reader.connection, RoadKnotId(id(2))).unwrap();
    k.knot.position.local[1] += 0.5;
    let deps = [
        RoadDependency {
            key: RoadRecordKey::Road(ROUTE),
            revision: 1,
        },
        RoadDependency {
            key: RoadRecordKey::Profile(PROFILE),
            revision: 1,
        },
    ];
    let saved = commit(
        f.writer()
            .apply_road_source_transaction(1, &[write(RoadSourceRecord::Knot(k), Some(1))], &deps)
            .unwrap(),
    );
    assert_eq!(saved.bounds.len(), 4);
    assert!(saved.dependencies.is_empty());
    assert_eq!(route(&reader.connection, ROUTE).unwrap().road.revision, 1);
    assert_eq!(
        span(&reader.connection, RoadSpanId(id(1)))
            .unwrap()
            .end
            .revision,
        2
    );
    assert_eq!(
        span(&reader.connection, RoadSpanId(id(2)))
            .unwrap()
            .start
            .revision,
        2
    );
    for record in before {
        if record.key() != RoadRecordKey::Knot(RoadKnotId(id(2))) {
            assert_eq!(required(&reader.connection, record.key()).unwrap(), record);
        }
    }
}
#[test]
fn shape_preserving_split_is_one_transaction_with_shared_knots() {
    let f = Fixture::new(1);
    let snapshot = f.snapshot();
    let (a, b) = split_span(
        &snapshot.roads.spans[0],
        8.0,
        0.4,
        RoadKnotId(id(3)),
        RoadSpanId(id(2)),
    )
    .unwrap();
    let records = [
        RoadSourceRecord::Knot(SourceRoadKnot {
            road: ROUTE,
            knot: a.start.clone(),
        }),
        RoadSourceRecord::Knot(SourceRoadKnot {
            road: ROUTE,
            knot: a.end.clone(),
        }),
        RoadSourceRecord::Knot(SourceRoadKnot {
            road: ROUTE,
            knot: b.end.clone(),
        }),
        RoadSourceRecord::Span(SourceRoadSpan {
            id: a.id,
            revision: 1,
            road: ROUTE,
            start: a.start.id,
            end: a.end.id,
        }),
        RoadSourceRecord::Span(SourceRoadSpan {
            id: b.id,
            revision: 1,
            road: ROUTE,
            start: b.start.id,
            end: b.end.id,
        }),
    ];
    let writes = records
        .into_iter()
        .map(|r| {
            let expected = snapshot
                .dependencies()
                .iter()
                .find(|d| d.key == r.key())
                .map(|d| d.revision);
            write(r, expected)
        })
        .collect::<Vec<_>>();
    commit(
        f.writer()
            .apply_road_source_transaction(1, &writes, &snapshot.dependencies())
            .unwrap(),
    );
    let after = f.snapshot();
    assert_eq!(after.roads.spans.len(), 2);
    assert_eq!(after.roads.spans[0].end, after.roads.spans[1].start);
    let s = f
        .reader()
        .read_environment_with_roads(
            SPACE,
            &crate::environment_dependency_cells(&[CellCoord::ZERO]).unwrap(),
            bounds(0, 0),
        )
        .unwrap();
    let mut old = s.clone();
    old.roads = snapshot;
    assert_eq!(compile(&old)[0].ground, compile(&s)[0].ground);
    assert_eq!(compile(&old)[0].vegetation, compile(&s)[0].vegetation);
}
#[test]
fn deletion_tombstones_prevent_stale_recreation_and_preserve_invalidation() {
    let f = Fixture::new(1);
    let key = RoadRecordKey::Span(RoadSpanId(id(1)));
    let source = required(&f.reader().connection, key).unwrap();
    let removed = commit(
        f.writer()
            .apply_road_source_transaction(
                1,
                &[RoadSourceWrite {
                    key,
                    expected_revision: Some(1),
                    record: None,
                }],
                &[],
            )
            .unwrap(),
    );
    assert_eq!(removed.bounds.len(), 1);
    assert!(f.snapshot().roads.spans.is_empty());
    let state = f.reader().read_road_records(&[key]).unwrap().remove(0);
    assert_eq!(state.revision, Some(2));
    assert!(state.record.is_none());
    assert!(matches!(
        f.writer()
            .apply_road_source_transaction(1, &[write(source.clone(), None)], &[])
            .unwrap(),
        RoadSourceWriteResult::Conflict(_)
    ));
    let deps = [
        RoadDependency {
            key: RoadRecordKey::Road(ROUTE),
            revision: 1,
        },
        RoadDependency {
            key: RoadRecordKey::Profile(PROFILE),
            revision: 1,
        },
        RoadDependency {
            key: RoadRecordKey::Knot(RoadKnotId(id(1))),
            revision: 1,
        },
        RoadDependency {
            key: RoadRecordKey::Knot(RoadKnotId(id(2))),
            revision: 1,
        },
    ];
    let restored = commit(
        f.writer()
            .apply_road_source_transaction(1, &[write(source, Some(2))], &deps)
            .unwrap(),
    );
    assert_eq!(restored.records[0].revision, Some(3));
    assert!(matches!(
        f.writer()
            .apply_road_source_transaction(
                1,
                &[RoadSourceWrite {
                    key,
                    expected_revision: Some(1),
                    record: None
                }],
                &[]
            )
            .unwrap(),
        RoadSourceWriteResult::Conflict(_)
    ));
}
#[test]
fn invalid_geometry_dependencies_or_topology_roll_back_the_whole_gesture() {
    let f = Fixture::new(1);
    let snapshot = f.snapshot();
    let before = f.records();
    let mut k = knot(&f.reader().connection, RoadKnotId(id(1))).unwrap();
    k.knot.position.local[0] = f64::NAN;
    assert!(
        f.writer()
            .apply_road_source_transaction(
                1,
                &[write(RoadSourceRecord::Knot(k), Some(1))],
                &snapshot.dependencies()
            )
            .is_err()
    );
    assert_eq!(before, f.records());
    let key = RoadRecordKey::Knot(RoadKnotId(id(1)));
    assert!(
        f.writer()
            .apply_road_source_transaction(
                1,
                &[RoadSourceWrite {
                    key,
                    expected_revision: Some(1),
                    record: None
                }],
                &[]
            )
            .is_err()
    );
    assert_eq!(before, f.records());
    let mut r = route(&f.reader().connection, ROUTE).unwrap();
    r.road.name = "Missing dependency CAS".into();
    assert!(
        f.writer()
            .apply_road_source_transaction(1, &[write(RoadSourceRecord::Road(r), Some(1))], &[])
            .is_err()
    );
    let mut duplicate = match required(
        &f.reader().connection,
        RoadRecordKey::Span(RoadSpanId(id(1))),
    )
    .unwrap()
    {
        RoadSourceRecord::Span(s) => s,
        _ => unreachable!(),
    };
    duplicate.id = RoadSpanId(id(9));
    assert!(
        f.writer()
            .apply_road_source_transaction(
                1,
                &[write(RoadSourceRecord::Span(duplicate), None)],
                &snapshot.dependencies()
            )
            .is_err()
    );
    assert_eq!(before, f.records());
}
#[test]
fn shared_profile_edits_have_paged_dependencies_and_validate_distant_corridors() {
    let f = Fixture::new(9);
    let mut p = profile(&f.reader().connection, PROFILE).unwrap();
    p.breakup = 0.95;
    let saved = commit(
        f.writer()
            .apply_road_source_transaction(
                1,
                &[write(RoadSourceRecord::Profile(p.clone()), Some(1))],
                &[],
            )
            .unwrap(),
    );
    assert!(saved.bounds.is_empty());
    assert_eq!(
        saved.dependencies,
        vec![RoadDependencySelector::Profile(PROFILE)]
    );
    let mut seen = vec![];
    let mut cursor = None;
    loop {
        let page = f
            .reader()
            .read_road_dependency_spans(RoadDependencySelector::Profile(PROFILE), cursor, 2)
            .unwrap();
        assert_eq!(page.road_revision, saved.revision);
        assert_eq!(page.library_revision, 1);
        seen.extend(page.spans.iter().map(|(id, _)| *id));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, (1..=9).map(|n| RoadSpanId(id(n))).collect::<Vec<_>>());
    let ground = f
        .reader()
        .read_road_dependency_spans(RoadDependencySelector::GroundPreset(p.ground), None, 16)
        .unwrap();
    assert_eq!(ground.spans.len(), 9);
    p.track_spacing = 5.0;
    assert!(
        f.writer()
            .apply_road_source_transaction(1, &[write(RoadSourceRecord::Profile(p), Some(2))], &[])
            .is_err()
    );
    assert_eq!(
        profile(&f.reader().connection, PROFILE).unwrap().revision,
        2
    );
}
#[test]
fn shared_ground_and_world_edits_cannot_break_offscreen_roads() {
    let f = Fixture::new(1);
    let reader = f.reader();
    let mut library = reader.read_environment_presets().unwrap();
    library.presets.retain(|p| {
        p.id != environment::fixtures::DRY_GROUND
            && p.id != environment::fixtures::DRY_MEADOW
            && p.id != environment::fixtures::CLEARING
    });
    // Also remove references via composed dry meadow; this fixture's map uses green only.
    assert!(
        f.writer()
            .apply_environment_source_transaction(1, Some(&library), &[], &[])
            .is_err()
    );
    assert_eq!(reader.read_environment_presets().unwrap().revision, 1);
    let mut definition = reader.read_environment_definitions().unwrap().remove(0);
    definition.surfaces.retain(|s| *s != SOIL);
    assert!(
        f.writer()
            .replace_environment_definition_if_revision(Some(1), &definition)
            .is_err()
    );
    assert_eq!(
        reader.read_environment_definitions().unwrap()[0].revision,
        1
    );
    let mut library = reader.read_environment_presets().unwrap();
    library
        .presets
        .iter_mut()
        .find(|p| p.id == environment::fixtures::DRY_GROUND)
        .unwrap()
        .name = "Renamed ground".into();
    f.writer()
        .apply_environment_source_transaction(1, Some(&library), &[], &[])
        .unwrap();
    assert!(matches!(
        f.writer()
            .apply_road_source_transaction(
                1,
                &[write(
                    RoadSourceRecord::Profile(profile(&reader.connection, PROFILE).unwrap()),
                    Some(1)
                )],
                &[]
            )
            .unwrap(),
        RoadSourceWriteResult::LibraryConflict { actual_revision: 2 }
    ));
}
#[test]
fn nearby_query_is_indexed_and_does_not_decode_a_long_routes_distant_knots() {
    let f = Fixture::new(1000);
    let c = Connection::open(&f.path).unwrap();
    c.execute(
        "UPDATE road_knots SET payload=x'00' WHERE id=?1",
        [id(1001).as_slice()],
    )
    .unwrap();
    let local = f.snapshot();
    assert!(!local.roads.truncated);
    assert_eq!(local.roads.spans.len(), 1);
    assert_eq!(local.roads.roads.len(), 1);
    assert_eq!(local.dependencies().len(), 5);
    let mut q=c.prepare("EXPLAIN QUERY PLAN SELECT span_id FROM road_span_cells WHERE world_space_id=1 AND cell_x=0 AND cell_z=0 ORDER BY span_id LIMIT 257").unwrap();
    let plan = q
        .query_map([], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join(" ");
    assert!(plan.contains("SEARCH") && plan.contains("INDEX"), "{plan}");
    assert!(
        f.reader()
            .read_roads_in_cells(SPACE, bounds(2499, 0))
            .is_err()
    );
}
#[test]
fn query_truncation_and_record_budgets_are_explicit() {
    let f = Fixture::new(1);
    let c = Connection::open(&f.path).unwrap();
    // Dense spatial membership can be inspected as partial metadata, never compiled as complete.
    let mut p = project();
    p.roads.records = fixture_records(1);
    for n in 2..=70u128 {
        let mut road = route(&c, ROUTE).unwrap();
        road.road.id = RoadId(id(10000 + n));
        p.roads.records.push(RoadSourceRecord::Road(road.clone()));
        for (offset, source_id) in [(0, 1), (1, 2)] {
            let mut k = knot(&c, RoadKnotId(id(source_id))).unwrap();
            k.road = road.road.id;
            k.knot.id = RoadKnotId(id(10000 + n * 2 + offset));
            p.roads.records.push(RoadSourceRecord::Knot(k));
        }
        p.roads.records.push(RoadSourceRecord::Span(SourceRoadSpan {
            id: RoadSpanId(id(10000 + n)),
            revision: 1,
            road: road.road.id,
            start: RoadKnotId(id(10000 + n * 2)),
            end: RoadKnotId(id(10001 + n * 2)),
        }));
    }
    let dense = f.directory.join("dense.sqlite");
    write_project_database(&dense, &p).unwrap();
    let reader = ProjectReader::open_read_only(&dense).unwrap();
    let snapshot = reader.read_roads_in_cells(SPACE, bounds(0, 0)).unwrap();
    assert!(snapshot.roads.truncated);
    assert_eq!(snapshot.roads.roads.len(), 64);
    assert!(snapshot.roads.validate().is_err());
    assert!(
        reader
            .read_roads_in_cells(
                SPACE,
                RoadCellBounds {
                    minimum: CellCoord::ZERO,
                    maximum: CellCoord { x: 100, z: 100 }
                }
            )
            .is_err()
    );
    let record = RoadSourceRecord::Profile(profile(&c, PROFILE).unwrap());
    assert!(
        f.writer()
            .apply_road_source_transaction(1, &vec![write(record, Some(1)); 65], &[])
            .is_err()
    );
}

#[test]
fn shared_profile_dependencies_cover_other_worlds_without_loading_their_controls() {
    let f = Fixture::new(1);
    let mut p = project();
    p.roads.records = fixture_records(1);
    let mut world = p.world_spaces[0].clone();
    world.id = WorldSpaceId(2);
    world.name = "Distant world".into();
    p.world_spaces.push(world);
    let mut d = p.environments[0].clone();
    d.space = WorldSpaceId(2);
    p.environments.push(d);
    for mut record in fixture_records(1) {
        match &mut record {
            RoadSourceRecord::Profile(_) => continue,
            RoadSourceRecord::Junction(_) => unreachable!("fixture has no junctions"),
            RoadSourceRecord::Road(r) => {
                r.space = WorldSpaceId(2);
                r.road.id = RoadId([2; 16]);
            }
            RoadSourceRecord::Knot(k) => {
                k.road = RoadId([2; 16]);
                k.knot.id.0[0] = 2;
            }
            RoadSourceRecord::Span(s) => {
                s.road = RoadId([2; 16]);
                s.id = RoadSpanId([2; 16]);
                s.start.0[0] = 2;
                s.end.0[0] = 2;
            }
        }
        p.roads.records.push(record);
    }
    let path = f.directory.join("two-worlds.sqlite");
    write_project_database(&path, &p).unwrap();
    let reader = ProjectReader::open_read_only(&path).unwrap();
    let mut writer = ProjectWriter::open(&path).unwrap();
    let page = reader
        .read_road_dependency_spans(RoadDependencySelector::Profile(PROFILE), None, 8)
        .unwrap();
    assert_eq!(
        page.spans
            .iter()
            .map(|(_, b)| b.space)
            .collect::<BTreeSet<_>>(),
        [SPACE, WorldSpaceId(2)].into_iter().collect()
    );
    let local = reader.read_roads_in_cells(SPACE, bounds(0, 0)).unwrap();
    assert_eq!(local.roads.roads.len(), 1);
    assert_eq!(local.roads.roads[0].id, ROUTE);
    let mut d = p.environments[1].clone();
    d.surfaces.retain(|s| *s != SOIL);
    let error = writer
        .replace_environment_definition_if_revision(Some(1), &d)
        .unwrap_err();
    assert!(error.to_string().contains("road Ground preset"), "{error}");
}
#[test]
fn deleting_a_dependency_cursor_changes_epoch_and_does_not_skip_the_next_span() {
    let f = Fixture::new(4);
    let selector = RoadDependencySelector::Road(ROUTE);
    let page = f
        .reader()
        .read_road_dependency_spans(selector, None, 2)
        .unwrap();
    let cursor = page.next_cursor.unwrap();
    commit(
        f.writer()
            .apply_road_source_transaction(
                1,
                &[RoadSourceWrite {
                    key: RoadRecordKey::Span(cursor),
                    expected_revision: Some(1),
                    record: None,
                }],
                &[],
            )
            .unwrap(),
    );
    let next = f
        .reader()
        .read_road_dependency_spans(selector, Some(cursor), 2)
        .unwrap();
    assert_ne!(page.road_revision, next.road_revision);
    assert_eq!(
        next.spans.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![RoadSpanId(id(3)), RoadSpanId(id(4))]
    );
    assert!(next.next_cursor.is_none());
}
#[test]
fn width_changes_reindex_old_and_new_cells_and_too_large_indexes_fail_atomically() {
    let f = Fixture::new(1);
    let old = f.snapshot();
    let reader = f.reader();
    let mut k = knot(&reader.connection, RoadKnotId(id(1))).unwrap();
    k.knot.width = 30.0;
    let saved = commit(
        f.writer()
            .apply_road_source_transaction(
                1,
                &[write(RoadSourceRecord::Knot(k), Some(1))],
                &old.dependencies(),
            )
            .unwrap(),
    );
    assert!(saved.bounds[1].bounds.minimum.z < saved.bounds[0].bounds.minimum.z);
    assert!(
        !reader
            .read_roads_in_cells(SPACE, bounds(0, -2))
            .unwrap()
            .roads
            .spans
            .is_empty()
    );
    let deps = f.snapshot().dependencies();
    let before = f.records();
    let mut k = knot(&reader.connection, RoadKnotId(id(1))).unwrap();
    k.knot.outgoing = [512.0, 512.0];
    assert!(
        f.writer()
            .apply_road_source_transaction(1, &[write(RoadSourceRecord::Knot(k), Some(2))], &deps)
            .is_err()
    );
    assert_eq!(f.records(), before);
}

#[test]
fn aggregate_spatial_index_work_is_bounded_and_rolls_back_partially_staged_indexes() {
    let f = Fixture::new(0);
    let mut records = fixture_records(12);
    for record in &mut records {
        if let RoadSourceRecord::Knot(k) = record {
            let n = u128::from_be_bytes(k.knot.id.0) - 1;
            k.knot.position = RoadPoint::from_relative(
                CellCoord::ZERO,
                [400.0 * n as f64, 400.0 * n as f64],
                8.0,
            )
            .unwrap();
        }
    }
    let writes = records
        .into_iter()
        .map(|r| write(r, None))
        .collect::<Vec<_>>();
    let error = f
        .writer()
        .apply_road_source_transaction(1, &writes, &[])
        .unwrap_err();
    assert!(
        error.to_string().contains("spatial-index write budget"),
        "{error}"
    );
    assert!(f.records().is_empty());
    assert_eq!(f.snapshot().revision, 1);
    assert!(f.snapshot().roads.spans.is_empty());
}

#[test]
fn authoring_snapshot_loads_incident_spans_once_without_walking_the_whole_route() {
    let fixture = Fixture::new(30);
    let snapshot = fixture
        .reader()
        .read_road_authoring_snapshot(SPACE, bounds(0, 0), &[])
        .unwrap();
    assert!(snapshot.complete_knots.contains(&RoadKnotId(id(2))));
    let spans = snapshot
        .records
        .iter()
        .filter_map(|r| {
            if let Some(RoadSourceRecord::Span(s)) = &r.record {
                Some(s.id)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert!(spans.contains(&RoadSpanId(id(2))));
    assert!(spans.len() < 5, "one hop must not load all 30 spans");
    assert!(!snapshot.complete_knots.contains(&RoadKnotId(id(30))));
}
#[test]
fn combined_road_and_preset_save_rolls_back_both_on_road_conflict() {
    let fixture = Fixture::new(1);
    let mut library = fixture.reader().read_environment_presets().unwrap();
    let original = library.clone();
    library.presets[0].name = "Shared edit with road".into();
    let mut record = fixture
        .records()
        .into_iter()
        .find(|r| matches!(r, RoadSourceRecord::Knot(_)))
        .unwrap();
    if let RoadSourceRecord::Knot(k) = &mut record {
        k.knot.width += 0.2;
    }
    let deps = fixture.snapshot().dependencies();
    let stale = fixture
        .writer()
        .apply_environment_and_roads_transaction(
            original.revision,
            Some(&library),
            &[],
            &[],
            &[write(record.clone(), Some(99))],
            &[],
        )
        .unwrap();
    assert!(matches!(
        stale,
        crate::EnvironmentSourceWriteResult::RoadConflict { .. }
    ));
    assert_eq!(
        fixture.reader().read_environment_presets().unwrap(),
        original
    );
    let outcome = fixture
        .writer()
        .apply_environment_and_roads_transaction(
            original.revision,
            Some(&library),
            &[],
            &[],
            &[write(record, Some(1))],
            &deps,
        )
        .unwrap();
    let crate::EnvironmentSourceWriteResult::Committed(commit) = outcome else {
        panic!("commit expected")
    };
    assert_eq!(commit.roads.unwrap().records.len(), 1);
    assert_eq!(commit.presets.unwrap().revision, original.revision + 1);
    assert_eq!(
        fixture.reader().read_environment_presets().unwrap().presets[0].name,
        "Shared edit with road"
    );
}

#[test]
fn distant_empty_authoring_window_still_loads_the_bounded_shared_style_library() {
    let fixture = Fixture::new(1);
    let snapshot = fixture
        .reader()
        .read_road_authoring_snapshot(SPACE, bounds(500, 500), &[])
        .unwrap();
    assert_eq!(snapshot.records.len(), 1);
    assert_eq!(snapshot.records[0].key, RoadRecordKey::Profile(PROFILE));
    assert!(snapshot.complete_knots.is_empty());
}

#[test]
fn authoring_usage_aggregates_distant_knots_without_loading_them() {
    let f = Fixture::new(80);
    let narrow = RoadKnotId(id(80));
    let RoadSourceRecord::Knot(mut k) = f
        .reader()
        .read_road_records(&[RoadRecordKey::Knot(narrow)])
        .unwrap()[0]
        .record
        .clone()
        .unwrap()
    else {
        panic!()
    };
    k.knot.width = 3.0;
    let result = f
        .writer()
        .apply_road_source_transaction(
            1,
            &[RoadSourceWrite {
                key: RoadRecordKey::Knot(narrow),
                expected_revision: Some(1),
                record: Some(RoadSourceRecord::Knot(k)),
            }],
            &[
                RoadDependency {
                    key: RoadRecordKey::Road(ROUTE),
                    revision: 1,
                },
                RoadDependency {
                    key: RoadRecordKey::Profile(PROFILE),
                    revision: 1,
                },
            ],
        )
        .unwrap();
    assert!(matches!(result, RoadSourceWriteResult::Committed(_)));
    let s = f
        .reader()
        .read_road_authoring_snapshot(
            SPACE,
            RoadCellBounds {
                minimum: CellCoord::ZERO,
                maximum: CellCoord::ZERO,
            },
            &[],
        )
        .unwrap();
    assert!(s.records.len() < 20);
    assert!(
        !s.records
            .iter()
            .any(|r| r.key == RoadRecordKey::Knot(narrow))
    );
    assert_eq!(s.style_usage[&PROFILE][0].minimum_width, Some(3.0));
    assert_eq!(s.style_usage[&PROFILE][0].road_count, 1);
    assert_eq!(s.route_minimum_widths[&ROUTE], Some(3.0));
}

fn junction_document(count: usize) -> ProjectDocument {
    let mut p = project();
    p.roads.records = fixture_records(count);
    let target = RoadKnotId(id(2));
    let point = match &p
        .roads
        .records
        .iter()
        .find(|r| r.key() == RoadRecordKey::Knot(target))
        .unwrap()
    {
        RoadSourceRecord::Knot(k) => k.knot.position,
        _ => unreachable!(),
    };
    let mut road = match &p.roads.records[1] {
        RoadSourceRecord::Road(r) => r.clone(),
        _ => unreachable!(),
    };
    road.road.id = RoadId([90; 16]);
    let start = RoadKnot {
        id: RoadKnotId([91; 16]),
        revision: 1,
        position: point,
        incoming: [0.0; 2],
        outgoing: [0.0, 4.0],
        width: 3.5,
    };
    let end = RoadKnot {
        id: RoadKnotId([92; 16]),
        revision: 1,
        position: RoadPoint::from_relative(
            point.cell,
            [point.local[0], point.local[1] + 12.0],
            8.0,
        )
        .unwrap(),
        incoming: [0.0, -4.0],
        outgoing: [0.0; 2],
        width: 3.5,
    };
    p.roads.records.extend([
        RoadSourceRecord::Road(road.clone()),
        RoadSourceRecord::Knot(SourceRoadKnot {
            road: road.road.id,
            knot: start.clone(),
        }),
        RoadSourceRecord::Knot(SourceRoadKnot {
            road: road.road.id,
            knot: end.clone(),
        }),
        RoadSourceRecord::Span(SourceRoadSpan {
            id: RoadSpanId([90; 16]),
            revision: 1,
            road: road.road.id,
            start: start.id,
            end: end.id,
        }),
        RoadSourceRecord::Junction(SourceRoadJunction {
            space: SPACE,
            junction: RoadJunction {
                id: RoadJunctionId([90; 16]),
                revision: 1,
                position: point,
                radius: 3.5,
                profile: PROFILE,
                seed: 23,
                knots: vec![target, start.id],
            },
        }),
    ]);
    p
}
#[test]
fn junction_queries_complete_one_connection_without_loading_the_route_network() {
    let f = Fixture::new(0);
    let mut p = junction_document(1000);
    let RoadSourceRecord::Junction(j) = p.roads.records.last_mut().unwrap() else {
        unreachable!()
    };
    j.junction.radius = 16.0;
    let path = f.directory.join("junction.sqlite");
    write_project_database(&path, &p).unwrap();
    let reader = ProjectReader::open_read_only(&path).unwrap();
    let source = reader.read_roads_in_cells(SPACE, bounds(2, 0)).unwrap();
    let dependencies = reader
        .read_road_dependency_spans(RoadDependencySelector::Road(ROUTE), None, 1)
        .unwrap();
    assert!(
        dependencies.spans[0]
            .1
            .bounds
            .contains(CellCoord { x: 2, z: 2 }),
        "shared-style invalidation must include the junction beyond the corridor padding"
    );
    assert_eq!(source.roads.junctions.len(), 1);
    assert!(source.roads.spans.len() < 8);
    let index = RoadDocumentIndex::new(&p.roads, &p.environments).unwrap();
    assert_eq!(
        index.snapshot(SPACE, bounds(2, 0), 8.0).unwrap(),
        source.roads
    );
    let authoring = reader
        .read_road_authoring_snapshot(
            SPACE,
            bounds(40, 40),
            &[RoadRecordKey::Junction(RoadJunctionId([90; 16]))],
        )
        .unwrap();
    assert!(authoring.records.len() < 20);
    assert!(authoring.complete_knots.contains(&RoadKnotId(id(2))));
    assert!(authoring.complete_knots.contains(&RoadKnotId([91; 16])));
    // Duplicate membership and foreign positions are rejected by the offline source path too.
    let mut bad = p.clone();
    let RoadSourceRecord::Junction(j) = bad.roads.records.last_mut().unwrap() else {
        unreachable!()
    };
    j.junction.position.local[0] += 0.5;
    assert!(RoadDocumentIndex::new(&bad.roads, &bad.environments).is_err());
}
#[test]
fn incomplete_junction_moves_and_stale_membership_roll_back_every_record() {
    let f = Fixture::new(0);
    let p = junction_document(2);
    let path = f.directory.join("junction.sqlite");
    write_project_database(&path, &p).unwrap();
    let reader = ProjectReader::open_read_only(&path).unwrap();
    let mut writer = ProjectWriter::open(&path).unwrap();
    let source = reader.read_roads_in_cells(SPACE, bounds(2, 0)).unwrap();
    let before = crate::read_project_database(&path).unwrap().roads;
    let mut record = reader
        .read_road_records(&[RoadRecordKey::Knot(RoadKnotId(id(2)))])
        .unwrap()
        .remove(0)
        .record
        .unwrap();
    let RoadSourceRecord::Knot(k) = &mut record else {
        unreachable!()
    };
    k.knot.position.local[0] += 0.5;
    assert!(
        writer
            .apply_road_source_transaction(
                1,
                &[write(record.clone(), Some(1))],
                &source.dependencies()
            )
            .is_err()
    );
    assert_eq!(crate::read_project_database(&path).unwrap().roads, before);
    let mut j = before
        .records
        .iter()
        .find(|r| matches!(r, RoadSourceRecord::Junction(_)))
        .unwrap()
        .clone();
    let RoadSourceRecord::Junction(node) = &mut j else {
        unreachable!()
    };
    node.junction.radius = 4.0;
    commit(
        writer
            .apply_road_source_transaction(1, &[write(j, Some(1))], &source.dependencies())
            .unwrap(),
    );
    assert!(matches!(
        writer
            .apply_road_source_transaction(1, &[write(record, Some(1))], &source.dependencies())
            .unwrap(),
        RoadSourceWriteResult::Conflict(_)
    ));
}
