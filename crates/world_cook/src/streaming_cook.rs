//! Production cook: one stable source snapshot, bounded local inputs, staged output.
use super::*;
use environment_compile::merge_runtime_catalogs;
use environment_cook::prepare_plans;
use world_db::{ProjectCookSnapshot, RuntimeCookWriter};

#[derive(Debug, Clone, Default)]
/// Logical per-cell input/output high-water marks, not total process RSS. Metadata,
/// SQLite page caches, output construction and compiler scratch space are separate.
pub struct CookStats {
    pub terrain_cells: u64,
    pub coverage_only_cells: u64,
    pub peak_source_height_samples: usize,
    pub peak_source_mask_bytes: usize,
    pub peak_source_manual_objects: usize,
    pub peak_source_road_spans: usize,
    pub peak_encoded_cell_bytes: u64,
    pub peak_decoded_cell_bytes: u64,
}
#[derive(Debug, Clone)]
pub struct CookReport {
    pub manifest: RuntimeManifest,
    pub stats: CookStats,
    pub materials: Option<TerrainMaterialBakeStats>,
}

pub fn cook_project_with_report(project_path: &Path, runtime_path: &Path) -> Result<CookReport> {
    cook_project_options(project_path, runtime_path, None)
}
/// Bake optional experimental ground composites using explicit preprocessed assets.
pub fn cook_project_with_materials(
    project_path: &Path,
    runtime_path: &Path,
    materials: &TerrainBakeLibrary,
) -> Result<CookReport> {
    cook_project_options(project_path, runtime_path, Some(materials))
}
fn cook_project_options(
    project_path: &Path,
    runtime_path: &Path,
    materials: Option<&TerrainBakeLibrary>,
) -> Result<CookReport> {
    if runtime_path.exists() && fs::canonicalize(project_path)? == fs::canonicalize(runtime_path)? {
        bail!("source and runtime paths must be different");
    }
    let snapshot = ProjectCookSnapshot::open(project_path)
        .with_context(|| format!("failed to open cook snapshot {}", project_path.display()))?;
    cook_snapshot_options(snapshot, runtime_path, materials)
}
#[cfg(test)]
fn cook_snapshot(snapshot: ProjectCookSnapshot, runtime_path: &Path) -> Result<CookReport> {
    cook_snapshot_options(snapshot, runtime_path, None)
}
fn cook_snapshot_options(
    snapshot: ProjectCookSnapshot,
    runtime_path: &Path,
    materials: Option<&TerrainBakeLibrary>,
) -> Result<CookReport> {
    let project = snapshot.catalog();
    let plans = prepare_plans(project)?;
    let catalog = merge_runtime_catalogs(&plans.values().collect::<Vec<_>>())?;
    let mut environment_hash = blake3::Hasher::new();
    for plan in plans.values() {
        environment_hash.update(&plan.fingerprint());
    }
    // This document contains bounded global metadata and no spatial data. Reusing the
    // existing packer keeps the validation/encoding contract common with reference tests.
    let header = build_compiled_runtime(
        project.clone(),
        CookedEnvironment::empty(catalog.clone(), *environment_hash.finalize().as_bytes()),
    )?;
    let staging = RuntimeStaging::new(runtime_path)?;
    let writer = RuntimeCookWriter::create(&staging.path, &header)?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"bounded-source-cook-v1");
    hash.update(&header.manifest.content_hash);
    let mut stats = CookStats::default();
    for world in &project.world_spaces {
        let mut cursor = None;
        loop {
            let keys = snapshot.next_cells(world.id, cursor)?;
            if keys.is_empty() {
                break;
            }
            let definition = project
                .environments
                .iter()
                .find(|d| d.space == world.id)
                .context("world with source cells requires an environment definition")?;
            let plan = &plans[&world.id];
            for &cell in &keys {
                let source = snapshot.read_cell(definition, cell).with_context(|| {
                    format!("could not read source cell {:?} {cell:?}", world.id)
                })?;
                stats.peak_source_height_samples = stats
                    .peak_source_height_samples
                    .max(source.terrain.cells.iter().map(|h| h.heights.len()).sum());
                stats.peak_source_mask_bytes = stats.peak_source_mask_bytes.max(
                    source
                        .coverage
                        .cells
                        .iter()
                        .flat_map(|c| &c.tiles)
                        .map(|t| t.samples.len())
                        .sum(),
                );
                stats.peak_source_manual_objects =
                    stats.peak_source_manual_objects.max(source.objects.len());
                stats.peak_source_road_spans = stats
                    .peak_source_road_spans
                    .max(source.roads.roads.spans.len());
                let Some(base) = source.source else {
                    plan.validate_coverage(&[cell], &source.coverage)?;
                    stats.coverage_only_cells += 1;
                    continue;
                };
                let compiled = plan
                    .compile_cell_with_terrain(
                        cell,
                        &source.coverage,
                        &source.roads.roads,
                        &source.terrain,
                        Default::default(),
                    )
                    .with_context(|| {
                        format!("could not compile source cell {:?} {cell:?}", world.id)
                    })?;
                let revision = source
                    .coverage
                    .cells
                    .iter()
                    .find(|c| c.cell == cell)
                    .context("missing requested coverage cell")?
                    .revision;
                let mut environment =
                    CookedEnvironment::empty(catalog.clone(), compiled.input_fingerprint);
                environment.push_cell(world.id, cell, i64::try_from(revision)?, compiled);
                let mut local = project.clone();
                local.cells.push(base);
                local.objects = source.objects;
                let batch = build_compiled_runtime(local, environment)?;
                stats.peak_encoded_cell_bytes = stats
                    .peak_encoded_cell_bytes
                    .max(batch.pages.iter().map(|p| p.payload.len() as u64).sum());
                stats.peak_decoded_cell_bytes = stats
                    .peak_decoded_cell_bytes
                    .max(batch.pages.iter().map(|p| p.decoded_bytes).sum());
                hash.update(&world.id.0.to_le_bytes());
                hash.update(&cell.x.to_le_bytes());
                hash.update(&cell.z.to_le_bytes());
                hash.update(&batch.manifest.content_hash);
                let descriptor = &batch.cells[0];
                hash.update(&descriptor.minimum_y.to_bits().to_le_bytes());
                hash.update(&descriptor.maximum_y.to_bits().to_le_bytes());
                hash.update(&descriptor.domain_mask.to_le_bytes());
                hash.update(&descriptor.source_revision.to_le_bytes());
                writer.append_cell(&batch)?;
                stats.terrain_cells += 1;
            }
            cursor = keys.last().copied();
        }
    }
    let manifest = writer.finish(*hash.finalize().as_bytes())?;
    // Release the read transaction before hierarchy work and publication. Every leaf
    // has already been evaluated from that one snapshot, including unsampled masks.
    drop(snapshot);
    let (manifest, materials) = finish_runtime_publication_with_materials(
        runtime_path,
        &staging.path,
        &manifest.world_spaces,
        materials,
    )?;
    Ok(CookReport {
        manifest,
        stats,
        materials,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use environment::*;
    use world_db::{
        DenseSourceWrite, DenseSourceWriteTransactionResult, ProjectWriter, RuntimeReader,
    };

    struct Fixture {
        dir: std::path::PathBuf,
    }
    impl Fixture {
        fn new(project: &ProjectDocument) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "yarra-stream-cook-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            let f = Self { dir };
            write_project_database(&f.source(), project).unwrap();
            f
        }
        fn source(&self) -> std::path::PathBuf {
            self.dir.join("source.sqlite")
        }
        fn runtime(&self) -> std::path::PathBuf {
            self.dir.join("runtime.sqlite")
        }
        fn assert_no_staging_files(&self) {
            assert!(fs::read_dir(&self.dir).unwrap().all(|e| {
                !e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".building")
            }));
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
    #[test]
    fn atmosphere_only_save_changes_generation_and_round_trips_without_changing_cells() {
        let fixture = Fixture::new(&terrain_fixture::mountain_project(1));
        let first = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        let id = first.manifest.default_world_space;
        let mut profile = first.manifest.default_world_space().atmosphere.clone();
        profile.night.exposure_ev100 = 7.5;
        profile.night.light_srgb = [0.4, 0.7, 1.0];
        let mut writer = ProjectWriter::open(&fixture.source()).unwrap();
        writer
            .write_atmospheres(&[world_db::AtmosphereWrite {
                space: id,
                expected_revision: 1,
                profile: profile.clone(),
            }])
            .unwrap();
        let old = RuntimeReader::open_immutable(&fixture.runtime()).unwrap();
        assert_ne!(old.manifest().default_world_space().atmosphere, profile);
        let second = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert_ne!(first.manifest.generation_id, second.manifest.generation_id);
        assert_eq!(first.stats.terrain_cells, second.stats.terrain_cells);
        let fresh = RuntimeReader::open_immutable(&fixture.runtime()).unwrap();
        assert_eq!(fresh.manifest().default_world_space().atmosphere, profile);
        assert_eq!(old.manifest().generation_id, first.manifest.generation_id);
    }

    fn assert_pages(reference: &RuntimeBuild, reader: &RuntimeReader) {
        for expected in &reference.pages {
            let actual = reader.read_page(expected.key).unwrap().unwrap();
            assert_eq!(
                actual.checksum, expected.checksum,
                "page {:?}",
                expected.key
            );
            assert_eq!(actual.decoded_bytes, expected.decoded_bytes);
            assert_eq!(actual.gpu_bytes_estimate, expected.gpu_bytes_estimate);
        }
    }
    fn collection_project() -> ProjectDocument {
        let mut project = road_demo::document();
        for record in &mut project.roads.records {
            if let world_db::RoadSourceRecord::Profile(p) = record {
                p.relief.road_depth = 0.1;
                p.relief.track_depth = 0.04;
            }
        }
        let id = PresetId([177; 16]);
        project.presets.presets.push(Preset {
            id,
            revision: 1,
            name: "Trees".into(),
            kind: PresetKind::AssetCollection(AssetCollection {
                id: OutputId([178; 16]),
                channel: ASSET_CHANNEL,
                assets: vec![CollectionAsset {
                    asset: project.assets[0].id,
                    weight: 1.0,
                    scale_min: 0.8,
                    scale_max: 1.2,
                }],
                spacing: 4.0,
                density: 1.0,
                seed: 1,
                max_slope_degrees: 60.0,
                road_clearance: 0.5,
            }),
        });
        let root = project
            .environments
            .iter()
            .find(|d| d.space == project.default_world_space)
            .unwrap()
            .layers[0]
            .preset;
        let PresetKind::Composition(children) = &mut project
            .presets
            .presets
            .iter_mut()
            .find(|p| p.id == root)
            .unwrap()
            .kind
        else {
            panic!()
        };
        children.push(PresetUse {
            id: PresetUseId([179; 16]),
            name: "Trees".into(),
            preset: id,
            overrides: vec![],
        });
        project
    }
    #[test]
    fn streamed_cook_matches_reference_pages_including_roads_and_collections() {
        let project = collection_project();
        let fixture = Fixture::new(&project);
        let reference = build_runtime(project).unwrap();
        assert!(
            reference
                .pages
                .iter()
                .any(|p| match p.clone().decode().unwrap().payload {
                    PagePayload::StaticObjects(p) => p.instances.iter().any(|i| i.generated),
                    _ => false,
                })
        );
        let report = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert_eq!(report.stats.terrain_cells, reference.cells.len() as u64);
        assert!(report.stats.peak_source_height_samples <= 9 * 33 * 33);
        assert!(report.stats.peak_source_mask_bytes <= world_db::MAX_ENVIRONMENT_MASK_BYTES);
        let reader = RuntimeReader::open_immutable(&fixture.runtime()).unwrap();
        assert_pages(&reference, &reader);
        assert_eq!(
            reader.manifest().vegetation_catalog,
            reference.manifest.vegetation_catalog
        );
        drop(reader);
        let repeat = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert_eq!(report.manifest.content_hash, repeat.manifest.content_hash);
        fixture.assert_no_staging_files();
    }
    #[test]
    fn one_snapshot_ignores_concurrent_edits_and_failed_cook_keeps_previous_runtime() {
        let project = road_demo::document();
        let fixture = Fixture::new(&project);
        let reference = build_runtime(project.clone()).unwrap();
        let snapshot = ProjectCookSnapshot::open(&fixture.source()).unwrap();
        let record = project
            .environment_cells
            .iter()
            .find(|r| r.space == project.default_world_space && r.cell == CellCoord::ZERO)
            .unwrap();
        let mut edited = record.clone();
        let n = project.environments[0].mask_resolution as usize;
        let index = (n / 2) * n + n / 2;
        edited.tiles[0].samples[index] = 255 - edited.tiles[0].samples[index];
        edited.source_revision += 1;
        let result = ProjectWriter::open(&fixture.source())
            .unwrap()
            .apply_dense_source_transaction(&[DenseSourceWrite::EnvironmentCoverage {
                expected_source_revision: Some(record.source_revision),
                record: edited,
            }])
            .unwrap();
        assert!(matches!(
            result,
            DenseSourceWriteTransactionResult::Committed(_)
        ));
        let old = cook_snapshot(snapshot, &fixture.runtime()).unwrap();
        assert_pages(
            &reference,
            &RuntimeReader::open_immutable(&fixture.runtime()).unwrap(),
        );
        let updated = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert_ne!(old.manifest.content_hash, updated.manifest.content_hash);
        // Invalid paint beyond the terrain must fail rather than silently disappear.
        let c = rusqlite::Connection::open(fixture.source()).unwrap();
        c.execute(
            "INSERT INTO environment_cells VALUES (?1,1000,1000,1)",
            [project.default_world_space.0],
        )
        .unwrap();
        c.execute(
            "INSERT INTO environment_coverage VALUES (?1,1000,1000,?2,?3)",
            rusqlite::params![
                project.default_world_space.0,
                [254_u8; 16].as_slice(),
                vec![0_u8; n * n]
            ],
        )
        .unwrap();
        drop(c);
        assert!(cook_project_with_report(&fixture.source(), &fixture.runtime()).is_err());
        assert_eq!(
            RuntimeReader::open_immutable(&fixture.runtime())
                .unwrap()
                .manifest()
                .content_hash,
            updated.manifest.content_hash
        );
        fixture.assert_no_staging_files();
    }
    #[test]
    fn source_and_runtime_must_be_distinct_and_catalog_overflow_is_explicit() {
        let project = road_demo::document();
        let fixture = Fixture::new(&project);
        assert!(cook_project_with_report(&fixture.source(), &fixture.source()).is_err());
        let c = rusqlite::Connection::open(fixture.source()).unwrap();
        c.execute(
            "UPDATE source_assets SET source_uri=CAST(zeroblob(?1) AS TEXT)",
            [world_db::MAX_COOK_CATALOG_BYTES as i64 + 1],
        )
        .unwrap();
        drop(c);
        let error = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap_err();
        assert!(format!("{error:#}").contains("catalog exceeds"));
        assert!(!fixture.runtime().exists());
        fixture.assert_no_staging_files();
    }
    #[test]
    fn missing_road_index_membership_is_rejected_before_publishing() {
        let project = road_demo::document();
        let fixture = Fixture::new(&project);
        let c = rusqlite::Connection::open(fixture.source()).unwrap();
        c.execute("DELETE FROM road_span_cells WHERE rowid IN (SELECT rowid FROM road_span_cells LIMIT 1)",[]).unwrap();
        drop(c);
        let error = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap_err();
        assert!(format!("{error:#}").contains("road spatial index"));
        assert!(!fixture.runtime().exists());
    }
    #[test]
    fn oversized_manual_cell_is_an_error_and_staged_writes_roll_back() {
        let project = road_demo::document();
        let fixture = Fixture::new(&project);
        let mut connection = rusqlite::Connection::open(fixture.source()).unwrap();
        let tx = connection.transaction().unwrap();
        {
            let mut q = tx
                .prepare(
                    "INSERT INTO object_placements VALUES (?1,?2,0,0,?3,1.0,0.0,1.0,0.0,1.0,1)",
                )
                .unwrap();
            for i in 0..=world_db::MAX_COOK_MANUAL_OBJECTS_PER_CELL {
                let id = ((245_u128 << 120) + i as u128).to_le_bytes();
                q.execute(rusqlite::params![
                    id.as_slice(),
                    project.default_world_space.0,
                    project.definitions[0].id.0.as_slice()
                ])
                .unwrap();
            }
        }
        tx.commit().unwrap();
        drop(connection);
        let error = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap_err();
        assert!(format!("{error:#}").contains("manual objects exceed"));
        assert!(!fixture.runtime().exists());
        fixture.assert_no_staging_files();
    }

    use crate::terrain_fixture::mountain_project;
    #[test]
    fn growing_landscape_keeps_source_sample_residency_constant() {
        let mut last = None;
        for half in [4, 8] {
            let fixture = Fixture::new(&mountain_project(half));
            let report = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
            assert_eq!(report.stats.terrain_cells, (half * 2).pow(2) as u64);
            assert_eq!(report.stats.peak_source_height_samples, 9 * 33 * 33);
            if let Some(previous) = last {
                assert_eq!(previous, report.stats.peak_source_height_samples);
            }
            last = Some(report.stats.peak_source_height_samples);
        }
    }
    #[test]
    #[ignore = "2km acceptance fixture; run after changing the production cook path"]
    fn two_kilometre_mountain_cooks_with_bounded_source_samples() {
        let fixture = Fixture::new(&mountain_project(32));
        let started = std::time::Instant::now();
        let report = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert_eq!(report.stats.terrain_cells, 4096);
        assert_eq!(report.stats.peak_source_height_samples, 9 * 33 * 33);
        let reader = RuntimeReader::open_immutable(&fixture.runtime()).unwrap();
        let roots = reader
            .read_terrain_roots(report.manifest.default_world_space)
            .unwrap();
        assert_eq!(roots.len(), 4);
        assert!(roots.iter().all(|r| r.key.level == 5));
        eprintln!(
            "2km terrain acceptance: {:.2}s, {:?}, {} coarse roots",
            started.elapsed().as_secs_f64(),
            report.stats,
            roots.len()
        );
    }
}
