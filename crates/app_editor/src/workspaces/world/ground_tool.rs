//! What the ground tools (areas, roads, environment paint) read from the viewport: the pointer,
//! the camera, resident terrain, and the states that pause pointer editing.
use bevy::{ecs::system::SystemParam, prelude::*, window::PrimaryWindow};
use engine::{StreamedTerrainSurface, WorldOrigin, WorldViewCamera};

use crate::{
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::EditorInputCapture,
    tools::{EditorToolDescriptor, EditorToolRegistry},
    workspaces::EditorWorkspace,
};

/// Held for camera moves and shortcuts, so a ground tool leaves the pointer alone meanwhile.
const MODIFIERS: [KeyCode; 6] = [
    KeyCode::SuperLeft,
    KeyCode::SuperRight,
    KeyCode::ControlLeft,
    KeyCode::ControlRight,
    KeyCode::AltLeft,
    KeyCode::AltRight,
];

#[derive(SystemParam)]
pub(crate) struct GroundToolInput<'w, 's> {
    pub(crate) window: Single<'w, 's, &'static Window, With<PrimaryWindow>>,
    pub(crate) camera:
        Single<'w, 's, (&'static Camera, &'static GlobalTransform), With<WorldViewCamera>>,
    pub(crate) terrain: Query<'w, 's, (Entity, &'static StreamedTerrainSurface)>,
    pub(crate) origin: Res<'w, WorldOrigin>,
    pub(crate) capture: Res<'w, EditorInputCapture>,
    pub(crate) buttons: Res<'w, ButtonInput<MouseButton>>,
    pub(crate) keys: Res<'w, ButtonInput<KeyCode>>,
    workspace: Res<'w, State<EditorWorkspace>>,
    tools: Res<'w, EditorToolRegistry>,
    publication: Res<'w, RuntimePublicationState>,
    save: Res<'w, EditorSaveCoordinator>,
}

impl GroundToolInput<'_, '_> {
    /// Whether `tool` is the active tool of the World workspace, which is showing.
    pub(crate) fn active(&self, tool: &EditorToolDescriptor) -> bool {
        *self.workspace.get() == EditorWorkspace::World
            && self
                .tools
                .active(EditorWorkspace::World)
                .is_some_and(|active| active.id == tool.id)
    }

    /// The pointer belongs to egui or the camera (right or middle button, a modifier key), or
    /// a save or publication is running.
    pub(crate) fn paused(&self) -> bool {
        self.capture.wants_pointer
            || self.buttons.pressed(MouseButton::Right)
            || self.buttons.pressed(MouseButton::Middle)
            || MODIFIERS.iter().any(|key| self.keys.pressed(*key))
            || self.publication.active()
            || self.save.active()
    }
}
