//! World workspace composition. Tools, window presentation and interaction systems have separate owners.
use crate::{
    domain_editing::{
        SourceWorkingSets, process_dense_save_completion, reconcile_dense_working_sets,
    },
    editing::{
        EditorHistory, EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft,
        process_project_save_completion,
    },
    saving::{EditorSaveCoordinator, drive_editor_save},
    shell::{EditorUiSet, EditorWindowRegistry},
    tools::{
        ENVIRONMENT_TOOL, EditorToolRegistry, OBJECT_TOOL, VEGETATION_TOOL, object_tool_active,
    },
    vegetation_authoring::{
        VEGETATION_WINDOW, VegetationAuthoringPlugin, process_vegetation_save_completion,
    },
    workspaces::{
        EditorWorkspace,
        world::{
            camera::{EditorCameraDrag, limit_detail_demand, setup_world_workspace},
            gizmo::{
                GizmoEditTransaction, apply_promoted_gizmo, configure_transform_gizmo,
                editor_gizmo_enabled, prepare_builtin_transform_gizmo_renderer,
            },
            input::{pick_source_object, suspend_world_workspace_interactions},
            objects::{
                EditorObjectPalette, reconcile_editor_selection, sync_cooked_visual_visibility,
                sync_promoted_editor_object,
            },
            overlay::{draw_editor_grid, draw_promoted_editor_object, draw_source_object_handles},
            ui::{
                ASSETS_WINDOW, DIAGNOSTICS_WINDOW, INSPECTOR_WINDOW, NAVIGATOR_WINDOW,
                WORLD_WINDOW, WorldWorkspaceUiState, assets_window, diagnostics_window,
                hierarchy_window, inspector_window, navigator_window, open_requested_presets,
                world_toolbar,
            },
        },
        world_workspace_active,
    },
};
use bevy::{
    gizmos::transform_gizmo::{TransformGizmoPlugin, TransformGizmoSystems},
    prelude::*,
};
use bevy_egui::EguiPrimaryContextPass;

pub(crate) use camera::{EditorCameraFocusRequest, update_editor_camera};
pub(crate) use ground_tool::GroundToolInput;
pub(crate) use input::handle_editor_shortcuts;
pub(crate) use overlay::EditorOverlayGizmos;
mod camera;
mod gizmo;
mod ground_tool;
mod input;
mod objects;
mod overlay;
mod ui;

pub(crate) struct WorldWorkspacePlugin;

impl Plugin for WorldWorkspacePlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<EditorToolRegistry>()
            .register(OBJECT_TOOL, true);
        app.world_mut()
            .resource_mut::<EditorToolRegistry>()
            .register(ENVIRONMENT_TOOL, false);
        app.world_mut()
            .resource_mut::<EditorToolRegistry>()
            .register(VEGETATION_TOOL, false);
        app.world_mut()
            .resource_mut::<EditorToolRegistry>()
            .register(crate::tools::ROAD_TOOL, false);
        app.world_mut()
            .resource_mut::<EditorToolRegistry>()
            .register(crate::tools::AREA_TOOL, false);
        for window in [
            WORLD_WINDOW,
            INSPECTOR_WINDOW,
            ASSETS_WINDOW,
            NAVIGATOR_WINDOW,
            DIAGNOSTICS_WINDOW,
            VEGETATION_WINDOW,
            crate::canopy::CANOPY_WINDOW,
            crate::atmosphere_authoring::WINDOW,
            crate::area_authoring::WINDOW,
        ] {
            app.world_mut()
                .resource_mut::<EditorWindowRegistry>()
                .register(window);
        }
        app.init_resource::<EditorCameraDrag>()
            .init_resource::<EditorCameraFocusRequest>()
            .init_resource::<EditorSelection>()
            .init_resource::<EditorObjectWorkingSet>()
            .init_resource::<EditorHistory>()
            .init_resource::<EditorObjectPalette>()
            .init_resource::<WorldWorkspaceUiState>()
            .init_resource::<TransformInspectorDraft>()
            .init_resource::<GizmoEditTransaction>()
            .init_resource::<SourceWorkingSets>()
            .init_resource::<EditorSaveCoordinator>()
            .init_gizmo_group::<EditorOverlayGizmos>()
            .add_plugins((
                TransformGizmoPlugin,
                VegetationAuthoringPlugin,
                crate::environment_paint::EnvironmentPaintPlugin,
                crate::road_authoring::RoadAuthoringPlugin,
                crate::canopy::EditorCanopyPlugin,
                crate::atmosphere_authoring::AtmosphereAuthoringPlugin,
                crate::area_authoring::AreaAuthoringPlugin,
            ))
            .configure_sets(
                PostUpdate,
                TransformGizmoSystems.run_if(editor_gizmo_enabled),
            )
            .add_systems(Startup, (setup_world_workspace, configure_transform_gizmo))
            .add_systems(PreUpdate, prepare_builtin_transform_gizmo_renderer)
            .add_systems(
                Update,
                reconcile_editor_selection.run_if(object_tool_active),
            )
            .add_systems(
                Update,
                (update_editor_camera, limit_detail_demand)
                    .chain()
                    .run_if(world_workspace_active),
            )
            .add_systems(
                Update,
                (
                    process_project_save_completion,
                    process_dense_save_completion,
                    process_vegetation_save_completion,
                    drive_editor_save,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                reconcile_dense_working_sets
                    .after(crate::project_store::ProjectStoreUpdate)
                    .run_if(world_workspace_active),
            )
            .add_systems(
                Update,
                handle_editor_shortcuts.run_if(world_workspace_active),
            )
            .add_systems(
                Update,
                pick_source_object
                    .run_if(world_workspace_active)
                    .run_if(object_tool_active),
            )
            .add_systems(
                PostUpdate,
                (sync_promoted_editor_object, sync_cooked_visual_visibility)
                    .chain()
                    .before(TransformGizmoSystems)
                    .run_if(world_workspace_active)
                    .run_if(object_tool_active),
            )
            .add_systems(
                PostUpdate,
                apply_promoted_gizmo
                    .after(TransformGizmoSystems)
                    .run_if(world_workspace_active)
                    .run_if(object_tool_active),
            )
            .add_systems(
                PostUpdate,
                draw_editor_grid
                    .after(apply_promoted_gizmo)
                    .run_if(world_workspace_active),
            )
            .add_systems(
                PostUpdate,
                (draw_source_object_handles, draw_promoted_editor_object)
                    .chain()
                    .after(apply_promoted_gizmo)
                    .run_if(world_workspace_active)
                    .run_if(object_tool_active),
            )
            .add_systems(
                OnExit(EditorWorkspace::World),
                suspend_world_workspace_interactions,
            )
            .add_systems(
                EguiPrimaryContextPass,
                (
                    world_toolbar,
                    hierarchy_window,
                    inspector_window,
                    assets_window,
                    navigator_window,
                    diagnostics_window,
                    open_requested_presets,
                )
                    .chain()
                    .run_if(world_workspace_active)
                    .in_set(EditorUiSet::Workspace),
            );
    }
}
