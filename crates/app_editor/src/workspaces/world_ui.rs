//! Floating-window UI for the World workspace.
//!
//! Window visibility is presentation state. Authoring behavior remains owned by the declarative
//! tool registry, so opening a palette or diagnostics window cannot silently widen source demand.

use std::collections::HashSet;

use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    ecs::system::SystemParam,
    gizmos::transform_gizmo::{TransformGizmoMode, TransformGizmoSettings},
    prelude::*,
};
use bevy_egui::egui;
use engine::{ActiveWorldSpace, StreamingStats, WorldCatalog, WorldOrigin, WorldViewpoint};
use world::StableObjectId;
use world_db::{SourceObjectTransform, SourceObjectViewRecord};

use super::{
    EditorWorkspace,
    world_impl::{
        EditorCameraFocusRequest, EditorObjectPalette, create_palette_object,
        source_object_position,
    },
};
use crate::{
    derived_jobs::{DerivedArtifactStore, DerivedJobScheduler},
    domain_editing::DenseDomainWorkingSets,
    editing::{
        EditorHistory, EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft,
        normalize_transform,
    },
    journal::EditorJournalStatus,
    navigation::ProjectNavigationStore,
    overview::{OverviewMode, OverviewProductKind, OverviewState},
    preview::{EditorPreviewMode, PreviewModeState, PreviewRuntimeDiagnostics},
    project_store::{ProjectEditorStore, ProjectQueryWindow},
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::{EditorUiFrame, EditorWindowDescriptor, EditorWindowId, EditorWindowRegistry},
    tools::{ENVIRONMENT_TOOL, EditorToolRegistry, OBJECT_TOOL, ROAD_TOOL, VEGETATION_TOOL},
    vegetation_authoring::{VEGETATION_WINDOW, VegetationAuthoringState},
};

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
    presets: ResMut<'w, super::presets::PresetAuthoringState>,
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

#[allow(clippy::too_many_arguments)]
fn draw_world_hierarchy(
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

#[allow(clippy::too_many_arguments)]
fn draw_context_inspector(
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

#[allow(clippy::too_many_arguments)]
fn draw_asset_browser(
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

#[allow(clippy::too_many_arguments)]
fn draw_navigator(
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

#[allow(clippy::too_many_arguments)]
fn draw_world_diagnostics(
    ui: &mut egui::Ui,
    diagnostics: &DiagnosticsStore,
    catalog: &WorldCatalog,
    viewpoint: &WorldViewpoint,
    origin: &WorldOrigin,
    stats: &StreamingStats,
    derived_jobs: &DerivedJobScheduler,
    derived_artifacts: &DerivedArtifactStore,
    dense_domains: &DenseDomainWorkingSets,
    journal: &EditorJournalStatus,
    navigation: &ProjectNavigationStore,
    overview: &OverviewState,
    preview: &PreviewModeState,
    preview_runtime: &PreviewRuntimeDiagnostics,
    publication: &RuntimePublicationState,
    project: &ProjectEditorStore,
    objects: &EditorObjectWorkingSet,
    history: &EditorHistory,
    tools: &EditorToolRegistry,
) {
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|diagnostic| diagnostic.smoothed())
        .unwrap_or_default();
    ui.monospace(format!("{fps:.1} FPS · 30 FPS authoring target"));
    ui.label(&stats.status);
    if !catalog.generation_id().is_empty() {
        ui.small(format!("Runtime generation {}", catalog.generation_id()));
    }
    ui.small(publication.status());
    if let Some(generation) = publication.published_generation() {
        ui.small(format!("Last publication adopted: {generation}"));
    }

    ui.separator();
    ui.heading("Logical viewpoint");
    if let Some(position) = viewpoint.position() {
        ui.monospace(format!(
            "space {} · cell {}, {} · local {:.2}, {:.2}, {:.2}",
            position.space.0,
            position.cell.x,
            position.cell.z,
            position.local[0],
            position.local[1],
            position.local[2]
        ));
    } else {
        ui.weak("Waiting for runtime manifest…");
    }
    ui.monospace(format!("origin {}, {}", origin.cell().x, origin.cell().z));

    ui.separator();
    ui.heading("Streaming");
    egui::Grid::new("streaming_diagnostics")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            diagnostic_row(ui, "Demanded", stats.demanded);
            diagnostic_row(ui, "Loading", stats.loading);
            diagnostic_row(ui, "Prepared", stats.prepared);
            diagnostic_row(ui, "Resident", stats.resident);
            diagnostic_row(ui, "Cooling", stats.cooling);
            diagnostic_row(ui, "Failed", stats.failed);
            diagnostic_row(ui, "Page entities", stats.owned_entities);
            diagnostic_row(ui, "Vegetation V2 pages", stats.vegetation_pages);
            ui.label("Decoded");
            ui.monospace(format_bytes(stats.decoded_bytes));
            ui.end_row();
            ui.label("GPU estimate");
            ui.monospace(format_bytes(stats.gpu_bytes_estimate));
            ui.end_row();
        });
    ui.small("Editor profile: 64 MiB decoded / 256 MiB estimated GPU");

    ui.separator();
    ui.heading("Authoring source");
    ui.label(project.status());
    if let Some(active_tool) = tools.active(EditorWorkspace::World) {
        ui.small(format!(
            "{} · {} source domain(s) · {:?}",
            active_tool.label,
            active_tool.source_domains.len(),
            active_tool.pinning
        ));
    }
    ui.small(format!(
        "{} coverage cell(s) · {} unsaved · {} retained",
        dense_domains.environment_record_count(),
        dense_domains.dirty_count(),
        format_bytes(dense_domains.retained_bytes() as u64),
    ));
    ui.small(format!(
        "{} · recovered this session {}",
        journal.message(),
        journal.recovered_entries()
    ));
    ui.small(format!(
        "Derived: {} pending / {} running · {} accepted / {} failed · {} cancelled / {} stale / {} capacity retries",
        derived_jobs.pending_count(),
        derived_jobs.running_count(),
        derived_artifacts.accepted_count(),
        derived_artifacts.failed_jobs(),
        derived_jobs.cancelled_count(),
        derived_jobs.stale_results(),
        derived_jobs.capacity_rejections()
    ));
    ui.small(format!(
        "{} · {} stale navigation result(s)",
        navigation.status(),
        navigation.stale_results()
    ));
    if let Some(error) = project.write_error() {
        ui.colored_label(
            egui::Color32::YELLOW,
            format!("Read-only authoring session: {error}"),
        );
    }
    if let Some(manifest) = project.manifest() {
        ui.small(format!(
            "Schema {} · {} world spaces · default {}",
            manifest.schema_version,
            manifest.world_spaces.len(),
            manifest.default_world_space.0
        ));
    }
    egui::Grid::new("source_cache_diagnostics")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            diagnostic_row(ui, "Cached cells", project.cells().len());
            diagnostic_row(ui, "Cached objects", project.objects().len());
            ui.label("Query");
            ui.monospace(if project.query_in_flight() {
                "in flight"
            } else {
                "idle"
            });
            ui.end_row();
            ui.label("Save");
            ui.monospace(if project.save_in_flight() {
                "in flight"
            } else {
                "idle"
            });
            ui.end_row();
            ui.label("Highest revision");
            ui.monospace(
                project
                    .highest_source_revision()
                    .map_or_else(|| "—".into(), |revision| revision.to_string()),
            );
            ui.end_row();
            ui.label("Completed queries");
            ui.monospace(project.completed_queries().to_string());
            ui.end_row();
            ui.label("Stale results");
            ui.monospace(project.stale_results().to_string());
            ui.end_row();
        });
    if let Some(window) = project.desired_window() {
        ui.small(format_source_window("Desired", window));
    }
    if let Some(window) = project.loaded_window() {
        ui.small(format_source_window("Loaded", window));
    }
    if project.cells_truncated() || project.objects_truncated() {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Source result reached its hard cache limit; authoring detail is incomplete.",
        );
    }

    ui.separator();
    ui.heading("Overview and preview");
    let ready_products = overview
        .products()
        .iter()
        .filter(|product| product.state == crate::overview::OverviewProductState::Ready)
        .count();
    ui.small(format!(
        "{:?} · {} coarse tiles · {ready_products}/{} products ready",
        overview.mode(),
        overview.tiles().len(),
        overview.products().len()
    ));
    ui.small(
        OverviewProductKind::ALL
            .into_iter()
            .map(|kind| {
                let ready = overview
                    .products()
                    .iter()
                    .filter(|product| {
                        product.kind == kind
                            && product.state == crate::overview::OverviewProductState::Ready
                    })
                    .count();
                format!("{} {ready}/{}", kind.label(), overview.tiles().len())
            })
            .collect::<Vec<_>>()
            .join(" · "),
    );
    let preview_descriptor = preview.requested().descriptor();
    ui.small(format!(
        "{} preview · generation {} · {} domain(s) · isolated simulation {}",
        preview.requested().label(),
        preview.generation(),
        preview_descriptor.domains.len(),
        preview_descriptor.isolated_simulation
    ));
    if preview.requested() != EditorPreviewMode::Authoring {
        ui.small(format!(
            "Preview host: {} cell(s), {} object(s), {} fixed simulation tick(s)",
            preview_runtime.rendered_cells,
            preview_runtime.rendered_objects,
            preview_runtime.simulation_ticks,
        ));
    }

    ui.separator();
    ui.heading("Local commands");
    ui.small(format!(
        "{} dirty object(s) · {} undo / {} redo · {} retained · {} checkpointed",
        objects.dirty_count(),
        history.undo_len(),
        history.redo_len(),
        format_bytes(history.retained_bytes() as u64),
        history.checkpointed_commands()
    ));
}

fn diagnostic_row(ui: &mut egui::Ui, label: &str, value: usize) {
    ui.label(label);
    ui.monospace(value.to_string());
    ui.end_row();
}

fn format_source_window(label: &str, window: ProjectQueryWindow) -> String {
    format!(
        "{label}: space {} · ({}, {})–({}, {})",
        window.space.0, window.minimum.x, window.minimum.z, window.maximum.x, window.maximum.z
    )
}

fn source_object_in_window(record: &SourceObjectViewRecord, window: ProjectQueryWindow) -> bool {
    record.object.space == window.space
        && record.object.owner_cell.x >= window.minimum.x
        && record.object.owner_cell.x <= window.maximum.x
        && record.object.owner_cell.z >= window.minimum.z
        && record.object.owner_cell.z <= window.maximum.z
}

fn format_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    format!("{:.1} MiB", bytes as f64 / MIB)
}

fn short_object_id(id: StableObjectId) -> String {
    id.0[..4].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn object_id_hex(id: StableObjectId) -> String {
    id.0.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
