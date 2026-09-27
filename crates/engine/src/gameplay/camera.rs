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
        app.add_message::<crate::WorldStartAdopted>()
            .add_systems(Startup, setup_camera)
            .add_systems(
                Update,
                (
                    frame_adopted_start.in_set(GameplaySystems::CameraInput),
                    update_camera_transform.in_set(GameplaySystems::CameraFollow),
                ),
            );
    }
}

/// Only MetalFX Temporal consumes prepass depth and motion, and the game adds the
/// prepass while Temporal is active. Otherwise it is pure cost: alpha-tested leaves
/// are drawn twice and Bevy 0.19 copies the full-resolution depth texture every
/// frame. On M2 Max with 4× MSAA it cost ~0.7 ms on the grass route at 2560×1440 and
/// ~1.0 ms of GPU time facing the forest in third person at 3456×1942.
pub const GAME_DEPTH_PREPASS_ENABLED: bool = false;

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

/// The orbit and haze that frame a start view, or the defaults without one.
fn start_rig(view: Option<&world::WorldViewBookmark>) -> (CameraRig, WorldEnvironmentCamera) {
    let Some(view) = view else {
        return (
            CameraRig {
                yaw: 45.0_f32.to_radians(),
                target_yaw: 45.0_f32.to_radians(),
                distance: CAMERA_DEFAULT_DISTANCE,
                target_distance: CAMERA_DEFAULT_DISTANCE,
                pitch_offset: 0.0,
            },
            WorldEnvironmentCamera::default(),
        );
    };
    let yaw = view.yaw_degrees.to_radians();
    (
        CameraRig {
            yaw,
            target_yaw: yaw,
            distance: view.distance,
            target_distance: view.distance,
            pitch_offset: view.pitch_degrees.to_radians()
                - (CAMERA_NEAR_PITCH
                    + (CAMERA_FAR_PITCH - CAMERA_NEAR_PITCH)
                        * normalized_camera_zoom(view.distance)),
        },
        WorldEnvironmentCamera::with_visibility(view.fog_visibility),
    )
}

/// Frames the world's own start once it arrives with the runtime (see `WorldStartAdopted`).
fn frame_adopted_start(
    mut adopted: MessageReader<crate::WorldStartAdopted>,
    mut camera: Query<(&mut CameraRig, &mut atmosphere::WorldEnvironmentView), With<MainCamera>>,
) {
    let Some(crate::WorldStartAdopted(view)) = adopted.read().last() else {
        return;
    };
    for (mut rig, mut environment) in &mut camera {
        (*rig, _) = start_rig(Some(view));
        environment.visibility_override = Some(view.fog_visibility);
    }
}

fn setup_camera(mut commands: Commands, start_view: Res<WorldStartView>) {
    let start = start_view
        .0
        .as_ref()
        .map_or(Vec3::ZERO, |v| Vec3::from_array(v.position));
    let (camera_rig, environment) = start_rig(start_view.0.as_ref());
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
