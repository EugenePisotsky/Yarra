//! Two-phase immutable database adoption. Candidate IO and terrain uploads cannot
//! change the live generation; only a confirmed commit can publish their results.
use super::*;

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
    config: Res<TerrainLodPreview>,
) {
    if let Some(candidate) = &reload.candidate {
        if let Err(error) =
            compatible_candidate(active.current, stream.manifest.as_ref(), candidate)
        {
            reload.failure = Some(error);
        }
    }
    if reload.in_flight.is_some() {
        return;
    }
    let Some((id, generation)) = reload.queued.take() else {
        return;
    };
    let Some(worker) = worker else {
        reload.queued = Some((id, generation));
        return;
    };
    match worker.requests.try_send(DatabaseRequest::Reload {
        request_id: id,
        expected_generation: generation.clone(),
    }) {
        Ok(()) => {
            reload.in_flight = Some((id, generation));
            reload.candidate = None;
            reload.commit_requested = false;
            reload.committed = false;
            reload.failure = None;
            reload.hierarchy = config.enabled;
        }
        Err(TrySendError::Full(_)) => reload.queued = Some((id, generation)),
        Err(TrySendError::Disconnected(_)) => {
            let error = "database request channel closed during generation preparation".to_string();
            reload.last_error = Some(error.clone());
            reload.completion = Some(Err(error));
        }
    }
}

pub(super) fn advance_reload(
    mut commands: Commands,
    worker: Option<Res<WorldDatabaseWorker>>,
    mut active: ResMut<ActiveWorldSpace>,
    mut catalog: ResMut<WorldCatalog>,
    mut viewpoint: ResMut<WorldViewpoint>,
    mut origin: ResMut<WorldOrigin>,
    mut stream: ResMut<WorldStream>,
    mut reload: ResMut<WorldGenerationReload>,
    mut entry: ResMut<terrain_lod::entry::TerrainEntry>,
    mut terrain: ResMut<terrain_lod::TerrainLodStream>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut images: ResMut<Assets<Image>>,
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
        if let Err(TrySendError::Full(_)) = worker
            .requests
            .try_send(DatabaseRequest::DiscardReload { request_id: id })
        {
            return;
        }
        entry.clear(&mut commands, &mut meshes, &tracker);
        reload.candidate = None;
        reload.in_flight = None;
        reload.commit_requested = false;
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
    if reload.hierarchy && !entry.ready_for(&expected, request) {
        return;
    }
    if !reload.commit_requested {
        match worker.requests.try_send(DatabaseRequest::CommitReload {
            request_id: id,
            expected_generation: expected,
        }) {
            Ok(()) => reload.commit_requested = true,
            Err(TrySendError::Full(_)) => (),
            Err(TrySendError::Disconnected(_)) => {
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
    clear_streamed_pages(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        &mut stream,
    );
    stream.definition_cache.clear();
    adopt_runtime_manifest(
        candidate,
        &mut active,
        &mut catalog,
        &mut viewpoint,
        &mut origin,
        &mut stream,
    );
    if reload.hierarchy {
        entry.commit(
            &mut terrain,
            &mut commands,
            &mut meshes,
            &tracker,
            origin.cell(),
            size,
        );
    }
    reload.in_flight = None;
    reload.commit_requested = false;
    reload.committed = false;
    reload.last_error = None;
    reload.completion = Some(Ok(expected));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(generation: &str) -> RuntimeManifest {
        RuntimeManifest {
            schema_version: 0,
            generation_id: generation.into(),
            content_hash: [0; 32],
            default_world_space: WorldSpaceId(1),
            world_spaces: vec![world_db::WorldSpaceRecord {
                id: WorldSpaceId(1),
                name: "test".into(),
                cell_size: 32.,
                minimum_y: 0.,
                maximum_y: 100.,
            }],
            vegetation_catalog: None,
        }
    }

    struct Harness {
        app: App,
        requests: Receiver<DatabaseRequest>,
        replies: Sender<DatabaseResult>,
        resident: Entity,
    }
    impl Harness {
        fn new() -> Self {
            let (requests, receiver) = bounded(1);
            let (replies, results) = bounded(8);
            let mut app = App::new();
            app.insert_resource(WorldDatabaseWorker {
                requests,
                results,
                thread: None,
            })
            .init_resource::<WorldGenerationReload>()
            .init_resource::<ActiveWorldSpace>()
            .init_resource::<WorldCatalog>()
            .init_resource::<WorldViewpoint>()
            .init_resource::<crate::WorldStartView>()
            .init_resource::<WorldOrigin>()
            .init_resource::<WorldStream>()
            .init_resource::<terrain_lod::TerrainLodStream>()
            .init_resource::<terrain_lod::entry::TerrainEntry>()
            .init_resource::<terrain_lod::UploadTracker>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<TerrainMaterial>>()
            .init_resource::<Assets<Image>>()
            .insert_resource(TerrainLodPreview {
                enabled: false,
                ..default()
            })
            .add_systems(
                Update,
                (receive_database_results, request_reload, advance_reload).chain(),
            );
            replies
                .send(DatabaseResult::Opened(Ok(manifest("old"))))
                .unwrap();
            app.update();
            let resident = app.world_mut().spawn_empty().id();
            app.world_mut().resource_mut::<WorldStream>().pages.insert(
                PageKey {
                    space: WorldSpaceId(1),
                    cell: CellCoord::ZERO,
                    domain: PageDomain::TerrainRender,
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
            let DatabaseRequest::Reload {
                request_id,
                expected_generation,
            } = self.requests.try_recv().unwrap()
            else {
                panic!("missing preparation request")
            };
            assert_eq!(expected_generation, "next");
            self.assert_old();
            request_id
        }
        fn prepared(&mut self, id: u64, candidate: RuntimeManifest) {
            self.replies
                .send(DatabaseResult::Reloaded {
                    request_id: id,
                    result: Ok(candidate),
                })
                .unwrap();
            self.app.update();
        }
        fn assert_old(&self) {
            let w = self.app.world();
            assert_eq!(w.resource::<WorldCatalog>().generation_id(), "old");
            assert_eq!(
                w.resource::<WorldStream>()
                    .manifest
                    .as_ref()
                    .unwrap()
                    .generation_id,
                "old"
            );
            assert!(w.get_entity(self.resident).is_ok());
            assert_eq!(w.resource::<WorldStream>().pages.len(), 1);
        }
        fn completion(&mut self) -> Option<Result<String, String>> {
            self.app
                .world_mut()
                .resource_mut::<WorldGenerationReload>()
                .take_completion()
        }
    }
    impl Drop for Harness {
        fn drop(&mut self) {
            // The fake worker has no consumer to drain a bounded shutdown request.
            while self.requests.try_recv().is_ok() {}
            self.app
                .world_mut()
                .remove_resource::<WorldDatabaseWorker>();
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
        assert!(
            matches!(h.requests.try_recv().unwrap(), DatabaseRequest::CommitReload { request_id, .. } if request_id == id)
        );
        h.assert_old();
        assert!(h.completion().is_none());
        h.replies
            .send(DatabaseResult::ReloadCommitted {
                request_id: id + 10,
                result: Ok(()),
            })
            .unwrap();
        h.app.update();
        h.assert_old();
        assert!(h.completion().is_none());
        h.replies
            .send(DatabaseResult::ReloadCommitted {
                request_id: id,
                result: Ok(()),
            })
            .unwrap();
        h.app.update();
        assert_eq!(h.completion(), Some(Ok("next".into())));
        assert_eq!(
            h.app.world().resource::<WorldCatalog>().generation_id(),
            "next"
        );
        assert!(h.app.world().get_entity(h.resident).is_err());
        assert!(h.app.world().resource::<WorldStream>().pages.is_empty());
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
                assert!(matches!(
                    h.requests.try_recv().unwrap(),
                    DatabaseRequest::CommitReload { .. }
                ));
                h.replies
                    .send(DatabaseResult::ReloadCommitted {
                        request_id: id,
                        result: Err("commit rejected".into()),
                    })
                    .unwrap();
            } else {
                h.replies
                    .send(DatabaseResult::Reloaded {
                        request_id: id,
                        result: Err("could not open candidate".into()),
                    })
                    .unwrap();
            }
            h.app.update();
            h.assert_old();
            assert!(h.completion().unwrap().is_err());
            assert!(
                matches!(h.requests.try_recv().unwrap(), DatabaseRequest::DiscardReload { request_id } if request_id == id)
            );
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
            .requests
            .try_send(DatabaseRequest::DiscardReload { request_id: 0 })
            .unwrap();
        h.prepared(id, manifest("next"));
        h.assert_old();
        assert!(
            !h.app
                .world()
                .resource::<WorldGenerationReload>()
                .commit_requested
        );
        assert!(h.completion().is_none());
        h.requests.try_recv().unwrap();
        h.app.update();
        assert!(
            h.app
                .world()
                .resource::<WorldGenerationReload>()
                .commit_requested
        );
        // Leave the commit occupying the queue while reporting its failure.
        h.replies
            .send(DatabaseResult::ReloadCommitted {
                request_id: id,
                result: Err("injected rejection".into()),
            })
            .unwrap();
        h.app.update();
        h.assert_old();
        assert!(h.completion().is_none());
        assert!(h.app.world().resource::<WorldGenerationReload>().active());
        assert!(matches!(
            h.requests.try_recv().unwrap(),
            DatabaseRequest::CommitReload { .. }
        ));
        h.app.update();
        h.assert_old();
        assert!(h.completion().unwrap().is_err());
        assert!(
            matches!(h.requests.try_recv().unwrap(), DatabaseRequest::DiscardReload { request_id } if request_id == id)
        );
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
            assert!(
                matches!(h.requests.try_recv().unwrap(), DatabaseRequest::DiscardReload { request_id } if request_id == id)
            );
        }
    }

    #[test]
    fn worker_keeps_both_snapshots_until_commit_and_rejects_stale_source_work() {
        use terrain_lod::{TerrainQuery, TerrainReply};
        let folder =
            std::env::temp_dir().join(format!("yarra-generation-worker-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("source.sqlite");
        let runtime = folder.join("runtime.sqlite");
        world_cook::create_demo_project(&source).unwrap();
        let mut project = world_db::read_project_database(&source).unwrap();
        project
            .cells
            .retain(|c| c.space == WorldSpaceId(1) && c.cell == CellCoord::ZERO);
        project.world_spaces[0].minimum_y = 0.;
        project.world_spaces[0].maximum_y = 100.;
        project.cells[0].height = 3.;
        project.objects.clear();
        project.environment_cells.clear();
        project.terrain_cell_heightfields.clear();
        std::fs::remove_file(&source).unwrap();
        world_db::write_project_database(&source, &project).unwrap();
        let old = world_cook::cook_project(&source, &runtime).unwrap();
        let (requests, input) = bounded(4);
        let (output, replies) = bounded(4);
        let path = runtime.clone();
        let worker = thread::spawn(move || database_worker(path, input, output));
        let recv = || replies.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(recv(), DatabaseResult::Opened(Ok(_))));
        project.cells[0].height = 8.;
        project.cells[0].source_revision += 1;
        let updated = folder.join("updated.sqlite");
        world_db::write_project_database(&updated, &project).unwrap();
        let next = world_cook::cook_project(&updated, &runtime).unwrap();
        assert_ne!(old.generation_id, next.generation_id);
        let roots = |generation: &str| {
            requests
                .send(DatabaseRequest::Terrain {
                    request_id: 100,
                    generation: generation.into(),
                    query: TerrainQuery::Roots(WorldSpaceId(1)),
                })
                .unwrap();
            let DatabaseResult::Terrain { result, .. } = recv() else {
                panic!("expected terrain reply")
            };
            result.map(|reply| {
                let TerrainReply::Metadata(m) = reply else {
                    panic!("expected roots")
                };
                assert_eq!(m.len(), 1);
                m[0].height_bounds[0]
            })
        };
        let index = |generation: &str| {
            requests
                .send(DatabaseRequest::ReadIndex {
                    generation: generation.into(),
                    revision: 1,
                    space: WorldSpaceId(1),
                    windows: vec![[CellCoord::ZERO; 2]],
                })
                .unwrap();
            let DatabaseResult::Index { result, .. } = recv() else {
                panic!("expected index")
            };
            result.map(|cells| {
                assert_eq!(cells.len(), 1);
                cells[0].minimum_y
            })
        };
        let page = |generation: &str| {
            requests
                .send(DatabaseRequest::ReadPage {
                    generation: generation.into(),
                    request_id: 101,
                    key: PageKey {
                        space: WorldSpaceId(1),
                        cell: CellCoord::ZERO,
                        domain: PageDomain::TerrainRender,
                        lod: 0,
                    },
                    height_only: true,
                })
                .unwrap();
            let DatabaseResult::Page { result, .. } = recv() else {
                panic!("expected page")
            };
            result.map(|p| {
                let encoded = p.unwrap().encoded;
                let checksum = encoded.checksum;
                encoded.decode().unwrap();
                checksum
            })
        };
        let old_page = page(&old.generation_id).unwrap();
        for id in [1, 2] {
            requests
                .send(DatabaseRequest::Reload {
                    request_id: id,
                    expected_generation: next.generation_id.clone(),
                })
                .unwrap();
            assert!(matches!(
                recv(),
                DatabaseResult::Reloaded { result: Ok(_), .. }
            ));
            // An older discard must not remove this operation's candidate.
            requests
                .send(DatabaseRequest::DiscardReload { request_id: 0 })
                .unwrap();
            assert!((roots(&old.generation_id).unwrap() - 3.).abs() < 0.01);
            assert!((roots(&next.generation_id).unwrap() - 8.).abs() < 0.01);
            assert!((index(&old.generation_id).unwrap() - 3.).abs() < 0.01);
            assert_eq!(page(&old.generation_id).unwrap(), old_page);
            assert!(index(&next.generation_id).is_err());
            assert!(page(&next.generation_id).is_err());
            for (wrong_id, wrong_generation) in [
                (id + 20, next.generation_id.clone()),
                (id, old.generation_id.clone()),
            ] {
                requests
                    .send(DatabaseRequest::CommitReload {
                        request_id: wrong_id,
                        expected_generation: wrong_generation,
                    })
                    .unwrap();
                assert!(matches!(
                    recv(),
                    DatabaseResult::ReloadCommitted { result: Err(_), .. }
                ));
            }
            if id == 1 {
                requests
                    .send(DatabaseRequest::DiscardReload { request_id: id })
                    .unwrap();
                assert!(roots(&next.generation_id).is_err());
                assert_eq!(page(&old.generation_id).unwrap(), old_page);
            }
        }
        requests
            .send(DatabaseRequest::CommitReload {
                request_id: 2,
                expected_generation: next.generation_id.clone(),
            })
            .unwrap();
        assert!(matches!(
            recv(),
            DatabaseResult::ReloadCommitted { result: Ok(()), .. }
        ));
        assert!(index(&old.generation_id).is_err());
        assert!(page(&old.generation_id).is_err());
        assert!(roots(&old.generation_id).is_err());
        assert!((index(&next.generation_id).unwrap() - 8.).abs() < 0.01);
        assert_ne!(page(&next.generation_id).unwrap(), old_page);
        requests.send(DatabaseRequest::Shutdown).unwrap();
        worker.join().unwrap();
        std::fs::remove_dir_all(folder).unwrap();
    }
}
