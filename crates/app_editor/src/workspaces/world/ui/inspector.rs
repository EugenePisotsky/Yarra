//! Context inspectors, object transform editing and source-conflict controls.
use crate::{
    domain_editing::DenseDomainWorkingSets,
    editing::{
        EditorHistory, EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft,
        normalize_transform,
    },
    project_store::ProjectEditorStore,
    tools::{ENVIRONMENT_TOOL, EditorToolRegistry, OBJECT_TOOL, ROAD_TOOL, VEGETATION_TOOL},
    workspaces::{
        EditorWorkspace,
        world::{camera::EditorCameraFocusRequest, objects::source_object_position},
    },
};
use bevy::{gizmos::transform_gizmo::TransformGizmoMode, prelude::*};
use bevy_egui::egui;
use engine::{WorldCatalog, WorldViewpoint};
use world::StableObjectId;
use world_db::SourceObjectTransform;

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_context_inspector(
    ui: &mut egui::Ui,
    catalog: &WorldCatalog,
    viewpoint: &WorldViewpoint,
    dense_domains: &mut DenseDomainWorkingSets,
    project: &ProjectEditorStore,
    selection: &mut EditorSelection,
    objects: &mut EditorObjectWorkingSet,
    history: &mut EditorHistory,
    draft: &mut TransformInspectorDraft,
    focus_request: &mut EditorCameraFocusRequest,
    tools: &EditorToolRegistry,
    gizmo_mode: TransformGizmoMode,
    roads: &mut crate::road_authoring::RoadToolState,
    paint: &mut crate::environment_paint::EnvironmentPaintState,
    layer_browser: &mut crate::environment_paint::EnvironmentLayerBrowser,
    environment_preview: &crate::environment_paint::EnvironmentPreview,
    space: Option<world::WorldSpaceId>,
    plants: Option<&vegetation::VegetationCatalog>,
    busy: bool,
) {
    let Some(active_tool) = tools.active(EditorWorkspace::World) else {
        ui.weak("No active World tool.");
        return;
    };
    if active_tool.id == OBJECT_TOOL.id {
        if selection.selected_id().is_none() {
            ui.heading("Visible assets");
            ui.weak("Select an asset in the viewport or World window to inspect it.");
            return;
        }
        draw_object_inspector(
            ui,
            catalog,
            viewpoint,
            selection,
            objects,
            history,
            draft,
            focus_request,
            gizmo_mode,
        );
        return;
    }

    if active_tool.id == ROAD_TOOL.id {
        crate::road_authoring::inspector(
            ui,
            roads,
            dense_domains,
            history,
            space,
            environment_preview,
            busy,
        );
        return;
    }
    if active_tool.id == ENVIRONMENT_TOOL.id {
        draw_dense_conflict_controls(ui, dense_domains, history);
        if let Some(status) = dense_domains.status() {
            ui.colored_label(egui::Color32::YELLOW, status);
        }
        if project.environment_coverage_truncated() {
            ui.weak("Paint area is incomplete; wait for the source query.");
        }
        crate::environment_paint::inspector(
            ui,
            paint,
            layer_browser,
            environment_preview,
            project,
            space,
            history,
            dense_domains,
            plants,
            busy,
        );
        return;
    }

    if active_tool.id == VEGETATION_TOOL.id {
        ui.heading("Vegetation");
        ui.label("Edit the active vegetation catalog in the Vegetation window.");
        ui.weak(
            "The validated draft drives the production GPU renderer. Save persists it to the project; Save & Publish also rebuilds the runtime database loaded by the game.",
        );
        return;
    }

    ui.weak("This tool has no inspector yet.");
}

fn draw_dense_conflict_controls(
    ui: &mut egui::Ui,
    dense_domains: &mut DenseDomainWorkingSets,
    history: &mut EditorHistory,
) {
    let conflicts = dense_domains.conflict_count();
    if conflicts == 0 {
        return;
    }
    ui.colored_label(
        egui::Color32::LIGHT_RED,
        format!("{conflicts} dense source conflict(s)"),
    );
    ui.small("Resolving a conflict clears command history because its old checkpoints are stale.");
    if ui
        .add_enabled(
            dense_domains.can_keep_local_conflicts(),
            egui::Button::new("Keep local on latest revision"),
        )
        .on_disabled_hover_text(
            "The database changed this record's shape; automatic byte rebasing is unsafe.",
        )
        .clicked()
        && dense_domains.keep_local_conflicts() > 0
    {
        history.clear();
    }
    if ui
        .add_enabled(
            dense_domains.can_accept_database_conflicts(),
            egui::Button::new("Use database version"),
        )
        .on_disabled_hover_text(
            "Missing cells or changed layer definitions require reloading the project.",
        )
        .clicked()
        && dense_domains.accept_database_conflicts() > 0
    {
        history.clear();
    }
    ui.separator();
}

#[allow(clippy::too_many_arguments)]
fn draw_object_inspector(
    ui: &mut egui::Ui,
    catalog: &WorldCatalog,
    viewpoint: &WorldViewpoint,
    selection: &mut EditorSelection,
    objects: &mut EditorObjectWorkingSet,
    history: &mut EditorHistory,
    draft: &mut TransformInspectorDraft,
    focus_request: &mut EditorCameraFocusRequest,
    gizmo_mode: TransformGizmoMode,
) {
    draft.sync(selection, objects);
    let Some(selected) = selection.selected_view(objects) else {
        return;
    };
    let selected_id = selected.object.id;
    let selected_ids = selection.selected_ids().to_vec();
    let selection_count = selected_ids.len();

    ui.heading(format!("Selection ({selection_count})"));
    ui.strong(&selected.definition.display_name);
    ui.monospace(format!("id       {}", object_id_hex(selected.object.id)));
    ui.monospace(format!("space    {}", selected.object.space.0));
    ui.monospace(format!("revision {}", selected.object.source_revision));

    let status_color = if objects.has_conflict(selected_id) {
        egui::Color32::LIGHT_RED
    } else if objects.dirty(selected_id) {
        egui::Color32::YELLOW
    } else {
        egui::Color32::GRAY
    };
    ui.colored_label(status_color, objects.status(selected_id));

    ui.add_space(4.0);
    ui.strong("Transform");
    if selection_count > 1 {
        ui.small(
            "The gizmo applies its delta to same-world companions; fields edit the active item.",
        );
    }
    match gizmo_mode {
        TransformGizmoMode::Translate => {
            ui.small("Move along world axes or edit normalized cell/local coordinates.");
        }
        TransformGizmoMode::Rotate => {
            ui.small("World objects remain upright; rotation edits yaw around world Y.");
        }
        TransformGizmoMode::Scale => {
            ui.small("World objects use uniform scale.");
        }
    }
    ui.add_enabled_ui(objects.can_edit(selected_id), |ui| {
        if let Some(transform) = draft.transform.as_mut() {
            egui::Grid::new("selected_object_transform")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Cell X");
                    ui.add(egui::DragValue::new(&mut transform.owner_cell.x).speed(1));
                    ui.end_row();
                    ui.label("Cell Z");
                    ui.add(egui::DragValue::new(&mut transform.owner_cell.z).speed(1));
                    ui.end_row();
                    for (label, value) in ["Local X", "Local Y", "Local Z"]
                        .into_iter()
                        .zip(transform.local_translation.iter_mut())
                    {
                        ui.label(label);
                        ui.add(egui::DragValue::new(value).speed(0.1));
                        ui.end_row();
                    }
                    let mut yaw_degrees = transform.yaw.to_degrees();
                    ui.label("Yaw°");
                    if ui
                        .add(egui::DragValue::new(&mut yaw_degrees).speed(0.5))
                        .changed()
                    {
                        transform.yaw = yaw_degrees.to_radians();
                    }
                    ui.end_row();
                    ui.label("Scale");
                    ui.add(
                        egui::DragValue::new(&mut transform.scale)
                            .speed(0.01)
                            .range(0.001..=10_000.0),
                    );
                    ui.end_row();
                });
        }
    });

    let current = SourceObjectTransform::from(&selected.object);
    let normalized = catalog
        .world_space(selected.object.space)
        .and_then(|space| {
            draft
                .transform
                .and_then(|value| normalize_transform(value, space.cell_size))
        });
    ui.horizontal(|ui| {
        let can_apply =
            objects.can_edit(selected_id) && normalized.is_some_and(|value| value != current);
        if ui
            .add_enabled(can_apply, egui::Button::new("Apply"))
            .clicked()
            && let Some(transform) = normalized
        {
            history.apply_transform(objects, selected_id, transform);
            draft.sync(selection, objects);
        }
        if ui
            .add_enabled(
                draft.transform.is_some_and(|value| value != current),
                egui::Button::new("Reset fields"),
            )
            .clicked()
        {
            draft.object = None;
            draft.sync(selection, objects);
        }
    });

    if objects.has_conflict(selected_id) {
        ui.separator();
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            "This asset changed in the database.",
        );
        if ui.button("Keep local on latest revision").clicked() {
            objects.rebase_conflict(selected_id, history);
            draft.sync(selection, objects);
        }
        if ui.button("Use database version").clicked() {
            objects.reload_conflict(selected_id, history);
            draft.sync(selection, objects);
        }
    }

    ui.separator();
    ui.horizontal(|ui| {
        let same_space = viewpoint
            .position()
            .is_some_and(|position| position.space == selected.object.space);
        if ui
            .add_enabled(same_space, egui::Button::new("Focus"))
            .clicked()
        {
            focus_request.0 = Some(source_object_position(&selected));
        }
        if ui
            .add_enabled(
                selection.can_change_selection(objects),
                egui::Button::new("Clear"),
            )
            .clicked()
        {
            selection.clear(objects);
            draft.sync(selection, objects);
        }
    });
    if ui
        .add_enabled(
            selected_ids.iter().all(|object| objects.can_edit(*object)),
            egui::Button::new(format!("Delete selection ({selection_count})")),
        )
        .clicked()
        && history.delete_many(objects, &selected_ids)
    {
        selection.clear(objects);
        draft.sync(selection, objects);
    }
    if viewpoint
        .position()
        .is_some_and(|position| position.space != selected.object.space)
    {
        ui.small("Switch to the selected asset's world space to focus it.");
    }
}

fn object_id_hex(id: StableObjectId) -> String {
    id.0.iter().map(|byte| format!("{byte:02x}")).collect()
}
