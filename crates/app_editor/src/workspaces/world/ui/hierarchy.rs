//! Active world/tool selection and the bounded visible-object list.
use crate::{
    editing::{EditorObjectWorkingSet, EditorSelection},
    project_store::{ProjectEditorStore, ProjectQueryWindow},
    shell::EditorWindowRegistry,
    tools::{ENVIRONMENT_TOOL, EditorToolRegistry, OBJECT_TOOL, ROAD_TOOL, VEGETATION_TOOL},
    vegetation_authoring::VEGETATION_WINDOW,
    workspaces::{EditorWorkspace, world::ui::WorldWorkspaceUiState},
};
use bevy::prelude::*;
use bevy_egui::egui;
use engine::{ActiveWorldSpace, WorldCatalog};
use std::collections::HashSet;
use world::StableObjectId;
use world_db::SourceObjectViewRecord;

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_world_hierarchy(
    ui: &mut egui::Ui,
    catalog: &WorldCatalog,
    project: &ProjectEditorStore,
    selection: &mut EditorSelection,
    objects: &mut EditorObjectWorkingSet,
    active_space: &mut ActiveWorldSpace,
    tools: &mut EditorToolRegistry,
    ui_state: &mut WorldWorkspaceUiState,
    windows: &mut EditorWindowRegistry,
) {
    let current_space = active_space.current();
    let current_space_name = current_space
        .and_then(|id| catalog.world_space(id))
        .map_or("Select world space", |space| space.name.as_str());
    egui::ComboBox::from_id_salt("world_hierarchy_space")
        .width(ui.available_width())
        .selected_text(current_space_name)
        .show_ui(ui, |ui| {
            for space in catalog.world_spaces() {
                let selected = current_space == Some(space.id);
                if ui
                    .selectable_label(selected, format!("{} ({})", space.name, space.id.0))
                    .clicked()
                    && !selected
                {
                    active_space.request(space.id, [0.0, 0.0, 0.0]);
                    ui.close();
                }
            }
        });

    ui.separator();
    let active_tool = tools.active(EditorWorkspace::World);
    if ui
        .selectable_label(active_tool.is_some_and(|t| t.id == ROAD_TOOL.id), "Roads")
        .clicked()
    {
        tools.set_active(EditorWorkspace::World, ROAD_TOOL.id);
    }
    if ui
        .selectable_label(
            active_tool.is_some_and(|tool| tool.id == ENVIRONMENT_TOOL.id),
            "Environment",
        )
        .clicked()
    {
        tools.set_active(EditorWorkspace::World, ENVIRONMENT_TOOL.id);
    }
    if ui
        .selectable_label(
            active_tool.is_some_and(|tool| tool.id == VEGETATION_TOOL.id),
            "Vegetation",
        )
        .clicked()
    {
        tools.set_active(EditorWorkspace::World, VEGETATION_TOOL.id);
        windows.set_open(VEGETATION_WINDOW.id, true);
    }
    let mut visible_assets = visible_asset_records(project, objects);
    let visible_count = visible_assets.len();
    let mut objects_active = tools
        .active(EditorWorkspace::World)
        .is_some_and(|tool| tool.id == OBJECT_TOOL.id);
    if ui
        .selectable_label(objects_active, format!("Visible assets ({visible_count})"))
        .clicked()
    {
        tools.set_active(EditorWorkspace::World, OBJECT_TOOL.id);
        objects_active = true;
    }
    if objects_active {
        ui.indent("visible_assets_tree", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut ui_state.visible_assets_search)
                    .hint_text("Filter visible assets"),
            );
            let search = ui_state.visible_assets_search.trim().to_lowercase();
            if !search.is_empty() {
                visible_assets.retain(|record| {
                    record
                        .definition
                        .display_name
                        .to_lowercase()
                        .contains(&search)
                        || record.definition.key.to_lowercase().contains(&search)
                        || short_object_id(record.object.id).contains(&search)
                });
            }
            egui::ScrollArea::vertical()
                .id_salt("world_visible_assets")
                .max_height((ui.available_height() - 28.0).max(120.0))
                .show(ui, |ui| {
                    for record in visible_assets {
                        let is_selected = selection.contains(record.object.id);
                        let active = selection.selected_id() == Some(record.object.id);
                        let label = format!(
                            "{}{} · {}, {}",
                            if active { "◆ " } else { "" },
                            record.definition.display_name,
                            record.object.owner_cell.x,
                            record.object.owner_cell.z
                        );
                        let response = ui
                            .add_enabled_ui(selection.can_change_selection(objects), |ui| {
                                ui.selectable_label(is_selected, label)
                            })
                            .inner;
                        if response.clicked() {
                            let additive = ui.input(|input| {
                                input.modifiers.shift
                                    || input.modifiers.command
                                    || input.modifiers.ctrl
                            });
                            if additive {
                                selection.toggle(record, objects);
                            } else {
                                selection.select(record, objects);
                            }
                        }
                    }
                });
            if visible_count == 0 {
                ui.weak("No authored assets in the loaded source window.");
            }
        });
    }
}

fn visible_asset_records(
    project: &ProjectEditorStore,
    objects: &EditorObjectWorkingSet,
) -> Vec<SourceObjectViewRecord> {
    let mut records = project
        .objects()
        .iter()
        .filter_map(|record| objects.resolve_source_view(record))
        .collect::<Vec<_>>();
    let source_ids = records
        .iter()
        .map(|record| record.object.id)
        .collect::<HashSet<_>>();
    if let Some(window) = project.loaded_window() {
        records.extend(objects.current_views().into_iter().filter(|record| {
            !source_ids.contains(&record.object.id) && source_object_in_window(record, window)
        }));
    }
    records.sort_by_key(|record| {
        (
            record.object.owner_cell.x,
            record.object.owner_cell.z,
            record.object.id.0,
        )
    });
    records
}

fn source_object_in_window(record: &SourceObjectViewRecord, window: ProjectQueryWindow) -> bool {
    record.object.space == window.space
        && record.object.owner_cell.x >= window.minimum.x
        && record.object.owner_cell.x <= window.maximum.x
        && record.object.owner_cell.z >= window.minimum.z
        && record.object.owner_cell.z <= window.maximum.z
}

pub(super) fn short_object_id(id: StableObjectId) -> String {
    id.0[..4].iter().map(|byte| format!("{byte:02x}")).collect()
}
