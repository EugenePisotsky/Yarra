//! World-authoring workspace registration.
//!
//! The implementation systems currently live in the crate composition module while they are
//! incrementally split by tool. This plugin is the ownership boundary: adding another workspace no
//! longer requires accumulating its resources, schedules, and UI in the editor shell.

use bevy::gizmos::transform_gizmo::{TransformGizmoPlugin, TransformGizmoSystems};
use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

use super::world_impl::{
    EditorCameraDrag, EditorCameraFocusRequest, EditorObjectPalette, EditorOverlayGizmos,
    GizmoEditTransaction, apply_promoted_gizmo, configure_transform_gizmo, draw_editor_grid,
    draw_promoted_editor_object, draw_source_object_handles, editor_gizmo_enabled,
    handle_editor_shortcuts, pick_source_object, prepare_builtin_transform_gizmo_renderer,
    reconcile_editor_selection, setup_world_workspace, suspend_world_workspace_interactions,
    sync_cooked_visual_visibility, sync_promoted_editor_object, update_editor_camera,
};
use super::world_ui::{
    ASSETS_WINDOW, DIAGNOSTICS_WINDOW, GROUND_COVER_WINDOW, GroundCoverCatalogUiState,
    INSPECTOR_WINDOW, NAVIGATOR_WINDOW, WORLD_WINDOW, WorldWorkspaceUiState, world_workspace_ui,
};
use super::{EditorWorkspace, world_workspace_active};
use crate::catalog_editing::{
    GroundCoverRegionWorkingSet, process_region_save_completion, reconcile_ground_cover_regions,
};
use crate::domain_editing::{
    DenseDomainWorkingSets, process_dense_save_completion, reconcile_dense_working_sets,
};
use crate::editing::{
    EditorHistory, EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft,
    process_project_save_completion,
};
use crate::ground_cover_catalog::{
    GroundCoverCatalogWorkingSet, process_catalog_save_completion, reconcile_ground_cover_catalog,
};
use crate::ground_cover_editing::{
    GroundCoverBrushGesture, GroundCoverToolState, cancel_ground_cover_brush,
    draw_ground_cover_brush, reconcile_ground_cover_tool_state, update_ground_cover_brush,
};
use crate::ground_cover_preview::GroundCoverPreviewPlugin;
use crate::overview::OverviewPlugin;
use crate::preview::{PreviewModesPlugin, authoring_preview_active};
use crate::saving::{EditorSaveCoordinator, drive_editor_save};
use crate::shell::{EditorUiSet, EditorWindowRegistry};
use crate::tools::{
    EditorToolRegistry, GROUND_COVER_TOOL, OBJECT_TOOL, TERRAIN_TOOL, object_tool_active,
};

pub(crate) struct WorldWorkspacePlugin;

impl Plugin for WorldWorkspacePlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<EditorToolRegistry>()
            .register(OBJECT_TOOL, true);
        app.world_mut()
            .resource_mut::<EditorToolRegistry>()
            .register(TERRAIN_TOOL, false);
        app.world_mut()
            .resource_mut::<EditorToolRegistry>()
            .register(GROUND_COVER_TOOL, false);
        for window in [
            WORLD_WINDOW,
            INSPECTOR_WINDOW,
            ASSETS_WINDOW,
            NAVIGATOR_WINDOW,
            DIAGNOSTICS_WINDOW,
            GROUND_COVER_WINDOW,
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
            .init_resource::<GroundCoverCatalogUiState>()
            .init_resource::<TransformInspectorDraft>()
            .init_resource::<GizmoEditTransaction>()
            .init_resource::<DenseDomainWorkingSets>()
            .init_resource::<GroundCoverRegionWorkingSet>()
            .init_resource::<GroundCoverCatalogWorkingSet>()
            .init_resource::<GroundCoverToolState>()
            .init_resource::<GroundCoverBrushGesture>()
            .init_resource::<EditorSaveCoordinator>()
            .init_gizmo_group::<EditorOverlayGizmos>()
            .add_plugins((
                TransformGizmoPlugin,
                OverviewPlugin,
                PreviewModesPlugin,
                GroundCoverPreviewPlugin,
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
            .add_systems(Update, update_editor_camera.run_if(world_workspace_active))
            .add_systems(
                Update,
                (
                    process_catalog_save_completion,
                    process_project_save_completion,
                    process_region_save_completion,
                    process_dense_save_completion,
                    drive_editor_save,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    reconcile_ground_cover_catalog,
                    reconcile_ground_cover_regions,
                    reconcile_dense_working_sets,
                )
                    .chain()
                    .run_if(world_workspace_active),
            )
            .add_systems(
                Update,
                handle_editor_shortcuts
                    .run_if(world_workspace_active)
                    .run_if(authoring_preview_active),
            )
            .add_systems(
                Update,
                pick_source_object
                    .run_if(world_workspace_active)
                    .run_if(authoring_preview_active)
                    .run_if(object_tool_active),
            )
            .add_systems(
                Update,
                (reconcile_ground_cover_tool_state, update_ground_cover_brush)
                    .chain()
                    .run_if(world_workspace_active)
                    .run_if(authoring_preview_active),
            )
            .add_systems(
                PostUpdate,
                (sync_promoted_editor_object, sync_cooked_visual_visibility)
                    .chain()
                    .before(TransformGizmoSystems)
                    .run_if(authoring_preview_active)
                    .run_if(object_tool_active),
            )
            .add_systems(
                PostUpdate,
                apply_promoted_gizmo
                    .after(TransformGizmoSystems)
                    .run_if(world_workspace_active)
                    .run_if(authoring_preview_active)
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
                draw_ground_cover_brush
                    .after(draw_editor_grid)
                    .run_if(world_workspace_active)
                    .run_if(authoring_preview_active),
            )
            .add_systems(
                PostUpdate,
                (draw_source_object_handles, draw_promoted_editor_object)
                    .chain()
                    .after(apply_promoted_gizmo)
                    .run_if(world_workspace_active)
                    .run_if(authoring_preview_active)
                    .run_if(object_tool_active),
            )
            .add_systems(
                OnExit(EditorWorkspace::World),
                (
                    suspend_world_workspace_interactions,
                    cancel_ground_cover_brush,
                ),
            )
            .add_systems(
                EguiPrimaryContextPass,
                world_workspace_ui
                    .run_if(world_workspace_active)
                    .in_set(EditorUiSet::Workspace),
            );
    }
}
