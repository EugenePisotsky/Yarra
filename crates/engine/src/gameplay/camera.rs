//! Orbit camera construction and follow. Device controls live in the input plugin.
use super::GameplaySystems;
use crate::{
    MsaaColorStorePolicy, WorldEnvironmentCamera, WorldRenderRoot, WorldStartView, WorldViewCamera,
    actor::CameraTarget,
};
use bevy::{core_pipeline::prepass::DepthPrepass, prelude::*, render::view::Msaa};

/// Spawns and follows the gameplay camera. Requires GameplayPlugin; input is optional.
pub struct GameCameraPlugin;
impl Plugin for GameCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_camera).add_systems(
            Update,
            update_camera_transform.in_set(GameplaySystems::CameraFollow),
        );
    }
}

/// The current mobile scene has no effects that consume prepass depth. In Bevy
/// 0.19 the pass also copies the full-resolution depth texture every frame.
/// Keep the audit baseline aligned with the normal game camera.
pub const GAME_DEPTH_PREPASS_ENABLED: bool = !cfg!(target_os = "ios");

pub(super) const CAMERA_MIN_DISTANCE: f32 = 4.0;
pub(super) const CAMERA_MAX_DISTANCE: f32 = 17.6;
pub(super) const CAMERA_ZOOM_REFERENCE_DISTANCE: f32 = 24.0;
pub(super) const CAMERA_DEFAULT_DISTANCE: f32 = CAMERA_MAX_DISTANCE;
pub(crate) const CAMERA_FOCUS_HEIGHT: f32 = 0.9;
pub(super) const CAMERA_NEAR_PITCH: f32 = 10.0_f32.to_radians();
pub(super) const CAMERA_FAR_PITCH: f32 = 55.0_f32.to_radians();

#[derive(Component)]
pub(super) struct MainCamera;

#[derive(Component)]
pub(super) struct CameraRig {
    pub(super) yaw: f32,
    pub(super) target_yaw: f32,
    pub(super) distance: f32,
    pub(super) target_distance: f32,
    pub(super) pitch_offset: f32,
}

fn setup_camera(mut commands: Commands, start_view: Res<WorldStartView>) {
    let start = start_view
        .0
        .as_ref()
        .map_or(Vec3::ZERO, |v| Vec3::from_array(v.position));
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
