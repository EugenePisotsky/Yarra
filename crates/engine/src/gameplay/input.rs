//! Native player intent and camera controls. Omit this plugin for scripted gameplay.
use super::{
    GameInputEnabled, GamePointerInputBlocked, GameplaySystems,
    camera::{CAMERA_MAX_DISTANCE, CAMERA_MIN_DISTANCE, CameraRig, MainCamera},
};
use crate::{
    CameraInputDiagnostics, StreamedTerrainSurface, WorldOrigin,
    actor::{CharacterGait, MoveIntent, PlayerControlled},
    sample_resident_terrain_surface,
};
use bevy::{
    input::{
        gestures::{PanGesture, PinchGesture},
        mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel},
    },
    prelude::*,
    window::{PrimaryWindow, Window},
};

/// Reads native player and orbit-camera input. Requires GameplayPlugin, Bevy InputPlugin,
/// and a gameplay camera for camera-relative controls. UI capture still permits keyboard/gamepad.
pub struct GameInputPlugin;
impl Plugin for GameInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TouchTapState>().add_systems(
            Update,
            (
                update_camera_controls.in_set(GameplaySystems::CameraInput),
                set_target_from_pointer.in_set(GameplaySystems::PointerInput),
                update_player_move_intent
                    .in_set(GameplaySystems::MoveIntent)
                    .run_if(resource_equals(GameInputEnabled(true))),
            ),
        );
    }
}

const CAMERA_STICK_DEAD_ZONE: f32 = 0.15;
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
const TOUCH_TAP_MAX_MOVEMENT: f32 = 18.0;

#[derive(Resource, Default)]
struct TouchTapState {
    primary_touch: Option<u64>,
    disqualified: bool,
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
    mut mouse_scroll: MessageReader<MouseWheel>,
    mut pan_gestures: MessageReader<PanGesture>,
    mut pinch_gestures: MessageReader<PinchGesture>,
    gamepads: Query<&Gamepad>,
    mut rig: Single<&mut CameraRig, With<MainCamera>>,
    mut diagnostics: Option<ResMut<CameraInputDiagnostics>>,
) {
    if let Some(d) = diagnostics.as_deref_mut() {
        *d = CameraInputDiagnostics {
            sequence: d.sequence + 1,
            read_at: Some(std::time::Instant::now()),
            enabled: enabled.0,
            pointer_blocked: pointer_blocked.0,
            startup_guard: time.elapsed_secs() <= 0.5,
            dt_secs: time.delta_secs(),
            yaw_before: rig.yaw,
            yaw_after: rig.yaw,
            target_yaw: rig.target_yaw,
            ..default()
        };
    }
    // Always consume native gestures, including while locked/captured, so unlocking cannot
    // replay a pending pan or pinch. Keyboard and gamepad remain usable over the HUD.
    let touch_pan: Vec2 = pan_gestures.read().map(|gesture| gesture.0).sum();
    let touch_pinch: f32 = pinch_gestures.read().map(|gesture| gesture.0).sum();
    // Clamp individual native events, not a frame's accumulated movement. At
    // 60 FPS the same gesture may put twice as many events in one update as at 120.
    let scroll: Vec2 = mouse_scroll
        .read()
        .map(|event| {
            let (limit, speed) = match event.unit {
                MouseScrollUnit::Line => (
                    3.0,
                    Vec2::new(CAMERA_WHEEL_ORBIT_SPEED, CAMERA_WHEEL_ZOOM_SPEED),
                ),
                MouseScrollUnit::Pixel => (
                    80.0,
                    Vec2::new(CAMERA_TRACKPAD_ORBIT_SPEED, CAMERA_TRACKPAD_ZOOM_SPEED),
                ),
            };
            if let Some(d) = diagnostics.as_deref_mut() {
                d.record_wheel(event, limit);
            }
            Vec2::new(event.x, event.y).clamp(Vec2::splat(-limit), Vec2::splat(limit)) * speed
        })
        .sum();
    if let Some(d) = diagnostics.as_deref_mut() {
        d.requested_orbit = -scroll.x;
    }
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

    let previous_target_yaw = rig.target_yaw;
    if !pointer_blocked.0 && time.elapsed_secs() > 0.5 {
        rig.target_yaw -= scroll.x;
        if let Some(d) = diagnostics.as_deref_mut() {
            d.applied_orbit = -scroll.x;
        }
        rig.target_distance =
            (rig.target_distance - scroll.y).clamp(CAMERA_MIN_DISTANCE, CAMERA_MAX_DISTANCE);
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

    rig.yaw = smooth_camera_orbit(
        rig.yaw,
        previous_target_yaw,
        rig.target_yaw,
        time.delta_secs(),
    );
    let zoom_blend = 1.0 - (-CAMERA_ZOOM_SMOOTHING * time.delta_secs()).exp();
    rig.distance += (rig.target_distance - rig.distance) * zoom_blend;
    if let Some(d) = diagnostics.as_deref_mut() {
        d.yaw_after = rig.yaw;
        d.target_yaw = rig.target_yaw;
    }
}

fn smooth_camera_orbit(yaw: f32, previous_target: f32, target: f32, dt: f32) -> f32 {
    let step = CAMERA_ORBIT_SMOOTHING * dt;
    if step <= 0.0 {
        return yaw;
    }
    let blend = -(-step).exp_m1();
    // Integrate the existing exponential smoother with gesture movement spread
    // across the elapsed interval. Applying the entire batch at the start of
    // the interval exaggerates packet-to-packet velocity changes at lower FPS.
    let movement_blend = if step < 0.001 {
        step * 0.5 - step * step / 6.0
    } else {
        1.0 - blend / step
    };
    yaw + (previous_target - yaw) * blend + (target - previous_target) * movement_blend
}

#[cfg(test)]
mod tests;
