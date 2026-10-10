//! Routes database replies to the consumer that sent each request.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn receive_database_results(
    mut terrain: ResMut<terrain_lod::TerrainLodStream>,
    mut entry: ResMut<terrain_lod::entry::TerrainEntry>,
    worker: Option<Res<WorldDatabaseWorker>>,
    mut active_space: ResMut<ActiveWorldSpace>,
    mut catalog: ResMut<WorldCatalog>,
    mut viewpoint: ResMut<WorldViewpoint>,
    (mut start_view, mut adopted): (
        ResMut<crate::WorldStartView>,
        MessageWriter<crate::WorldStartAdopted>,
    ),
    mut origin: ResMut<WorldOrigin>,
    mut stream: ResMut<WorldStream>,
    mut residency: ResMut<SourceResidency>,
    mut reload: ResMut<WorldGenerationReload>,
    mut far: ResMut<far_objects::FarObjects>,
    mut mist: ResMut<valley_mist::MistTerrain>,
) {
    let Some(worker) = worker else {
        return;
    };
    loop {
        let (id, result) = match worker.try_recv() {
            Ok(reply) => reply,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                if reload.active() {
                    reload.failure =
                        Some("database worker stopped during generation adoption".into());
                }
                if !matches!(stream.phase, StreamPhase::Failed(_)) {
                    stream.phase = StreamPhase::Failed("database worker stopped".into());
                }
                break;
            }
        };
        match result {
            DatabaseResult::Terrain(result) => {
                if mist.owns_request(id) {
                    mist.receive(id, result);
                } else if entry.owns_request(id) {
                    entry.receive(id, result);
                } else {
                    terrain.receive(id, result);
                }
            }
            DatabaseResult::Opened(Ok(manifest)) => {
                info!(
                    "opened runtime world generation {} with {} spaces from SQLite",
                    manifest.generation_id,
                    manifest.world_spaces.len()
                );
                if active_space.current.is_none() {
                    active_space.current = Some(manifest.default_world_space);
                }
                catalog.adopt(&manifest);
                if viewpoint.position.is_none() {
                    // An explicit start view wins; otherwise play starts where the
                    // world says.
                    if start_view.0.is_none()
                        && let Some(view) = &manifest.start_view
                    {
                        info!(
                            "starting at the world's start view ({:.0}, {:.0}, {:.0})",
                            view.position[0], view.position[1], view.position[2]
                        );
                        start_view.0 = Some(view.clone());
                        adopted.write(crate::WorldStartAdopted(view.clone()));
                    }
                    viewpoint.position = Some(WorldPosition::from_world(
                        manifest.default_world_space,
                        start_view
                            .0
                            .as_ref()
                            .map_or([0.; 3], |v| v.position.map(f64::from)),
                        manifest.default_world_space().cell_size,
                    ));
                }
                if origin.space.is_none() {
                    origin.space = Some(manifest.default_world_space);
                    origin.cell = CellCoord::ZERO;
                }
                stream.manifest = Some(manifest);
                stream.phase = StreamPhase::Ready;
            }
            DatabaseResult::Opened(Err(error)) => {
                error!("{error}");
                stream.phase = StreamPhase::Failed(error);
            }
            DatabaseResult::Reloaded(result) => {
                if reload
                    .in_flight
                    .as_ref()
                    .is_some_and(|(sent, _)| *sent == id)
                {
                    match result {
                        Ok(manifest) => reload.candidate = Some(manifest),
                        Err(error) => reload.failure = Some(error),
                    }
                }
            }
            DatabaseResult::ReloadCommitted(result) => {
                if reload.commit == Some(id) {
                    match result {
                        Ok(()) => reload.committed = true,
                        Err(error) => reload.failure = Some(error),
                    }
                }
            }
            DatabaseResult::Index(result) => {
                let Some((_, space)) = stream.requested_index.filter(|(sent, _)| *sent == id)
                else {
                    continue;
                };
                if active_space.current != Some(space) {
                    continue;
                }
                stream.requested_index = None;
                match result {
                    Ok(descriptors) => stream.descriptors = descriptors,
                    Err(error) => {
                        error!("world index request failed: {error}");
                        stream.phase = StreamPhase::Failed(error);
                    }
                }
            }
            DatabaseResult::Page { key, result } => residency.receive_page(id, key, result),
            DatabaseResult::FarObjects(result) => far.receive(id, result),
        }
    }
}
