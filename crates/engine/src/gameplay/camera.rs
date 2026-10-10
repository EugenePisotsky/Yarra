//! Orbit camera construction and follow. Device controls live in the input plugin.
use super::GameplaySystems;
use crate::{
    MsaaColorStorePolicy, WorldEnvironmentCamera, WorldRenderRoot, WorldStartView, WorldViewCamera,
    actor::CameraTarget,
};
use bevy::{prelude::*, render::view::Msaa};

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

/// The orbit that frames a start view, or the default without one. Haze visibility comes from
/// the world's atmosphere profile, not the view.
fn start_rig(view: Option<&world::WorldViewBookmark>) -> CameraRig {
    let Some(view) = view else {
        return CameraRig {
            yaw: 45.0_f32.to_radians(),
            target_yaw: 45.0_f32.to_radians(),
            distance: CAMERA_DEFAULT_DISTANCE,
            target_distance: CAMERA_DEFAULT_DISTANCE,
            pitch_offset: 0.0,
        };
    };
    let yaw = view.yaw_degrees.to_radians();
    CameraRig {
        yaw,
        target_yaw: yaw,
        distance: view.distance,
        target_distance: view.distance,
        pitch_offset: view.pitch_degrees.to_radians()
            - (CAMERA_NEAR_PITCH
                + (CAMERA_FAR_PITCH - CAMERA_NEAR_PITCH) * normalized_camera_zoom(view.distance)),
    }
}

/// Frames the world's own start once it arrives with the runtime (see `WorldStartAdopted`).
fn frame_adopted_start(
    mut adopted: MessageReader<crate::WorldStartAdopted>,
    mut camera: Query<&mut CameraRig, With<MainCamera>>,
) {
    let Some(crate::WorldStartAdopted(view)) = adopted.read().last() else {
        return;
    };
    for mut rig in &mut camera {
        *rig = start_rig(Some(view));
    }
}

fn setup_camera(mut commands: Commands, start_view: Res<WorldStartView>) {
    let start = start_view
        .0
        .as_ref()
        .map_or(Vec3::ZERO, |v| Vec3::from_array(v.position));
    let camera_rig = start_rig(start_view.0.as_ref());
    commands.spawn((
        Camera3d::default(),
        crate::WORLD_TONEMAPPING,
        start_view.projection(),
        WorldEnvironmentCamera::default(),
        Msaa::Sample4,
        MsaaColorStorePolicy::Automatic,
        camera_transform(start, &camera_rig),
        camera_rig,
        MainCamera,
        WorldViewCamera,
        WorldRenderRoot,
        Name::new("Main camera"),
    ));
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
    let focus = object_position + Vec3::Y * CAMERA_FOCUS_HEIGHT;
    orbit_transform(focus, rig.yaw, pitch, rig.distance)
}

/// A camera `distance` metres from `focus` looking at it, `pitch` radians above it and `yaw`
/// radians round it (0 on its +Z side). The game, start views and editor cameras orbit so.
pub fn orbit_transform(focus: Vec3, yaw: f32, pitch: f32, distance: f32) -> Transform {
    let horizontal = distance * pitch.cos();
    let offset = Vec3::new(
        yaw.sin() * horizontal,
        distance * pitch.sin(),
        yaw.cos() * horizontal,
    );
    Transform::from_translation(focus + offset).looking_at(focus, Vec3::Y)
}
