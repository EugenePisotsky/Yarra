//! Production cook: one stable source snapshot, bounded local inputs, staged output.
use super::*;
use cell_cook::CookCatalog;
use environment_compile::CompilePlan;
use environment_cook::prepare_plans;
use std::collections::BTreeSet;
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
    /// Wall-clock time compiling and writing cells, then building the hierarchy and control
    /// tiles and validating the publication.
    pub cell_seconds: f64,
    pub publish_seconds: f64,
    /// The part of `publish_seconds` spent building the terrain hierarchy.
    pub hierarchy_seconds: f64,
    /// Continued the previous publication: `terrain_cells` counts only changed cells.
    pub incremental: bool,
    /// Recompiled cells whose ground page changed; only their terrain nodes are rebuilt.
    pub changed_ground_cells: u64,
    /// Impostor-drawn objects regrouped into far-object blocks, rebuilt by every cook.
    pub far_object_blocks: usize,
    pub far_objects: usize,
}

#[derive(Debug, Clone)]
pub struct CookReport {
    pub manifest: RuntimeManifest,
    pub stats: CookStats,
}

/// Cooks continue the existing runtime at `runtime_path` when it was cooked from the same
/// global inputs and terrain cells, recompiling only cells whose inputs changed.
pub fn cook_project_with_report(project_path: &Path, runtime_path: &Path) -> Result<CookReport> {
    cook_project_options(project_path, runtime_path, true)
}

/// Recompiles every cell, ignoring the existing runtime; for sources edited without
/// revision bumps.
pub fn cook_project_fresh(project_path: &Path, runtime_path: &Path) -> Result<CookReport> {
    cook_project_options(project_path, runtime_path, false)
}

fn cook_project_options(
    project_path: &Path,
    runtime_path: &Path,
    incremental: bool,
) -> Result<CookReport> {
    if runtime_path.exists() && fs::canonicalize(project_path)? == fs::canonicalize(runtime_path)? {
        bail!("source and runtime paths must be different");
    }

    let snapshot = ProjectCookSnapshot::open(project_path)
        .with_context(|| format!("failed to open cook snapshot {}", project_path.display()))?;
    cook_snapshot_options(snapshot, runtime_path, incremental)
}

#[cfg(test)]
fn cook_snapshot(snapshot: ProjectCookSnapshot, runtime_path: &Path) -> Result<CookReport> {
    cook_snapshot_options(snapshot, runtime_path, true)
}

fn cook_snapshot_options(
    snapshot: ProjectCookSnapshot,
    runtime_path: &Path,
    incremental: bool,
) -> Result<CookReport> {
    let project = snapshot.catalog();
    let plans = prepare_plans(project)?;
    let mut environment = blake3::Hasher::new();

    for plan in plans.values() {
        environment.update(&plan.fingerprint());
    }

    let catalog = CookCatalog::new(project, *environment.finalize().as_bytes())?;
    let header_hash = catalog.header.manifest.content_hash;
    let mut stats = CookStats::default();
    let start = std::time::Instant::now();
    let mut fingerprints = BTreeMap::new();

    for world in &project.world_spaces {
        match project.environments.iter().find(|d| d.space == world.id) {
            Some(definition) => {
                let cells = snapshot.cell_fingerprints(definition, &header_hash)?;
                fingerprints.insert(world.id, cells);
            }
            None if !snapshot.next_cells(world.id, None)?.is_empty() => {
                bail!("world with source cells requires an environment definition")
            }
            None => {}
        }
    }

    let staging = RuntimeStaging::new(runtime_path)?;
    let writer = match incremental
        .then(|| continue_publication(runtime_path, &staging.path, &catalog, &fingerprints))
        .transpose()?
        .flatten()
    {
        Some(writer) => {
            stats.incremental = true;
            writer
        }
        None => RuntimeCookWriter::create(&staging.path, &catalog.header)?,
    };
    let mut changes = Changes {
        cells: Default::default(),
        ground: Default::default(),
    };

    for world in &project.world_spaces {
        let Some(cells) = fingerprints.get(&world.id) else {
            continue;
        };
        let previous = if stats.incremental {
            writer.cell_fingerprints(world.id)?
        } else {
            BTreeMap::new()
        };

        // Unchanged cells keep their published pages. Paint-only cells have no pages and
        // are validated again every time.
        let keys: Vec<_> = cells
            .iter()
            .filter(|(cell, fingerprint)| previous.get(cell) != Some(&fingerprint.input))
            .map(|(&cell, _)| cell)
            .collect();
        let definition = project
            .environments
            .iter()
            .find(|d| d.space == world.id)
            .context("world with source cells requires an environment definition")?;

        // Cells whose control tile changed: their soft layer, and their neighbours', read it.
        let mut controls = BTreeSet::new();

        for batch in keys.chunks(world_db::COOK_KEY_BATCH) {
            let input = CellInput {
                snapshot: &snapshot,
                catalog: &catalog,
                plan: &plans[&world.id],
                definition,
            };

            for (cell, cooked) in batch.iter().zip(compile_cells(input, batch, &mut stats)?) {
                let Some(cooked) = cooked else {
                    stats.coverage_only_cells += 1;
                    continue;
                };
                stats.peak_encoded_cell_bytes = stats
                    .peak_encoded_cell_bytes
                    .max(cooked.pages.iter().map(|p| p.payload.len() as u64).sum());
                stats.peak_decoded_cell_bytes = stats
                    .peak_decoded_cell_bytes
                    .max(cooked.pages.iter().map(|p| p.decoded_bytes).sum());

                if stats.incremental {
                    // Cells whose ground page is unchanged keep their terrain nodes.
                    let ground = PageKey {
                        space: world.id,
                        cell: *cell,
                        domain: PageDomain::Terrain,
                    };
                    let before = writer.page_checksum(ground)?;
                    let after = cooked.pages.iter().find(|p| p.key == ground);

                    if before != after.map(|p| p.checksum) {
                        changes.ground.entry(world.id).or_default().insert(*cell);
                    }

                    if writer.control_payload(world.id, *cell)?.as_ref() != Some(&cooked.control) {
                        controls.insert(*cell);
                    }

                    writer.remove_cell(world.id, *cell)?;
                }

                writer.append_cell(&cooked, cells[cell].input)?;
                changes.cells.entry(world.id).or_default().insert(*cell);
                stats.terrain_cells += 1;
            }
        }

        for cell in controls
            .iter()
            .flat_map(|&c| world::soft_layer_neighbours(c))
        {
            let ground = PageKey {
                space: world.id,
                cell,
                domain: PageDomain::Terrain,
            };

            if writer.page_checksum(ground)?.is_some() {
                changes.ground.entry(world.id).or_default().insert(cell);
            }
        }
    }

    let far = writer.rebuild_far_objects()?;
    stats.far_object_blocks = far.blocks;
    stats.far_objects = far.instances;
    let manifest = writer.finish()?;
    stats.cell_seconds = start.elapsed().as_secs_f64();
    let start = std::time::Instant::now();

    // Release the read transaction before hierarchy work and publication. Every leaf
    // has already been evaluated from that one snapshot, including unsampled masks.
    drop(snapshot);
    stats.changed_ground_cells = changes.ground.values().map(|c| c.len() as u64).sum();
    let (manifest, hierarchy_seconds) = publish_staged_runtime(
        runtime_path,
        &staging.path,
        &manifest.world_spaces,
        stats.incremental.then_some(&changes),
    )?;
    stats.publish_seconds = start.elapsed().as_secs_f64();
    stats.hierarchy_seconds = hierarchy_seconds;
    Ok(CookReport { manifest, stats })
}

/// A copy of the previous publication to continue from, when it was cooked from the same
/// global inputs and the same set of terrain cells. Otherwise the cook starts afresh.
fn continue_publication(
    runtime_path: &Path,
    staging_path: &Path,
    catalog: &CookCatalog,
    fingerprints: &BTreeMap<WorldSpaceId, BTreeMap<CellCoord, world_db::CellFingerprint>>,
) -> Result<Option<RuntimeCookWriter>> {
    if !runtime_path.exists() {
        return Ok(None);
    }

    // A copy-on-write clone on APFS; the published file is never opened for writing.
    fs::copy(runtime_path, staging_path)?;
    let Ok(writer) = RuntimeCookWriter::open_incremental(staging_path) else {
        fs::remove_file(staging_path)?;
        return Ok(None);
    };
    let manifest = &catalog.header.manifest;
    let mut same = writer.header_hash()? == manifest.content_hash;

    for (space, cells) in fingerprints {
        if !same {
            break;
        }

        let terrain: BTreeSet<_> = cells
            .iter()
            .filter(|(_, f)| f.source)
            .map(|(&cell, _)| cell)
            .collect();
        same = writer
            .cell_fingerprints(*space)?
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            == terrain;
    }

    if !same {
        drop(writer);
        fs::remove_file(staging_path)?;
        return Ok(None);
    }

    // Outside the header hash, so a moved start does not recook anything.
    writer.set_start_view(manifest.start_view.as_ref())?;
    writer.set_gameplay_areas(&manifest.gameplay_areas)?;
    Ok(Some(writer))
}

/// What every cell of one world is compiled and encoded with.
#[derive(Clone, Copy)]
struct CellInput<'a> {
    snapshot: &'a ProjectCookSnapshot,
    catalog: &'a CookCatalog,
    plan: &'a CompilePlan,
    definition: &'a environment::EnvironmentDefinition,
}

/// Reads a batch of cells here and compiles and encodes them on every core; `None` for
/// paint-only cells, which are validated. Output order matches `batch`.
fn compile_cells(
    input: CellInput,
    batch: &[CellCoord],
    stats: &mut CookStats,
) -> Result<Vec<Option<RuntimeCell>>> {
    let space = input.definition.space;
    let sources = batch
        .iter()
        .map(|&cell| {
            let source = input
                .snapshot
                .read_cell(input.definition, cell)
                .with_context(|| format!("could not read source cell {space:?} {cell:?}"))?;
            Ok((cell, source))
        })
        .collect::<Result<Vec<_>>>()?;

    for (_, source) in &sources {
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
    }

    parallel::map(&sources, |(cell, source)| -> Result<Option<RuntimeCell>> {
        let cell = *cell;
        let Some(base) = &source.source else {
            input.plan.validate_coverage(&[cell], &source.coverage)?;
            return Ok(None);
        };
        let compiled = input
            .plan
            .compile_cell_with_terrain(
                cell,
                &source.coverage,
                &source.roads.roads,
                &source.terrain,
                Default::default(),
            )
            .with_context(|| format!("could not compile source cell {space:?} {cell:?}"))?;
        input
            .catalog
            .encode_cell(base, &source.objects, compiled)
            .with_context(|| format!("could not encode cell {space:?} {cell:?}"))
            .map(Some)
    })
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use environment::*;

    use world_db::{
        EnvironmentCellWrite, EnvironmentCellWriteResult, ProjectWriter, RuntimeReader,
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
        profile.clouds = world::clouds::CloudSettings::overcast();
        profile.clouds.seed = 192;
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

    #[test]
    fn repainting_an_area_reaches_the_runtime_without_recooking_a_cell() {
        let fixture = Fixture::new(&terrain_fixture::mountain_project(1));
        let first = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert!(first.manifest.gameplay_areas.is_empty());
        let areas = [world::GameplayArea {
            name: "camp/fire".into(),
            space: first.manifest.default_world_space,
            points: vec![[0., 0.], [6., 0.], [6., 6.]],
            height: None,
        }];
        assert!(matches!(
            ProjectWriter::open(&fixture.source())
                .unwrap()
                .write_gameplay_areas(1, &areas)
                .unwrap(),
            world_db::GameplayAreasWriteResult::Committed(_)
        ));
        let second = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert!(second.stats.incremental);
        assert_eq!(second.stats.terrain_cells, 0);
        assert_eq!(second.manifest.content_hash, first.manifest.content_hash);

        // The publication is a new generation all the same, so a running game sees it.
        assert_ne!(second.manifest.generation_id, first.manifest.generation_id);
        let reader = RuntimeReader::open_immutable(&fixture.runtime()).unwrap();
        assert_eq!(&*reader.manifest().gameplay_areas, &areas[..]);

        // A cook from scratch carries them too, and is the same generation.
        let full = fixture.dir.join("full.sqlite");
        cook_project_fresh(&fixture.source(), &full).unwrap();
        let reader = RuntimeReader::open_immutable(&full).unwrap();
        assert_eq!(&*reader.manifest().gameplay_areas, &areas[..]);
        assert_eq!(
            reader.manifest().generation_id,
            second.manifest.generation_id
        );
    }

    /// The control tiles of `cell` and its ancestors in both runtimes are the same.
    fn assert_same_controls(a: &Path, b: &Path, space: WorldSpaceId, cell: CellCoord) {
        let (a, b) = (
            RuntimeReader::open_immutable(a).unwrap(),
            RuntimeReader::open_immutable(b).unwrap(),
        );
        let mut key = world::TerrainNodeKey::leaf(space, cell);

        for _ in 0..=world::CONTROL_TOP_LEVEL {
            assert_eq!(
                a.read_terrain_controls(&[key]).unwrap(),
                b.read_terrain_controls(&[key]).unwrap(),
                "{key:?}"
            );
            key = key.parent().unwrap().unwrap();
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
    fn cook_scatters_collections_beside_roads_and_repeats_exactly() {
        let project = collection_project();
        let fixture = Fixture::new(&project);
        let report = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert_eq!(report.stats.terrain_cells, project.cells.len() as u64);
        assert!(report.stats.peak_source_height_samples <= 9 * 33 * 33);
        assert!(report.stats.peak_source_mask_bytes <= world_db::MAX_ENVIRONMENT_MASK_BYTES);
        let reader = RuntimeReader::open_immutable(&fixture.runtime()).unwrap();
        assert!(
            crate::decoded_pages(&reader, project.default_world_space)
                .into_iter()
                .any(|page| match page.payload {
                    PagePayload::StaticObjects(p) => p.instances.iter().any(|i| i.generated),
                    _ => false,
                })
        );
        assert_eq!(
            reader.manifest().vegetation_catalog,
            project.vegetation_catalog
        );
        drop(reader);
        let repeat = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert_eq!(report.manifest.content_hash, repeat.manifest.content_hash);
        let fresh = fixture.dir.join("fresh.sqlite");
        let fresh = cook_project_fresh(&fixture.source(), &fresh).unwrap();
        assert_eq!(report.manifest.content_hash, fresh.manifest.content_hash);
        fixture.assert_no_staging_files();
    }

    #[test]
    fn incremental_cook_recompiles_changed_cells_and_matches_a_full_cook() {
        let project = road_demo::document();
        let fixture = Fixture::new(&project);
        let first = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert!(!first.stats.incremental);
        let unchanged = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert!(unchanged.stats.incremental);
        assert_eq!(unchanged.stats.terrain_cells, 0);
        assert_eq!(unchanged.manifest.content_hash, first.manifest.content_hash);

        // Paint one cell: it and the neighbours whose halo reads it are recompiled.
        repaint(&project, &fixture, CellCoord { x: 2, z: 3 });
        let incremental = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert!(incremental.stats.incremental);
        assert_eq!(incremental.stats.terrain_cells, 9);
        assert_ne!(
            incremental.manifest.content_hash,
            first.manifest.content_hash
        );
        let full_path = fixture.dir.join("full.sqlite");
        let full = cook_project_fresh(&fixture.source(), &full_path).unwrap();
        assert!(!full.stats.incremental);
        assert_eq!(
            incremental.manifest.content_hash,
            full.manifest.content_hash
        );
        let painted = CellCoord { x: 2, z: 3 };
        let space = project.default_world_space;
        assert_same_controls(&fixture.runtime(), &full_path, space, painted);

        // Reshape the middle of one cell, within the demo's narrow height bounds. Samples next
        // to the border stay put, so neighbours' shared border normals do not change and only
        // this leaf and its ancestors move.
        let heightfield = project
            .terrain_cell_heightfields
            .iter()
            .find(|h| h.space == project.default_world_space && h.cell == CellCoord { x: 2, z: 3 })
            .unwrap();
        let n = usize::from(heightfield.resolution);
        let mut heights = heightfield.heights.clone();

        for z in 2..n - 2 {
            for x in 2..n - 2 {
                let h = &mut heights[z * n + x];
                *h += if *h > 0.0 { -0.25 } else { 0.25 };
            }
        }

        let bytes: Vec<u8> = heights.iter().flat_map(|h| h.to_le_bytes()).collect();
        rusqlite::Connection::open(fixture.source())
            .unwrap()
            .execute(
                "UPDATE terrain_cell_heightfields SET heights=?1, source_revision=source_revision+1 \
                 WHERE world_space_id=?2 AND cell_x=2 AND cell_z=3",
                rusqlite::params![bytes, project.default_world_space.0],
            )
            .unwrap();
        let raised = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert!(raised.stats.incremental);
        assert_eq!(raised.stats.changed_ground_cells, 1);
        let full = cook_project_fresh(&fixture.source(), &full_path).unwrap();
        assert_eq!(raised.manifest.content_hash, full.manifest.content_hash);
        assert_ne!(
            raised.manifest.content_hash,
            incremental.manifest.content_hash
        );
        fixture.assert_no_staging_files();
    }

    /// Paints a band across a cell: every layer's interior samples, so the ground shown changes;
    /// borders are shared with the neighbouring cells.
    fn repaint(project: &ProjectDocument, fixture: &Fixture, cell: CellCoord) {
        let record = project
            .environment_cells
            .iter()
            .find(|r| r.space == project.default_world_space && r.cell == cell)
            .unwrap();
        let mut edited = record.clone();
        let n = project.environments[0].mask_resolution as usize;

        for tile in &mut edited.tiles {
            for y in n / 3..n / 2 {
                for x in 1..n - 1 {
                    let sample = &mut tile.samples[y * n + x];
                    *sample = 255 - *sample;
                }
            }
        }

        assert!(matches!(
            ProjectWriter::open(&fixture.source())
                .unwrap()
                .apply_environment_cell_transaction(&[EnvironmentCellWrite {
                    expected_source_revision: Some(record.source_revision),
                    record: edited,
                }])
                .unwrap(),
            EnvironmentCellWriteResult::Committed(_)
        ));
    }

    #[test]
    fn repainting_soft_ground_rebuilds_the_neighbouring_leaves_its_layer_reaches() {
        // The demo's ground is meadow and dry meadow: snow on one, banks where they meet.
        let mut project = road_demo::document();
        let meadow = project
            .terrain_surfaces
            .iter_mut()
            .find(|s| s.key == "meadow")
            .unwrap();
        meadow.soft_depth = 0.4;

        let fixture = Fixture::new(&project);
        cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        let space = project.default_world_space;
        let node = |path: &Path, cell| {
            RuntimeReader::open_immutable(path)
                .unwrap()
                .read_terrain_node(world::TerrainNodeKey::leaf(space, cell))
                .unwrap()
                .unwrap()
                .decode()
                .unwrap()
        };
        let neighbour = CellCoord { x: 1, z: 3 };
        let before = node(&fixture.runtime(), neighbour);
        assert!(
            !before.soft_depths.is_empty(),
            "the fixture lies under a soft layer"
        );
        let published = fixture.dir.join("published.sqlite");
        std::fs::copy(fixture.runtime(), &published).unwrap();

        let painted = CellCoord { x: 2, z: 3 };
        repaint(&project, &fixture, painted);
        let incremental = cook_project_with_report(&fixture.source(), &fixture.runtime()).unwrap();
        assert!(incremental.stats.incremental);
        assert_ne!(
            node(&fixture.runtime(), neighbour),
            before,
            "the neighbour's layer reads the repainted cell"
        );
        let full_path = fixture.dir.join("full.sqlite");
        let full = cook_project_fresh(&fixture.source(), &full_path).unwrap();
        assert_eq!(
            incremental.manifest.content_hash,
            full.manifest.content_hash
        );
        assert_eq!(
            node(&fixture.runtime(), neighbour),
            node(&full_path, neighbour)
        );

        // The editor's live preview of the repainted cell over the old publication draws the
        // neighbour as the cook does.
        let leaf = crate::published_terrain_leaf(
            &RuntimeReader::open_immutable(&full_path).unwrap(),
            space,
            painted,
        )
        .unwrap()
        .unwrap();
        let preview = crate::preview_terrain(
            &RuntimeReader::open_immutable(&published).unwrap(),
            space,
            [(painted, std::sync::Arc::new(leaf))].into(),
            || false,
        )
        .unwrap();
        let key = world::TerrainNodeKey::leaf(space, neighbour);
        assert_eq!(*preview.nodes[&key], node(&full_path, neighbour));
        fixture.assert_no_staging_files();
    }

    #[test]
    fn one_snapshot_ignores_concurrent_edits_and_failed_cook_keeps_previous_runtime() {
        let project = road_demo::document();
        let fixture = Fixture::new(&project);
        let reference = crate::cook_document(&project, &fixture.dir.join("reference")).unwrap();
        let snapshot = ProjectCookSnapshot::open(&fixture.source()).unwrap();
        let record = project
            .environment_cells
            .iter()
            .find(|r| r.space == project.default_world_space && r.cell == CellCoord::ZERO)
            .unwrap();
        let mut edited = record.clone();
        let n = project.environments[0].mask_resolution as usize;

        // A patch of every layer, so the ground shown changes.
        for tile in &mut edited.tiles {
            for y in n / 2 - 2..n / 2 + 2 {
                for x in n / 2 - 2..n / 2 + 2 {
                    let sample = &mut tile.samples[y * n + x];
                    *sample = 255 - *sample;
                }
            }
        }

        edited.source_revision += 1;
        let result = ProjectWriter::open(&fixture.source())
            .unwrap()
            .apply_environment_cell_transaction(&[EnvironmentCellWrite {
                expected_source_revision: Some(record.source_revision),
                record: edited,
            }])
            .unwrap();
        assert!(matches!(result, EnvironmentCellWriteResult::Committed(_)));
        let old = cook_snapshot(snapshot, &fixture.runtime()).unwrap();
        assert_eq!(
            old.manifest.content_hash,
            RuntimeReader::open_immutable(&reference)
                .unwrap()
                .manifest()
                .content_hash
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
