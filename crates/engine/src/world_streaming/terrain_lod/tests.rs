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
    world_cook::cook_project(&source, &runtime).unwrap();
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
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<PipelinedRenderingPlugin>(),
    )
    .add_plugins((
        terrain_render::TerrainRenderPlugin,
        WorldStreamingPlugin::editor(&runtime),
    ))
    .insert_resource(TerrainLodPreview {
        enabled: true,
        ..default()
    })
    .init_resource::<Pixels>()
    .add_systems(Startup, setup);
    let deadline = std::time::Instant::now() + Duration::from_secs(55);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    settle(&mut app, deadline);
    let first = app.world().resource::<TerrainLodStats>().clone();
    assert!(first.patches >= 4);
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
    // Withhold GPU acknowledgements for a forced coarse replacement. Even completed
    // CPU mesh jobs must not remove the last valid cover.
    let pinned: BTreeSet<_> = app
        .world()
        .resource::<TerrainLodStream>()
        .active
        .keys()
        .copied()
        .collect();
    {
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
    app.world_mut()
        .resource_mut::<ActiveWorldSpace>()
        .request(WorldSpaceId(2), [0.; 3]);
    for _ in 0..15 {
        app.update();
        std::thread::sleep(Duration::from_millis(5));
    }
    let stream = app.world().resource::<TerrainLodStream>();
    assert_eq!(stream.identity.as_ref().unwrap().1, WorldSpaceId(2));
    assert!(stream.active.is_empty() && stream.meshes.is_empty() && stream.nodes.is_empty());
    drop(app);
    std::fs::remove_dir_all(folder).unwrap();
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
    assert_eq!(area, 4096);
    assert!(stream.active.len() <= 512);
    assert!(stream.decoded_bytes() <= MAX_NODE_BYTES);
    assert!(stream.mesh_bytes() <= MAX_MESH_BYTES);
    assert!(stream.metadata.len() <= MAX_METADATA);
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
        WorldViewCamera,
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 20000.,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, -0.4, 0.)),
    ));
    commands.insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.1)));
}
