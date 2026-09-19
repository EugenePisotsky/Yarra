use super::*;
use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    render::{
        RenderPlugin,
        gpu_readback::{Readback, ReadbackComplete},
        pipelined_rendering::PipelinedRenderingPlugin,
    },
    window::ExitCondition,
    winit::WinitPlugin,
};

#[test]
fn hierarchy_is_default_and_legacy_requires_an_explicit_request() {
    assert!(TerrainLodPreview::from_args(["yarra-app-game"]).enabled);
    assert!(
        TerrainLodPreview::from_args(["yarra-app-editor", "--world-db", "world.sqlite"]).enabled
    );
    assert!(!TerrainLodPreview::from_args(["yarra-app-game", "--terrain-legacy"]).enabled);
}

#[test]
fn late_database_reply_cannot_enter_a_new_world_or_generation() {
    let mut stream = TerrainLodStream {
        next_id: 40,
        identity: Some(("new-generation".into(), WorldSpaceId(2))),
        ..default()
    };
    stream.receive(39, Err("an old generation failed".into()));
    assert!(stream.error.is_none());
    assert!(stream.metadata.is_empty());
    let (requests, _request_receiver) = bounded(4);
    let (_result_sender, results) = bounded(4);
    let worker = WorldDatabaseWorker {
        requests,
        results,
        thread: None,
    };
    stream.request(&worker, TerrainQuery::Roots(WorldSpaceId(2)));
    assert!(stream.pending.contains_key(&41));
    stream.receive(41, Ok(TerrainReply::Metadata(vec![])));
    assert_eq!(stream.roots, Some(vec![]));
}

#[derive(Resource, Clone, Default)]
struct Pixels(Arc<Mutex<Vec<u8>>>);

/// Exercises the actual database worker, task pool, GPU upload acknowledgements and
/// complete-cover swaps. It is short functional validation, not a frame-time benchmark.
#[test]
#[ignore = "requires native Metal/GPU; cooks a temporary 2 km mountain"]
fn mountain_cover_uploads_draws_moves_and_rebases() {
    let folder = std::env::temp_dir().join(format!("yarra-lod-gpu-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let source = folder.join("source.sqlite");
    let runtime = folder.join("runtime.sqlite");
    world_cook::create_mountain_fixture(&source).unwrap();
    // A second, small terrain world exercises an actual uploaded destination,
    // rather than treating an empty root list as sufficient coverage.
    let mut project = world_db::read_project_database(&source).unwrap();
    for space in &mut project.world_spaces {
        space.cell_size = 32.;
        space.maximum_y = 1600.;
    }
    let cells: Vec<_> = project
        .cells
        .iter()
        .filter(|c| (0..2).contains(&c.cell.x) && (0..2).contains(&c.cell.z))
        .cloned()
        .map(|mut c| {
            c.space = WorldSpaceId(2);
            c
        })
        .collect();
    let fields: Vec<_> = project
        .terrain_cell_heightfields
        .iter()
        .filter(|c| (0..2).contains(&c.cell.x) && (0..2).contains(&c.cell.z))
        .cloned()
        .map(|mut c| {
            c.space = WorldSpaceId(2);
            c
        })
        .collect();
    project.cells.extend(cells);
    project.terrain_cell_heightfields.extend(fields);
    std::fs::remove_file(&source).unwrap();
    world_db::write_project_database(&source, &project).unwrap();
    world_cook::cook_project(&source, &runtime).unwrap();
    // Synthetic baked maps exercise real DB material IO without local source textures.
    // All drawable nodes are populated, preserving the production coverage contract.
    let store = world_db::TerrainMaterialCookStore::open_staging(&runtime).unwrap();
    for space in [WorldSpaceId(1), WorldSpaceId(2)] {
        for level in 0..=world::MAX_TERRAIN_NODE_LEVEL {
            let mut after = None;
            loop {
                let keys = store.keys(space, level, after).unwrap();
                if keys.is_empty() {
                    break;
                }
                for (key, drawable) in keys {
                    after = Some(CellCoord {
                        x: key.0.x,
                        z: key.0.z,
                    });
                    if drawable {
                        store
                            .insert(&world::TerrainComposite {
                                key,
                                fingerprint: [7; 32],
                                mips: (0..world::TERRAIN_COMPOSITE_MIPS)
                                    .map(|m| {
                                        let n = world::TerrainComposite::mip_size(m).pow(2);
                                        world::TerrainCompositeMip {
                                            color: [70, 150, 45, 255].repeat(n),
                                            response: [128, 128, 255, 255].repeat(n),
                                        }
                                    })
                                    .collect(),
                            })
                            .unwrap();
                    }
                }
            }
        }
    }
    store.finish().unwrap();
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path: format!("{}/../../assets", env!("CARGO_MANIFEST_DIR")),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                synchronous_pipeline_compilation: false,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<PipelinedRenderingPlugin>(),
    )
    .add_plugins((
        terrain_render::TerrainRenderPlugin,
        WorldStreamingPlugin::editor(&runtime),
    ))
    .init_resource::<Pixels>()
    .add_systems(Startup, setup)
    .add_systems(Update, crate::ground_characters_to_streamed_terrain);
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    assert!(app.world().resource::<TerrainLodPreview>().enabled);
    settle(&mut app, deadline);
    let first = app.world().resource::<TerrainLodStats>().clone();
    assert!(first.patches >= 4);
    assert_eq!(first.material_status, "streamed baked ground");
    assert!(first.material_detail_tiles > 0 && first.material_detail_tiles <= 128);
    let uploads = first.material_detail_uploads;
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(
        app.world()
            .resource::<TerrainLodStats>()
            .material_detail_uploads,
        uploads,
        "stationary camera must not keep uploading material pixels"
    );
    assert!(first.material_tiles > 0);
    assert!(first.material_tiles < first.patches);
    assert!(first.levels.keys().any(|&l| l > 0));
    let pixels = app.world().resource::<Pixels>().0.lock().unwrap().clone();
    assert!(
        pixels
            .chunks_exact(4)
            .filter(|p| p[1] > p[0] && p[1] > p[2])
            .count()
            > 512,
        "mountain geometry was not rendered"
    );
    if let Some(path) = std::env::var_os("YARRA_LOD_CAPTURE") {
        std::fs::write(path, &pixels).unwrap();
    }
    live_authoring_handoff(&mut app, &runtime, deadline);
    // Withhold GPU acknowledgements for a forced coarse replacement. Even completed
    // CPU mesh jobs must not remove the last valid cover.
    let pinned: BTreeSet<_> = app
        .world()
        .resource::<TerrainLodStream>()
        .active
        .keys()
        .copied()
        .collect();
    force_coarse_target(&mut app);
    app.world()
        .resource::<UploadTracker>()
        .0
        .lock()
        .unwrap()
        .pause_acknowledgements = true;
    for _ in 0..24 {
        app.update();
        assert_eq!(
            pinned,
            app.world()
                .resource::<TerrainLodStream>()
                .active
                .keys()
                .copied()
                .collect()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.world().resource::<TerrainLodStream>().target.is_some());
    app.world()
        .resource::<UploadTracker>()
        .0
        .lock()
        .unwrap()
        .pause_acknowledgements = false;
    // Hold the first blend pose while asynchronously compiled morph pipelines settle.
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::ZERO,
    ));
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "morph readiness timeout: {:?}",
            app.world().resource::<TerrainLodStats>()
        );
        app.update();
        assert_valid_cover(&app);
        if app
            .world()
            .resource::<TerrainLodStream>()
            .transition
            .as_ref()
            .is_some_and(|t| t.running())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    pump_pixels(&mut app);
    let before_morph = app.world().resource::<Pixels>().0.lock().unwrap().clone();
    let mismatch = pixels
        .chunks_exact(4)
        .zip(before_morph.chunks_exact(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 3))
        .count();
    assert!(
        mismatch < 512 * 512 / 100,
        "morph start popped: {mismatch} pixels changed"
    );
    app.world_mut()
        .resource_mut::<TerrainLodStream>()
        .transition
        .as_mut()
        .unwrap()
        .set_fraction(0.5);
    pump_pixels(&mut app);
    assert_eq!(
        app.world().resource::<TerrainLodStats>().morph_weight,
        Some(0.5)
    );
    let middle = app.world().resource::<Pixels>().0.lock().unwrap().clone();
    assert!(
        middle
            .chunks_exact(4)
            .filter(|p| p[1] > p[0] && p[1] > p[2])
            .count()
            > 512
    );
    assert_ne!(
        middle, before_morph,
        "GPU morph weight did not change the surface"
    );
    // Rebase during a held intermediate pose: geometry and shared weights persist.
    let morph_entities = app
        .world()
        .resource::<TerrainLodStream>()
        .transition
        .as_ref()
        .unwrap()
        .entities
        .clone();
    set_origin(&mut app, CellCoord { x: 10, z: -10 });
    pump_pixels(&mut app);
    assert_eq!(
        morph_entities,
        app.world()
            .resource::<TerrainLodStream>()
            .transition
            .as_ref()
            .unwrap()
            .entities
    );
    assert_eq!(
        app.world().resource::<TerrainLodStats>().morph_weight,
        Some(0.5)
    );
    let rebased = app.world().resource::<Pixels>().0.lock().unwrap().clone();
    let mismatch = middle
        .chunks_exact(4)
        .zip(rebased.chunks_exact(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 3))
        .count();
    assert!(
        mismatch < 512 * 512 / 100,
        "rebase changed morph image: {mismatch}"
    );
    set_origin(&mut app, CellCoord { x: 0, z: 0 });
    // Teleport an actor into a held intermediate pose, independently of the far
    // overview camera. It must wait for exact ground and be grounded before shown.
    let actor = app
        .world_mut()
        .spawn((
            crate::actor::TerrainGrounded,
            Transform::from_xyz(600., -100., -500.),
            Visibility::Inherited,
        ))
        .id();
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(actor).unwrap(),
        Visibility::Hidden
    );
    assert!(app.world().resource::<TerrainLodStats>().contact_handoffs > 0);
    app.insert_resource(bevy::time::TimeUpdateStrategy::Automatic);
    settle(&mut app, deadline);
    let world = app.world();
    let position = world.get::<GlobalTransform>(actor).unwrap().translation();
    let sampled = world
        .resource::<TerrainLodStream>()
        .sample_contact_height(
            position,
            world.resource::<WorldOrigin>(),
            world.resource::<TerrainContactReadiness>(),
        )
        .expect("offscreen grounded actor did not get contact detail");
    assert!((sampled - position.y).abs() <= 0.001);
    assert_eq!(
        *world.get::<Visibility>(actor).unwrap(),
        Visibility::Inherited
    );
    assert_eq!(world.resource::<TerrainLodStats>().blocked_actors, 0);
    // A smaller geometry budget must shed visual detail while keeping the actor's
    // certified surface through an actual uploaded cover replacement.
    let original_settings = app.world().resource::<TerrainLodPreview>().settings.clone();
    // Make distant detail compete even in this small 512-pixel test view.
    {
        let mut config = app.world_mut().resource_mut::<TerrainLodPreview>();
        config.settings.refine_pixels = 0.125;
        config.settings.collapse_pixels = 0.0625;
    }
    settle(&mut app, deadline);
    let before_budget = app.world().resource::<TerrainLodStats>().triangles;
    assert!(before_budget > 128 * 2048);
    app.world_mut()
        .resource_mut::<TerrainLodPreview>()
        .settings
        .max_triangles = 128 * 2048;
    settle(&mut app, deadline);
    let limited = app.world().resource::<TerrainLodStats>();
    println!(
        "Live terrain budget: {before_budget} -> {} triangles, {} blocked actors",
        limited.triangles, limited.blocked_actors
    );
    assert!(limited.triangles <= 128 * 2048);
    assert_eq!(limited.blocked_actors, 0);
    assert_eq!(
        *app.world().get::<Visibility>(actor).unwrap(),
        Visibility::Inherited
    );
    app.world_mut().resource_mut::<TerrainLodPreview>().settings = original_settings;
    settle(&mut app, deadline);
    let world = app.world();
    // An obsolete coarse target must not take away an actor's certified ground.
    let protected: BTreeSet<_> = world
        .resource::<TerrainLodStream>()
        .active
        .keys()
        .copied()
        .collect();
    force_coarse_target(&mut app);
    for _ in 0..8 {
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(actor).unwrap(),
            Visibility::Inherited
        );
        assert_eq!(
            protected,
            app.world()
                .resource::<TerrainLodStream>()
                .active
                .keys()
                .copied()
                .collect()
        );
    }
    grass_source_gate(&mut app);
    app.world_mut().despawn(actor);
    settle(&mut app, deadline);
    // Looking steeply down over the summit, then teleport near the valley floor.
    for (eye, target) in [
        (Vec3::new(40., 1600., 0.), Vec3::new(40., 100., -1.)),
        (Vec3::new(-350., 160., -450.), Vec3::new(100., 700., 0.)),
    ] {
        let w = app.world_mut();
        let mut query = w.query_filtered::<&mut Transform, With<WorldViewCamera>>();
        *query.single_mut(w).unwrap() =
            Transform::from_translation(eye).looking_at(target, Vec3::Z);
        settle(&mut app, deadline);
        assert_valid_cover(&app);
    }
    settle_sources(&mut app, deadline);
    {
        let w = app.world_mut();
        let stats = w.resource::<StreamingStats>();
        assert!(
            stats.height_only_pages > 49,
            "camera sources did not extend beyond the local index: {}",
            stats.height_only_pages
        );
        assert_eq!(
            stats.gpu_bytes_estimate, 0,
            "source pages allocated GPU resources"
        );
        assert!(stats.source_demand_error.is_none());
        assert_eq!(stats.budget_waiting, 0);
        eprintln!(
            "LOD sources: {} indexed cells, {} height-only pages, {} decoded bytes",
            stats.indexed_cells, stats.height_only_pages, stats.decoded_bytes
        );
        let mut surfaces = w.query_filtered::<Option<&Mesh3d>, With<StreamedTerrainSurface>>();
        assert!(surfaces.iter(w).all(|mesh| mesh.is_none()));
        assert_eq!(w.resource::<Assets<TerrainMaterial>>().len(), 0);
    }
    let before: BTreeSet<_> = app
        .world()
        .resource::<TerrainLodStream>()
        .active
        .keys()
        .map(|(k, _)| *k)
        .collect();
    // A render-origin change does not flush the hierarchy or change canonical demand.
    let offset = Vec3::new(320., 0., -320.);
    {
        let w = app.world_mut();
        let mut origin = w.resource_mut::<WorldOrigin>();
        origin.cell = CellCoord { x: 10, z: -10 };
        let mut vp = w.resource_mut::<WorldViewpoint>();
        vp.position = Some(WorldPosition {
            space: WorldSpaceId(1),
            cell: CellCoord { x: 10, z: -10 },
            local: [0.; 3],
        });
        let mut query = w.query_filtered::<&mut Transform, With<WorldViewCamera>>();
        query.single_mut(w).unwrap().translation -= offset;
    }
    settle(&mut app, deadline);
    let after: BTreeSet<_> = app
        .world()
        .resource::<TerrainLodStream>()
        .active
        .keys()
        .map(|(k, _)| *k)
        .collect();
    assert_eq!(before, after);
    assert_valid_cover(&app);
    eprintln!(
        "2 km GPU LOD: initial={first:?}; final={:?}",
        app.world().resource::<TerrainLodStats>()
    );
    // Cancelling a live group on a world change releases transient entities and data.
    force_coarse_target(&mut app);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::ZERO,
    ));
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "cancel test readiness timeout"
        );
        app.update();
        let stream = app.world().resource::<TerrainLodStream>();
        assert!(stream.error.is_none(), "{:?}", stream.error);
        if stream.transition.as_ref().is_some_and(|t| t.running()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let cancelled: Vec<_> = app
        .world()
        .resource::<TerrainLodStream>()
        .transition
        .as_ref()
        .unwrap()
        .entities
        .values()
        .copied()
        .collect();
    app.world()
        .resource::<UploadTracker>()
        .0
        .lock()
        .unwrap()
        .pause_material_acknowledgements = true;
    app.world_mut()
        .resource_mut::<ActiveWorldSpace>()
        .request(WorldSpaceId(2), [0.; 3]);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "entry staging timed out"
        );
        app.update();
        assert_eq!(
            app.world().resource::<ActiveWorldSpace>().current(),
            Some(WorldSpaceId(1))
        );
        for &entity in &cancelled {
            assert!(app.world().get_entity(entity).is_ok());
        }
        let stage = app.world().resource::<entry::TerrainEntry>();
        let current = app.world().resource::<TerrainLodStream>();
        assert!(
            current.composites.bytes + stage.test_stream().composites.bytes
                <= material::MAX_MATERIAL_BYTES
        );
        assert!(current.decoded_bytes() + stage.test_stream().decoded_bytes() <= MAX_NODE_BYTES);
        assert!(current.mesh_bytes() + stage.test_stream().mesh_bytes() + 84 <= MAX_MESH_BYTES);
        if !stage.test_stream().composites.resident.is_empty()
            && stage
                .test_stream()
                .target_uploaded(app.world().resource::<UploadTracker>())
            && app
                .world()
                .resource::<UploadTracker>()
                .0
                .lock()
                .unwrap()
                .entry_pipelines_ready
        {
            assert!(
                !stage
                    .test_stream()
                    .materials_ready(app.world().resource::<UploadTracker>())
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..6 {
        app.update();
        assert_eq!(
            app.world().resource::<ActiveWorldSpace>().current(),
            Some(WorldSpaceId(1))
        );
    }
    // Meshes and pipelines are ready, but withheld material bindings still block entry.
    // Failed entry releases only staged resources, preserving the current morph.
    app.world_mut()
        .resource_mut::<entry::TerrainEntry>()
        .test_fail("injected destination decode failure");
    app.update();
    assert!(
        app.world()
            .resource::<ActiveWorldSpace>()
            .transition_error()
            .unwrap()
            .contains("decode failure")
    );
    for &entity in &cancelled {
        assert!(app.world().get_entity(entity).is_ok());
    }
    assert!(!app.world().resource::<entry::TerrainEntry>().pending());
    // Retry, then replace it with a same-world request before its upload finishes.
    app.world_mut()
        .resource_mut::<ActiveWorldSpace>()
        .request(WorldSpaceId(2), [0.; 3]);
    for _ in 0..4 {
        app.update();
    }
    app.world_mut()
        .resource_mut::<ActiveWorldSpace>()
        .request(WorldSpaceId(1), [0.; 3]);
    app.update();
    assert!(!app.world().resource::<entry::TerrainEntry>().pending());
    app.world_mut()
        .resource_mut::<ActiveWorldSpace>()
        .request(WorldSpaceId(2), [0.; 3]);
    app.world()
        .resource::<UploadTracker>()
        .0
        .lock()
        .unwrap()
        .pause_material_acknowledgements = false;
    while app.world().resource::<ActiveWorldSpace>().current() != Some(WorldSpaceId(2)) {
        assert!(
            std::time::Instant::now() < deadline,
            "entry commit timed out: {:?}",
            app.world().resource::<TerrainLodStats>()
        );
        app.update();
        std::thread::sleep(Duration::from_millis(5));
    }
    let stream = app.world().resource::<TerrainLodStream>();
    assert_eq!(stream.identity.as_ref().unwrap().1, WorldSpaceId(2));
    assert!(!stream.active.is_empty() && !stream.meshes.is_empty() && !stream.nodes.is_empty());
    assert_eq!(
        stream
            .active
            .keys()
            .map(|(k, _)| 1_u64 << (2 * k.level))
            .sum::<u64>(),
        4
    );
    assert!(
        stream
            .active
            .keys()
            .all(|(k, _)| k.space == WorldSpaceId(2))
    );
    assert!(stream.transition.is_none());
    for entity in cancelled {
        assert!(app.world().get_entity(entity).is_err());
    }
    assert!(
        app.world()
            .resource::<UploadTracker>()
            .0
            .lock()
            .unwrap()
            .probe
            .is_none()
    );
    let mut morphs = app.world_mut().query::<&bevy::mesh::morph::MorphWeights>();
    assert_eq!(morphs.iter(app.world()).count(), 0);
    publication_reload(&mut app, project, &runtime, &folder, deadline);
    drop(app);
    std::fs::remove_dir_all(folder).unwrap();
}

fn live_authoring_handoff(app: &mut App, runtime: &std::path::Path, deadline: std::time::Instant) {
    use world::{TerrainMaterialKey, TerrainPreviewProducts};
    let reader = RuntimeReader::open_immutable(runtime).unwrap();
    let (identity, root, patch, meshes_before) = {
        let stream = app.world().resource::<TerrainLodStream>();
        (
            stream.identity.clone().unwrap(),
            stream.roots.as_ref().unwrap()[0],
            *stream.active.keys().next().unwrap(),
            stream
                .active
                .keys()
                .map(|p| (*p, stream.meshes[p].handle.id()))
                .collect::<BTreeMap<_, _>>(),
        )
    };
    let key = TerrainMaterialKey(root);
    let baseline = reader
        .read_terrain_composite(key)
        .unwrap()
        .unwrap()
        .decode()
        .unwrap();
    let mut painted = baseline.clone();
    painted.fingerprint = [171; 32];
    for mip in &mut painted.mips {
        for p in mip.color.chunks_exact_mut(4) {
            p[..3].copy_from_slice(&[180, 50, 20]);
        }
    }
    let mut products = TerrainPreviewProducts::default();
    products.composites.insert(key, Arc::new(painted));
    let request = |revision, products| TerrainPreviewRequest {
        revision,
        generation: identity.0.clone(),
        space: identity.1,
        products: Arc::new(products),
    };
    app.world_mut().resource_mut::<LiveTerrainPreview>().request =
        Some(request(1, products.clone()));
    app.world()
        .resource::<UploadTracker>()
        .0
        .lock()
        .unwrap()
        .pause_material_acknowledgements = true;
    for _ in 0..8 {
        app.update();
    }
    assert!(app.world().resource::<LiveTerrainPreview>().ready.is_none());
    assert_eq!(
        app.world().resource::<TerrainLodStream>().overlay_revision,
        0
    );
    // Supersede an uploaded-but-unacknowledged paint revision. It must never be
    // committed when its old GPU acknowledgement arrives.
    app.world_mut().resource_mut::<LiveTerrainPreview>().request =
        Some(request(2, products.clone()));
    app.world()
        .resource::<UploadTracker>()
        .0
        .lock()
        .unwrap()
        .pause_material_acknowledgements = false;
    wait_live(app, 2, deadline);
    assert!(app.world().resource::<TerrainLodStream>().overlay.is_none());
    app.world_mut().resource_mut::<LiveTerrainPreview>().commit = Some(2);
    app.update();
    assert_eq!(
        app.world().resource::<LiveTerrainPreview>().applied,
        Some(2)
    );
    let stream = app.world().resource::<TerrainLodStream>();
    for (p, id) in &meshes_before {
        assert_eq!(
            stream.meshes[p].handle.id(),
            *id,
            "paint allocated a replacement geometry mesh"
        );
    }
    let mut node = reader
        .read_terrain_node(patch.0)
        .unwrap()
        .unwrap()
        .decode()
        .unwrap();
    let original = node.clone();
    let field = node.heightfield.as_mut().unwrap();
    let mid = field.heights.len() / 2;
    field.heights[mid] += 0.25;
    products.nodes.insert(patch.0, Arc::new(node));
    app.world_mut().resource_mut::<LiveTerrainPreview>().request =
        Some(request(3, products.clone()));
    wait_live(app, 3, deadline);
    assert_eq!(
        app.world().resource::<TerrainLodStream>().meshes[&patch]
            .handle
            .id(),
        meshes_before[&patch]
    );
    app.world_mut().resource_mut::<LiveTerrainPreview>().commit = Some(3);
    app.update();
    assert_ne!(
        app.world().resource::<TerrainLodStream>().meshes[&patch]
            .handle
            .id(),
        meshes_before[&patch]
    );
    // Undo uses explicit original products and waits for the same complete handoff.
    products.nodes.insert(patch.0, Arc::new(original.clone()));
    products.composites.insert(key, Arc::new(baseline.clone()));
    app.world_mut().resource_mut::<LiveTerrainPreview>().request = Some(request(4, products));
    wait_live(app, 4, deadline);
    app.world_mut().resource_mut::<LiveTerrainPreview>().commit = Some(4);
    app.update();
    let stream = app.world().resource::<TerrainLodStream>();
    assert_eq!(stream.nodes[&patch.0], original);
    assert_eq!(*stream.overlay.as_ref().unwrap().composites[&key], baseline);
    app.world_mut().resource_mut::<LiveTerrainPreview>().request = None;
    settle(app, deadline);
}
fn wait_live(app: &mut App, revision: u64, deadline: std::time::Instant) {
    loop {
        app.update();
        let live = app.world().resource::<LiveTerrainPreview>();
        assert!(live.error.is_none(), "{:?}", live.error);
        if live.ready == Some(revision) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "live preview stalled at {revision}: {:?}",
            app.world().resource::<TerrainLodStats>()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn publication_reload(
    app: &mut App,
    mut project: world_db::ProjectDocument,
    runtime: &std::path::Path,
    folder: &std::path::Path,
    deadline: std::time::Instant,
) {
    settle_sources(app, deadline);
    let old_generation = app
        .world()
        .resource::<WorldCatalog>()
        .generation_id()
        .to_owned();
    let old_terrain: Vec<_> = app
        .world()
        .resource::<TerrainLodStream>()
        .active
        .values()
        .copied()
        .collect();
    let old_sources: Vec<_> = app
        .world()
        .resource::<WorldStream>()
        .pages
        .values()
        .flat_map(|p| match p {
            PageState::Resident(a) => a.entities.clone(),
            _ => vec![],
        })
        .collect();
    assert!(!old_terrain.is_empty() && !old_sources.is_empty());
    // A small second publication changes actual relief, not just the generation label.
    project.cells.retain(|c| c.space == WorldSpaceId(2));
    project
        .terrain_cell_heightfields
        .retain(|c| c.space == WorldSpaceId(2));
    for c in &mut project.cells {
        c.height = 45.;
        c.source_revision += 1;
    }
    for c in &mut project.terrain_cell_heightfields {
        c.heights.fill(45.);
        c.source_revision += 1;
    }
    let source = folder.join("publication.sqlite");
    world_db::write_project_database(&source, &project).unwrap();
    let next = world_cook::cook_project(&source, runtime).unwrap();
    assert_ne!(next.generation_id, old_generation);
    app.world()
        .resource::<UploadTracker>()
        .0
        .lock()
        .unwrap()
        .pause_acknowledgements = true;
    let assert_old = |app: &App| {
        assert_eq!(
            app.world().resource::<WorldCatalog>().generation_id(),
            old_generation
        );
        assert_eq!(
            app.world()
                .resource::<TerrainLodStream>()
                .identity
                .as_ref()
                .unwrap()
                .0,
            old_generation
        );
        for &e in old_terrain.iter().chain(&old_sources) {
            assert!(app.world().get_entity(e).is_ok());
        }
    };
    for attempt in 0..2 {
        assert!(
            app.world_mut()
                .resource_mut::<WorldGenerationReload>()
                .request(next.generation_id.clone())
        );
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "publication staging timeout: {:?}",
                app.world().resource::<WorldGenerationReload>()
            );
            app.update();
            assert_old(app);
            assert!(
                app.world()
                    .resource::<WorldGenerationReload>()
                    .completion
                    .is_none()
            );
            let stage = app.world().resource::<entry::TerrainEntry>();
            let current = app.world().resource::<TerrainLodStream>();
            assert!(
                current.decoded_bytes() + stage.test_stream().decoded_bytes() <= MAX_NODE_BYTES
            );
            assert!(current.mesh_bytes() + stage.test_stream().mesh_bytes() + 84 <= MAX_MESH_BYTES);
            if !stage.test_stream().meshes.is_empty() && stage.test_stream().builds.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        for _ in 0..6 {
            app.update();
            assert_old(app);
            assert!(
                !app.world()
                    .resource::<WorldGenerationReload>()
                    .commit_requested
            );
        }
        if attempt == 0 {
            app.world_mut()
                .resource_mut::<entry::TerrainEntry>()
                .test_fail("injected publication decode failure");
            loop {
                assert!(
                    std::time::Instant::now() < deadline,
                    "publication rejection timeout"
                );
                app.update();
                assert_old(app);
                if let Some(result) = app
                    .world_mut()
                    .resource_mut::<WorldGenerationReload>()
                    .take_completion()
                {
                    assert!(result.unwrap_err().contains("publication decode failure"));
                    break;
                }
            }
            assert!(!app.world().resource::<entry::TerrainEntry>().pending());
            assert!(
                app.world()
                    .resource::<entry::TerrainEntry>()
                    .test_stream()
                    .meshes
                    .is_empty()
            );
            settle_sources(app, deadline);
        }
    }
    app.world()
        .resource::<UploadTracker>()
        .0
        .lock()
        .unwrap()
        .pause_acknowledgements = false;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "publication commit timeout: {:?}",
            app.world().resource::<WorldGenerationReload>()
        );
        app.update();
        if let Some(result) = app
            .world_mut()
            .resource_mut::<WorldGenerationReload>()
            .take_completion()
        {
            assert_eq!(result.unwrap(), next.generation_id);
            break;
        }
        assert_old(app);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        app.world().resource::<WorldCatalog>().generation_id(),
        next.generation_id
    );
    let terrain = app.world().resource::<TerrainLodStream>();
    assert_eq!(
        terrain.identity.as_ref().unwrap(),
        &(next.generation_id.clone(), WorldSpaceId(2))
    );
    assert!(!terrain.active.is_empty());
    for d in terrain.descriptors.values() {
        assert!((d.height_bounds[0] - 45.).abs() < 0.03);
    }
    for &e in old_terrain.iter().chain(&old_sources) {
        assert!(app.world().get_entity(e).is_err());
    }
    assert!(!app.world().resource::<entry::TerrainEntry>().pending());
    assert_valid_cover(app);
    settle_sources(app, deadline);
    let mut query = app.world_mut().query::<&StreamedTerrainSurface>();
    let sources: Vec<_> = query.iter(app.world()).collect();
    assert!(!sources.is_empty());
    for s in sources {
        assert_eq!(s.key.space, WorldSpaceId(2));
        assert!((s.heightfield.sample([16., 16.], s.cell_size).height - 45.).abs() < 0.03);
    }
    eprintln!(
        "Publication GPU handoff: rejected candidate retained old cover/sources; retry adopted {} with 45 m relief",
        next.generation_id
    );
}

fn grass_source_gate(app: &mut App) {
    use vegetation_render::{VegetationDebugScene, VegetationTerrainGate};
    // Use real cooked relief under the offscreen actor, with the normal source
    // resource bridge. The renderer-side gate itself has separate GPU coverage.
    let cell = CellCoord { x: 18, z: -16 };
    let field = app.world().resource::<TerrainLodStream>().nodes
        [&TerrainNodeKey::leaf(WorldSpaceId(1), cell)]
        .heightfield
        .as_ref()
        .unwrap()
        .clone();
    let mut scene = VegetationDebugScene::reference().scene().clone();
    scene.pages.truncate(1);
    let page = &mut scene.pages[0];
    page.origin_xz = [576., -512.];
    page.size = 32.;
    page.surface = vegetation::VegetationSurfaceField {
        resolution: field.resolution,
        heights: field.heights,
        normals_oct: field.normals_oct,
        validity: vec![255; usize::from(field.resolution).pow(2)],
    };
    let id = VegetationTerrainGate::page_id(page);
    app.insert_resource(VegetationDebugScene::new(scene.clone()).unwrap())
        .init_resource::<VegetationTerrainGate>();
    app.update();
    assert_eq!(
        *app.world().resource::<VegetationTerrainGate>(),
        VegetationTerrainGate::default()
    );
    for height in &mut scene.pages[0].surface.heights {
        *height += 0.05;
    }
    app.world_mut()
        .resource_mut::<VegetationDebugScene>()
        .replace(scene.clone())
        .unwrap();
    app.update();
    assert!(
        app.world()
            .resource::<VegetationTerrainGate>()
            .blocked_pages
            .contains(&id)
    );
    assert_eq!(
        app.world()
            .resource::<TerrainLodStats>()
            .mismatched_grass_pages,
        1
    );
    for height in &mut scene.pages[0].surface.heights {
        *height -= 0.05;
    }
    app.world_mut()
        .resource_mut::<VegetationDebugScene>()
        .replace(scene)
        .unwrap();
    app.update();
    assert_eq!(
        *app.world().resource::<VegetationTerrainGate>(),
        VegetationTerrainGate::default()
    );
    assert_eq!(
        app.world()
            .resource::<TerrainLodStats>()
            .mismatched_grass_pages,
        0
    );
    app.world_mut().remove_resource::<VegetationDebugScene>();
    app.world_mut().remove_resource::<VegetationTerrainGate>();
}

fn settle(app: &mut App, deadline: std::time::Instant) {
    let mut stable = 0;
    let mut previous = BTreeSet::new();
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "LOD timeout: {:?}",
            app.world().resource::<TerrainLodStats>()
        );
        app.update();
        let stream = app.world().resource::<TerrainLodStream>();
        assert!(stream.error.is_none(), "{:?}", stream.error);
        let keys = stream.active.keys().copied().collect::<BTreeSet<_>>();
        stable = if !keys.is_empty()
            && keys == previous
            && stream.pending.is_empty()
            && stream.decodes.is_empty()
            && stream.builds.is_empty()
            && stream.target.is_none()
        {
            stable + 1
        } else {
            0
        };
        previous = keys;
        assert_valid_cover(app);
        if stable >= 8 {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn settle_sources(app: &mut App, deadline: std::time::Instant) {
    let mut stable = 0;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "source readiness timeout"
        );
        app.update();
        let stream = app.world().resource::<WorldStream>();
        assert!(matches!(stream.phase, StreamPhase::Ready));
        assert!(stream.demand_error.is_none(), "{:?}", stream.demand_error);
        let pending = stream
            .pages
            .values()
            .filter(|p| {
                matches!(
                    p,
                    PageState::Loading { .. } | PageState::Decoding { .. } | PageState::Prepared(_)
                )
            })
            .count();
        assert!(pending <= MAX_DATABASE_REQUESTS_IN_FLIGHT);
        assert!(
            stream
                .pages
                .values()
                .all(|p| !matches!(p, PageState::Failed(_)))
        );
        stable = if stream.requested_index.is_none()
            && !stream.desired.is_empty()
            && stream
                .desired
                .iter()
                .all(|k| matches!(stream.pages.get(k), Some(PageState::Resident(_))))
        {
            stable + 1
        } else {
            0
        };
        if stable >= 8 {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn assert_valid_cover(app: &App) {
    let stream = app.world().resource::<TerrainLodStream>();
    if stream.active.is_empty() {
        return;
    }
    let area: u64 = stream
        .active
        .keys()
        .map(|(k, _)| 1_u64 << (2 * k.level))
        .sum();
    let expected_area = if stream.identity.as_ref().unwrap().1 == WorldSpaceId(1) {
        4096
    } else {
        4
    };
    assert_eq!(area, expected_area);
    assert!(stream.active.len() <= 512);
    assert!(stream.decoded_bytes() <= MAX_NODE_BYTES);
    assert!(stream.mesh_bytes() <= MAX_MESH_BYTES);
    assert!(stream.metadata.len() <= MAX_METADATA);
    assert!(stream.composites.bytes <= material::MAX_MATERIAL_BYTES);
    assert!(stream.composites.metadata_len() <= 2048);
    assert!(
        stream
            .composites
            .detail
            .as_ref()
            .is_none_or(|d| d.count() <= 128)
    );
    assert!(stream.materials_ready(app.world().resource::<UploadTracker>()));
    for (&(key, _), &entity) in &stream.active {
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<TerrainCompositeMaterial>>(entity)
                .unwrap()
                .0,
            stream.patch_material(key)
        );
    }
    if let Some(t) = &stream.transition {
        assert!(t.patches <= 1024 && t.triangles <= 2 * 1048576);
        if t.running() {
            let drawn_area: u64 = stream
                .active
                .iter()
                .filter(|(p, _)| t.keeps(p))
                .map(|((k, _), _)| 1_u64 << (2 * k.level))
                .chain(t.entities.keys().map(|k| 1_u64 << (2 * k.level)))
                .sum();
            assert_eq!(drawn_area, expected_area);
            for (p, &e) in &stream.active {
                assert_eq!(
                    *app.world().get::<Visibility>(e).unwrap() != Visibility::Hidden,
                    t.keeps(p)
                );
            }
            for (&k, &e) in &t.entities {
                assert_eq!(
                    *app.world().get::<Transform>(e).unwrap(),
                    patch_transform(k, app.world().resource::<WorldOrigin>().cell(), 32.)
                );
            }
        }
    }
    let tracker = app.world().resource::<UploadTracker>().0.lock().unwrap();
    for patch in stream.active.keys() {
        assert!(tracker.ready.contains(&stream.meshes[patch].handle.id()));
    }
}
fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut image = Image::new_target_texture(
        512,
        512,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        None,
    );
    image.texture_descriptor.usage |= bevy::render::render_resource::TextureUsages::COPY_SRC;
    let target = images.add(image);
    commands.spawn(Readback::texture(target.clone())).observe(
        |event: On<ReadbackComplete>, pixels: Res<Pixels>| {
            *pixels.0.lock().unwrap() = event.data.clone();
        },
    );
    commands.spawn((
        Camera3d::default(),
        Camera { ..default() },
        RenderTarget::Image(target.into()),
        Projection::Perspective(PerspectiveProjection {
            far: 10000.,
            ..default()
        }),
        Transform::from_xyz(1800., 1500., 2200.).looking_at(Vec3::new(0., 650., 0.), Vec3::Y),
        Msaa::Off,
        bevy::core_pipeline::prepass::DepthPrepass,
        bevy::core_pipeline::prepass::NormalPrepass,
        bevy::core_pipeline::prepass::MotionVectorPrepass,
        WorldViewCamera,
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 20000.,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, -0.4, 0.)),
    ));
    commands.insert_resource(bevy::light::DirectionalLightShadowMap { size: 256 });
    commands.insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.1)));
}

fn pump_pixels(app: &mut App) {
    for _ in 0..12 {
        app.update();
        assert_valid_cover(app);
        assert!(app.world().resource::<TerrainLodStream>().error.is_none());
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn set_origin(app: &mut App, cell: CellCoord) {
    let w = app.world_mut();
    let old = w.resource::<WorldOrigin>().cell();
    w.resource_mut::<WorldOrigin>().cell = cell;
    w.resource_mut::<WorldViewpoint>().position = Some(WorldPosition {
        space: WorldSpaceId(1),
        cell,
        local: [0.; 3],
    });
    let mut camera = w.query_filtered::<&mut Transform, With<WorldViewCamera>>();
    camera.single_mut(w).unwrap().translation -= Vec3::new(
        (cell.x - old.x) as f32 * 32.,
        0.,
        (cell.z - old.z) as f32 * 32.,
    );
}

fn force_coarse_target(app: &mut App) {
    let mut stream = app.world_mut().resource_mut::<TerrainLodStream>();
    let patches = stream
        .roots
        .as_ref()
        .unwrap()
        .iter()
        .map(|&k| (k, StitchEdges::default()))
        .collect();
    stream.target = Some(PlannedCover {
        patches,
        requests: BTreeSet::new(),
        stats: lod::CoverStats {
            triangles: 8192,
            ..default()
        },
        balanced: true,
    });
}
