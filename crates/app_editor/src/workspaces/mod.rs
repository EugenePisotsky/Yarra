//! Editor workspace identity and shell-level registration.
//!
//! Each substantial workflow owns its resources, viewport cameras, UI, and systems. Systems that
//! belong to a workspace use the active [`EditorWorkspace`] state as their run condition; project
//! workers and save completion remain shell-level services so in-flight work can finish while the
//! user visits another workspace.

mod animation;
mod world;
pub(crate) mod world_impl;
mod world_ui;

use std::collections::HashSet;

use bevy::prelude::*;

pub(crate) use animation::{AnimationWorkspaceCamera, AnimationWorkspacePlugin};
pub(crate) use world::WorldWorkspacePlugin;

#[derive(States, Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EditorWorkspace {
    #[default]
    World,
    Animation,
}

impl EditorWorkspace {
    pub(crate) const ALL: [Self; 2] = [Self::World, Self::Animation];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::World => "World",
            Self::Animation => "Animation",
        }
    }
}

pub(crate) struct EditorWorkspacesPlugin;

impl Plugin for EditorWorkspacesPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<EditorWorkspace>()
            .init_resource::<EditorFramePacing>()
            .add_systems(
                OnEnter(EditorWorkspace::Animation),
                request_full_rate_preview,
            )
            .add_systems(
                OnExit(EditorWorkspace::Animation),
                release_full_rate_preview,
            );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FramePacingOwner {
    AnimationWorkspace,
    WorldGameplayPreview,
}

#[derive(Resource, Default)]
pub(crate) struct EditorFramePacing {
    full_rate_owners: HashSet<FramePacingOwner>,
}

impl EditorFramePacing {
    pub(crate) fn full_rate_preview(&self) -> bool {
        !self.full_rate_owners.is_empty()
    }

    pub(crate) fn request(&mut self, owner: FramePacingOwner) {
        self.full_rate_owners.insert(owner);
    }

    pub(crate) fn release(&mut self, owner: FramePacingOwner) {
        self.full_rate_owners.remove(&owner);
    }
}

fn request_full_rate_preview(mut pacing: ResMut<EditorFramePacing>) {
    pacing.request(FramePacingOwner::AnimationWorkspace);
}

fn release_full_rate_preview(mut pacing: ResMut<EditorFramePacing>) {
    pacing.release(FramePacingOwner::AnimationWorkspace);
}

pub(crate) fn world_workspace_active(workspace: Res<State<EditorWorkspace>>) -> bool {
    *workspace.get() == EditorWorkspace::World
}

pub(crate) fn animation_workspace_active(workspace: Res<State<EditorWorkspace>>) -> bool {
    *workspace.get() == EditorWorkspace::Animation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_is_the_default_and_workspace_labels_are_stable() {
        assert_eq!(EditorWorkspace::default(), EditorWorkspace::World);
        assert_eq!(
            EditorWorkspace::ALL.map(EditorWorkspace::label),
            ["World", "Animation"]
        );
    }

    #[test]
    fn animation_workspace_owns_the_full_rate_preview_request() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_plugins(EditorWorkspacesPlugin);
        app.update();
        assert!(
            !app.world()
                .resource::<EditorFramePacing>()
                .full_rate_preview()
        );

        app.world_mut()
            .resource_mut::<NextState<EditorWorkspace>>()
            .set(EditorWorkspace::Animation);
        app.update();
        assert!(
            app.world()
                .resource::<EditorFramePacing>()
                .full_rate_preview()
        );

        app.world_mut()
            .resource_mut::<NextState<EditorWorkspace>>()
            .set(EditorWorkspace::World);
        app.update();
        assert!(
            !app.world()
                .resource::<EditorFramePacing>()
                .full_rate_preview()
        );
    }
}
