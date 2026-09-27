//! Explicit launch override shared by game, editor and scripted landscape tests.
use bevy::prelude::*;
use world::WorldViewBookmark;

/// Culling distance of world views. Depth is infinite reverse-Z, so this only decides what is
/// drawn: kilometre-scale terrain and the sea out to the default 20 km haze visibility.
pub const WORLD_VIEW_DISTANCE: f32 = 20_000.0;

#[derive(Resource, Default)]
pub struct WorldStartView(pub Option<WorldViewBookmark>);

/// Sent once when the runtime opens without an explicit start view and the world has its own.
/// The player and camera were spawned at the origin before it was known; they move there.
#[derive(Message, Clone, Debug)]
pub struct WorldStartAdopted(pub WorldViewBookmark);

impl WorldStartView {
    pub fn projection(&self) -> Projection {
        Projection::Perspective(PerspectiveProjection {
            far: self.0.as_ref().map_or(WORLD_VIEW_DISTANCE, |view| {
                WORLD_VIEW_DISTANCE.max(view.fog_visibility)
            }),
            ..default()
        })
    }

    pub fn load(path: Option<&std::path::Path>) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read start view {path:?}: {e}"))?;
        let view: WorldViewBookmark =
            ron::from_str(&text).map_err(|e| format!("invalid start view {path:?}: {e}"))?;
        view.validate()?;
        Ok(Self(Some(view)))
    }

    pub fn camera_at(view: &WorldViewBookmark, position: Vec3) -> Transform {
        let pitch = view.pitch_degrees.to_radians();
        let yaw = view.yaw_degrees.to_radians();
        let focus = position + Vec3::Y * crate::gameplay::CAMERA_FOCUS_HEIGHT;
        let offset = Vec3::new(
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            yaw.cos() * pitch.cos(),
        ) * view.distance;
        Transform::from_translation(focus + offset).looking_at(focus, Vec3::Y)
    }
}
