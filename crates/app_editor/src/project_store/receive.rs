//! Applies what the project database thread answered to the editor's view of the project.
use super::*;

pub(super) fn receive_project_results(
    worker: Option<Res<ProjectWorker>>,
    mut store: ResMut<ProjectEditorStore>,
) {
    let Some(worker) = worker else {
        return;
    };
    loop {
        match worker.try_recv() {
            Ok(ProjectResult::SaveAtmospheres(id, result)) => {
                let result = *result;
                if store.atmosphere_save_in_flight != Some(id) {
                    continue;
                }
                store.atmosphere_save_in_flight = None;
                if let Ok(world_db::AtmosphereWriteResult::Committed(records)) = &result {
                    store.source_epoch = store.source_epoch.wrapping_add(1).max(1);
                    if let Some(manifest) = &mut store.manifest {
                        for (id, revision, profile) in records {
                            if let Some(space) =
                                manifest.world_spaces.iter_mut().find(|s| s.id == *id)
                            {
                                space.atmosphere = profile.clone();
                                space.atmosphere_revision = *revision;
                            }
                        }
                    }
                }
                store.atmosphere_completion = Some((id, result));
            }

            Ok(ProjectResult::SaveGameplayAreas(id, result)) => {
                if store.area_save_in_flight != Some(id) {
                    continue;
                }
                store.area_save_in_flight = None;
                if let Ok(world_db::GameplayAreasWriteResult::Committed(record)) = &result {
                    // Publishing compares epochs to know the source moved on.
                    store.source_epoch = store.source_epoch.wrapping_add(1).max(1);
                    store.gameplay_areas = Some(record.clone());
                }
                store.area_completion = Some((id, result));
            }

            Ok(ProjectResult::Opened(result)) => match result {
                Ok(opened) => {
                    store.gameplay_areas = Some(opened.gameplay_areas);
                    store.manifest = Some(opened.manifest);
                    store.vegetation_catalog = opened.vegetation_catalog;
                    store.environments = opened.environments;
                    store.presets = opened.presets;
                    store.terrain_resources = opened.terrain_resources;
                    store.height_steps = opened.height_steps;
                    store.write_error = opened.write_error;
                    store.phase = ProjectStorePhase::Ready;
                    store.source_epoch = store.source_epoch.max(1);
                }
                Err(error) => store.phase = ProjectStorePhase::Failed(error),
            },
            Ok(ProjectResult::Query {
                revision,
                window,
                domains,
                result,
            }) => {
                if store.in_flight.is_some_and(|query| {
                    query.revision == revision && query.window == window && query.domains == domains
                }) {
                    store.in_flight = None;
                }
                if store.desired_window != Some(window) || store.desired_domains != domains {
                    store.stale_results = store.stale_results.saturating_add(1);
                    continue;
                }
                match result {
                    Ok(snapshot) => {
                        store.cells = snapshot.cells;
                        store.objects = snapshot.objects;
                        store.environment_cells = snapshot.environment_cells;
                        if let Some(snapshot) = &snapshot.environment_snapshot
                            && let Some(definition) = store
                                .environments
                                .iter_mut()
                                .find(|d| d.space == snapshot.definition.space)
                        {
                            *definition = snapshot.definition.clone();
                        }
                        if let Some(environment) = &snapshot.environment_snapshot {
                            store.presets = Some(environment.presets.clone());
                        }
                        store.environment_snapshot = snapshot.environment_snapshot;
                        store.cells_truncated = snapshot.cells_truncated;
                        store.objects_truncated = snapshot.objects_truncated;
                        store.environment_coverage_truncated =
                            snapshot.environment_coverage_truncated;
                        store.loaded_window = Some(window);
                        store.loaded_domains = domains;
                        store.failed_window = None;
                        store.failed_domains = ProjectSourceDomains::default();
                        store.query_error = None;
                        store.completed_queries = store.completed_queries.saturating_add(1);
                    }
                    Err(error) => {
                        store.failed_window = Some(window);
                        store.failed_domains = domains;
                        store.query_error = Some(error);
                    }
                }
            }
            Ok(ProjectResult::SaveObjectTransaction { request_id, result }) => {
                if store.save_in_flight == Some(request_id) {
                    store.save_in_flight = None;
                }
                let outcome = match result {
                    Ok(ObjectWriteTransactionResult::Committed(commits)) => {
                        store.source_epoch = store.source_epoch.wrapping_add(1).max(1);
                        store.loaded_window = None;
                        store.objects.clear();
                        store.objects_truncated = false;
                        ObjectSaveOutcome::Committed(commits)
                    }
                    Ok(ObjectWriteTransactionResult::Conflict { object, actual }) => {
                        ObjectSaveOutcome::Conflict { object, actual }
                    }
                    Err(error) => ObjectSaveOutcome::Failed(error),
                };
                store.save_completion = Some(ObjectSaveCompletion {
                    request_id,
                    outcome,
                });
            }
            Ok(ProjectResult::SaveDenseTransaction { request_id, result }) => {
                if store.dense_save_in_flight == Some(request_id) {
                    store.dense_save_in_flight = None;
                }
                let outcome = match result {
                    Ok(EnvironmentSourceWriteResult::Committed(commits)) => {
                        if let Some(presets) = &commits.presets {
                            store.presets = Some(presets.clone());
                        }
                        for definition in &commits.definitions {
                            if let Some(old) = store
                                .environments
                                .iter_mut()
                                .find(|d| d.space == definition.space)
                            {
                                *old = definition.clone();
                            }
                        }
                        store.source_epoch = store.source_epoch.wrapping_add(1).max(1);
                        store.loaded_window = None;
                        store.environment_cells.clear();
                        store.environment_snapshot = None;
                        DenseSaveOutcome::Committed(commits)
                    }
                    Ok(EnvironmentSourceWriteResult::RoadConflict { actual }) => {
                        DenseSaveOutcome::RoadConflict { actual }
                    }
                    Ok(EnvironmentSourceWriteResult::LibraryConflict { actual }) => {
                        store.presets = Some(actual.clone());
                        DenseSaveOutcome::LibraryConflict { actual }
                    }
                    Ok(EnvironmentSourceWriteResult::DefinitionConflict { space, actual }) => {
                        DenseSaveOutcome::DefinitionConflict { space, actual }
                    }
                    Ok(EnvironmentSourceWriteResult::CoverageConflict { key, actual }) => {
                        DenseSaveOutcome::Conflict { key, actual }
                    }
                    Err(error) => DenseSaveOutcome::Failed(error),
                };
                store.dense_save_completion = Some(DenseSaveCompletion {
                    request_id,
                    outcome,
                });
            }
            Ok(ProjectResult::SaveVegetationCatalog {
                request_id,
                replacement,
                result,
            }) => {
                if store.vegetation_save_in_flight == Some(request_id) {
                    store.vegetation_save_in_flight = None;
                }
                let outcome = match result {
                    Ok(VegetationCatalogWriteResult::Committed) => {
                        store.source_epoch = store.source_epoch.wrapping_add(1).max(1);
                        store.vegetation_catalog = Some(replacement.clone());
                        VegetationSaveOutcome::Committed(replacement)
                    }
                    Ok(VegetationCatalogWriteResult::Conflict { actual }) => {
                        store.vegetation_catalog = actual.clone();
                        VegetationSaveOutcome::Conflict { actual }
                    }
                    Err(error) => VegetationSaveOutcome::Failed(error),
                };
                store.vegetation_save_completion = Some(VegetationSaveCompletion {
                    request_id,
                    outcome,
                });
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                store.phase = ProjectStorePhase::Failed("project database worker stopped".into());
                break;
            }
        }
    }
}
