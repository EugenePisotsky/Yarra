mod actor;
mod terrain_raycast;
pub use terrain_raycast::raycast_resident_terrain;
mod character;
mod character_catalog;
mod msaa_store;
mod world_streaming;

use std::{f32::consts::TAU, path::PathBuf};

pub use atmosphere::{
    ApplyAtmosphere, AtmosphereOwner, AtmosphereState, WorldEnvironmentCamera,
    WorldEnvironmentPlugin, WorldEnvironmentView, WorldSun,
};
use bevy::{
    core_pipeline::prepass::DepthPrepass,
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    input::gestures::{PanGesture, PinchGesture},
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    prelude::*,
    render::view::Msaa,
    window::{Monitor, PrimaryMonitor, PrimaryWindow, Window},
};
pub use character::{
    CharacterPresentationPreview, CharacterPresentationPreviewPlugin, CharacterPreviewClip,
};
pub use character_catalog::{
    CharacterMovementContextDefinition, CharacterPresentationCatalogSummary,
    CharacterPresentationProfileSummary, CharacterPreviewClipDefinition, CharacterPreviewClipRole,
    DEFAULT_CHARACTER_PRESENTATION_ID, load_character_presentation_catalog_summary,
};
pub use msaa_store::{MsaaColorStorePlugin, MsaaColorStorePolicy};
use terrain_render::{TerrainMacroVariation, TerrainRenderPlugin};
pub use world_streaming::{
    ActiveWorldSpace, GameplayObject, GeneratedEnvironmentObject, LiveTerrainPreview,
    StreamedTerrainSurface, StreamedVegetationFieldPage, StreamedVisualObject, StreamingStats,
    TerrainContactReadiness, TerrainLodPreview, TerrainLodStats, TerrainPreviewRequest,
    WorldCatalog, WorldDetailDemand, WorldGenerationReload, WorldOrigin, WorldRenderRoot,
    WorldSpaceInfo, WorldStreamingConfig, WorldStreamingPlugin, WorldStreamingSystems,
    WorldViewCamera, WorldViewpoint, sample_resident_terrain_surface, spawn_collection_visual,
};

use crate::{
    actor::{
        CameraTarget, CharacterGait, CharacterMotion, CharacterMotor, MoveIntent, PlayerControlled,
        TerrainGrounded, WorldStreamFocus, advance_character_motors,
    },
    character::{
        CharacterPresentationPlugin, CharacterPresentationRef, CharacterPresentationResolveSet,
    },
};

const CAMERA_STICK_DEAD_ZONE: f32 = 0.15;
const CAMERA_MIN_DISTANCE: f32 = 4.0;
const CAMERA_MAX_DISTANCE: f32 = 17.6;
const CAMERA_ZOOM_REFERENCE_DISTANCE: f32 = 24.0;
const CAMERA_DEFAULT_DISTANCE: f32 = CAMERA_MAX_DISTANCE;
const CAMERA_FOCUS_HEIGHT: f32 = 0.9;
const CAMERA_NEAR_PITCH: f32 = 10.0_f32.to_radians();
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
const DEBUG_SUN_CYCLE_SECONDS: f32 = 45.0;
const DEBUG_SUN_MIN_ELEVATION: f32 = 12.0_f32.to_radians();
const DEBUG_SUN_MAX_ELEVATION: f32 = 55.0_f32.to_radians();

/// The current mobile scene has no effects that consume prepass depth. In Bevy
/// 0.19 the pass also copies the full-resolution depth texture every frame.
/// Keep the audit baseline aligned with the normal game camera.
pub const GAME_DEPTH_PREPASS_ENABLED: bool = !cfg!(target_os = "ios");

mod start_view;
pub use start_view::WorldStartView;

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
            MsaaColorStorePlugin,
            WorldEnvironmentPlugin::game(),
            TerrainRenderPlugin,
            CharacterPresentationPlugin,
            WorldStreamingPlugin::game(self.runtime_database.clone()),
        ))
        .init_resource::<TouchTapState>()
        .init_resource::<WorldStartView>()
        .init_resource::<GameInputEnabled>()
        .init_resource::<GamePointerInputBlocked>()
        .init_resource::<DemoSunMotion>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                update_camera_controls,
                set_target_from_pointer,
                update_player_move_intent.run_if(resource_equals(GameInputEnabled(true))),
                advance_character_motors
                    .after(CharacterPresentationResolveSet)
                    .run_if(resource_equals(GameInputEnabled(true))),
                ground_characters_to_streamed_terrain,
                update_target_indicator,
                update_camera_transform,
                update_demo_sun_motion,
            )
                .chain()
                .in_set(GameInputSystems),
        )
        .add_systems(
            Update,
            update_performance_label.after(update_demo_sun_motion),
        );
    }
}

/// Enables gameplay controls and actor movement. Profiling tools may explicitly freeze them.
#[derive(Resource, Clone, Copy, PartialEq, Eq)]
pub struct GameInputEnabled(pub bool);

impl Default for GameInputEnabled {
    fn default() -> Self {
        Self(true)
    }
}

/// UI pointer capture does not suppress keyboard or gamepad input.
#[derive(Resource, Default)]
pub struct GamePointerInputBlocked(pub bool);

/// UI input routing must run before gameplay reads input.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GameInputSystems;

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

#[derive(Resource, Default)]
struct DemoSunMotion {
    enabled: bool,
    cycle: f32,
    base_azimuth: Option<f32>,
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
    start_view: Res<WorldStartView>,
) {
    let start = start_view
        .0
        .as_ref()
        .map_or(Vec3::ZERO, |v| Vec3::from_array(v.position));
    commands.spawn((
        Transform::from_translation(start),
        Visibility::Inherited,
        MoveIntent::default(),
        CharacterMotor::default(),
        CharacterMotion::default(),
        CharacterPresentationRef::new(DEFAULT_CHARACTER_PRESENTATION_ID),
        PlayerControlled,
        TerrainGrounded,
        CameraTarget,
        WorldStreamFocus,
        WorldRenderRoot,
        Name::new("Player actor root"),
    ));

    let mut camera_rig = CameraRig {
        yaw: 45.0_f32.to_radians(),
        target_yaw: 45.0_f32.to_radians(),
        distance: CAMERA_DEFAULT_DISTANCE,
        target_distance: CAMERA_DEFAULT_DISTANCE,
        pitch_offset: 0.0,
    };
    let mut environment = WorldEnvironmentCamera::default();
    if let Some(view) = &start_view.0 {
        camera_rig.yaw = view.yaw_degrees.to_radians();
        camera_rig.target_yaw = camera_rig.yaw;
        camera_rig.distance = view.distance;
        camera_rig.target_distance = view.distance;
        camera_rig.pitch_offset = view.pitch_degrees.to_radians()
            - (CAMERA_NEAR_PITCH
                + (CAMERA_FAR_PITCH - CAMERA_NEAR_PITCH) * normalized_camera_zoom(view.distance));
        environment = WorldEnvironmentCamera::with_visibility(view.fog_visibility);
    }
    let mut camera = commands.spawn((
        Camera3d::default(),
        start_view.projection(),
        environment,
        Msaa::Sample4,
        MsaaColorStorePolicy::Automatic,
        camera_transform(start, &camera_rig),
        camera_rig,
        MainCamera,
        WorldViewCamera,
        WorldRenderRoot,
        Name::new("Main camera"),
    ));
    if GAME_DEPTH_PREPASS_ENABLED {
        camera.insert(DepthPrepass);
    }

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
        WorldRenderRoot,
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

fn update_demo_sun_motion(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion: ResMut<DemoSunMotion>,
    sun: Single<&Transform, With<WorldSun>>,
    mut atmosphere: ResMut<AtmosphereState>,
) {
    let current_direction: Vec3 = sun.back().into();
    let base_azimuth = *motion
        .base_azimuth
        .get_or_insert_with(|| current_direction.x.atan2(current_direction.z));
    if keys.just_pressed(KeyCode::KeyU) {
        motion.enabled = !motion.enabled;
        warn!(
            "bounded daytime grass-lighting stress test: {}",
            if motion.enabled { "on" } else { "off" }
        );
    }
    if motion.enabled {
        motion.cycle = (motion.cycle + time.delta_secs() / DEBUG_SUN_CYCLE_SECONDS).fract();
        let azimuth = base_azimuth + motion.cycle * TAU;
        let elevation_blend = 0.5 - 0.5 * (motion.cycle * TAU).cos();
        let elevation = DEBUG_SUN_MIN_ELEVATION
            + (DEBUG_SUN_MAX_ELEVATION - DEBUG_SUN_MIN_ELEVATION) * elevation_blend;
        let horizontal = elevation.cos();
        let direction_to_sun = Vec3::new(
            azimuth.sin() * horizontal,
            elevation.sin(),
            azimuth.cos() * horizontal,
        );
        atmosphere.direction_override = Some(direction_to_sun);
    } else {
        atmosphere.direction_override = None;
    }
}

fn set_target_from_pointer(
    enabled: Res<GameInputEnabled>,
    pointer_blocked: Res<GamePointerInputBlocked>,
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    mut touch_tap: ResMut<TouchTapState>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<MainCamera>>,
    origin: Res<WorldOrigin>,
    terrain_pages: Query<&StreamedTerrainSurface>,
    mut player: Single<(&Transform, &mut MoveIntent), With<PlayerControlled>>,
) {
    if !enabled.0 || pointer_blocked.0 {
        *touch_tap = TouchTapState::default();
        return;
    }
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
    let point =
        raycast_streamed_terrain(ray.origin, direction, &origin, &terrain_pages).or_else(|| {
            if direction.y.abs() < 1.0e-5 {
                return None;
            }
            let distance = (player.0.translation.y - ray.origin.y) / direction.y;
            (distance >= 0.0).then(|| ray.origin + direction * distance)
        });
    if let Some(point) = point {
        player.1.set_destination(point, CharacterGait::Walk);
    }
}

fn raycast_streamed_terrain(
    ray_origin: Vec3,
    ray_direction: Vec3,
    origin: &WorldOrigin,
    terrain_pages: &Query<&StreamedTerrainSurface>,
) -> Option<Vec3> {
    const STEP_METERS: f32 = 0.75;
    const MAXIMUM_DISTANCE: f32 = 256.0;
    const REFINEMENT_STEPS: usize = 12;

    let mut previous: Option<(f32, f32)> = None;
    let step_count = (MAXIMUM_DISTANCE / STEP_METERS) as usize;
    for step in 0..=step_count {
        let distance = step as f32 * STEP_METERS;
        let point = ray_origin + ray_direction * distance;
        let Some(surface) =
            sample_resident_terrain_surface(origin, terrain_pages.iter(), [point.x, point.z])
        else {
            previous = None;
            continue;
        };
        let clearance = point.y - surface.height;
        let Some((previous_distance, previous_clearance)) = previous else {
            previous = Some((distance, clearance));
            continue;
        };
        if previous_clearance >= 0.0 && clearance <= 0.0 {
            let mut above = previous_distance;
            let mut below = distance;
            for _ in 0..REFINEMENT_STEPS {
                let middle = (above + below) * 0.5;
                let middle_point = ray_origin + ray_direction * middle;
                let Some(middle_surface) = sample_resident_terrain_surface(
                    origin,
                    terrain_pages.iter(),
                    [middle_point.x, middle_point.z],
                ) else {
                    break;
                };
                if middle_point.y >= middle_surface.height {
                    above = middle;
                } else {
                    below = middle;
                }
            }
            let hit = ray_origin + ray_direction * ((above + below) * 0.5);
            let surface =
                sample_resident_terrain_surface(origin, terrain_pages.iter(), [hit.x, hit.z])?;
            return Some(Vec3::new(hit.x, surface.height, hit.z));
        }
        previous = Some((distance, clearance));
    }
    None
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

fn ground_characters_to_streamed_terrain(
    origin: Res<WorldOrigin>,
    terrain_pages: Query<&StreamedTerrainSurface>,
    mut actors: Query<&mut Transform, With<TerrainGrounded>>,
    lod: Res<world_streaming::terrain_lod::TerrainLodStream>,
    lod_config: Res<TerrainLodPreview>,
    readiness: Res<TerrainContactReadiness>,
) {
    for mut transform in &mut actors {
        if lod_config.enabled {
            if let Some(height) =
                lod.sample_contact_height(transform.translation, &origin, &readiness)
            {
                transform.translation.y = height;
            }
            continue;
        }
        if let Some(surface) = sample_resident_terrain_surface(
            &origin,
            terrain_pages.iter(),
            [transform.translation.x, transform.translation.z],
        ) {
            transform.translation.y = surface.height;
        }
    }
}

fn update_target_indicator(
    intent: Single<&MoveIntent, With<PlayerControlled>>,
    origin: Res<WorldOrigin>,
    terrain_pages: Query<&StreamedTerrainSurface>,
    mut indicator: TargetIndicatorState,
) {
    if let Some(target) = intent.destination() {
        let ground_height =
            sample_resident_terrain_surface(&origin, terrain_pages.iter(), [target.x, target.z])
                .map_or(target.y, |surface| surface.height);
        indicator.0.translation =
            Vec3::new(target.x, ground_height + TARGET_INDICATOR_HEIGHT, target.z);
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
    enabled: Res<GameInputEnabled>,
    pointer_blocked: Res<GamePointerInputBlocked>,
    time: Res<Time>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    mut pan_gestures: MessageReader<PanGesture>,
    mut pinch_gestures: MessageReader<PinchGesture>,
    gamepads: Query<&Gamepad>,
    mut rig: Single<&mut CameraRig, With<MainCamera>>,
) {
    // Always consume native gestures, including while locked/captured, so unlocking cannot
    // replay a pending pan or pinch. Keyboard and gamepad remain usable over the HUD.
    let touch_pan: Vec2 = pan_gestures.read().map(|gesture| gesture.0).sum();
    let touch_pinch: f32 = pinch_gestures.read().map(|gesture| gesture.0).sum();
    if !enabled.0 {
        return;
    }
    if !pointer_blocked.0 && mouse_buttons.pressed(MouseButton::Right) {
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

    if !pointer_blocked.0 && time.elapsed_secs() > 0.5 {
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

    if !pointer_blocked.0 && touch_pan != Vec2::ZERO {
        rig.target_yaw -= touch_pan.x * CAMERA_TOUCH_ORBIT_SPEED;
        rig.target_distance = (rig.target_distance + touch_pan.y * CAMERA_TOUCH_PAN_ZOOM_SPEED)
            .clamp(CAMERA_MIN_DISTANCE, CAMERA_MAX_DISTANCE);
    }

    if !pointer_blocked.0 && touch_pinch != 0.0 {
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
    mut camera: Single<(&mut Transform, &CameraRig), With<MainCamera>>,
) {
    *camera.0 = camera_transform(object.translation, camera.1);
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
    terrain_macro: Res<TerrainMacroVariation>,
    sun_motion: Res<DemoSunMotion>,
    camera: Single<&CameraRig, With<MainCamera>>,
    sun: Single<&Transform, With<WorldSun>>,
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
    let direction_to_sun: Vec3 = sun.back().into();
    let sun_elevation = direction_to_sun.y.clamp(-1.0, 1.0).asin().to_degrees();
    let sun_azimuth = direction_to_sun.x.atan2(direction_to_sun.z).to_degrees();

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
                 Vegetation V2: {} resident field pages\n\
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
                stats.vegetation_pages,
                stats.gameplay_objects,
                stats.cached_definitions,
                stats.decoded_bytes as f64 / (1024.0 * 1024.0),
                stats.gpu_bytes_estimate as f64 / (1024.0 * 1024.0),
            )
        })
        .unwrap_or_else(|| "World: initializing".into());

    **label = Text::new(format!(
        "Tap / left click: move | WASD / left stick: direct movement | Tab: change area | V: terrain macro | U: sun motion\n\
         Two-finger horizontal / right drag / right stick: orbit\n\
         Pinch / two-finger vertical / wheel: smooth zoom\n\
         Actor: {:?} {:?} | {:.2} m/s | playback {:.2}x\n\
         Camera: {distance:.2} m (target {target_distance:.2} m) | normalized zoom: {normalized_zoom:.3}\n\
         Sun: {sun_elevation:.1} deg elevation | {sun_azimuth:.1} deg azimuth | motion: {sun_motion}\n\
         Terrain macro: {terrain_macro}\n\
         VSync baseline: {fps:.0} FPS | {frame_time:.2} ms | {display_refresh}\n\
         {streaming}",
        player_motion.gait,
        player_motion.phase,
        player_motion.speed_mps,
        player_motion.playback_rate,
        distance = camera.distance,
        target_distance = camera.target_distance,
        normalized_zoom = normalized_camera_zoom(camera.distance),
        sun_motion = if sun_motion.enabled { "on" } else { "off" },
        terrain_macro = terrain_macro.label(),
    ));
}

#[cfg(test)]
mod input_tests {
    use super::*;

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
            .add_systems(Startup, setup);
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

    #[test]
    fn camera_gestures_work_and_are_not_replayed_after_ui_capture_or_unlock() {
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin)
            .init_resource::<Time>()
            .init_resource::<GameInputEnabled>()
            .init_resource::<GamePointerInputBlocked>()
            .add_systems(Update, update_camera_controls);
        let camera = app
            .world_mut()
            .spawn((
                MainCamera,
                CameraRig {
                    yaw: 0.0,
                    target_yaw: 0.0,
                    distance: CAMERA_DEFAULT_DISTANCE,
                    target_distance: CAMERA_DEFAULT_DISTANCE,
                    pitch_offset: 0.0,
                },
            ))
            .id();
        let gesture = |app: &mut App| {
            app.world_mut()
                .write_message(PanGesture(Vec2::new(20.0, 0.0)));
            app.world_mut().write_message(PinchGesture(0.1));
            app.update();
        };
        app.world_mut().resource_mut::<GameInputEnabled>().0 = false;
        gesture(&mut app);
        app.world_mut().resource_mut::<GameInputEnabled>().0 = true;
        app.update();
        assert_eq!(
            app.world().get::<CameraRig>(camera).unwrap().target_yaw,
            0.0
        );
        app.world_mut().resource_mut::<GamePointerInputBlocked>().0 = true;
        gesture(&mut app);
        app.world_mut().resource_mut::<GamePointerInputBlocked>().0 = false;
        app.update();
        let rig = app.world().get::<CameraRig>(camera).unwrap();
        assert_eq!(rig.target_yaw, 0.0);
        assert_eq!(rig.target_distance, CAMERA_DEFAULT_DISTANCE);
        gesture(&mut app);
        let rig = app.world().get::<CameraRig>(camera).unwrap();
        assert!(rig.target_yaw < 0.0, "uncaptured pan must orbit");
        assert!(
            rig.target_distance < CAMERA_DEFAULT_DISTANCE,
            "uncaptured pinch must zoom"
        );
    }
}
