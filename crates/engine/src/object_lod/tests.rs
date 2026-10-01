use super::*;
use std::f32::consts::{FRAC_PI_4, FRAC_PI_8};

const PERSPECTIVE: LodProjection = LodProjection {
    pixels_per_metre: 1000.0,
    orthographic: false,
};

#[test]
fn lod_bands_meet_at_switch_distances_and_crossfade_around_them() {
    // A 1 m object at 1000 px/m switches where it is 320, 160 and 80 px tall.
    let thresholds = [320.0, 160.0, 80.0, 0.0];
    let range = |index| PERSPECTIVE.range(&thresholds, 1.0, index);
    let around = |d: f32| d * 0.9..d * 1.1;
    assert_eq!(range(0).start_margin, 0.0..0.0);
    assert_eq!(range(0).end_margin, around(3.125));
    assert_eq!(range(1).start_margin, range(0).end_margin);
    assert_eq!(range(1).end_margin, around(6.25));
    assert_eq!(range(2).start_margin, range(1).end_margin);
    assert_eq!(range(3).start_margin, around(12.5));
    assert_eq!(range(3).end_margin, f32::MAX..f32::MAX);
    assert!(range(0).is_visible_at_all(3.0) && range(1).is_visible_at_all(3.0));
    assert!(!range(1).is_visible_at_all(2.5) && !range(0).is_visible_at_all(3.5));
    assert!(range(3).is_visible_at_all(1.0e6));
}

#[test]
fn equal_thresholds_leave_the_skipped_lod_no_band() {
    let range = PERSPECTIVE.range(&[320.0, 320.0, 80.0, 0.0], 1.0, 1);
    assert!((0..100).all(|d| !range.is_visible_at_all(d as f32 * 0.25)));
}

#[test]
fn orthographic_views_keep_the_one_matching_lod_at_any_distance() {
    let projection = LodProjection {
        pixels_per_metre: 100.0,
        orthographic: true,
    };
    let thresholds = [160.0, 80.0, 0.0];
    // 1 m tall at 100 px/m: 100 px, which is LOD1.
    for (index, visible) in [(0, false), (1, true), (2, false)] {
        let range = projection.range(&thresholds, 1.0, index);
        assert_eq!(range.is_visible_at_all(1.0e4), visible, "LOD{index}");
    }
}

fn app(camera: Transform) -> App {
    let mut app = App::new();
    app.add_plugins((
        bevy::app::TaskPoolPlugin::default(),
        TransformPlugin,
        ObjectLodPlugin,
    ));
    app.world_mut().spawn((
        WorldViewCamera,
        Camera {
            computed: bevy::camera::ComputedCameraValues {
                clip_from_view: Mat4::perspective_infinite_reverse_rh(FRAC_PI_4, 1., 0.1),
                target_info: Some(bevy::camera::RenderTargetInfo {
                    physical_size: UVec2::splat(1000),
                    scale_factor: 1.,
                }),
                ..default()
            },
            ..default()
        },
        camera,
    ));
    app
}

/// An object with two LOD scenes, each holding one mesh entity, as a spawned scene would.
fn object(app: &mut App, height: f32) -> (Entity, [Entity; 2]) {
    let variants = [160.0, 0.0]
        .into_iter()
        .enumerate()
        .map(|(lod, minimum_screen_height)| ScreenSpaceLodVariant {
            lod: lod as u8,
            scene: Handle::default(),
            minimum_screen_height,
        })
        .collect();
    let world = app.world_mut();
    let meshes = [0, 1].map(|_| world.spawn(Mesh3d(Handle::default())).id());
    let scenes = [0, 1].map(|index| world.spawn(LodScene(index)).add_child(meshes[index]).id());
    let root = world
        .spawn((ScreenSpaceLod::new(variants, height), Transform::default()))
        .add_children(&scenes)
        .id();
    (root, meshes)
}

fn lod(app: &App, entity: Entity) -> &ScreenSpaceLod {
    app.world().get::<ScreenSpaceLod>(entity).unwrap()
}

#[test]
fn meshes_inside_each_lod_scene_get_that_lods_distance_band() {
    let mut app = app(Transform::from_xyz(0., 2., 20.).looking_at(Vec3::new(0., 2., 0.), Vec3::Y));
    let (root, meshes) = object(&mut app, 4.);
    app.update();
    // 500 px over tan(22.5°) per metre at 1 m: the 4 m object is 160 px tall at ~30 m.
    let switch = 4. * 500. / FRAC_PI_8.tan() / 160.;
    let near = app.world().get::<VisibilityRange>(meshes[0]).unwrap();
    let far = app.world().get::<VisibilityRange>(meshes[1]).unwrap();
    assert!(
        (near.end_margin.start - switch * 0.9).abs() < 0.01,
        "{:?}",
        near.end_margin
    );
    assert_eq!(near.end_margin, far.start_margin);
    assert_eq!(far.end_margin, f32::MAX..f32::MAX);
    // 20 m away is inside LOD0's band.
    assert_eq!(lod(&app, root).current_lod(), 0);
}

#[test]
fn projected_size_follows_distance_not_camera_pitch() {
    let measure = |camera: Transform| {
        let mut app = app(camera);
        let (root, _) = object(&mut app, 4.);
        app.update();
        lod(&app, root).projected_height()
    };
    // The same tree 20 m away, seen side-on and from straight above.
    let side = measure(Transform::from_xyz(0., 2., 20.).looking_at(Vec3::new(0., 2., 0.), Vec3::Y));
    let above =
        measure(Transform::from_xyz(0., 22., 0.).looking_at(Vec3::new(0., 2., 0.), Vec3::Z));
    let expected = 4. * 500. / FRAC_PI_8.tan() / 20.;
    assert!((side - expected).abs() < 0.5, "{side} vs {expected}");
    assert!(
        (above - side).abs() < 0.01,
        "looking down must not shrink the tree: {above}"
    );
}

#[test]
fn object_detail_scale_moves_switch_distances_and_keeps_its_clamps() {
    let camera = Transform::from_xyz(0., 2., 20.).looking_at(Vec3::new(0., 2., 0.), Vec3::Y);
    let end_of_lod0 = |scale: f32| {
        let mut app = app(camera);
        app.insert_resource(VisualLodScale(scale));
        let (_, meshes) = object(&mut app, 4.);
        app.update();
        app.world()
            .get::<VisibilityRange>(meshes[0])
            .unwrap()
            .end_margin
            .start
    };
    let normal = end_of_lod0(1.0);
    assert!((end_of_lod0(2.0) - 2. * normal).abs() < 0.01);
    assert!(
        (end_of_lod0(0.0) - 0.25 * normal).abs() < 0.01,
        "zero clamps to 0.25"
    );
    assert!(
        (end_of_lod0(100.0) - 4. * normal).abs() < 0.01,
        "large values clamp to 4"
    );
}
