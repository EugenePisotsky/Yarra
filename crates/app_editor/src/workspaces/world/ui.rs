//! World toolbar and floating-window composition. Visibility does not activate authoring tools.
use crate::{
    derived_jobs::{DerivedArtifactStore, DerivedJobScheduler},
    domain_editing::DenseDomainWorkingSets,
    editing::{EditorHistory, EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft},
    journal::EditorJournalStatus,
    navigation::ProjectNavigationStore,
    overview::OverviewState,
    preview::{EditorPreviewMode, PreviewModeState, PreviewRuntimeDiagnostics},
    project_store::ProjectEditorStore,
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::{EditorUiFrame, EditorWindowDescriptor, EditorWindowId, EditorWindowRegistry},
    tools::{EditorToolRegistry, OBJECT_TOOL},
    vegetation_authoring::VegetationAuthoringState,
    workspaces::{
        EditorWorkspace,
        world::{
            camera::EditorCameraFocusRequest,
            objects::EditorObjectPalette,
            ui::{
                assets::draw_asset_browser, diagnostics::draw_world_diagnostics,
                hierarchy::draw_world_hierarchy, inspector::draw_context_inspector,
                navigator::draw_navigator,
            },
        },
    },
};
use bevy::{
    diagnostic::DiagnosticsStore,
    ecs::system::SystemParam,
    gizmos::transform_gizmo::{TransformGizmoMode, TransformGizmoSettings},
    prelude::*,
};
use bevy_egui::egui;
use engine::{ActiveWorldSpace, StreamingStats, WorldCatalog, WorldOrigin, WorldViewpoint};

mod assets;
mod diagnostics;
mod hierarchy;
mod inspector;
mod navigator;

pub(crate) const WORLD_WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
    id: EditorWindowId("world.hierarchy"),
    workspace: EditorWorkspace::World,
    label: "World",
    default_open: true,
};
pub(crate) const INSPECTOR_WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
    id: EditorWindowId("world.inspector"),
    workspace: EditorWorkspace::World,
    label: "Inspector",
    default_open: true,
};
pub(crate) const ASSETS_WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
    id: EditorWindowId("world.assets"),
    workspace: EditorWorkspace::World,
    label: "Assets",
    default_open: false,
};
pub(crate) const NAVIGATOR_WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
    id: EditorWindowId("world.navigator"),
    workspace: EditorWorkspace::World,
    label: "Navigator",
    default_open: false,
};
pub(crate) const DIAGNOSTICS_WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
    id: EditorWindowId("world.diagnostics"),
    workspace: EditorWorkspace::World,
    label: "Diagnostics",
    default_open: false,
};

#[derive(Resource, Default)]
pub(crate) struct WorldWorkspaceUiState {
    visible_assets_search: String,
}

#[derive(SystemParam)]
pub(crate) struct WorldWorkspaceUiResources<'w> {
    diagnostics: Res<'w, DiagnosticsStore>,
    catalog: Res<'w, WorldCatalog>,
    viewpoint: Res<'w, WorldViewpoint>,
    origin: Res<'w, WorldOrigin>,
    stats: Res<'w, StreamingStats>,
    derived_jobs: Res<'w, DerivedJobScheduler>,
    derived_artifacts: Res<'w, DerivedArtifactStore>,
    dense_domains: ResMut<'w, DenseDomainWorkingSets>,
    vegetation: ResMut<'w, VegetationAuthoringState>,
    roads: ResMut<'w, crate::road_authoring::RoadToolState>,
    paint: ResMut<'w, crate::environment_paint::EnvironmentPaintState>,
    layer_browser: ResMut<'w, crate::environment_paint::EnvironmentLayerBrowser>,
    presets: ResMut<'w, crate::workspaces::presets::PresetAuthoringState>,
    next_workspace: ResMut<'w, NextState<EditorWorkspace>>,
    environment_preview: Res<'w, crate::environment_paint::EnvironmentPreview>,
    journal: Res<'w, EditorJournalStatus>,
    navigation: ResMut<'w, ProjectNavigationStore>,
    overview: Res<'w, OverviewState>,
    preview: ResMut<'w, PreviewModeState>,
    preview_runtime: Res<'w, PreviewRuntimeDiagnostics>,
    publication: ResMut<'w, RuntimePublicationState>,
    save: ResMut<'w, EditorSaveCoordinator>,
    project: ResMut<'w, ProjectEditorStore>,
    selection: ResMut<'w, EditorSelection>,
    objects: ResMut<'w, EditorObjectWorkingSet>,
    history: ResMut<'w, EditorHistory>,
    object_palette: ResMut<'w, EditorObjectPalette>,
    gizmo_settings: ResMut<'w, TransformGizmoSettings>,
    transform_draft: ResMut<'w, TransformInspectorDraft>,
    focus_request: ResMut<'w, EditorCameraFocusRequest>,
    active_space: ResMut<'w, ActiveWorldSpace>,
    tools: ResMut<'w, EditorToolRegistry>,
    ui_state: ResMut<'w, WorldWorkspaceUiState>,
    windows: ResMut<'w, EditorWindowRegistry>,
}

pub(crate) fn world_workspace_ui(
    mut frame: ResMut<EditorUiFrame>,
    resources: WorldWorkspaceUiResources,
) -> Result {
    let WorldWorkspaceUiResources {
        diagnostics,
        catalog,
        viewpoint,
        origin,
        stats,
        derived_jobs,
        derived_artifacts,
        mut dense_domains,
        vegetation,
        mut roads,
        mut paint,
        mut layer_browser,
        mut presets,
        mut next_workspace,
        environment_preview,
        journal,
        mut navigation,
        overview,
        mut preview,
        preview_runtime,
        publication,
        mut save,
        mut project,
        mut selection,
        mut objects,
        mut history,
        mut object_palette,
        mut gizmo_settings,
        mut transform_draft,
        mut focus_request,
        mut active_space,
        mut tools,
        mut ui_state,
        mut windows,
    } = resources;
    let Some(viewport_ui) = frame.0.as_mut() else {
        return Ok(());
    };

    egui::Panel::top("editor_world_toolbar").show(viewport_ui, |ui| {
        if presets.dirty() {ui.colored_label(egui::Color32::YELLOW,"An unapplied preset draft is waiting in Presets. Apply or discard it before saving.");}
        ui.horizontal(|ui| {
            let remaining_changes = objects.dirty_count()
                + dense_domains.dirty_count()
                + vegetation.dirty_count();
            let has_dirty_source = remaining_changes > 0;
            let source_action_available = !paint.has_unapplied_changes() && !presets.dirty() && !save.active()
                && !project.save_in_flight()
                && !objects.saving()
                && !dense_domains.saving()
                && !dense_domains.gesture_active
                && dense_domains.atmospheres.gesture.is_none()
                && !vegetation.saving()
                && !objects.has_any_conflict()
                && !dense_domains.has_any_conflict()
                && !vegetation.has_conflict()
                && !publication.active()
                && project.write_error().is_none();
            let can_save = has_dirty_source && source_action_available;
            if ui
                .add_enabled(can_save, egui::Button::new("Save"))
                .on_hover_text("Save all local source changes (Cmd+S)")
                .clicked()
            {
                save.request_save();
            }
            let can_publish = (project.source_epoch() > 0 || has_dirty_source)
                && source_action_available;
            let publish_label = if has_dirty_source {
                "Save & Publish"
            } else {
                "Publish"
            };
            if ui
                .add_enabled(can_publish, egui::Button::new(publish_label))
                .on_hover_text(if has_dirty_source {
                    "Save every local source change, then cook, validate, publish, and adopt it"
                } else {
                    "Cook, validate, atomically publish, and adopt a new immutable runtime generation"
                })
                .clicked()
            {
                save.request_publish();
            }
            if ui
                .add_enabled(
                    history.undo_len() > 0
                        && !paint.has_unapplied_changes()
                        && !presets.dirty()
                        && !publication.active()
                        && !save.active()
                        && !objects.saving()
                        && !dense_domains.saving()
                        && !dense_domains.gesture_active
                        && dense_domains.atmospheres.gesture.is_none()
                        && !vegetation.saving()
                        && !objects.has_any_conflict()
                        && !dense_domains.has_any_conflict()
                        && !vegetation.has_conflict(),
                    egui::Button::new("Undo"),
                )
                .on_hover_text("Undo the last command (Cmd+Z)")
                .clicked()
            {
                history.undo(&mut objects, &mut dense_domains);
                transform_draft.sync(&selection, &objects);
            }
            if ui
                .add_enabled(
                    history.redo_len() > 0
                        && !paint.has_unapplied_changes()
                        && !presets.dirty()
                        && !publication.active()
                        && !save.active()
                        && !objects.saving()
                        && !dense_domains.saving()
                        && !dense_domains.gesture_active
                        && dense_domains.atmospheres.gesture.is_none()
                        && !vegetation.saving()
                        && !objects.has_any_conflict()
                        && !dense_domains.has_any_conflict()
                        && !vegetation.has_conflict(),
                    egui::Button::new("Redo"),
                )
                .on_hover_text("Redo the last command (Cmd+Shift+Z)")
                .clicked()
            {
                history.redo(&mut objects, &mut dense_domains);
                transform_draft.sync(&selection, &objects);
            }

            ui.separator();
            let object_tool_active = tools
                .active(EditorWorkspace::World)
                .is_some_and(|tool| tool.id == OBJECT_TOOL.id);
            ui.add_enabled_ui(object_tool_active, |ui| {
                ui.selectable_value(
                    &mut gizmo_settings.mode,
                    TransformGizmoMode::Translate,
                    "1 Move",
                );
                ui.selectable_value(
                    &mut gizmo_settings.mode,
                    TransformGizmoMode::Rotate,
                    "2 Yaw",
                );
                ui.selectable_value(
                    &mut gizmo_settings.mode,
                    TransformGizmoMode::Scale,
                    "3 Scale",
                );
            });
            ui.separator();
            egui::ComboBox::from_id_salt("world_preview_mode")
                .selected_text(format!("Preview: {}", preview.requested().label()))
                .show_ui(ui, |ui| {
                    for mode in EditorPreviewMode::ALL {
                        if ui
                            .selectable_label(preview.requested() == mode, mode.label())
                            .clicked()
                        {
                            preview.request(mode);
                            ui.close();
                        }
                    }
                });

            if save.active()
                || project.save_in_flight()
                || objects.saving()
                || dense_domains.saving()
                || vegetation.saving()
            {
                ui.separator();
                ui.spinner();
                ui.weak(if save.active() {
                    save.status(remaining_changes)
                } else {
                    "Saving source changes…".into()
                });
            } else if objects.dirty_count() > 0 {
                ui.separator();
                ui.menu_button(
                    egui::RichText::new(format!("{} unsaved", objects.dirty_count()))
                        .color(egui::Color32::YELLOW),
                    |ui| {
                        if ui.button("Discard all local changes").clicked() {
                            objects.discard_all(&mut history);
                            transform_draft.sync(&selection, &objects);
                            ui.close();
                        }
                    },
                );
            } else if dense_domains.dirty_count() > 0 {
                ui.separator();
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("{} environment change(s) unsaved", dense_domains.dirty_count()),
                );
            } else if vegetation.dirty_count() > 0 {
                ui.separator();
                ui.colored_label(egui::Color32::YELLOW, "Vegetation catalog unsaved");
            }
            if objects.has_any_conflict()
                || dense_domains.has_any_conflict()
                || vegetation.has_conflict()
            {
                ui.separator();
                ui.colored_label(egui::Color32::LIGHT_RED, "Source conflict");
            }
            if publication.active() {
                ui.separator();
                ui.spinner();
                ui.weak(publication.status());
            } else if let Some(error) = publication.failure() {
                ui.separator();
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!("Publication failed: {error}"),
                );
            }
        });
    });

    let context = viewport_ui.ctx().clone();
    let workspace_rect = viewport_ui.available_rect_before_wrap();
    let margin = 12.0;

    if windows.is_open(WORLD_WINDOW.id) {
        let mut open = true;
        egui::Window::new("World")
            .id(egui::Id::new(WORLD_WINDOW.id.0))
            .open(&mut open)
            .default_pos([
                workspace_rect.left() + margin,
                workspace_rect.top() + margin,
            ])
            .default_size([285.0, 560.0])
            .constrain_to(workspace_rect)
            .resizable(true)
            .show(&context, |ui| {
                draw_world_hierarchy(
                    ui,
                    &catalog,
                    &project,
                    &mut selection,
                    &mut objects,
                    &mut active_space,
                    &mut tools,
                    &mut ui_state,
                    &mut windows,
                );
            });
        windows.set_open(WORLD_WINDOW.id, open);
    }

    if windows.is_open(INSPECTOR_WINDOW.id) {
        let mut open = true;
        egui::Window::new("Inspector")
            .id(egui::Id::new(INSPECTOR_WINDOW.id.0))
            .open(&mut open)
            .default_pos([
                workspace_rect.right() - 332.0 - margin,
                workspace_rect.top() + margin,
            ])
            .default_size([332.0, 560.0])
            .constrain_to(workspace_rect)
            .resizable(true)
            .vscroll(true)
            .show(&context, |ui| {
                draw_context_inspector(
                    ui,
                    &catalog,
                    &viewpoint,
                    &mut dense_domains,
                    &project,
                    &mut selection,
                    &mut objects,
                    &mut history,
                    &mut transform_draft,
                    &mut focus_request,
                    &tools,
                    gizmo_settings.mode,
                    &mut roads,
                    &mut paint,
                    &mut layer_browser,
                    &environment_preview,
                    origin.space(),
                    vegetation.study_source().map(|(catalog, _, _)| catalog),
                    save.active()
                        || publication.active()
                        || project.save_in_flight()
                        || presets.dirty(),
                );
            });
        windows.set_open(INSPECTOR_WINDOW.id, open);
    }

    if windows.is_open(ASSETS_WINDOW.id) {
        let mut open = true;
        egui::Window::new("Assets")
            .id(egui::Id::new(ASSETS_WINDOW.id.0))
            .open(&mut open)
            .default_pos([workspace_rect.left() + 315.0, workspace_rect.top() + margin])
            .default_size([430.0, 480.0])
            .constrain_to(workspace_rect)
            .resizable(true)
            .show(&context, |ui| {
                draw_asset_browser(
                    ui,
                    &viewpoint,
                    &mut navigation,
                    &mut project,
                    &mut selection,
                    &mut objects,
                    &mut history,
                    &mut object_palette,
                    &mut transform_draft,
                    &mut tools,
                );
            });
        windows.set_open(ASSETS_WINDOW.id, open);
    }

    if windows.is_open(NAVIGATOR_WINDOW.id) {
        let mut open = true;
        egui::Window::new("Navigator")
            .id(egui::Id::new(NAVIGATOR_WINDOW.id.0))
            .open(&mut open)
            .default_pos([workspace_rect.left() + 315.0, workspace_rect.top() + 70.0])
            .default_size([390.0, 440.0])
            .constrain_to(workspace_rect)
            .resizable(true)
            .show(&context, |ui| {
                draw_navigator(
                    ui,
                    &catalog,
                    &viewpoint,
                    &overview,
                    &mut navigation,
                    &mut selection,
                    &mut objects,
                    &mut transform_draft,
                    &mut focus_request,
                    &mut tools,
                );
            });
        windows.set_open(NAVIGATOR_WINDOW.id, open);
    }

    if windows.is_open(DIAGNOSTICS_WINDOW.id) {
        let mut open = true;
        egui::Window::new("Diagnostics")
            .id(egui::Id::new(DIAGNOSTICS_WINDOW.id.0))
            .open(&mut open)
            .default_pos([
                workspace_rect.center().x - 220.0,
                workspace_rect.bottom() - 430.0,
            ])
            .default_size([440.0, 410.0])
            .constrain_to(workspace_rect)
            .resizable(true)
            .vscroll(true)
            .show(&context, |ui| {
                draw_world_diagnostics(
                    ui,
                    &diagnostics,
                    &catalog,
                    &viewpoint,
                    &origin,
                    &stats,
                    &derived_jobs,
                    &derived_artifacts,
                    &dense_domains,
                    &journal,
                    &navigation,
                    &overview,
                    &preview,
                    &preview_runtime,
                    &publication,
                    &project,
                    &objects,
                    &history,
                    &tools,
                );
            });
        windows.set_open(DIAGNOSTICS_WINDOW.id, open);
    }

    if let Some((space, preset)) = paint.preset_request.take() {
        presets.open(space, preset);
        next_workspace.set(EditorWorkspace::Presets);
    }
    if let Some((space, style)) = roads.style_request.take() {
        presets.open_road(space, style);
        next_workspace.set(EditorWorkspace::Presets);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{vegetation_authoring::VEGETATION_WINDOW, workspaces::EditorWorkspace};

    #[test]
    fn world_registers_two_primary_and_four_optional_windows() {
        let descriptors = [
            WORLD_WINDOW,
            INSPECTOR_WINDOW,
            ASSETS_WINDOW,
            NAVIGATOR_WINDOW,
            DIAGNOSTICS_WINDOW,
            VEGETATION_WINDOW,
        ];
        assert_eq!(descriptors.len(), 6);
        assert_eq!(
            descriptors
                .iter()
                .filter(|descriptor| descriptor.default_open)
                .map(|descriptor| descriptor.label)
                .collect::<Vec<_>>(),
            vec!["World", "Inspector"]
        );
        assert!(
            descriptors
                .iter()
                .all(|descriptor| descriptor.workspace == EditorWorkspace::World)
        );
    }
}
