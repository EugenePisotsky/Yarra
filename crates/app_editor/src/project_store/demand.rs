//! Which window of the project the editor needs, and sending queries and saves to the thread.
use super::*;

pub(super) fn validate_project_catalog(
    runtime: Res<WorldCatalog>,
    mut store: ResMut<ProjectEditorStore>,
) {
    let Some(source) = store.manifest.as_ref() else {
        return;
    };
    if runtime.world_spaces().is_empty() {
        return;
    }
    store.catalog_compatible = Some(
        source.world_spaces.len() == runtime.world_spaces().len()
            && source.world_spaces.iter().all(|source_space| {
                runtime
                    .world_space(source_space.id)
                    .is_some_and(|runtime_space| {
                        runtime_space.name == source_space.name
                            && runtime_space.cell_size.to_bits() == source_space.cell_size.to_bits()
                    })
            }),
    );
}

pub(super) fn update_project_query_demand(
    viewpoint: Res<WorldViewpoint>,
    workspace: Res<State<EditorWorkspace>>,
    tools: Res<EditorToolRegistry>,
    mut store: ResMut<ProjectEditorStore>,
) {
    if !matches!(store.phase, ProjectStorePhase::Ready) || store.catalog_compatible == Some(false) {
        return;
    }
    let Some(tool) = tools.active(*workspace.get()) else {
        return;
    };
    let desired_domains = ProjectSourceDomains::from_tool(tool);
    if !desired_domains.any() {
        return;
    }
    let Some(position) = viewpoint.position() else {
        return;
    };
    let center = if desired_domains.environment_coverage {
        store
            .environment_focus
            .filter(|(space, _)| *space == position.space)
            .map_or(position.cell, |(_, cell)| cell)
    } else {
        position.cell
    };
    let desired = ProjectQueryWindow::around(position.space, center);
    if store.desired_window == Some(desired) && store.desired_domains == desired_domains {
        return;
    }

    if store
        .loaded_window
        .is_some_and(|loaded| loaded.space != desired.space)
        || store.loaded_domains != desired_domains
    {
        store.loaded_window = None;
        store.cells.clear();
        store.objects.clear();
        store.environment_cells.clear();
        store.environment_snapshot = None;
        store.cells_truncated = false;
        store.objects_truncated = false;
        store.environment_coverage_truncated = false;
    }
    store.desired_window = Some(desired);
    store.desired_domains = desired_domains;
    if store.failed_window != Some(desired) || store.failed_domains != desired_domains {
        store.failed_window = None;
        store.failed_domains = ProjectSourceDomains::default();
        store.query_error = None;
    }
}

pub(super) fn dispatch_project_query(
    worker: Option<Res<ProjectWorker>>,
    mut store: ResMut<ProjectEditorStore>,
) {
    if !matches!(store.phase, ProjectStorePhase::Ready)
        || store.catalog_compatible == Some(false)
        || store.in_flight.is_some()
    {
        return;
    }
    let Some(window) = store.desired_window else {
        return;
    };
    let domains = store.desired_domains;
    if (store.loaded_window == Some(window) && store.loaded_domains == domains)
        || (store.failed_window == Some(window) && store.failed_domains == domains)
    {
        return;
    }
    let Some(worker) = worker else {
        return;
    };

    let revision = store.next_revision.wrapping_add(1).max(1);
    match worker.try_send(ProjectRequest::Query {
        revision,
        window,
        domains,
    }) {
        Ok(()) => {
            store.next_revision = revision;
            store.in_flight = Some(InFlightQuery {
                revision,
                window,
                domains,
            });
        }
        Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => {
            store.phase = ProjectStorePhase::Failed("project request channel closed".into());
        }
    }
}

pub(super) fn dispatch_project_save(
    worker: Option<Res<ProjectWorker>>,
    mut store: ResMut<ProjectEditorStore>,
) {
    if !matches!(store.phase, ProjectStorePhase::Ready)
        || store.save_in_flight.is_some()
        || store.dense_save_in_flight.is_some()
        || store.vegetation_save_in_flight.is_some()
        || store.atmosphere_save_in_flight.is_some()
        || store.area_save_in_flight.is_some()
    {
        return;
    }
    let Some(worker) = worker else {
        return;
    };

    if let Some((id, revision, areas)) = store.pending_area_save.clone() {
        match worker.try_send(ProjectRequest::SaveGameplayAreas(id, revision, areas)) {
            Ok(()) => {
                store.pending_area_save = None;
                store.area_save_in_flight = Some(id);
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                store.write_error = Some("Project writer stopped".into());
            }
        }
        return;
    }

    if let Some((id, writes)) = store.pending_atmosphere_save.clone() {
        match worker.try_send(ProjectRequest::SaveAtmospheres(id, writes)) {
            Ok(()) => {
                store.pending_atmosphere_save = None;
                store.atmosphere_save_in_flight = Some(id);
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                store.write_error = Some("Project writer stopped".into());
            }
        }
        return;
    }

    if let Some(request) = store.pending_save.clone() {
        match worker.try_send(ProjectRequest::SaveObjectTransaction(request.clone())) {
            Ok(()) => {
                store.pending_save = None;
                store.save_in_flight = Some(request.request_id);
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                store.phase = ProjectStorePhase::Failed("project request channel closed".into());
            }
        }
        return;
    }

    if let Some(request) = store.pending_dense_save.clone() {
        match worker.try_send(ProjectRequest::SaveDenseTransaction(request.clone())) {
            Ok(()) => {
                store.pending_dense_save = None;
                store.dense_save_in_flight = Some(request.request_id);
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                store.phase = ProjectStorePhase::Failed("project request channel closed".into());
            }
        }
        return;
    }

    if let Some(request) = store.pending_vegetation_save.clone() {
        match worker.try_send(ProjectRequest::SaveVegetationCatalog(request.clone())) {
            Ok(()) => {
                store.pending_vegetation_save = None;
                store.vegetation_save_in_flight = Some(request.request_id);
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                store.phase = ProjectStorePhase::Failed("project request channel closed".into());
            }
        }
    }
}
