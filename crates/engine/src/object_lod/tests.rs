use super::*;
#[test]
fn screen_space_lod_selection_has_hysteresis_in_both_directions() {
    let minimums = [320.0, 160.0, 80.0, 0.0];
    let select = |current, height| {
        select_lod_index(minimums.len(), current, height, |index| minimums[index])
    };

    assert_eq!(select(0, 300.0), 0);
    assert_eq!(select(0, 280.0), 1);
    assert_eq!(select(1, 350.0), 1);
    assert_eq!(select(1, 360.0), 0);
    assert_eq!(select(3, 85.0), 3);
    assert_eq!(select(3, 90.0), 2);
    assert_eq!(select(2, 60.0), 3);
}

fn app(install: bool) -> App {
    let mut app = App::new();
    app.add_plugins((bevy::app::TaskPoolPlugin::default(), TransformPlugin))
        .init_resource::<Assets<WorldAsset>>();
    if install {
        app.add_plugins(ObjectLodPlugin);
    }
    app.world_mut().spawn((
        WorldViewCamera,
        Camera {
            computed: bevy::camera::ComputedCameraValues {
                clip_from_view: Mat4::orthographic_rh(-5., 5., -5., 5., 0.1, 100.),
                target_info: Some(bevy::camera::RenderTargetInfo {
                    physical_size: UVec2::splat(1000),
                    scale_factor: 1.,
                }),
                ..default()
            },
            ..default()
        },
        Transform::from_xyz(0., 0., 10.),
    ));
    app
}

fn object(app: &mut App, height: f32) -> Entity {
    let scenes = app.world().resource::<Assets<WorldAsset>>();
    let variants = [160., 80., 0.]
        .into_iter()
        .enumerate()
        .map(|(lod, minimum_screen_height)| ScreenSpaceLodVariant {
            lod: lod as u8,
            scene: scenes.reserve_handle(),
            minimum_screen_height,
        })
        .collect();
    let lod = ScreenSpaceLod::new(variants, height);
    app.world_mut()
        .spawn((lod.scene_root(), lod, Transform::default()))
        .id()
}

fn current(app: &App, object: Entity) -> u8 {
    let world = app.world();
    let lod = world.get::<ScreenSpaceLod>(object).unwrap();
    assert_eq!(
        world.get::<WorldAssetRoot>(object).unwrap().0,
        lod.variants[lod.current].scene
    );
    lod.current_lod()
}

#[test]
fn plugin_observes_propagated_scale_and_keeps_unprojectable_objects_unchanged() {
    let mut app = app(true);
    let entity = object(&mut app, 1.);
    app.update();
    assert_eq!(current(&app, entity), 1);
    app.world_mut().get_mut::<Transform>(entity).unwrap().scale = Vec3::splat(3.);
    app.update();
    assert_eq!(
        current(&app, entity),
        0,
        "new global scale must be used this frame"
    );
    let height = app
        .world()
        .get::<ScreenSpaceLod>(entity)
        .unwrap()
        .projected_height();
    assert!((height - 300.).abs() < 0.01);
    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation
        .z = 20.;
    app.world_mut().resource_mut::<VisualLodScale>().0 = 0.25;
    app.update();
    assert_eq!(
        current(&app, entity),
        0,
        "behind-camera objects must retain their scene"
    );
    assert_eq!(
        app.world()
            .get::<ScreenSpaceLod>(entity)
            .unwrap()
            .projected_height(),
        height
    );
}

#[test]
fn object_lod_budget_limits_switches_and_catches_up_on_the_next_frame() {
    let mut app = app(true);
    let objects: Vec<_> = (0..MAX_LOD_SWITCHES_PER_FRAME + 3)
        .map(|_| object(&mut app, 2.))
        .collect();
    app.update();
    assert_eq!(
        objects.iter().filter(|&&e| current(&app, e) == 0).count(),
        MAX_LOD_SWITCHES_PER_FRAME
    );
    // Even deferred switches retain current projected-size telemetry.
    assert!(objects.iter().all(|&e| {
        (app.world()
            .get::<ScreenSpaceLod>(e)
            .unwrap()
            .projected_height()
            - 200.)
            .abs()
            < 0.01
    }));
    app.update();
    assert!(objects.iter().all(|&e| current(&app, e) == 0));
}

#[test]
fn plugin_omission_keeps_initial_scene_and_scale_controls_keep_their_clamps() {
    let mut omitted = app(false);
    let entity = object(&mut omitted, 8.);
    omitted.update();
    assert!(!omitted.world().contains_resource::<VisualLodScale>());
    assert_eq!(current(&omitted, entity), 2);
    omitted.add_plugins(ObjectLodPlugin);
    omitted.world_mut().resource_mut::<VisualLodScale>().0 = 0.;
    omitted.update();
    assert_eq!(current(&omitted, entity), 0, "zero scale clamps to 0.25");
    let small = object(&mut omitted, 0.3);
    omitted.world_mut().resource_mut::<VisualLodScale>().0 = 100.;
    omitted.update();
    assert_eq!(current(&omitted, small), 1, "large scale clamps to 4.0");
}
