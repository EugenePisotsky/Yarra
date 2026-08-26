mod actor;
mod character;
mod character_catalog;
mod world_streaming;

use std::path::PathBuf;

use bevy::{
    camera::Exposure,
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    input::gestures::{PanGesture, PinchGesture},
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    light::CascadeShadowConfigBuilder,
    prelude::*,
    render::view::Msaa,
    window::{Monitor, PrimaryMonitor, PrimaryWindow, Window},
};
use ground_cover::{GroundCoverDebug, GroundCoverInteractor, GroundCoverPlugin, GroundCoverView};
use terrain_render::{TerrainMacroVariation, TerrainRenderPlugin};
pub use world_streaming::{ActiveWorldSpace, GameplayObject};
use world_streaming::{StreamingStats, WorldStreamingPlugin};

use crate::{
    actor::{
        CameraTarget, CharacterGait, CharacterMotion, CharacterMotor, MoveIntent, PlayerControlled,
        WorldStreamFocus, advance_character_motors,
    },
    character::{
        CharacterPresentationPlugin, CharacterPresentationRef, CharacterPresentationResolveSet,
    },
    character_catalog::DEFAULT_CHARACTER_PRESENTATION_ID,
};

const CAMERA_STICK_DEAD_ZONE: f32 = 0.15;
const CAMERA_MIN_DISTANCE: f32 = 4.0;
const CAMERA_MAX_DISTANCE: f32 = 17.6;
const CAMERA_ZOOM_REFERENCE_DISTANCE: f32 = 24.0;
const CAMERA_DEFAULT_DISTANCE: f32 = CAMERA_MAX_DISTANCE;
const CAMERA_FOCUS_HEIGHT: f32 = 0.9;
const CAMERA_NEAR_PITCH: f32 = 18.0_f32.to_radians();
const CAMERA_FAR_PITCH: f32 = 55.0_f32.to_radians();
const CAMERA_MAX_PITCH_OFFSET: f32 = 15.0_f32.to_radians();
const CAMERA_MOUSE_ORBIT_SPEED: f32 = 0.006;
const CAMERA_GAMEPAD_ORBIT_SPEED: f32 = 1.8;
const CAMERA_TRACKPAD_ORBIT_SPEED: f32 = 0.003;
const CAMERA_WHEEL_ORBIT_SPEED: f32 = 0.08;
const CAMERA_TRACKPAD_ZOOM_SPEED: f32 = 0.012;
const CAMERA_WHEEL_ZOOM_SPEED: f32 = 0.8;
const CAMERA_TOUCH_ORBIT_SPEED: f32 = 0.006;
const CAMERA_TOUCH_PAN_ZOOM_SPEED: f32 = 0.025;
const CAMERA_TOUCH_PINCH_ZOOM_SPEED: f32 = 8.0;
const CAMERA_ORBIT_SMOOTHING: f32 = 20.0;
const CAMERA_ZOOM_SMOOTHING: f32 = 8.0;
const TARGET_INDICATOR_HEIGHT: f32 = 0.025;
const TOUCH_TAP_MAX_MOVEMENT: f32 = 18.0;

/// The complete gameplay surface for the first vertical slice.
///
/// The application crate owns the executable and platform window. This plugin
/// owns the reusable game scene, input, movement, and diagnostics.
pub struct MinimalGamePlugin {
    runtime_database: PathBuf,
}

impl MinimalGamePlugin {
    pub fn new(runtime_database: impl Into<PathBuf>) -> Self {
        Self {
            runtime_database: runtime_database.into(),
        }
    }
}

impl Plugin for MinimalGamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            FrameTimeDiagnosticsPlugin::default(),
            GroundCoverPlugin,
            TerrainRenderPlugin,
            CharacterPresentationPlugin,
            WorldStreamingPlugin::new(self.runtime_database.clone()),
        ))
        .init_resource::<TouchTapState>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                update_camera_controls,
                set_target_from_pointer,
                update_player_move_intent,
                advance_character_motors.after(CharacterPresentationResolveSet),
                update_target_indicator,
                update_camera_transform,
            )
                .chain(),
        )
        .add_systems(Update, update_performance_label);
    }
}

#[derive(Component)]
pub(crate) struct MainCamera;

#[derive(Component)]
struct CameraRig {
    yaw: f32,
    target_yaw: f32,
    distance: f32,
    target_distance: f32,
    pitch_offset: f32,
}

#[derive(Component)]
struct TargetIndicator;

#[derive(Component)]
struct PerformanceLabel;

#[derive(Resource, Default)]
struct TouchTapState {
    primary_touch: Option<u64>,
    disqualified: bool,
}

type TargetIndicatorState<'w, 's> = Single<
    'w,
    's,
    (&'static mut Transform, &'static mut Visibility),
    (With<TargetIndicator>, Without<PlayerControlled>),
>;

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Match the procedural meadow reference environment so authored terrain
    // response is evaluated under its intended exposure and lighting.
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.72, 0.78, 0.74),
        brightness: 270.0,
        ..default()
    });

    let start = Vec3::ZERO;
    commands.spawn((
        Transform::from_translation(start),
        Visibility::Inherited,
        MoveIntent::default(),
        CharacterMotor::default(),
        CharacterMotion::default(),
        CharacterPresentationRef::new(DEFAULT_CHARACTER_PRESENTATION_ID),
        PlayerControlled,
        CameraTarget,
        WorldStreamFocus,
        GroundCoverInteractor::character(),
        Name::new("Player actor root"),
    ));

    commands.spawn((
        DirectionalLight {
            color: Color::srgb(0.63, 0.65, 0.81),
            illuminance: 8_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            num_cascades: 3,
            first_cascade_far_bound: 20.0,
            maximum_distance: 80.0,
            ..default()
        }
        .build(),
        Transform::from_translation(Vec3::new(7.878_527_6, 8.691_806, -12.131_867))
            .looking_at(Vec3::ZERO, Vec3::Y),
        Name::new("Sun"),
    ));

    let camera_rig = CameraRig {
        yaw: 45.0_f32.to_radians(),
        target_yaw: 45.0_f32.to_radians(),
        distance: CAMERA_DEFAULT_DISTANCE,
        target_distance: CAMERA_DEFAULT_DISTANCE,
        pitch_offset: 0.0,
    };
    commands.spawn((
        Camera3d::default(),
        Exposure { ev100: 10.4 },
        Msaa::Off,
        GroundCoverView {
            normalized_zoom: normalized_camera_zoom(camera_rig.distance),
        },
        camera_transform(start, &camera_rig),
        camera_rig,
        MainCamera,
        Name::new("Main camera"),
    ));

    commands.spawn((
        Mesh3d(
            meshes.add(
                Torus::new(0.55, 0.68)
                    .mesh()
                    .minor_resolution(8)
                    .major_resolution(48),
            ),
        ),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.72, 0.12),
            unlit: true,
            ..default()
        })),
        Transform::from_xyz(0.0, TARGET_INDICATOR_HEIGHT, 0.0),
        Visibility::Hidden,
        TargetIndicator,
        Name::new("Movement target indicator"),
    ));

    commands.spawn((
        Text::new("Left click: move | WASD / left stick: direct movement"),
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
    touches: Res<Touches>,
    mut touch_tap: ResMut<TouchTapState>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut player: Single<(&Transform, &mut MoveIntent), With<PlayerControlled>>,
) {
    let pointer_position = if mouse.just_pressed(MouseButton::Left) {
        window.cursor_position()
    } else {
        touch_tap_position(&touches, &mut touch_tap)
    };
    let Some(cursor) = pointer_position else {
        return;
    };
    let Ok(ray) = camera.0.viewport_to_world(camera.1, cursor) else {
        return;
    };
    let direction = *ray.direction;
    if direction.y.abs() < 1.0e-5 {
        return;
    }
    let distance = (player.0.translation.y - ray.origin.y) / direction.y;
    if distance < 0.0 {
        return;
    }

    let point = ray.origin + direction * distance;
    let ground_height = player.0.translation.y;
    player.1.set_destination(
        Vec3::new(point.x, ground_height, point.z),
        CharacterGait::Walk,
    );
}

fn touch_tap_position(touches: &Touches, state: &mut TouchTapState) -> Option<Vec2> {
    let active_touch_count = touches.iter().count();

    for touch in touches.iter_just_pressed() {
        if state.primary_touch.is_none() && active_touch_count == 1 {
            state.primary_touch = Some(touch.id());
            state.disqualified = false;
        } else {
            state.disqualified = true;
        }
    }
    if active_touch_count > 1 {
        state.disqualified = true;
    }

    if let Some(touch) = touches
        .iter_just_released()
        .find(|touch| Some(touch.id()) == state.primary_touch)
    {
        let is_tap = !state.disqualified && touch.distance().length() <= TOUCH_TAP_MAX_MOVEMENT;
        let position = is_tap.then(|| touch.position());
        state.primary_touch = None;
        state.disqualified = false;
        return position;
    }

    if touches.any_just_canceled() {
        state.primary_touch = None;
        state.disqualified = false;
    }
    None
}

fn update_player_move_intent(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    camera: Single<&GlobalTransform, With<MainCamera>>,
    mut intent: Single<&mut MoveIntent, With<PlayerControlled>>,
) {
    let direct_input = direct_movement_input(&keys, &gamepads);
    if direct_input != Vec2::ZERO {
        let camera_forward = camera.forward();
        let forward = Vec3::new(camera_forward.x, 0.0, camera_forward.z).normalize_or_zero();
        let camera_right = camera.right();
        let right = Vec3::new(camera_right.x, 0.0, camera_right.z).normalize_or_zero();
        let direction = (right * direct_input.x + forward * direct_input.y).normalize_or_zero();
        intent.set_direct(
            Vec2::new(direction.x, direction.z),
            direct_input.length().min(1.0),
            None,
        );
    } else {
        intent.set_direct(Vec2::ZERO, 0.0, None);
    }
}

fn update_target_indicator(
    intent: Single<&MoveIntent, With<PlayerControlled>>,
    mut indicator: TargetIndicatorState,
) {
    if let Some(target) = intent.destination() {
        indicator.0.translation = Vec3::new(target.x, TARGET_INDICATOR_HEIGHT, target.z);
        *indicator.1 = Visibility::Visible;
    } else {
        *indicator.1 = Visibility::Hidden;
    }
}

fn direct_movement_input(keys: &ButtonInput<KeyCode>, gamepads: &Query<&Gamepad>) -> Vec2 {
    let mut keyboard = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyA) {
        keyboard.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        keyboard.x += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        keyboard.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyW) {
        keyboard.y += 1.0;
    }
    if keyboard != Vec2::ZERO {
        return keyboard.normalize();
    }

    gamepads
        .iter()
        .map(|gamepad| {
            Vec2::new(
                gamepad.get(GamepadAxis::LeftStickX).unwrap_or_default(),
                gamepad.get(GamepadAxis::LeftStickY).unwrap_or_default(),
            )
        })
        .map(|stick| stick.clamp_length_max(1.0))
        .max_by(|left, right| left.length_squared().total_cmp(&right.length_squared()))
        .unwrap_or_default()
}

fn apply_stick_dead_zone(stick: Vec2) -> Vec2 {
    let length = stick.length().min(1.0);
    if length <= CAMERA_STICK_DEAD_ZONE {
        return Vec2::ZERO;
    }

    stick.normalize_or_zero() * ((length - CAMERA_STICK_DEAD_ZONE) / (1.0 - CAMERA_STICK_DEAD_ZONE))
}

fn update_camera_controls(
    time: Res<Time>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    mut pan_gestures: MessageReader<PanGesture>,
    mut pinch_gestures: MessageReader<PinchGesture>,
    gamepads: Query<&Gamepad>,
    mut rig: Single<&mut CameraRig, With<MainCamera>>,
) {
    if mouse_buttons.pressed(MouseButton::Right) {
        rig.yaw -= mouse_motion.delta.x * CAMERA_MOUSE_ORBIT_SPEED;
        rig.target_yaw = rig.yaw;
        rig.pitch_offset = (rig.pitch_offset + mouse_motion.delta.y * CAMERA_MOUSE_ORBIT_SPEED)
            .clamp(-CAMERA_MAX_PITCH_OFFSET, CAMERA_MAX_PITCH_OFFSET);
    }

    let right_stick = gamepads
        .iter()
        .map(|gamepad| {
            Vec2::new(
                gamepad.get(GamepadAxis::RightStickX).unwrap_or_default(),
                gamepad.get(GamepadAxis::RightStickY).unwrap_or_default(),
            )
        })
        .map(apply_stick_dead_zone)
        .max_by(|left, right| left.length_squared().total_cmp(&right.length_squared()))
        .unwrap_or_default();
    let gamepad_yaw_delta = right_stick.x * CAMERA_GAMEPAD_ORBIT_SPEED * time.delta_secs();
    if gamepad_yaw_delta != 0.0 {
        rig.yaw -= gamepad_yaw_delta;
        rig.target_yaw = rig.yaw;
    }
    rig.pitch_offset = (rig.pitch_offset
        + right_stick.y * CAMERA_GAMEPAD_ORBIT_SPEED * time.delta_secs())
    .clamp(-CAMERA_MAX_PITCH_OFFSET, CAMERA_MAX_PITCH_OFFSET);

    if time.elapsed_secs() > 0.5 {
        let (orbit_delta, zoom_delta) = match mouse_scroll.unit {
            MouseScrollUnit::Line => (
                mouse_scroll.delta.x.clamp(-3.0, 3.0) * CAMERA_WHEEL_ORBIT_SPEED,
                mouse_scroll.delta.y.clamp(-3.0, 3.0) * CAMERA_WHEEL_ZOOM_SPEED,
            ),
            MouseScrollUnit::Pixel => (
                mouse_scroll.delta.x.clamp(-80.0, 80.0) * CAMERA_TRACKPAD_ORBIT_SPEED,
                mouse_scroll.delta.y.clamp(-80.0, 80.0) * CAMERA_TRACKPAD_ZOOM_SPEED,
            ),
        };
        rig.target_yaw -= orbit_delta;
        rig.target_distance =
            (rig.target_distance - zoom_delta).clamp(CAMERA_MIN_DISTANCE, CAMERA_MAX_DISTANCE);
    }

    let touch_pan: Vec2 = pan_gestures.read().map(|gesture| gesture.0).sum();
    if touch_pan != Vec2::ZERO {
        rig.target_yaw -= touch_pan.x * CAMERA_TOUCH_ORBIT_SPEED;
        rig.target_distance = (rig.target_distance + touch_pan.y * CAMERA_TOUCH_PAN_ZOOM_SPEED)
            .clamp(CAMERA_MIN_DISTANCE, CAMERA_MAX_DISTANCE);
    }

    let touch_pinch: f32 = pinch_gestures.read().map(|gesture| gesture.0).sum();
    if touch_pinch != 0.0 {
        rig.target_distance = (rig.target_distance - touch_pinch * CAMERA_TOUCH_PINCH_ZOOM_SPEED)
            .clamp(CAMERA_MIN_DISTANCE, CAMERA_MAX_DISTANCE);
    }

    let orbit_blend = 1.0 - (-CAMERA_ORBIT_SMOOTHING * time.delta_secs()).exp();
    rig.yaw += (rig.target_yaw - rig.yaw) * orbit_blend;
    let zoom_blend = 1.0 - (-CAMERA_ZOOM_SMOOTHING * time.delta_secs()).exp();
    rig.distance += (rig.target_distance - rig.distance) * zoom_blend;
}

fn update_camera_transform(
    object: Single<&Transform, (With<CameraTarget>, Without<MainCamera>)>,
    mut camera: Single<(&mut Transform, &CameraRig, &mut GroundCoverView), With<MainCamera>>,
) {
    *camera.0 = camera_transform(object.translation, camera.1);
    camera.2.normalized_zoom = normalized_camera_zoom(camera.1.distance);
}

fn normalized_camera_zoom(distance: f32) -> f32 {
    ((distance - CAMERA_MIN_DISTANCE) / (CAMERA_ZOOM_REFERENCE_DISTANCE - CAMERA_MIN_DISTANCE))
        .clamp(0.0, 1.0)
}

fn camera_transform(object_position: Vec3, rig: &CameraRig) -> Transform {
    let zoom = normalized_camera_zoom(rig.distance);
    let pitch =
        (CAMERA_NEAR_PITCH + (CAMERA_FAR_PITCH - CAMERA_NEAR_PITCH) * zoom + rig.pitch_offset)
            .clamp(5.0_f32.to_radians(), 80.0_f32.to_radians());
    let horizontal_distance = rig.distance * pitch.cos();
    let focus = object_position + Vec3::Y * CAMERA_FOCUS_HEIGHT;
    let offset = Vec3::new(
        rig.yaw.sin() * horizontal_distance,
        rig.distance * pitch.sin(),
        rig.yaw.cos() * horizontal_distance,
    );

    Transform::from_translation(focus + offset).looking_at(focus, Vec3::Y)
}

fn update_performance_label(
    diagnostics: Res<DiagnosticsStore>,
    streaming: Option<Res<StreamingStats>>,
    ground_cover_debug: Res<GroundCoverDebug>,
    terrain_macro: Res<TerrainMacroVariation>,
    camera: Single<(&CameraRig, &GroundCoverView), With<MainCamera>>,
    player_motion: Single<&CharacterMotion, With<PlayerControlled>>,
    primary_monitor: Option<Single<&Monitor, With<PrimaryMonitor>>>,
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
    let display_refresh = primary_monitor
        .and_then(|monitor| monitor.refresh_rate_millihertz)
        .map(|millihertz| format!("{:.0} Hz display max", millihertz as f32 / 1_000.0))
        .unwrap_or_else(|| "display rate unknown".into());

    let streaming = streaming
        .map(|stats| {
            let lods = if stats.lod_counts.is_empty() {
                "none".into()
            } else {
                stats
                    .lod_counts
                    .iter()
                    .map(|(lod, count)| format!("L{lod}: {count}"))
                    .collect::<Vec<_>>()
                    .join(" | ")
            };
            format!(
                "World: {}\nPages: {} demanded | {} loading | {} resident | {} cooling | {} failed\n\
                 Visual LODs: {} | projected height: {:.0}-{:.0} px\n\
                 Ground cover: {} resident clusters | debug: {}\n\
                 Nearby gameplay: {} objects | {} definitions cached\n\
                 Residency: {:.2} MiB decoded | {:.2} MiB estimated GPU",
                stats.status,
                stats.demanded,
                stats.loading,
                stats.resident,
                stats.cooling,
                stats.failed,
                lods,
                stats.minimum_projected_height,
                stats.maximum_projected_height,
                stats.ground_cover_clusters,
                ground_cover_debug.mode.label(),
                stats.gameplay_objects,
                stats.cached_definitions,
                stats.decoded_bytes as f64 / (1024.0 * 1024.0),
                stats.gpu_bytes_estimate as f64 / (1024.0 * 1024.0),
            )
        })
        .unwrap_or_else(|| "World: initializing".into());

    **label = Text::new(format!(
        "Tap / left click: move | WASD / left stick: direct movement | Tab: change area | G: grass debug | V: terrain macro\n\
         Two-finger horizontal / right drag / right stick: orbit\n\
         Pinch / two-finger vertical / wheel: smooth zoom\n\
         Actor: {:?} {:?} | {:.2} m/s | playback {:.2}x\n\
         Camera: {distance:.2} m (target {target_distance:.2} m) | normalized zoom: {normalized_zoom:.3}\n\
         Terrain macro: {terrain_macro}\n\
         VSync baseline: {fps:.0} FPS | {frame_time:.2} ms | {display_refresh}\n\
         {streaming}",
        player_motion.gait,
        player_motion.phase,
        player_motion.speed_mps,
        player_motion.playback_rate,
        distance = camera.0.distance,
        target_distance = camera.0.target_distance,
        normalized_zoom = camera.1.normalized_zoom,
        terrain_macro = terrain_macro.label(),
    ));
}
