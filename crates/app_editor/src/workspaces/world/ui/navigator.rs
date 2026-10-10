//! The Navigator window: the outliner of placed objects.
use crate::{
    editing::{EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft},
    listings::ProjectListings,
    tools::{EditorToolRegistry, OBJECT_TOOL},
    workspaces::EditorWorkspace,
};
use bevy::prelude::*;
use bevy_egui::egui;

pub(super) fn draw_navigator(
    ui: &mut egui::Ui,
    listings: &mut ProjectListings,
    selection: &mut EditorSelection,
    objects: &mut EditorObjectWorkingSet,
    transform_draft: &mut TransformInspectorDraft,
    tools: &mut EditorToolRegistry,
) {
    ui.heading("Project objects");
    let mut search = listings.outliner_search().to_owned();
    if ui
        .add(egui::TextEdit::singleline(&mut search).hint_text("Search project objects"))
        .changed()
    {
        listings.search_outliner(search);
    }
    let records = listings.outliner_records().to_vec();
    egui::ScrollArea::vertical()
        .id_salt("project_object_outliner")
        .max_height(190.0)
        .show(ui, |ui| {
            for record in records {
                let is_selected = selection.contains(record.object.id);
                let label = format!(
                    "{} · {}, {}",
                    record.definition.display_name,
                    record.object.owner_cell.x,
                    record.object.owner_cell.z
                );
                if ui.selectable_label(is_selected, label).clicked() {
                    tools.set_active(EditorWorkspace::World, OBJECT_TOOL.id);
                    let additive = ui.input(|input| {
                        input.modifiers.shift || input.modifiers.command || input.modifiers.ctrl
                    });
                    if additive {
                        selection.toggle(record, objects);
                    } else {
                        selection.select(record, objects);
                    }
                    transform_draft.sync(selection, objects);
                }
            }
        });
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                listings.outliner_has_previous(),
                egui::Button::new("Previous"),
            )
            .clicked()
        {
            listings.outliner_previous();
        }
        if ui
            .add_enabled(listings.outliner_has_next(), egui::Button::new("Next"))
            .clicked()
        {
            listings.outliner_next();
        }
    });
}
