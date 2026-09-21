use super::*;
use std::time::Duration;
use world::{CellCoord, PageDomain, PageKey, WorldSpaceId};

struct EmptyRuntime {
    folder: PathBuf,
    path: PathBuf,
}
impl EmptyRuntime {
    fn new(name: &str) -> Self {
        let folder = std::env::temp_dir().join(format!("yarra-db-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("runtime.sqlite");
        world_db::write_runtime_database(
            &path,
            &world_db::RuntimeBuild {
                manifest: world_db::RuntimeManifest {
                    schema_version: world::RUNTIME_SCHEMA_VERSION,
                    generation_id: "empty".into(),
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
                    }],
                    vegetation_catalog: None,
                },
                cells: vec![],
                pages: vec![],
                terrain_surfaces: vec![],
                terrain_texture_sets: vec![],
                terrain_texture_layers: vec![],
                terrain_profiles: vec![],
                assets: vec![],
                definitions: vec![],
                dependencies: vec![],
                definition_dependencies: vec![],
                terrain_surface_dependencies: vec![],
            },
        )
        .unwrap();
        Self { folder, path }
    }
}
impl Drop for EmptyRuntime {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.folder).unwrap();
    }
}

fn wait_until(ready: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "worker did not reach the expected channel wait"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn assert_drop_finishes(worker: WorldDatabaseWorker) {
    let (done, finished) = bounded(1);
    let teardown = thread::spawn(move || {
        drop(worker);
        done.send(()).unwrap();
    });
    finished
        .recv_timeout(Duration::from_secs(5))
        .expect("worker shutdown deadlocked");
    teardown.join().unwrap();
}

#[test]
fn shutdown_interrupts_full_reply_and_request_queues() {
    let runtime = EmptyRuntime::new("saturated-shutdown");
    let worker = WorldDatabaseWorker::spawn(runtime.path.clone());
    assert_eq!(worker.requests.capacity(), Some(16));
    assert_eq!(worker.results.capacity(), Some(32));
    assert!(matches!(
        worker.results.recv_timeout(Duration::from_secs(5)),
        Ok(DatabaseResult::Opened(Ok(_)))
    ));
    let query = || DatabaseRequest::ReadIndex {
        generation: "empty".into(),
        revision: 1,
        space: WorldSpaceId(1),
        windows: vec![[CellCoord::ZERO; 2]],
    };
    // Fill replies, then leave the reader blocked delivering one more result.
    for _ in 0..RESULT_CAPACITY {
        worker
            .requests
            .send_timeout(query(), Duration::from_secs(5))
            .unwrap();
    }
    wait_until(|| worker.results.is_full());
    worker.try_send(query()).unwrap();
    wait_until(|| worker.requests.is_empty());
    // Further work must stay bounded, even with no reply consumer making progress.
    for _ in 0..REQUEST_CAPACITY {
        worker.try_send(query()).unwrap();
    }
    assert!(matches!(
        worker.try_send(query()),
        Err(TrySendError::Full(_))
    ));
    assert_drop_finishes(worker);
}

#[test]
fn shutdown_interrupts_idle_and_startup_reply_waits() {
    let runtime = EmptyRuntime::new("idle-shutdown");
    let worker = WorldDatabaseWorker::spawn(runtime.path.clone());
    assert!(matches!(
        worker.results.recv_timeout(Duration::from_secs(5)),
        Ok(DatabaseResult::Opened(Ok(_)))
    ));
    assert_drop_finishes(worker);

    // A rendezvous reply channel forces startup to wait for a consumer, too.
    let worker = WorldDatabaseWorker::spawn_with_capacity(runtime.path.clone(), 1, 0);
    assert_drop_finishes(worker);
}

#[test]
fn failed_open_reports_the_path_then_disconnects_without_creating_a_database() {
    let path = std::env::temp_dir().join(format!("yarra-missing-db-{}.sqlite", std::process::id()));
    assert!(!path.exists());
    let worker = WorldDatabaseWorker::spawn(path.clone());
    let DatabaseResult::Opened(Err(error)) =
        worker.results.recv_timeout(Duration::from_secs(5)).unwrap()
    else {
        panic!("missing runtime must fail to open");
    };
    assert!(error.contains(path.to_str().unwrap()));
    assert!(matches!(
        worker.results.recv_timeout(Duration::from_secs(5)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    ));
    assert!(!path.exists());
    assert_drop_finishes(worker);
}

#[test]
fn worker_keeps_both_snapshots_until_commit_and_rejects_stale_source_work() {
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
    let worker = WorldDatabaseWorker::spawn(runtime.clone());
    let requests = &worker.requests;
    let replies = &worker.results;
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
    drop(worker);
    std::fs::remove_dir_all(folder).unwrap();
}
#[test]
fn database_worker_reopens_the_exact_published_generation() {
    let folder = std::env::temp_dir().join(format!("yarra-worker-reopen-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let source = folder.join("project.sqlite");
    let path = folder.join("runtime.sqlite");
    world_cook::create_demo_project(&source).unwrap();
    world_cook::cook_project(&source, &path).unwrap();
    let worker = WorldDatabaseWorker::spawn(path);
    let requests = &worker.requests;
    let result_receiver = &worker.results;

    let DatabaseResult::Opened(Ok(manifest)) = result_receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
    else {
        panic!("runtime worker did not open the current cooked world");
    };
    let expected = manifest.generation_id;
    requests
        .send(DatabaseRequest::Reload {
            request_id: 4,
            expected_generation: "not-the-published-generation".into(),
        })
        .unwrap();
    let DatabaseResult::Reloaded {
        request_id: 4,
        result: Err(error),
    } = result_receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
    else {
        panic!("runtime worker accepted the wrong generation identity");
    };
    assert!(error.contains("generation mismatch"));

    requests
        .send(DatabaseRequest::Reload {
            request_id: 5,
            expected_generation: expected.clone(),
        })
        .unwrap();
    let DatabaseResult::Reloaded {
        request_id,
        result: Ok(reloaded),
    } = result_receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
    else {
        panic!("runtime worker did not reopen the published generation");
    };
    assert_eq!(request_id, 5);
    assert_eq!(reloaded.generation_id, expected);

    drop(worker);
    std::fs::remove_dir_all(folder).unwrap();
}
