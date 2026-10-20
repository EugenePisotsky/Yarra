//! The World toolbar: save, publish, undo and redo, the gizmo mode, and what is unsaved,
//! saving or in conflict.
use crate::{
    activity::{SourceActions, SourceActivity},
    domain_editing::SourceWorkingSets,
    editing::{EditorHistory, EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft},
    environment_paint::EnvironmentPaintState,
    project_store::ProjectEditorStore,
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::EditorUiFrame,
    tools::{EditorToolRegistry, OBJECT_TOOL},
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
    let activity = SourceActivity::of(&project, &save, &publication, &objects, &dense_domains)
        .with_drafts(paint.has_unapplied_changes() || presets.dirty());
    egui::Panel::top("editor_world_toolbar").show(viewport_ui, |ui| {
        if presets.dirty() {
            ui.colored_label(
                egui::Color32::YELLOW,
                "An unapplied preset draft is waiting in Presets. \
                 Apply or discard it before saving.",
            );
        }

        ui.horizontal(|ui| {
            let remaining_changes = objects.dirty_count() + dense_domains.dirty_count();
            let stepped = SourceActions {
                activity,
                saved_before: project.source_epoch() > 0,
                history: &mut history,
                objects: &mut objects,
                dense: &mut dense_domains,
                save: &mut save,
            }
            .buttons(ui);

            if stepped {
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
                    format!(
                        "{} environment change(s) unsaved",
                        dense_domains.dirty_count()
                    ),
                );
            }

            if objects.has_any_conflict() || dense_domains.has_any_conflict() {
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
