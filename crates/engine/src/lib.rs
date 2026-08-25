use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
    window::{PrimaryWindow, Window},
};

const GROUND_SIZE: f32 = 80.0;
const OBJECT_SPEED_METERS_PER_SECOND: f32 = 7.0;
const OBJECT_HALF_HEIGHT: f32 = 0.5;

/// The complete gameplay surface for the first vertical slice.
///
/// The application crate owns the executable and platform window. This plugin
/// owns the reusable game scene, input, movement, and diagnostics.
pub struct MinimalGamePlugin;

impl Plugin for MinimalGamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(FrameTimeDiagnosticsPlugin::default())
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (
                    set_target_from_pointer,
                    move_object,
                    update_performance_label,
                ),
            );
    }
}

#[derive(Component)]
struct MainCamera;

#[derive(Component)]
struct MovableObject;

#[derive(Component, Deref, DerefMut)]
struct MovementTarget(Vec3);

#[derive(Component)]
struct PerformanceLabel;

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(GROUND_SIZE, GROUND_SIZE))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.28, 0.24),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Name::new("Ground"),
    ));

    let start = Vec3::new(0.0, OBJECT_HALF_HEIGHT, 0.0);
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.95, 0.42, 0.14),
            perceptual_roughness: 0.7,
            ..default()
        })),
        Transform::from_translation(start),
        MovementTarget(start),
        MovableObject,
        Name::new("Movable object"),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.9, -0.7, 0.0)),
        Name::new("Sun"),
    ));

    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(12.0, 16.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
        MainCamera,
        Name::new("Main camera"),
    ));

    commands.spawn((
        Text::new("Left click the ground to move"),
        TextFont {
            font_size: FontSize::Px(16.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: px(14),
            left: px(14),
            ..default()
        },
        PerformanceLabel,
        Name::new("Performance label"),
    ));
}

fn set_target_from_pointer(
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut target: Single<&mut MovementTarget, With<MovableObject>>,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(ray) = camera.0.viewport_to_world(camera.1, cursor) else {
        return;
    };
    let direction = *ray.direction;
    if direction.y.abs() < 1.0e-5 {
        return;
    }
    let distance = -ray.origin.y / direction.y;
    if distance < 0.0 {
        return;
    }

    let point = ray.origin + direction * distance;
    target.0 = Vec3::new(point.x, OBJECT_HALF_HEIGHT, point.z);
}

fn move_object(
    time: Res<Time>,
    mut object: Single<(&mut Transform, &MovementTarget), With<MovableObject>>,
) {
    let offset = **object.1 - object.0.translation;
    let distance = offset.length();
    if distance <= 0.001 {
        object.0.translation = **object.1;
        return;
    }

    let step = OBJECT_SPEED_METERS_PER_SECOND * time.delta_secs();
    object.0.translation += offset * (step / distance).min(1.0);
}

fn update_performance_label(
    diagnostics: Res<DiagnosticsStore>,
    mut label: Single<&mut Text, With<PerformanceLabel>>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
) {
    *elapsed += time.delta_secs();
    if *elapsed < 0.25 {
        return;
    }
    *elapsed = 0.0;

    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|diagnostic| diagnostic.smoothed())
        .unwrap_or_default();
    let frame_time = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|diagnostic| diagnostic.smoothed())
        .unwrap_or_default();

    **label = Text::new(format!(
        "Left click the ground to move\nUncapped baseline: {fps:.0} FPS · {frame_time:.2} ms"
    ));
}
