//! The world view's camera, as systems look it up.
use crate::WorldViewCamera;
use bevy::{camera::primitives::Frustum, ecs::system::SystemParam, prelude::*};

/// The world view's camera: the game's, or the editor's World workspace camera, which is
/// inactive while another workspace shows.
#[derive(SystemParam)]
pub struct ActiveWorldView<'w, 's> {
    cameras: Query<
        'w,
        's,
        (
            Entity,
            &'static Camera,
            &'static GlobalTransform,
            &'static Frustum,
        ),
        With<WorldViewCamera>,
    >,
}

/// One world-view camera.
#[derive(Clone, Copy)]
pub struct WorldView<'a> {
    pub entity: Entity,
    pub camera: &'a Camera,
    pub transform: &'a GlobalTransform,
    pub frustum: &'a Frustum,
}

impl ActiveWorldView<'_, '_> {
    /// The camera while it renders.
    pub fn active(&self) -> Option<WorldView<'_>> {
        self.views().find(|view| view.camera.is_active)
    }

    /// The camera whether or not it renders: object LODs, wind and weather keep following it
    /// while another editor workspace shows.
    pub fn current(&self) -> Option<WorldView<'_>> {
        self.views().next()
    }

    fn views(&self) -> impl Iterator<Item = WorldView<'_>> {
        self.cameras
            .iter()
            .map(|(entity, camera, transform, frustum)| WorldView {
                entity,
                camera,
                transform,
                frustum,
            })
    }
}
