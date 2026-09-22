//! Vegetation study workspace composition.
use crate::{
    shell::EditorUiSet,
    workspaces::{
        EditorWorkspace,
        vegetation::{
            capture::capture,
            lifecycle::{enter, leave},
            state::StudyState,
            study::StudyLaunch,
            ui::ui,
            viewport::{setup, sync, sync_scale_figure},
        },
    },
};
use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;
use std::path::PathBuf;

pub(crate) use viewport::VegetationWorkspaceCamera;
mod capture;
mod comparison;
mod ground;
mod ground_treatment;
mod lifecycle;
mod references;
mod stage;
mod state;
mod study;
mod ui;
mod viewport;

pub(crate) struct VegetationWorkspacePlugin {
    pub runtime_database: PathBuf,
}

impl Plugin for VegetationWorkspacePlugin {
    fn build(&self, app: &mut App) {
        ground_treatment::register(app);
        let launch = StudyLaunch::parse(std::env::args().skip(1)).unwrap_or_else(|e| {
            eprintln!("Vegetation study: {e}");
            std::process::exit(2);
        });
        app.insert_resource(StudyState::new(launch))
            .insert_resource(ground::GroundDatabase(self.runtime_database.clone()))
            .init_resource::<ground::StudyGroundAssets>()
            .add_systems(Startup, setup)
            .add_systems(Startup, ground::setup.after(setup))
            .add_systems(
                PreUpdate,
                references::configure_texture_limit
                    .after(bevy_egui::EguiPreUpdateSet::ProcessInput)
                    .before(bevy_egui::EguiPreUpdateSet::BeginPass),
            )
            .add_systems(
                Update,
                ground::sync.run_if(in_state(EditorWorkspace::Vegetation)),
            )
            .add_systems(OnEnter(EditorWorkspace::Vegetation), enter)
            .add_systems(OnExit(EditorWorkspace::Vegetation), leave)
            .add_systems(Update, sync.run_if(in_state(EditorWorkspace::Vegetation)))
            .add_systems(
                PostUpdate,
                sync_scale_figure
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            )
            .add_systems(
                PostUpdate,
                capture
                    .after(sync_scale_figure)
                    .before(bevy_egui::EguiPostUpdateSet::EndPass)
                    .run_if(in_state(EditorWorkspace::Vegetation)),
            )
            .add_systems(
                EguiPrimaryContextPass,
                ui.run_if(in_state(EditorWorkspace::Vegetation))
                    .in_set(EditorUiSet::Workspace),
            );
    }
}
