//! Paged asset definitions and undoable placement at the viewpoint.
use crate::{
    editing::{EditorHistory, EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft},
    navigation::ProjectNavigationStore,
    project_store::ProjectEditorStore,
    tools::{EditorToolRegistry, OBJECT_TOOL},
    workspaces::{
        EditorWorkspace,
        world::objects::{EditorObjectPalette, create_palette_object},
    },
};
use bevy::prelude::*;
use bevy_egui::egui;
use engine::WorldViewpoint;

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_asset_browser(
    ui: &mut egui::Ui,
    viewpoint: &WorldViewpoint,
    navigation: &mut ProjectNavigationStore,
    project: &mut ProjectEditorStore,
    selection: &mut EditorSelection,
    objects: &mut EditorObjectWorkingSet,
    history: &mut EditorHistory,
    object_palette: &mut EditorObjectPalette,
    transform_draft: &mut TransformInspectorDraft,
    tools: &mut EditorToolRegistry,
) {
    let mut search = navigation.palette_search().to_owned();
    if ui
        .add(egui::TextEdit::singleline(&mut search).hint_text("Search asset definitions"))
        .changed()
    {
        navigation.search_palette(search);
    }
    let palette_records = navigation.palette_records().to_vec();
    egui::ScrollArea::vertical()
        .id_salt("object_definition_palette")
        .max_height((ui.available_height() - 76.0).max(140.0))
        .show(ui, |ui| {
            for record in &palette_records {
                let selected = object_palette.selected == Some(record.definition.id);
                if ui
                    .selectable_label(
                        selected,
                        format!(
                            "{}\n{}",
                            record.definition.display_name, record.definition.key
                        ),
                    )
                    .clicked()
                {
                    object_palette.selected = Some(record.definition.id);
                }
            }
        });
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                navigation.palette_has_previous(),
                egui::Button::new("Previous"),
            )
            .clicked()
        {
            navigation.palette_previous();
        }
        if ui
            .add_enabled(navigation.palette_has_next(), egui::Button::new("Next"))
            .clicked()
        {
            navigation.palette_next();
        }
        ui.weak(format!("{} on page", palette_records.len()));
    });

    let viewpoint_position = viewpoint.position();
    let source_cell_available = viewpoint_position.is_some_and(|position| {
        project
            .cells()
            .iter()
            .any(|cell| cell.space == position.space && cell.cell == position.cell)
    });
    let selected_definition = object_palette.selected.and_then(|definition| {
        palette_records
            .iter()
            .find(|record| record.definition.id == definition)
            .cloned()
    });
    if ui
        .add_enabled(
            selected_definition.is_some()
                && source_cell_available
                && !objects.saving()
                && !objects.has_any_conflict(),
            egui::Button::new("Add at viewpoint"),
        )
        .clicked()
        && let (Some(palette_record), Some(position)) = (selected_definition, viewpoint_position)
    {
        create_palette_object(palette_record, position, selection, objects, history);
        transform_draft.sync(selection, objects);
        tools.set_active(EditorWorkspace::World, OBJECT_TOOL.id);
    }
    if viewpoint_position.is_some() && !source_cell_available {
        ui.small("The viewpoint cell has no loaded authoring source cell.");
    } else {
        ui.small("Placement is an undoable local command; Cmd+S saves it.");
    }
}
