//! Vegetation catalog authoring composition and shared editor entry points.
use crate::{
    shell::{EditorUiSet, EditorWindowDescriptor, EditorWindowId},
    vegetation_authoring::{
        model::adopt_project_catalog, preview::sync_live_preview, ui::vegetation_authoring_ui,
    },
    workspaces::{EditorWorkspace, world_workspace_active},
};
use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;
use vegetation::VegetationScene;
use vegetation_render::{VegetationRenderPlugin, VegetationSceneState};

pub(crate) use model::{VegetationAuthoringState, process_vegetation_save_completion};
pub(crate) use species::draw_population_colors;
pub(crate) use ui::draw_vegetation_authoring;
mod curve;
mod model;
mod population;
mod preview;
mod species;
mod ui;
mod widgets;

pub(crate) const VEGETATION_WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
    id: EditorWindowId("world.vegetation"),
    workspace: EditorWorkspace::World,
    label: "Vegetation",
    default_open: false,
};
pub(crate) struct VegetationAuthoringPlugin;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct VegetationPreviewSync;
impl Plugin for VegetationAuthoringPlugin {
    fn build(&self, app: &mut App) {
        // The editor explicitly opts into render diagnostics; the renderer itself is reusable
        // by game compositions that omit instrumentation.
        #[cfg(not(target_os = "ios"))]
        app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin);
        app.add_plugins(VegetationRenderPlugin)
            .init_resource::<VegetationAuthoringState>()
            .insert_resource(
                VegetationSceneState::new(VegetationScene {
                    catalog: vegetation::fixtures::reference_catalog(),
                    pages: Vec::new(),
                })
                .expect("the empty vegetation authoring scene is valid"),
            )
            .add_systems(
                Update,
                (adopt_project_catalog, sync_live_preview)
                    .chain()
                    .in_set(VegetationPreviewSync)
                    .after(engine::WorldStreamingSystems),
            )
            .add_systems(
                EguiPrimaryContextPass,
                vegetation_authoring_ui
                    .run_if(world_workspace_active)
                    .in_set(EditorUiSet::Workspace),
            );
    }
}
