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
    if !matches!(store.phase, ProjectStorePhase::Ready) || store.writer_busy() {
        return;
    }

    let Some(worker) = worker else {
        return;
    };

    // One save at a time, areas and atmospheres first. A stopped writer fails the small
    // saves with a write error and the transactions with the whole store, as before.
    let store = store.as_mut();

    if let Some((id, request)) = store.area_saves.pending() {
        let sent = worker.try_send(ProjectRequest::SaveGameplayAreas(id, request));
        follow_send(sent, &mut store.area_saves, || {
            store.write_error = Some("Project writer stopped".into())
        });
    } else if let Some((id, request)) = store.atmosphere_saves.pending() {
        let sent = worker.try_send(ProjectRequest::SaveAtmospheres(id, request));
        follow_send(sent, &mut store.atmosphere_saves, || {
            store.write_error = Some("Project writer stopped".into())
        });
    } else if let Some((id, request)) = store.object_saves.pending() {
        let sent = worker.try_send(ProjectRequest::SaveObjectTransaction(id, request));
        follow_send(sent, &mut store.object_saves, || {
            store.phase = ProjectStorePhase::Failed("project request channel closed".into())
        });
    } else if let Some((id, request)) = store.dense_saves.pending() {
        let sent = worker.try_send(ProjectRequest::SaveDenseTransaction(id, request));
        follow_send(sent, &mut store.dense_saves, || {
            store.phase = ProjectStorePhase::Failed("project request channel closed".into())
        });
    }
}

/// A sent request is in flight; a full queue tries again next frame.
fn follow_send<Q: Clone, A>(
    sent: Result<(), TrySendError<ProjectRequest>>,
    channel: &mut SaveChannel<Q, A>,
    stopped: impl FnOnce(),
) {
    match sent {
        Ok(()) => channel.sent(),
        Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => stopped(),
    }
}
