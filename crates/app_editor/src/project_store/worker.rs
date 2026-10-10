//! The project database thread. It opens the project once, then answers window queries and
//! saves one at a time, in the order they were queued.
use super::*;

pub(super) fn start_project_worker(mut commands: Commands, path: Res<ProjectDatabasePath>) {
    let database_path = path.0.clone();
    let worker = ProjectWorker::spawn(
        "yarra-project-db",
        PROJECT_REQUEST_CAPACITY,
        PROJECT_REQUEST_CAPACITY + 1,
        move |requests, results| project_worker(database_path, requests, results),
    )
    .expect("failed to spawn the project database worker");
    commands.insert_resource(worker);
}

pub(super) fn project_worker(
    path: PathBuf,
    requests: Receiver<ProjectRequest>,
    results: Sender<ProjectResult>,
) {
    let reader = match ProjectReader::open_read_only(&path) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = results.send(ProjectResult::Opened(Err(format!(
                "could not open {}: {error}",
                path.display()
            ))));
            return;
        }
    };
    let mut writer = ProjectWriter::open(&path)
        .map_err(|error| format!("could not open {} for authoring: {error}", path.display()));
    let opened = open_snapshot(&reader, writer.as_ref().err().cloned());
    let failed = opened.is_err();
    if results.send(ProjectResult::Opened(opened)).is_err() || failed {
        return;
    }
    while let Ok(request) = requests.recv() {
        let result = match request {
            ProjectRequest::Query {
                revision,
                window,
                domains,
            } => ProjectResult::Query {
                revision,
                window,
                domains,
                result: query_window(&reader, window, domains).map_err(|error| error.to_string()),
            },
            ProjectRequest::SaveObjectTransaction(request) => {
                ProjectResult::SaveObjectTransaction {
                    request_id: request.request_id,
                    result: save(&mut writer, |writer| {
                        writer.apply_object_transaction(&request.writes)
                    }),
                }
            }
            ProjectRequest::SaveDenseTransaction(request) => ProjectResult::SaveDenseTransaction {
                request_id: request.request_id,
                result: save(&mut writer, |writer| {
                    writer.apply_environment_and_roads_transaction(
                        request.library_revision,
                        request.presets.as_ref(),
                        &request.definitions,
                        &request.writes,
                        &request.roads,
                        &request.road_dependencies,
                    )
                }),
            },
            ProjectRequest::SaveVegetationCatalog(request) => {
                let result = save(&mut writer, |writer| {
                    writer.replace_vegetation_catalog_if_matches(
                        request.expected.as_ref(),
                        &request.replacement,
                    )
                });
                ProjectResult::SaveVegetationCatalog {
                    request_id: request.request_id,
                    replacement: request.replacement,
                    result,
                }
            }
            ProjectRequest::SaveAtmospheres(id, writes) => ProjectResult::SaveAtmospheres(
                id,
                Box::new(save(&mut writer, |writer| {
                    writer.write_atmospheres(&writes)
                })),
            ),
            ProjectRequest::SaveGameplayAreas(id, expected_revision, areas) => {
                ProjectResult::SaveGameplayAreas(
                    id,
                    save(&mut writer, |writer| {
                        writer.write_gameplay_areas(expected_revision, &areas)
                    }),
                )
            }
        };
        if results.send(result).is_err() {
            return;
        }
    }
}

/// Everything the editor needs before its first window query.
fn open_snapshot(
    reader: &ProjectReader,
    write_error: Option<String>,
) -> Result<ProjectOpenSnapshot, String> {
    let (vegetation_catalog, presets, environments) = reader
        .read_environment_catalog()
        .map_err(|error| format!("could not read the project vegetation catalog: {error}"))?;
    let terrain_resources = environments
        .iter()
        .map(|d| reader.read_environment_terrain_resources(d.space))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not read environment materials: {error}"))?;
    let height_steps = environments
        .iter()
        .map(|d| {
            reader
                .read_terrain_height_step(d.space)
                .map(|step| (d.space, step))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()
        .map_err(|e| format!("could not read terrain height grids: {e}"))?;
    let gameplay_areas = reader
        .read_gameplay_areas()
        .map_err(|e| format!("could not read gameplay areas: {e}"))?;
    Ok(ProjectOpenSnapshot {
        gameplay_areas,
        manifest: reader.manifest().clone(),
        vegetation_catalog,
        environments,
        presets: Some(presets),
        terrain_resources,
        height_steps,
        write_error,
    })
}

fn query_window(
    reader: &ProjectReader,
    window: ProjectQueryWindow,
    domains: ProjectSourceDomains,
) -> Result<ProjectWindowSnapshot, world_db::WorldDbError> {
    let cells = reader.read_cells(
        window.space,
        window.minimum,
        window.maximum,
        MAX_SOURCE_CELLS,
    )?;
    let objects = domains
        .objects
        .then(|| {
            reader.read_object_views_in_cells(
                window.space,
                window.minimum,
                window.maximum,
                MAX_SOURCE_OBJECTS,
            )
        })
        .transpose()?;
    let environment_snapshot = domains
        .environment_coverage
        .then(|| {
            let mut core = Vec::new();
            for x in window.minimum.x..=window.maximum.x {
                for z in window.minimum.z..=window.maximum.z {
                    core.push(CellCoord { x, z });
                }
            }
            reader.read_environment_snapshot(
                window.space,
                &world_db::environment_dependency_cells(&core)?,
            )
        })
        .transpose()?;
    let environment_cells = environment_snapshot
        .as_ref()
        .map_or_else(Vec::new, |snapshot| {
            snapshot
                .coverage
                .cells
                .iter()
                .filter(|c| {
                    c.cell.x >= window.minimum.x
                        && c.cell.x <= window.maximum.x
                        && c.cell.z >= window.minimum.z
                        && c.cell.z <= window.maximum.z
                })
                .map(|c| SourceEnvironmentCellRecord {
                    space: window.space,
                    cell: c.cell,
                    source_revision: c.revision as i64,
                    definition_revision: snapshot.definition.revision,
                    tiles: c.tiles.clone(),
                })
                .collect()
        });
    Ok(ProjectWindowSnapshot {
        cells: cells.records,
        objects: objects
            .as_ref()
            .map_or_else(Vec::new, |query| query.records.clone()),
        environment_cells,
        environment_snapshot,
        cells_truncated: cells.truncated,
        objects_truncated: objects.is_some_and(|query| query.truncated),
        environment_coverage_truncated: false,
    })
}

/// Runs one save, or reports why the project could not be opened for writing.
fn save<T, E: ToString>(
    writer: &mut Result<ProjectWriter, String>,
    write: impl FnOnce(&mut ProjectWriter) -> Result<T, E>,
) -> Result<T, String> {
    match writer {
        Ok(writer) => write(writer).map_err(|error| error.to_string()),
        Err(error) => Err(error.clone()),
    }
}
