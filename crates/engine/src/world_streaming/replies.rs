//! Routes database replies to the consumer that sent each request.
use super::*;

/// What opening the runtime database sets up: the active space, the catalog, where the
/// view starts and the render origin.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct Opening<'w> {
    active_space: ResMut<'w, ActiveWorldSpace>,
    catalog: ResMut<'w, WorldCatalog>,
    viewpoint: ResMut<'w, WorldViewpoint>,
    start_view: ResMut<'w, crate::WorldStartView>,
    adopted: MessageWriter<'w, crate::WorldStartAdopted>,
    origin: ResMut<'w, WorldOrigin>,
}

impl Opening<'_> {
    /// Adopts a newly opened generation. Where the view starts and the origin are set only
    /// the first time: an explicit start view wins, otherwise play starts where the world
    /// says.
    fn adopt(&mut self, manifest: &world_db::RuntimeManifest) {
        info!(
            "opened runtime world generation {} with {} spaces from SQLite",
            manifest.generation_id,
            manifest.world_spaces.len()
        );

        if self.active_space.current.is_none() {
            self.active_space.current = Some(manifest.default_world_space);
        }

        self.catalog.adopt(manifest);

        if self.viewpoint.position.is_none() {
            if self.start_view.0.is_none()
                && let Some(view) = &manifest.start_view
            {
                info!(
                    "starting at the world's start view ({:.0}, {:.0}, {:.0})",
                    view.position[0], view.position[1], view.position[2]
                );
                self.start_view.0 = Some(view.clone());
                self.adopted.write(crate::WorldStartAdopted(view.clone()));
            }

            let start = self
                .start_view
                .0
                .as_ref()
                .map_or([0.; 3], |v| v.position.map(f64::from));
            self.viewpoint.position = Some(WorldPosition::from_world(
                manifest.default_world_space,
                start,
                manifest.default_world_space().cell_size,
            ));
        }

        if self.origin.space.is_none() {
            self.origin.space = Some(manifest.default_world_space);
            self.origin.cell = CellCoord::ZERO;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn receive_database_results(
    worker: Option<Res<WorldDatabaseWorker>>,
    mut opening: Opening,
    mut stream: ResMut<WorldStream>,
    mut residency: ResMut<SourceResidency>,
    mut reload: ResMut<WorldGenerationReload>,
    mut far: ResMut<far_objects::FarObjects>,
    mut mist: ResMut<valley_mist::MistTerrain>,
    mut clipmap: ResMut<terrain_clipmap::ClipmapTerrain>,
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
                } else if clipmap.owns_request(id) {
                    clipmap.receive(id, result);
                }
            }
            DatabaseResult::Opened(Ok(manifest)) => {
                opening.adopt(&manifest);
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

                if opening.active_space.current != Some(space) {
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
