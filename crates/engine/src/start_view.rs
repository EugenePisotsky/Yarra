//! Explicit launch override shared by game, editor and scripted landscape tests.
use bevy::prelude::*;
use world::WorldViewBookmark;

#[derive(Resource, Default)]
pub struct WorldStartView(pub Option<WorldViewBookmark>);

impl WorldStartView {
    pub fn projection(&self) -> Projection {
        let mut perspective = PerspectiveProjection::default();
        if let Some(view) = &self.0 {
            perspective.far = perspective.far.max(view.fog_visibility);
        }
        Projection::Perspective(perspective)
    }

    pub fn from_args() -> Result<Self, String> {
        let mut args = std::env::args_os();
        let Some(_) = args.find(|arg| arg == "--start-view") else {
            return Ok(Self::default());
        };
        let path = args
            .next()
            .ok_or("--start-view requires a RON viewpoint file")?;
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read start view {path:?}: {e}"))?;
        let view: WorldViewBookmark =
            ron::from_str(&text).map_err(|e| format!("invalid start view {path:?}: {e}"))?;
        view.validate()?;
        Ok(Self(Some(view)))
    }

    pub fn camera_at(view: &WorldViewBookmark, position: Vec3) -> Transform {
        let pitch = view.pitch_degrees.to_radians();
        let yaw = view.yaw_degrees.to_radians();
        let focus = position + Vec3::Y * super::CAMERA_FOCUS_HEIGHT;
        let offset = Vec3::new(
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            yaw.cos() * pitch.cos(),
        ) * view.distance;
        Transform::from_translation(focus + offset).looking_at(focus, Vec3::Y)
    }
}
