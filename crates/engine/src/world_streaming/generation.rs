//! Two-phase immutable database adoption. Candidate IO and terrain uploads cannot
//! change the live generation; only a confirmed commit can publish their results.
use super::*;

/// Explicit handshake for replacing the streamer's immutable SQLite snapshot.
///
/// Publishing code first atomically replaces the database file, then requests the exact expected
/// generation here. The worker prepares a second reader while the current one stays live.
/// A complete uploaded terrain cover must also be ready.
/// Only a matching worker commit acknowledgement replaces the catalog and source pages;
/// preparation failures discard the candidate and retain the current generation.
#[derive(Resource, Debug, Default)]
pub struct WorldGenerationReload {
    pub(super) queued: Option<String>,
    /// The preparation request and the generation it expects.
    pub(super) in_flight: Option<(RequestId, String)>,
    pub(super) completion: Option<Result<String, String>>,
    pub(super) candidate: Option<RuntimeManifest>,
    /// The commit request, once sent.
    pub(super) commit: Option<RequestId>,
    pub(super) committed: bool,
    pub(super) failure: Option<String>,
    pub(super) last_error: Option<String>,
}

impl WorldGenerationReload {
    pub fn request(&mut self, expected_generation: impl Into<String>) -> bool {
        let expected_generation = expected_generation.into();
        if expected_generation.is_empty() || self.queued.is_some() || self.in_flight.is_some() {
            return false;
        }
        self.completion = None;
        self.last_error = None;
        self.queued = Some(expected_generation);
        true
    }

    pub fn active(&self) -> bool {
        self.queued.is_some() || self.in_flight.is_some()
    }

    pub fn take_completion(&mut self) -> Option<Result<String, String>> {
        self.completion.take()
    }
}

fn compatible_candidate(
    active: Option<WorldSpaceId>,
    old: Option<&RuntimeManifest>,
    next: &RuntimeManifest,
) -> Result<(), String> {
    let Some(space) = active else {
        return Ok(());
    };
    let destination = next.world_space(space).ok_or(
        "published generation removes the active world; enter another world before adopting it",
    )?;
    if old
        .and_then(|m| m.world_space(space))
        .is_some_and(|s| s.cell_size.to_bits() != destination.cell_size.to_bits())
    {
        return Err("published generation changes the active world's cell size; reopen the world to adopt its new grid".into());
    }
    Ok(())
}

pub(super) fn request_reload(
    worker: Option<Res<WorldDatabaseWorker>>,
    active: Res<ActiveWorldSpace>,
    stream: Res<WorldStream>,
    mut reload: ResMut<WorldGenerationReload>,
) {
    if let Some(candidate) = &reload.candidate
        && let Err(error) =
            compatible_candidate(active.current, stream.manifest.as_ref(), candidate)
    {
        reload.failure = Some(error);
    }
    if reload.in_flight.is_some() {
        return;
    }
    let Some(generation) = reload.queued.take() else {
        return;
    };
    let Some(worker) = worker else {
        reload.queued = Some(generation);
        return;
    };
    match worker.send(DatabaseRequest::Reload {
        expected_generation: generation.clone(),
    }) {
        Ok(id) => {
            reload.in_flight = Some((id, generation));
            reload.candidate = None;
            reload.commit = None;
            reload.committed = false;
            reload.failure = None;
        }
        Err(NotSent::Full) => reload.queued = Some(generation),
        Err(NotSent::Stopped) => {
            let error = "database request channel closed during generation preparation".to_string();
            reload.last_error = Some(error.clone());
            reload.completion = Some(Err(error));
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn advance_reload(
    mut commands: Commands,
    worker: Option<Res<WorldDatabaseWorker>>,
    mut active: ResMut<ActiveWorldSpace>,
    mut catalog: ResMut<WorldCatalog>,
    mut viewpoint: ResMut<WorldViewpoint>,
    mut origin: ResMut<WorldOrigin>,
    mut stream: ResMut<WorldStream>,
    mut residency: ResMut<SourceResidency>,
    mut reload: ResMut<WorldGenerationReload>,
    mut entry: ResMut<terrain_lod::entry::TerrainEntry>,
    mut terrain: ResMut<terrain_lod::TerrainLodStream>,
    mut meshes: ResMut<Assets<Mesh>>,
    tracker: Res<terrain_lod::UploadTracker>,
) {
    let Some((id, expected)) = reload.in_flight.clone() else {
        return;
    };
    let Some(worker) = worker else {
        return;
    };
    if let Some(error) = reload.failure.clone() {
        // Keep the operation single-flight until the candidate's reader is queued
        // for release. A later retry cannot be cleared by this discard's identity.
        if let Err(NotSent::Full) = worker.send(DatabaseRequest::DiscardReload { reload: id }) {
            return;
        }
        entry.clear(&mut commands, &mut meshes, &tracker);
        reload.candidate = None;
        reload.in_flight = None;
        reload.commit = None;
        reload.committed = false;
        reload.failure = None;
        reload.last_error = Some(error.clone());
        reload.completion = Some(Err(error));
        return;
    }
    let Some(candidate) = &reload.candidate else {
        return;
    };
    let space = active.current.unwrap_or(candidate.default_world_space);
    let request = WorldSpaceTransition {
        space,
        local_position: [0.; 3],
    };
    if !entry.ready_for(&expected, request) {
        return;
    }
    if reload.commit.is_none() {
        match worker.send(DatabaseRequest::CommitReload {
            reload: id,
            expected_generation: expected,
        }) {
            Ok(commit) => reload.commit = Some(commit),
            Err(NotSent::Full) => (),
            Err(NotSent::Stopped) => {
                reload.failure = Some("database worker stopped before generation commit".into())
            }
        }
        return;
    }
    if !reload.committed {
        return;
    }
    // All remaining operations are infallible moves of already-validated data.
    // Deferred entity changes, catalog identity and completion share this Update.
    let candidate = reload.candidate.take().unwrap();
    let size = candidate.world_space(space).unwrap().cell_size;
    clear_streamed_pages(&mut commands, &mut stream, &mut residency);
    residency.definition_cache.clear();
    adopt_runtime_manifest(
        candidate,
        &mut active,
        &mut catalog,
        &mut viewpoint,
        &mut origin,
        &mut stream,
    );
    entry.commit(
        &mut terrain,
        &mut commands,
        &mut meshes,
        &tracker,
        origin.cell(),
        size,
    );
    reload.in_flight = None;
    reload.commit = None;
    reload.committed = false;
    reload.last_error = None;
    reload.completion = Some(Ok(expected));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::{Receiver, Sender};

    #[test]
    fn generation_reload_is_an_exact_single_flight_handshake() {
        let mut reload = WorldGenerationReload::default();
        assert!(!reload.request(""));
        assert!(reload.request("generation-a"));
        assert!(reload.active());
        assert!(!reload.request("generation-b"));

        let queued = reload.queued.take().unwrap();
        reload.in_flight = Some((1, queued));
        reload.in_flight = None;
        reload.completion = Some(Ok("generation-a".into()));
        assert_eq!(reload.take_completion(), Some(Ok("generation-a".into())));
        assert!(!reload.active());
        assert!(reload.request("generation-b"));
    }

    fn manifest(generation: &str) -> RuntimeManifest {
        RuntimeManifest {
            schema_version: 0,
            generation_id: generation.into(),
            content_hash: [0; 32],
            default_world_space: WorldSpaceId(1),
            world_spaces: vec![world_db::WorldSpaceRecord {
                atmosphere: Default::default(),
                atmosphere_revision: 1,
                id: WorldSpaceId(1),
                name: "test".into(),
                cell_size: 32.,
                minimum_y: 0.,
                maximum_y: 100.,
                sea_level: None,
            }],
            vegetation_catalog: None,
            start_view: None,
            // One area named after the generation, to see which set the catalog holds.
            gameplay_areas: vec![world::GameplayArea {
                name: generation.into(),
                space: WorldSpaceId(1),
                points: vec![[0., 0.], [4., 0.], [0., 4.]],
                height: None,
            }]
            .into(),
        }
    }

    struct Harness {
        app: App,
        requests: Receiver<(RequestId, DatabaseRequest)>,
        replies: Sender<(RequestId, DatabaseResult)>,
        resident: Entity,
    }
    impl Harness {
        fn new() -> Self {
            let (worker, receiver, replies) = WorldDatabaseWorker::test_channel_pair(1, 8);
            let mut app = App::new();
            app.insert_resource(worker)
                .init_resource::<WorldGenerationReload>()
                .init_resource::<ActiveWorldSpace>()
                .init_resource::<WorldCatalog>()
                .init_resource::<WorldViewpoint>()
                .init_resource::<crate::WorldStartView>()
                .add_message::<crate::WorldStartAdopted>()
                .init_resource::<WorldOrigin>()
                .init_resource::<WorldStream>()
                .init_resource::<SourceResidency>()
                .init_resource::<super::far_objects::FarObjects>()
                .init_resource::<super::valley_mist::MistTerrain>()
                .init_resource::<terrain_lod::TerrainLodStream>()
                .init_resource::<terrain_lod::entry::TerrainEntry>()
                .init_resource::<terrain_lod::UploadTracker>()
                .init_resource::<Assets<Mesh>>()
                .add_systems(
                    Update,
                    (
                        super::super::replies::receive_database_results,
                        request_reload,
                        advance_reload,
                    )
                        .chain(),
                );
            replies
                .send((0, DatabaseResult::Opened(Ok(manifest("old")))))
                .unwrap();
            app.update();
            let resident = app.world_mut().spawn_empty().id();
            app.world_mut()
                .resource_mut::<SourceResidency>()
                .pages
                .insert(
                    PageKey {
                        space: WorldSpaceId(1),
                        cell: CellCoord::ZERO,
                        domain: PageDomain::Terrain,
                        lod: 0,
                    },
                    PageState::Resident(PageAttachment {
                        entities: vec![resident],
                        ..default()
                    }),
                );
            Self {
                app,
                requests: receiver,
                replies,
                resident,
            }
        }
        fn request(&mut self) -> u64 {
            assert!(
                self.app
                    .world_mut()
                    .resource_mut::<WorldGenerationReload>()
                    .request("next")
            );
            self.app.update();
            let (
                id,
                DatabaseRequest::Reload {
                    expected_generation,
                },
            ) = self.requests.try_recv().unwrap()
            else {
                panic!("missing preparation request")
            };
            assert_eq!(expected_generation, "next");
            self.assert_old();
            id
        }
        fn prepared(&mut self, id: u64, candidate: RuntimeManifest) {
            // The terrain hand-off has its own tests; here the destination cover is ready.
            let world = self.app.world_mut();
            let space = world.resource::<ActiveWorldSpace>().current.unwrap();
            world
                .resource_mut::<terrain_lod::entry::TerrainEntry>()
                .test_ready(
                    &candidate.generation_id,
                    WorldSpaceTransition {
                        space,
                        local_position: [0.; 3],
                    },
                );
            self.replies
                .send((id, DatabaseResult::Reloaded(Ok(candidate))))
                .unwrap();
            self.app.update();
        }
        /// The commit request for the reload `id`.
        fn commit(&mut self, id: u64) -> u64 {
            let (commit, DatabaseRequest::CommitReload { reload, .. }) =
                self.requests.try_recv().unwrap()
            else {
                panic!("missing commit request")
            };
            assert_eq!(reload, id);
            commit
        }
        fn discarded(&mut self, id: u64) -> bool {
            matches!(
                self.requests.try_recv().unwrap(),
                (_, DatabaseRequest::DiscardReload { reload }) if reload == id
            )
        }
        fn assert_old(&self) {
            let w = self.app.world();
            assert_eq!(w.resource::<WorldCatalog>().generation_id(), "old");
            let areas = w.resource::<WorldCatalog>().gameplay_areas();
            assert!(areas.find("old").is_some() && areas.find("next").is_none());
            assert_eq!(
                w.resource::<WorldStream>()
                    .manifest
                    .as_ref()
                    .unwrap()
                    .generation_id,
                "old"
            );
            assert!(w.get_entity(self.resident).is_ok());
            assert_eq!(w.resource::<SourceResidency>().pages.len(), 1);
        }
        fn completion(&mut self) -> Option<Result<String, String>> {
            self.app
                .world_mut()
                .resource_mut::<WorldGenerationReload>()
                .take_completion()
        }
    }
    #[test]
    fn publication_changes_live_state_only_after_matching_commit_acknowledgement() {
        let mut h = Harness::new();
        let id = h.request();
        h.app
            .world_mut()
            .resource_mut::<ActiveWorldSpace>()
            .request(WorldSpaceId(2), [1., 2., 3.]);
        h.prepared(id, manifest("next"));
        let commit = h.commit(id);
        h.assert_old();
        assert!(h.completion().is_none());
        h.replies
            .send((commit + 10, DatabaseResult::ReloadCommitted(Ok(()))))
            .unwrap();
        h.app.update();
        h.assert_old();
        assert!(h.completion().is_none());
        h.replies
            .send((commit, DatabaseResult::ReloadCommitted(Ok(()))))
            .unwrap();
        h.app.update();
        assert_eq!(h.completion(), Some(Ok("next".into())));
        assert_eq!(
            h.app.world().resource::<WorldCatalog>().generation_id(),
            "next"
        );
        let areas = h.app.world().resource::<WorldCatalog>().gameplay_areas();
        assert!(areas.find("next").is_some() && areas.find("old").is_none());
        assert!(h.app.world().get_entity(h.resident).is_err());
        assert!(h.app.world().resource::<SourceResidency>().pages.is_empty());
        assert_eq!(
            h.app
                .world()
                .resource::<ActiveWorldSpace>()
                .requested
                .unwrap()
                .space,
            WorldSpaceId(2)
        );
    }

    #[test]
    fn preparation_and_commit_failures_preserve_residency_and_allow_retry() {
        for fail_commit in [false, true] {
            let mut h = Harness::new();
            let id = h.request();
            if fail_commit {
                h.prepared(id, manifest("next"));
                let commit = h.commit(id);
                h.replies
                    .send((
                        commit,
                        DatabaseResult::ReloadCommitted(Err("commit rejected".into())),
                    ))
                    .unwrap();
            } else {
                h.replies
                    .send((
                        id,
                        DatabaseResult::Reloaded(Err("could not open candidate".into())),
                    ))
                    .unwrap();
            }
            h.app.update();
            h.assert_old();
            assert!(h.completion().unwrap().is_err());
            assert!(h.discarded(id));
            assert!(h.request() > id);
        }
    }

    #[test]
    fn bounded_channel_retries_commit_and_discard_without_premature_completion() {
        let mut h = Harness::new();
        let id = h.request();
        h.app
            .world()
            .resource::<WorldDatabaseWorker>()
            .send(DatabaseRequest::DiscardReload { reload: 0 })
            .unwrap();
        h.prepared(id, manifest("next"));
        h.assert_old();
        let commit = |h: &Harness| h.app.world().resource::<WorldGenerationReload>().commit;
        assert!(commit(&h).is_none());
        assert!(h.completion().is_none());
        h.requests.try_recv().unwrap();
        h.app.update();
        let sent = commit(&h).unwrap();
        // Leave the commit occupying the queue while reporting its failure.
        h.replies
            .send((
                sent,
                DatabaseResult::ReloadCommitted(Err("injected rejection".into())),
            ))
            .unwrap();
        h.app.update();
        h.assert_old();
        assert!(h.completion().is_none());
        assert!(h.app.world().resource::<WorldGenerationReload>().active());
        assert_eq!(h.commit(id), sent);
        h.app.update();
        h.assert_old();
        assert!(h.completion().unwrap().is_err());
        assert!(h.discarded(id));
    }

    #[test]
    fn incompatible_active_world_is_rejected_before_commit() {
        for remove in [false, true] {
            let mut h = Harness::new();
            let id = h.request();
            let mut next = manifest("next");
            if remove {
                next.world_spaces[0].id = WorldSpaceId(2);
                next.default_world_space = WorldSpaceId(2);
            } else {
                next.world_spaces[0].cell_size = 64.;
            }
            h.prepared(id, next);
            h.assert_old();
            let error = h.completion().unwrap().unwrap_err();
            assert!(error.contains(if remove {
                "removes the active world"
            } else {
                "cell size"
            }));
            assert!(h.discarded(id));
        }
    }
}
