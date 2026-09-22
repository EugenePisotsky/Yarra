//! Project-object navigation and overview tile focus.
use crate::{
    editing::{EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft},
    navigation::ProjectNavigationStore,
    overview::{OverviewMode, OverviewState},
    tools::{EditorToolRegistry, OBJECT_TOOL},
    workspaces::{EditorWorkspace, world::camera::EditorCameraFocusRequest},
};
use bevy::prelude::*;
use bevy_egui::egui;
use engine::{WorldCatalog, WorldViewpoint};

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_navigator(
    ui: &mut egui::Ui,
    catalog: &WorldCatalog,
    viewpoint: &WorldViewpoint,
    overview: &OverviewState,
    navigation: &mut ProjectNavigationStore,
    selection: &mut EditorSelection,
    objects: &mut EditorObjectWorkingSet,
    transform_draft: &mut TransformInspectorDraft,
    focus_request: &mut EditorCameraFocusRequest,
    tools: &mut EditorToolRegistry,
) {
    ui.heading("Project objects");
    let mut search = navigation.outliner_search().to_owned();
    if ui
        .add(egui::TextEdit::singleline(&mut search).hint_text("Search project objects"))
        .changed()
    {
        navigation.search_outliner(search);
    }
    let records = navigation.outliner_records().to_vec();
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
                navigation.outliner_has_previous(),
                egui::Button::new("Previous"),
            )
            .clicked()
        {
            navigation.outliner_previous();
        }
        if ui
            .add_enabled(navigation.outliner_has_next(), egui::Button::new("Next"))
            .clicked()
        {
            navigation.outliner_next();
        }
    });

    if overview.mode() == OverviewMode::Overview
        && let Some(current) = viewpoint.position()
        && let Some(space) = catalog.world_space(current.space)
    {
        ui.separator();
        ui.heading("Overview tiles");
        let tiles = overview.tiles().to_vec();
        egui::Grid::new("overview_tile_jump_grid")
            .num_columns(5)
            .show(ui, |ui| {
                for (index, tile) in tiles.into_iter().enumerate() {
                    if ui.button(format!("{}, {}", tile.x, tile.z)).clicked() {
                        focus_request.0 =
                            Some(overview.focus_position(tile, current, space.cell_size));
                    }
                    if index % 5 == 4 {
                        ui.end_row();
                    }
                }
            });
    }
}
