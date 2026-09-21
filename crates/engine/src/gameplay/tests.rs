use super::camera::MainCamera;
use super::*;
use crate::actor::PlayerControlled;
use crate::{
    StreamedTerrainSurface, TerrainContactReadiness, TerrainLodPreview,
    actor::{CharacterGait, CharacterMotorConfig, MoveIntent},
    world_streaming::{terrain_lod::TerrainLodStream, test_world_resources},
};
use target::TargetIndicator;

fn headless_game(input: bool, camera: bool, marker: bool) -> App {
    let (_, origin) = test_world_resources(world::WorldSpaceId(1), world::CellCoord::ZERO, None);
    let mut app = App::new();
    app.add_plugins((
        bevy::app::TaskPoolPlugin::default(),
        AssetPlugin::default(),
        TransformPlugin,
    ))
    .init_asset::<Gltf>()
    .init_asset::<WorldAsset>()
    .init_asset::<AnimationGraph>()
    .init_asset::<AnimationClip>()
    .init_resource::<Time>()
    .insert_resource(origin)
    .insert_resource(TerrainLodPreview {
        enabled: false,
        ..default()
    })
    .init_resource::<TerrainLodStream>()
    .init_resource::<TerrainContactReadiness>()
    .insert_resource(WorldStartView(Some(world::WorldViewBookmark {
        position: [4., 0., 4.],
        yaw_degrees: 45.,
        pitch_degrees: 18.,
        distance: 9.7,
        fog_visibility: 2500.,
        route: vec![],
    })));
    let mut plugins = GameplayPlugins.build();
    if input {
        app.add_plugins(bevy::input::InputPlugin);
    } else {
        plugins = plugins.disable::<GameInputPlugin>();
    }
    if !camera {
        plugins = plugins.disable::<GameCameraPlugin>();
    }
    if marker {
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>();
    } else {
        plugins = plugins.disable::<MovementTargetPlugin>();
    }
    app.add_plugins(plugins);
    app.world_mut().spawn(StreamedTerrainSurface {
        key: world::PageKey {
            space: world::WorldSpaceId(1),
            cell: world::CellCoord::ZERO,
            domain: world::PageDomain::TerrainRender,
            lod: 0,
        },
        cell_size: 16.,
        heightfield: world::TerrainHeightfield::from_heights(2, &[5.; 4], 5., 5., 16.).unwrap(),
    });
    tick(&mut app);
    app
}

fn tick(app: &mut App) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs_f32(1. / 60.));
    app.update();
}

fn player(app: &mut App) -> Entity {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, With<PlayerControlled>>()
        .single(world)
        .unwrap()
}

fn camera_transform(app: &mut App) -> Transform {
    let world = app.world_mut();
    *world
        .query_filtered::<&Transform, With<MainCamera>>()
        .single(world)
        .unwrap()
}

#[test]
fn optional_gameplay_plugins_omit_entities_assets_and_native_input_dependencies() {
    for input in [false, true] {
        for camera in [false, true] {
            for marker in [false, true] {
                let mut app = headless_game(input, camera, marker);
                tick(&mut app);
                let actor = player(&mut app);
                assert!(app.world().get::<CharacterMotorConfig>(actor).is_some());
                let world = app.world_mut();
                assert_eq!(
                    world
                        .query_filtered::<Entity, With<MainCamera>>()
                        .iter(world)
                        .count(),
                    usize::from(camera)
                );
                assert_eq!(
                    world
                        .query_filtered::<Entity, With<TargetIndicator>>()
                        .iter(world)
                        .count(),
                    usize::from(marker)
                );
                assert_eq!(world.contains_resource::<Assets<Mesh>>(), marker);
                assert_eq!(world.contains_resource::<ButtonInput<KeyCode>>(), input);
            }
        }
    }
}

#[test]
fn scripted_movement_grounds_then_follows_and_marks_the_destination_in_the_same_frame() {
    let mut app = headless_game(false, true, true);
    let actor = player(&mut app);
    app.add_systems(
        Update,
        (|mut intent: Single<&mut MoveIntent, With<PlayerControlled>>| {
            intent.set_destination(Vec3::new(8., 0., 4.), CharacterGait::Walk);
        })
        .in_set(GameplaySystems::MoveIntent),
    );
    tick(&mut app);
    let position = app.world().get::<Transform>(actor).unwrap().translation;
    assert!(
        position.x > 4.,
        "scripted intent must reach the motor in the same frame"
    );
    assert_eq!(position.y, 5.);
    let view = app.world().resource::<WorldStartView>().0.as_ref().unwrap();
    let expected_camera = WorldStartView::camera_at(view, position);
    assert!(
        camera_transform(&mut app)
            .translation
            .distance(expected_camera.translation)
            < 0.0001
    );
    let world = app.world_mut();
    let (transform, visibility) = world
        .query_filtered::<(&Transform, &Visibility), With<TargetIndicator>>()
        .single(world)
        .unwrap();
    assert_eq!(transform.translation, Vec3::new(8., 5.025, 4.));
    assert_eq!(*visibility, Visibility::Visible);

    // Repro routes must win over normal follow, including when input is omitted.
    app.add_systems(
        Update,
        (|mut camera: Single<&mut Transform, With<MainCamera>>| {
            camera.translation = Vec3::splat(99.);
        })
        .after(GameplaySystems::CameraFollow),
    );
    tick(&mut app);
    assert_eq!(camera_transform(&mut app).translation, Vec3::splat(99.));
}

#[test]
fn freeze_applies_before_movement_while_grounding_and_camera_follow_continue() {
    #[derive(Resource)]
    struct Locked(bool);
    let mut app = headless_game(false, true, false);
    let actor = player(&mut app);
    app.world_mut()
        .get_mut::<MoveIntent>(actor)
        .unwrap()
        .set_destination(Vec3::new(8., 0., 4.), CharacterGait::Walk);
    app.insert_resource(Locked(true)).add_systems(
        Update,
        (|lock: Res<Locked>, mut enabled: ResMut<GameInputEnabled>| {
            enabled.0 = !lock.0;
        })
        .before(GameplaySystems::CameraInput),
    );
    // Streaming replaces contact data before gameplay, even during an input lock.
    app.add_systems(
        Update,
        (|mut surfaces: Query<&mut StreamedTerrainSurface>| {
            for mut surface in &mut surfaces {
                surface.heightfield =
                    world::TerrainHeightfield::from_heights(2, &[7.; 4], 7., 7., 16.).unwrap();
            }
        })
        .in_set(WorldStreamingSystems),
    );
    tick(&mut app);
    let frozen = app.world().get::<Transform>(actor).unwrap().translation;
    assert_eq!(frozen, Vec3::new(4., 7., 4.));
    assert!(
        app.world()
            .get::<MoveIntent>(actor)
            .unwrap()
            .destination()
            .is_some()
    );
    let expected_camera = WorldStartView::camera_at(
        app.world().resource::<WorldStartView>().0.as_ref().unwrap(),
        frozen,
    );
    assert!(
        camera_transform(&mut app)
            .translation
            .distance(expected_camera.translation)
            < 0.0001
    );
    app.world_mut().resource_mut::<Locked>().0 = false;
    tick(&mut app);
    assert!(app.world().get::<Transform>(actor).unwrap().translation.x > frozen.x);
}

#[test]
fn keyboard_overrides_a_pointer_destination_and_remains_active_during_ui_capture() {
    let mut app = headless_game(true, true, false);
    let actor = player(&mut app);
    let before = app.world().get::<Transform>(actor).unwrap().translation;
    let forward = camera_transform(&mut app).forward();
    let forward = Vec3::new(forward.x, 0., forward.z).normalize();
    app.add_systems(
        Update,
        (
            (|mut captured: ResMut<GamePointerInputBlocked>| {
                captured.0 = true;
            })
            .before(GameplaySystems::CameraInput),
            (|mut intent: Single<&mut MoveIntent, With<PlayerControlled>>| {
                intent.set_destination(Vec3::new(8., 0., 4.), CharacterGait::Walk);
            })
            .in_set(GameplaySystems::PointerInput),
        ),
    );
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);
    tick(&mut app);
    assert!(
        app.world()
            .get::<MoveIntent>(actor)
            .unwrap()
            .destination()
            .is_none()
    );
    assert!(
        app.world()
            .get::<Transform>(actor)
            .unwrap()
            .translation
            .distance(before)
            > 0.
    );
    // The motor turns gradually toward camera-relative input; it must not snap on frame one.
    for _ in 0..60 {
        tick(&mut app);
    }
    let movement = app.world().get::<Transform>(actor).unwrap().translation - before;
    assert!(
        movement.dot(forward) > 0.,
        "keyboard input must move along the camera plane"
    );
}

#[test]
fn launch_bookmark_spawns_actor_and_camera_at_the_elevated_view() {
    let view = world::WorldViewBookmark {
        position: [0., 192., 0.],
        yaw_degrees: 45.,
        pitch_degrees: 18.,
        distance: 9.7,
        fog_visibility: 2500.,
        route: vec![],
    };
    let expected = WorldStartView::camera_at(&view, Vec3::from_array(view.position));
    let mut app = App::new();
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(WorldStartView(Some(view)))
        .add_systems(Startup, actors::spawn_player)
        .add_plugins(GameCameraPlugin);
    app.update();
    let world = app.world_mut();
    let actor = world
        .query_filtered::<&Transform, With<PlayerControlled>>()
        .single(world)
        .unwrap();
    assert_eq!(actor.translation, Vec3::new(0., 192., 0.));
    let (camera, projection) = world
        .query_filtered::<(&Transform, &Projection), With<MainCamera>>()
        .single(world)
        .unwrap();
    assert!(camera.translation.distance(expected.translation) < 0.001);
    assert!(camera.forward().distance(*expected.forward()) < 0.0001);
    assert!(camera.up().distance(*expected.up()) < 0.0001);
    assert!(matches!(projection, Projection::Perspective(p) if p.far == 2500.));
}
