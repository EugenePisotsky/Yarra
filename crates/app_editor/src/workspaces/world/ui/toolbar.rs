//! The World toolbar: save, publish, undo and redo, the gizmo mode, and what is unsaved,
//! saving or in conflict.
use crate::{
    domain_editing::SourceWorkingSets,
    editing::{EditorHistory, EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft},
    environment_paint::EnvironmentPaintState,
    project_store::ProjectEditorStore,
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::EditorUiFrame,
    tools::{EditorToolRegistry, OBJECT_TOOL},
    vegetation_authoring::VegetationAuthoringState,
    workspaces::{EditorWorkspace, presets::PresetAuthoringState},
};
use bevy::{
    gizmos::transform_gizmo::{TransformGizmoMode, TransformGizmoSettings},
    prelude::*,
};
use bevy_egui::egui;

#[allow(clippy::too_many_arguments)] // Bevy system parameters.
pub(crate) fn world_toolbar(
    mut frame: ResMut<EditorUiFrame>,
    presets: Res<PresetAuthoringState>,
    paint: Res<EnvironmentPaintState>,
    vegetation: Res<VegetationAuthoringState>,
    publication: Res<RuntimePublicationState>,
    project: Res<ProjectEditorStore>,
    selection: Res<EditorSelection>,
    tools: Res<EditorToolRegistry>,
    mut save: ResMut<EditorSaveCoordinator>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut dense_domains: ResMut<SourceWorkingSets>,
    mut history: ResMut<EditorHistory>,
    mut transform_draft: ResMut<TransformInspectorDraft>,
    mut gizmo_settings: ResMut<TransformGizmoSettings>,
) {
    let Some(viewport_ui) = frame.0.as_mut() else {
        return;
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
}
