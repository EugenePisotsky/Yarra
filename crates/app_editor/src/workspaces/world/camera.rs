//! World camera setup, logical focus, orbit/pan/zoom and rebase-aware navigation.
use crate::{
    shell::EditorInputCapture,
    workspaces::world::input::{control_pressed, shift_pressed},
};
use bevy::{
    core_pipeline::prepass::DepthPrepass,
    gizmos::transform_gizmo::TransformGizmoCamera,
    input::{
        gestures::PinchGesture,
        mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    },
    prelude::*,
    render::view::Msaa,
};
use engine::{
    WorldCatalog, WorldEnvironmentCamera, WorldOrigin, WorldStreamingConfig, WorldViewCamera,
    WorldViewpoint,
};
use std::f32::consts::FRAC_PI_4;
use world::{CellCoord, WorldPosition};

const MIN_CAMERA_DISTANCE: f32 = 1.0;
const MAX_CAMERA_DISTANCE: f32 = 4_000.0;
const MIN_CAMERA_NAVIGATION_SCALE: f32 = 6.0;

#[derive(Component, Debug)]
pub(crate) struct EditorCamera {
    focus: Option<WorldPosition>,
    pub(crate) distance: f32,
    yaw: f32,
    pitch: f32,
}

#[derive(Resource, Default)]
pub(crate) struct EditorCameraDrag {
    orbiting: bool,
    panning: bool,
    dollying: bool,
}

#[derive(Resource, Default)]
pub(crate) struct EditorCameraFocusRequest(pub(crate) Option<WorldPosition>);
pub(crate) fn setup_world_workspace(
    mut commands: Commands,
    start_view: Option<Res<engine::WorldStartView>>,
) {
    let mut controller = EditorCamera {
        focus: None,
        distance: 48.0,
        yaw: FRAC_PI_4,
        pitch: 0.58,
    };
    let mut environment = WorldEnvironmentCamera::default();
    let mut transform = Transform::from_xyz(24.0, 26.0, 24.0).looking_at(Vec3::ZERO, Vec3::Y);
    if let Some(view) = start_view.as_ref().and_then(|s| s.0.as_ref()) {
        controller.distance = view.distance;
        controller.yaw = view.yaw_degrees.to_radians();
        controller.pitch = view.pitch_degrees.to_radians();
        environment = WorldEnvironmentCamera::with_visibility(view.fog_visibility);
        transform = engine::WorldStartView::camera_at(view, Vec3::from_array(view.position));
    }
    commands.spawn((
        Camera3d::default(),
        start_view
            .as_ref()
            .map_or_else(Projection::default, |s| s.projection()),
        environment,
        Msaa::Off,
        DepthPrepass,
        transform,
        controller,
        WorldViewCamera,
        TransformGizmoCamera,
        Name::new("Editor world camera"),
    ));
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_editor_camera(
    time: Res<Time>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    mut pinch_gestures: MessageReader<PinchGesture>,
    capture: Res<EditorInputCapture>,
    streaming_config: Res<WorldStreamingConfig>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    mut viewpoint: ResMut<WorldViewpoint>,
    mut drag: ResMut<EditorCameraDrag>,
    mut focus_request: ResMut<EditorCameraFocusRequest>,
    mut camera: Single<(&mut EditorCamera, &mut Transform), With<WorldViewCamera>>,
) {
    if let Some(requested_focus) = focus_request.0.take() {
        camera.0.focus = Some(requested_focus);
    }
    let incoming = viewpoint.position();
    if camera.0.focus.is_none()
        || incoming.is_some_and(|position| {
            camera
                .0
                .focus
                .is_some_and(|focus| focus.space != position.space)
        })
    {
        camera.0.focus = incoming;
    }
    let Some(mut focus) = camera.0.focus else {
        return;
    };
    let Some(space) = catalog.world_space(focus.space) else {
        return;
    };

    if mouse_buttons.just_pressed(MouseButton::Right) && !capture.wants_pointer {
        drag.orbiting = !shift_pressed(&keys);
        drag.panning = shift_pressed(&keys);
        drag.dollying = false;
    }
    if mouse_buttons.just_pressed(MouseButton::Middle) && !capture.wants_pointer {
        drag.dollying = control_pressed(&keys);
        drag.panning = !drag.dollying && shift_pressed(&keys);
        drag.orbiting = !drag.dollying && !drag.panning;
    }
    if mouse_buttons.just_released(MouseButton::Right) {
        drag.orbiting = false;
        if !mouse_buttons.pressed(MouseButton::Middle) {
            drag.panning = false;
            drag.dollying = false;
        }
    }
    if mouse_buttons.just_released(MouseButton::Middle)
        && !mouse_buttons.pressed(MouseButton::Right)
    {
        drag.orbiting = false;
        drag.panning = false;
        drag.dollying = false;
    }

    if drag.orbiting {
        camera.0.yaw -= mouse_motion.delta.x * 0.006;
        camera.0.pitch = (camera.0.pitch + mouse_motion.delta.y * 0.006).clamp(0.08, 1.48);
    }
    if drag.panning {
        let scale = camera_navigation_scale(camera.0.distance) * 0.0018;
        let delta = *camera.1.right() * (-mouse_motion.delta.x * scale)
            + *camera.1.up() * (mouse_motion.delta.y * scale);
        focus = focus.translated(delta.to_array(), space.cell_size);
    }
    if drag.dollying {
        let forward = *camera.1.forward();
        zoom_editor_camera(
            &mut camera.0,
            &mut focus,
            forward,
            -mouse_motion.delta.y * 0.012,
            space.cell_size,
        );
    }

    if !capture.wants_pointer {
        match mouse_scroll.unit {
            MouseScrollUnit::Line if mouse_scroll.delta.y != 0.0 => {
                let forward = *camera.1.forward();
                zoom_editor_camera(
                    &mut camera.0,
                    &mut focus,
                    forward,
                    mouse_scroll.delta.y * 0.12,
                    space.cell_size,
                );
            }
            MouseScrollUnit::Pixel if mouse_scroll.delta != Vec2::ZERO => {
                if shift_pressed(&keys) {
                    camera.0.yaw -= mouse_scroll.delta.x * 0.004;
                    camera.0.pitch =
                        (camera.0.pitch + mouse_scroll.delta.y * 0.004).clamp(0.08, 1.48);
                } else if control_pressed(&keys) {
                    let forward = *camera.1.forward();
                    zoom_editor_camera(
                        &mut camera.0,
                        &mut focus,
                        forward,
                        mouse_scroll.delta.y / 80.0,
                        space.cell_size,
                    );
                } else {
                    let scale = camera_navigation_scale(camera.0.distance) * 0.0018;
                    let delta = *camera.1.right() * (-mouse_scroll.delta.x * scale)
                        + *camera.1.up() * (mouse_scroll.delta.y * scale);
                    focus = focus.translated(delta.to_array(), space.cell_size);
                }
            }
            _ => {}
        }
        let pinch: f32 = pinch_gestures.read().map(|gesture| gesture.0).sum();
        if pinch != 0.0 {
            let forward = *camera.1.forward();
            zoom_editor_camera(
                &mut camera.0,
                &mut focus,
                forward,
                pinch * 1.6,
                space.cell_size,
            );
        }
    } else {
        pinch_gestures.clear();
    }

    if mouse_buttons.pressed(MouseButton::Right) && !capture.wants_keyboard {
        let mut movement = Vec3::ZERO;
        if keys.pressed(KeyCode::KeyW) {
            movement.z -= 1.0;
        }
        if keys.pressed(KeyCode::KeyS) {
            movement.z += 1.0;
        }
        if keys.pressed(KeyCode::KeyA) {
            movement.x -= 1.0;
        }
        if keys.pressed(KeyCode::KeyD) {
            movement.x += 1.0;
        }
        if keys.pressed(KeyCode::KeyE) {
            movement.y += 1.0;
        }
        if keys.pressed(KeyCode::KeyQ) {
            movement.y -= 1.0;
        }
        if movement != Vec3::ZERO {
            let speed = camera_navigation_scale(camera.0.distance) * time.delta_secs() * 0.8;
            let delta = (*camera.1.right() * movement.x
                + Vec3::Y * movement.y
                + *camera.1.forward() * movement.z)
                * speed;
            focus = focus.translated(delta.to_array(), space.cell_size);
        }
    }

    camera.0.focus = Some(focus);
    viewpoint.set(focus);
    let render_origin = anticipated_render_origin(
        origin.cell(),
        focus.cell,
        streaming_config.floating_origin_threshold_cells(),
    );
    *camera.1 = editor_camera_transform(&camera.0, render_origin, space.cell_size);
}

fn anticipated_render_origin(
    current: CellCoord,
    focus: CellCoord,
    threshold: Option<u32>,
) -> CellCoord {
    if threshold.is_some_and(|threshold| current.chebyshev_distance(focus) > threshold) {
        focus
    } else {
        current
    }
}

fn camera_navigation_scale(distance: f32) -> f32 {
    distance.max(MIN_CAMERA_NAVIGATION_SCALE)
}

fn camera_zoom_step(distance: f32, amount: f32) -> (f32, f32) {
    let amount = amount.clamp(-4.0, 4.0);
    let scale = camera_navigation_scale(distance);
    let requested = distance + scale * ((-amount).exp() - 1.0);
    if requested < MIN_CAMERA_DISTANCE {
        (MIN_CAMERA_DISTANCE, MIN_CAMERA_DISTANCE - requested)
    } else {
        (requested.min(MAX_CAMERA_DISTANCE), 0.0)
    }
}

fn zoom_editor_camera(
    camera: &mut EditorCamera,
    focus: &mut WorldPosition,
    forward: Vec3,
    amount: f32,
    cell_size: f32,
) {
    let (distance, forward_travel) = camera_zoom_step(camera.distance, amount);
    camera.distance = distance;
    if forward_travel > 0.0 {
        *focus = focus.translated((forward * forward_travel).to_array(), cell_size);
    }
}

fn editor_camera_transform(
    camera: &EditorCamera,
    origin_cell: CellCoord,
    cell_size: f32,
) -> Transform {
    let focus = Vec3::from_array(
        camera
            .focus
            .expect("editor camera transform requires a logical focus")
            .relative_to(origin_cell, cell_size),
    );
    let horizontal = camera.distance * camera.pitch.cos();
    let offset = Vec3::new(
        horizontal * camera.yaw.sin(),
        camera.distance * camera.pitch.sin(),
        horizontal * camera.yaw.cos(),
    );
    Transform::from_translation(focus + offset).looking_at(focus, Vec3::Y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        shell::{
            AUTHORING_FRAME_RATE, EditorUiCamera, editor_ui_camera, editor_winit_settings,
            setup_editor_shell, sync_workspace_cameras,
        },
        workspaces::{
            AnimationWorkspaceCamera, EditorWorkspace, EditorWorkspacesPlugin,
            animation::setup_animation_workspace,
        },
    };
    use bevy::{camera::CameraOutputMode, render::view::Msaa, winit::UpdateMode};
    use engine::WorldViewCamera;
    use std::time::Duration;
    use world::CellCoord;

    #[test]
    fn anticipated_origin_keeps_remote_camera_coordinates_small() {
        let far = CellCoord {
            x: 1_000_000,
            z: -1_000_000,
        };
        assert_eq!(
            anticipated_render_origin(CellCoord::ZERO, far, Some(8)),
            far
        );
    }
    #[test]
    fn editor_frame_pacing_is_reactive_and_input_driven() {
        let UpdateMode::Reactive {
            wait,
            react_to_device_events,
            react_to_user_events,
            react_to_window_events,
            ..
        } = editor_winit_settings().focused_mode
        else {
            panic!("the editor should use reactive frame pacing");
        };
        assert_eq!(wait, Duration::from_secs_f64(1.0 / AUTHORING_FRAME_RATE));
        assert!(react_to_device_events);
        assert!(react_to_user_events);
        assert!(react_to_window_events);
    }
    #[test]
    fn workspace_switch_activates_only_its_viewport_camera() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_plugins(EditorWorkspacesPlugin)
            .add_systems(Update, sync_workspace_cameras);
        let world_camera = app
            .world_mut()
            .spawn((Camera::default(), WorldViewCamera))
            .id();
        let fallback_camera = app
            .world_mut()
            .spawn((Camera::default(), AnimationWorkspaceCamera))
            .id();

        app.update();
        assert!(app.world().get::<Camera>(world_camera).unwrap().is_active);
        assert!(
            !app.world()
                .get::<Camera>(fallback_camera)
                .unwrap()
                .is_active
        );

        app.world_mut()
            .resource_mut::<NextState<EditorWorkspace>>()
            .set(EditorWorkspace::Animation);
        app.update();
        assert!(!app.world().get::<Camera>(world_camera).unwrap().is_active);
        assert!(
            app.world()
                .get::<Camera>(fallback_camera)
                .unwrap()
                .is_active
        );
    }
    #[test]
    fn editor_ui_camera_clears_only_its_transparent_intermediate_target() {
        let camera = editor_ui_camera();
        assert!(matches!(
            camera.clear_color,
            ClearColorConfig::Custom(color) if color == Color::NONE
        ));
        assert!(matches!(
            camera.output_mode,
            CameraOutputMode::Write {
                blend_state: Some(_),
                clear_color: ClearColorConfig::None,
            }
        ));
    }
    #[test]
    fn workspace_camera_stack_uses_one_msaa_mode() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_plugins(EditorWorkspacesPlugin)
            .add_systems(
                Startup,
                (
                    setup_editor_shell,
                    setup_world_workspace,
                    setup_animation_workspace,
                ),
            );
        app.update();

        let world = app.world_mut();
        let world_msaa = *world
            .query_filtered::<&Msaa, With<WorldViewCamera>>()
            .single(world)
            .unwrap();
        let fallback_msaa = *world
            .query_filtered::<&Msaa, With<AnimationWorkspaceCamera>>()
            .single(world)
            .unwrap();
        let ui_msaa = *world
            .query_filtered::<&Msaa, With<EditorUiCamera>>()
            .single(world)
            .unwrap();
        assert_eq!(world_msaa, Msaa::Off);
        assert_eq!(fallback_msaa, world_msaa);
        assert_eq!(ui_msaa, world_msaa);
    }
    #[test]
    fn close_camera_zoom_advances_instead_of_stalling_at_the_clamp() {
        let (distance, forward_travel) = camera_zoom_step(MIN_CAMERA_DISTANCE, 0.12);
        assert_eq!(distance, MIN_CAMERA_DISTANCE);
        assert!(forward_travel > 0.5);

        let (distance, forward_travel) = camera_zoom_step(MIN_CAMERA_DISTANCE, -0.12);
        assert!(distance > MIN_CAMERA_DISTANCE);
        assert_eq!(forward_travel, 0.0);
    }
    #[test]
    fn navigation_sensitivity_has_a_close_range_floor() {
        assert_eq!(camera_navigation_scale(1.0), MIN_CAMERA_NAVIGATION_SCALE);
        assert_eq!(camera_navigation_scale(20.0), 20.0);
    }
}
