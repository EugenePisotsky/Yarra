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
use uuid::Uuid;
use world::{
    GroundCoverBladeRecipe, GroundCoverPresetId, GroundCoverRegionId, GroundCoverVisualId,
    StableObjectId, generate_ground_cover_card_artwork,
};
use world_db::{
    SourceGroundCoverCardVisualRecord, SourceGroundCoverPresetRecord,
    SourceGroundCoverRegionRecord, SourceGroundCoverVisualDefinition,
    SourceGroundCoverVisualRecord, SourceObjectTransform, SourceObjectViewRecord,
};

use super::{
    EditorWorkspace,
    world_impl::{
        EditorCameraFocusRequest, EditorObjectPalette, create_palette_object,
        source_object_position,
    },
};
use crate::{
    catalog_editing::GroundCoverRegionWorkingSet,
    derived_jobs::{DerivedArtifactStore, DerivedJobScheduler},
    domain_editing::DenseDomainWorkingSets,
    editing::{
        EditorHistory, EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft,
        normalize_transform,
    },
    ground_cover_catalog::GroundCoverCatalogWorkingSet,
    ground_cover_editing::{GroundCoverBrushMode, GroundCoverToolState},
    journal::EditorJournalStatus,
    navigation::ProjectNavigationStore,
    overview::{OverviewMode, OverviewProductKind, OverviewState},
    preview::{EditorPreviewMode, PreviewModeState, PreviewRuntimeDiagnostics},
    project_store::{ProjectEditorStore, ProjectQueryWindow},
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::{EditorUiFrame, EditorWindowDescriptor, EditorWindowId, EditorWindowRegistry},
    tools::{EditorToolRegistry, GROUND_COVER_TOOL, OBJECT_TOOL, TERRAIN_TOOL},
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
pub(crate) const GROUND_COVER_WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
    id: EditorWindowId("world.ground_cover"),
    workspace: EditorWorkspace::World,
    label: "Ground Cover",
    default_open: false,
};

#[derive(Resource, Default)]
pub(crate) struct WorldWorkspaceUiState {
    visible_assets_search: String,
}

#[derive(Resource, Default)]
pub(crate) struct GroundCoverCatalogUiState {
    catalog_revision: u64,
    selected_preset: Option<GroundCoverPresetId>,
    preset_draft: Option<SourceGroundCoverPresetRecord>,
    selected_visual: Option<GroundCoverVisualId>,
    visual_draft: Option<SourceGroundCoverVisualRecord>,
    selected_region: Option<GroundCoverRegionId>,
    region_draft: Option<SourceGroundCoverRegionRecord>,
    region_revision: u64,
    artwork_preview_key: Option<ArtworkPreviewKey>,
    artwork_preview_variant: u8,
    artwork_preview_texture: Option<egui::TextureHandle>,
}

#[derive(Clone, Copy, PartialEq)]
struct ArtworkPreviewKey {
    recipe: GroundCoverBladeRecipe,
    bottom_color: [f32; 3],
    top_color: [f32; 3],
    variant: u8,
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
    regions: ResMut<'w, GroundCoverRegionWorkingSet>,
    ground_cover_catalog: ResMut<'w, GroundCoverCatalogWorkingSet>,
    ground_cover_tool: ResMut<'w, GroundCoverToolState>,
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
    ground_cover_catalog_ui: ResMut<'w, GroundCoverCatalogUiState>,
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
        mut regions,
        mut ground_cover_catalog,
        mut ground_cover_tool,
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
        mut ground_cover_catalog_ui,
        mut windows,
    } = resources;
    let Some(viewport_ui) = frame.0.as_mut() else {
        return Ok(());
    };

    egui::Panel::top("editor_world_toolbar").show(viewport_ui, |ui| {
        ui.horizontal(|ui| {
            let remaining_changes = objects.dirty_count()
                + ground_cover_catalog.dirty_count()
                + regions.dirty_count()
                + dense_domains.dirty_count();
            let has_dirty_source = remaining_changes > 0;
            let source_action_available = !save.active()
                && !project.save_in_flight()
                && !objects.saving()
                && !ground_cover_catalog.saving()
                && !regions.saving()
                && !dense_domains.saving()
                && !objects.has_any_conflict()
                && !ground_cover_catalog.has_any_conflict()
                && !regions.has_any_conflict()
                && !dense_domains.has_any_conflict()
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
                        && !save.active()
                        && !objects.saving()
                        && !ground_cover_catalog.saving()
                        && !regions.saving()
                        && !dense_domains.saving()
                        && !objects.has_any_conflict()
                        && !ground_cover_catalog.has_any_conflict()
                        && !regions.has_any_conflict()
                        && !dense_domains.has_any_conflict(),
                    egui::Button::new("Undo"),
                )
                .on_hover_text("Undo the last command (Cmd+Z)")
                .clicked()
            {
                history.undo_with_catalog(
                    &mut objects,
                    &mut dense_domains,
                    &mut regions,
                    &mut ground_cover_catalog,
                );
                transform_draft.sync(&selection, &objects);
            }
            if ui
                .add_enabled(
                    history.redo_len() > 0
                        && !save.active()
                        && !objects.saving()
                        && !ground_cover_catalog.saving()
                        && !regions.saving()
                        && !dense_domains.saving()
                        && !objects.has_any_conflict()
                        && !ground_cover_catalog.has_any_conflict()
                        && !regions.has_any_conflict()
                        && !dense_domains.has_any_conflict(),
                    egui::Button::new("Redo"),
                )
                .on_hover_text("Redo the last command (Cmd+Shift+Z)")
                .clicked()
            {
                history.redo_with_catalog(
                    &mut objects,
                    &mut dense_domains,
                    &mut regions,
                    &mut ground_cover_catalog,
                );
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
            let ground_cover_active = tools
                .active(EditorWorkspace::World)
                .is_some_and(|tool| tool.id == GROUND_COVER_TOOL.id);
            if ground_cover_active {
                ui.separator();
                ui.selectable_value(
                    &mut ground_cover_tool.mode,
                    GroundCoverBrushMode::Paint,
                    "Paint",
                );
                ui.selectable_value(
                    &mut ground_cover_tool.mode,
                    GroundCoverBrushMode::Erase,
                    "Erase",
                );
                ui.add(
                    egui::DragValue::new(&mut ground_cover_tool.radius)
                        .range(0.25..=16.0)
                        .speed(0.1)
                        .suffix(" m"),
                )
                .on_hover_text("Brush radius");
                let mut hardness_percent = ground_cover_tool.hardness * 100.0;
                if ui
                    .add(
                        egui::DragValue::new(&mut hardness_percent)
                            .range(0.0..=100.0)
                            .speed(1.0)
                            .suffix("%"),
                    )
                    .on_hover_text("Brush hardness: full-strength core before the soft edge")
                    .changed()
                {
                    ground_cover_tool.hardness = hardness_percent / 100.0;
                }
            }

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
                || ground_cover_catalog.saving()
                || regions.saving()
                || dense_domains.saving()
            {
                ui.separator();
                ui.spinner();
                ui.weak(if save.active() {
                    save.status(remaining_changes)
                } else {
                    "Saving source changes…".into()
                });
            } else if ground_cover_catalog.dirty_count() > 0 {
                ui.separator();
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!(
                        "{} ground-cover definition(s) unsaved",
                        ground_cover_catalog.dirty_count()
                    ),
                );
            } else if regions.dirty_count() > 0 {
                ui.separator();
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("{} grass region(s) unsaved", regions.dirty_count()),
                );
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
                    format!("{} grass cell(s) unsaved", dense_domains.dirty_count()),
                );
            }
            if objects.has_any_conflict()
                || ground_cover_catalog.has_any_conflict()
                || regions.has_any_conflict()
                || dense_domains.has_any_conflict()
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
                    &dense_domains,
                    &mut regions,
                    &mut selection,
                    &mut objects,
                    &mut history,
                    &mut active_space,
                    &mut tools,
                    &mut ground_cover_tool,
                    &mut ui_state,
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
                    &mut regions,
                    &ground_cover_catalog,
                    &project,
                    &mut selection,
                    &mut objects,
                    &mut history,
                    &mut transform_draft,
                    &mut focus_request,
                    &tools,
                    &mut ground_cover_tool,
                    gizmo_settings.mode,
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

    if windows.is_open(GROUND_COVER_WINDOW.id) {
        let mut open = true;
        egui::Window::new("Ground Cover")
            .id(egui::Id::new(GROUND_COVER_WINDOW.id.0))
            .open(&mut open)
            .default_pos([workspace_rect.left() + 315.0, workspace_rect.top() + 70.0])
            .default_size([390.0, 620.0])
            .constrain_to(workspace_rect)
            .resizable(true)
            .vscroll(true)
            .show(&context, |ui| {
                draw_ground_cover_catalog(
                    ui,
                    &project,
                    &mut ground_cover_catalog,
                    &mut regions,
                    &ground_cover_tool,
                    &mut history,
                    &mut ground_cover_catalog_ui,
                );
            });
        windows.set_open(GROUND_COVER_WINDOW.id, open);
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

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_world_hierarchy(
    ui: &mut egui::Ui,
    catalog: &WorldCatalog,
    project: &ProjectEditorStore,
    dense_domains: &DenseDomainWorkingSets,
    regions: &mut GroundCoverRegionWorkingSet,
    selection: &mut EditorSelection,
    objects: &mut EditorObjectWorkingSet,
    history: &mut EditorHistory,
    active_space: &mut ActiveWorldSpace,
    tools: &mut EditorToolRegistry,
    ground_cover_tool: &mut GroundCoverToolState,
    ui_state: &mut WorldWorkspaceUiState,
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
        .selectable_label(
            active_tool.is_some_and(|tool| tool.id == TERRAIN_TOOL.id),
            "Terrain",
        )
        .clicked()
    {
        tools.set_active(EditorWorkspace::World, TERRAIN_TOOL.id);
    }
    if ui
        .selectable_label(
            active_tool.is_some_and(|tool| tool.id == GROUND_COVER_TOOL.id),
            "Grass",
        )
        .clicked()
    {
        tools.set_active(EditorWorkspace::World, GROUND_COVER_TOOL.id);
    }
    let ground_cover_active = tools
        .active(EditorWorkspace::World)
        .is_some_and(|tool| tool.id == GROUND_COVER_TOOL.id);
    if ground_cover_active {
        let current_regions = regions.current_records(project);
        ui.indent("ground_cover_layers", |ui| {
            for layer in project
                .ground_cover_layers()
                .iter()
                .filter(|layer| Some(layer.space) == current_space)
            {
                let layer_regions = current_regions
                    .iter()
                    .filter(|region| region.layer == layer.id)
                    .collect::<Vec<_>>();
                egui::CollapsingHeader::new(format!(
                    "{} ({})",
                    layer.display_name,
                    layer_regions.len()
                ))
                .default_open(true)
                .show(ui, |ui| {
                    for region in layer_regions {
                        let selected = ground_cover_tool.selected_region == Some(region.id);
                        let label = if region.enabled {
                            region.display_name.clone()
                        } else {
                            format!("{} (disabled)", region.display_name)
                        };
                        if ui.selectable_label(selected, label).clicked() {
                            ground_cover_tool.selected_region = Some(region.id);
                        }
                    }
                });
            }
            if current_regions
                .iter()
                .all(|region| Some(region.space) != current_space)
            {
                ui.weak("No grass regions in this world space.");
            }

            ui.horizontal(|ui| {
                let creation = current_space.and_then(|space| {
                    if let Some(selected) = ground_cover_tool.selected_region.and_then(|id| {
                        current_regions
                            .iter()
                            .find(|region| region.id == id && region.space == space)
                    }) {
                        return Some((space, selected.layer, selected.preset));
                    }
                    let layer = project
                        .ground_cover_layers()
                        .iter()
                        .find(|layer| layer.space == space && layer.enabled)?;
                    let preset = project
                        .ground_cover_presets()
                        .iter()
                        .find(|preset| preset.enabled)?;
                    Some((space, layer.id, preset.id))
                });
                if ui
                    .add_enabled(creation.is_some(), egui::Button::new("+ Region"))
                    .on_hover_text("Create an empty region in the first enabled layer")
                    .clicked()
                {
                    let (space, layer, preset) = creation.expect("button requires a region target");
                    let region = SourceGroundCoverRegionRecord {
                        id: GroundCoverRegionId(*Uuid::new_v4().as_bytes()),
                        layer,
                        space,
                        preset,
                        display_name: format!("New region {}", current_regions.len() + 1),
                        enabled: true,
                        density_multiplier: 1.0,
                        source_revision: 0,
                    };
                    let id = region.id;
                    if history.create_region(regions, region) {
                        ground_cover_tool.selected_region = Some(id);
                    }
                }

                let selected = ground_cover_tool
                    .selected_region
                    .and_then(|id| regions.current(project, id));
                let can_delete = selected.as_ref().is_some_and(|region| {
                    !dense_domains.ground_cover_region_has_coverage(region.id)
                        && !project
                            .ground_cover_masks()
                            .iter()
                            .any(|mask| mask.region == region.id)
                });
                if ui
                    .add_enabled(can_delete, egui::Button::new("Delete"))
                    .on_hover_text(
                        "Delete an empty region. The database checks all cells again when saving.",
                    )
                    .clicked()
                {
                    let selected = selected.expect("button requires an empty selected region");
                    if history.delete_region(regions, selected) {
                        ground_cover_tool.selected_region = None;
                    }
                }
            });
        });
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
    regions: &mut GroundCoverRegionWorkingSet,
    ground_cover_catalog: &GroundCoverCatalogWorkingSet,
    project: &ProjectEditorStore,
    selection: &mut EditorSelection,
    objects: &mut EditorObjectWorkingSet,
    history: &mut EditorHistory,
    draft: &mut TransformInspectorDraft,
    focus_request: &mut EditorCameraFocusRequest,
    tools: &EditorToolRegistry,
    ground_cover_tool: &mut GroundCoverToolState,
    gizmo_mode: TransformGizmoMode,
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

    if active_tool.id == TERRAIN_TOOL.id {
        ui.heading("Terrain");
        draw_dense_conflict_controls(ui, dense_domains, history);
        ui.label("Terrain settings apply to the active bounded cell patch.");
        draw_dense_domain_inspector(ui, viewpoint, dense_domains, project, true);
        ui.separator();
        ui.weak("Terrain brush controls will use the existing bounded patch command seam.");
        return;
    }

    ui.heading("Grass");
    draw_dense_conflict_controls(ui, dense_domains, history);
    if regions.conflict_count() > 0 {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            format!("{} region source conflict(s)", regions.conflict_count()),
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    regions.can_keep_local_conflicts(),
                    egui::Button::new("Keep local"),
                )
                .on_hover_text(
                    "Rebase compatible create/delete/update intent onto the latest source",
                )
                .clicked()
                && regions.keep_local_conflicts() > 0
            {
                history.clear();
            }
            if ui.button("Accept database").clicked() && regions.accept_database_conflicts() > 0 {
                history.clear();
            }
        });
    } else if let Some(status) = regions.status() {
        ui.colored_label(egui::Color32::YELLOW, status);
        if ui.small_button("Dismiss").clicked() {
            regions.dismiss_status();
        }
    }
    if let Some(region) = ground_cover_tool
        .selected_region
        .and_then(|selected| regions.current(project, selected))
    {
        ui.strong(&region.display_name);
        ui.monospace(format!("space    {}", region.space.0));
        ui.monospace(format!("revision {}", region.source_revision));
        if let Some(preset) = ground_cover_catalog
            .current_presets(project)
            .into_iter()
            .find(|preset| preset.id == region.preset)
        {
            ui.label(format!("Preset: {}", preset.display_name));
            ui.small(format!(
                "{:.1} plants/m² × {:.2} region density",
                preset.density_per_square_meter, region.density_multiplier
            ));
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut ground_cover_tool.mode,
                GroundCoverBrushMode::Paint,
                "Paint",
            );
            ui.selectable_value(
                &mut ground_cover_tool.mode,
                GroundCoverBrushMode::Erase,
                "Erase",
            );
        });
        ui.horizontal(|ui| {
            ui.label("Radius");
            ui.add(
                egui::Slider::new(&mut ground_cover_tool.radius, 0.25..=16.0)
                    .logarithmic(true)
                    .suffix(" m"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Hardness");
            let mut hardness_percent = ground_cover_tool.hardness * 100.0;
            if ui
                .add(egui::Slider::new(&mut hardness_percent, 0.0..=100.0).suffix("%"))
                .on_hover_text("The inner part is solid; the outer part fades smoothly")
                .changed()
            {
                ground_cover_tool.hardness = hardness_percent / 100.0;
            }
        });
        let mask_resolution = dense_domains
            .ground_cover_region_resolution(region.id)
            .or_else(|| {
                project
                    .ground_cover_masks()
                    .iter()
                    .find(|mask| mask.region == region.id)
                    .map(|mask| mask.resolution)
            })
            .unwrap_or(16);
        if let Some(space) = catalog.world_space(region.space) {
            ui.small(format!(
                "{}×{} mask · {:.2} m runtime clusters · area-sampled soft edge",
                mask_resolution,
                mask_resolution,
                space.cell_size / f32::from(mask_resolution)
            ));
        }
        ui.small(format!(
            "Drag in the viewport to {} coverage. One drag is one undo command.",
            ground_cover_tool.mode.label().to_lowercase()
        ));
    } else {
        ui.weak("Select a region in the World window before painting.");
    }
    draw_dense_domain_inspector(ui, viewpoint, dense_domains, project, false);
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
            "A disappeared dense record cannot be accepted until dense tombstones are supported.",
        )
        .clicked()
        && dense_domains.accept_database_conflicts() > 0
    {
        history.clear();
    }
    ui.separator();
}

fn draw_dense_domain_inspector(
    ui: &mut egui::Ui,
    viewpoint: &WorldViewpoint,
    dense_domains: &DenseDomainWorkingSets,
    project: &ProjectEditorStore,
    terrain: bool,
) {
    if let Some(position) = viewpoint.position() {
        ui.monospace(format!("Cell {}, {}", position.cell.x, position.cell.z));
        let ready = dense_domains.contains_patch(position.space, position.cell, terrain);
        ui.label(if ready {
            "Source patch ready"
        } else {
            "No source patch at the viewpoint"
        });
    } else {
        ui.weak("Waiting for a logical viewpoint…");
    }
    if terrain {
        ui.small(format!(
            "{} loaded terrain page(s)",
            dense_domains.terrain_record_count()
        ));
        if project.terrain_weights_truncated() {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Terrain query reached its bounded limit",
            );
        }
    } else {
        ui.small(format!(
            "{} layer(s) · {} loaded mask(s)",
            dense_domains.ground_cover_layer_count(),
            dense_domains.ground_cover_record_count()
        ));
        if project.ground_cover_masks_truncated() {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Grass query reached its bounded limit",
            );
        }
    }
    if let Some(status) = dense_domains.status() {
        ui.colored_label(egui::Color32::YELLOW, status);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_ground_cover_catalog(
    ui: &mut egui::Ui,
    project: &ProjectEditorStore,
    catalog: &mut GroundCoverCatalogWorkingSet,
    regions: &mut GroundCoverRegionWorkingSet,
    ground_cover_tool: &GroundCoverToolState,
    history: &mut EditorHistory,
    state: &mut GroundCoverCatalogUiState,
) {
    ui.heading("Ground Cover Definitions");
    ui.small(
        "Presets and visuals are project source. Apply creates one undo command; Save writes all pending definitions atomically in bounded batches.",
    );

    if catalog.conflict_count() > 0 {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            format!("{} catalog conflict(s)", catalog.conflict_count()),
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    catalog.can_keep_local_conflicts(),
                    egui::Button::new("Keep local on latest"),
                )
                .clicked()
                && catalog.keep_local_conflicts() > 0
            {
                history.clear();
                state.preset_draft = None;
                state.visual_draft = None;
            }
            if ui.button("Use database version").clicked()
                && catalog.accept_database_conflicts() > 0
            {
                history.clear();
                state.preset_draft = None;
                state.visual_draft = None;
            }
        });
    } else if let Some(status) = catalog.status() {
        ui.colored_label(egui::Color32::YELLOW, status);
        if ui.small_button("Dismiss").clicked() {
            catalog.dismiss_failures();
        }
    }

    let presets = catalog.current_presets(project);
    let visuals = catalog.current_visuals(project);
    if state.selected_preset.is_none() {
        state.selected_preset = presets.first().map(|record| record.id);
    }
    if state.selected_visual.is_none() {
        state.selected_visual = visuals.first().map(|record| record.id);
    }
    if state.catalog_revision != catalog.edit_revision() {
        state.catalog_revision = catalog.edit_revision();
        state.preset_draft = state
            .selected_preset
            .and_then(|id| catalog.current_preset(project, id));
        state.visual_draft = state
            .selected_visual
            .and_then(|id| catalog.current_visual(project, id));
    }

    ui.separator();
    ui.strong("Preset");
    let preset_label = state
        .selected_preset
        .and_then(|id| presets.iter().find(|record| record.id == id))
        .map_or("Select preset", |record| record.display_name.as_str());
    egui::ComboBox::from_id_salt("ground_cover_catalog_preset")
        .width(ui.available_width())
        .selected_text(preset_label)
        .show_ui(ui, |ui| {
            for preset in &presets {
                if ui
                    .selectable_label(
                        state.selected_preset == Some(preset.id),
                        &preset.display_name,
                    )
                    .clicked()
                {
                    state.selected_preset = Some(preset.id);
                    state.preset_draft = Some(preset.clone());
                    ui.close();
                }
            }
        });
    if state.preset_draft.as_ref().map(|record| record.id) != state.selected_preset {
        state.preset_draft = state
            .selected_preset
            .and_then(|id| catalog.current_preset(project, id));
    }
    let mut created_preset = None;
    let mut deleted_preset = false;
    if let Some(draft) = state.preset_draft.as_mut() {
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut draft.display_name);
        });
        ui.checkbox(&mut draft.enabled, "Enabled");
        ui.horizontal(|ui| {
            ui.label("Visual");
            let selected = visuals
                .iter()
                .find(|visual| visual.id == draft.visual)
                .map_or("Missing visual", |visual| visual.display_name.as_str());
            egui::ComboBox::from_id_salt("ground_cover_preset_visual")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for visual in &visuals {
                        ui.selectable_value(&mut draft.visual, visual.id, &visual.display_name);
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Plants / m²");
            ui.add(
                egui::DragValue::new(&mut draft.density_per_square_meter)
                    .range(0.05..=100.0)
                    .speed(0.1),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Seed");
            ui.add(egui::DragValue::new(&mut draft.seed).speed(1));
        });
        let current = catalog.current_preset(project, draft.id);
        let can_apply = current.as_ref().is_some_and(|current| current != draft)
            && !draft.display_name.trim().is_empty()
            && draft.density_per_square_meter.is_finite()
            && draft.density_per_square_meter > 0.0
            && !catalog.saving()
            && !catalog.has_any_conflict();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(can_apply, egui::Button::new("Apply preset"))
                .clicked()
                && let Some(before) = current.clone()
            {
                let after = draft.clone();
                if history.update_ground_cover_preset(catalog, before, after.clone()) {
                    *draft = after;
                }
            }
            if ui.small_button("Reset").clicked()
                && let Some(current) = catalog.current_preset(project, draft.id)
            {
                *draft = current;
            }
            if ui.small_button("Duplicate").clicked()
                && let Some(source) = current.clone()
            {
                let mut duplicate = source;
                let id = Uuid::new_v4();
                duplicate.id = GroundCoverPresetId(*id.as_bytes());
                duplicate.key = duplicate_catalog_key(&duplicate.key, id);
                duplicate.display_name = format!("{} Copy", duplicate.display_name);
                duplicate.source_revision = 0;
                if history.create_ground_cover_preset(catalog, duplicate.clone()) {
                    created_preset = Some(duplicate);
                }
            }
            if ui
                .add_enabled(current.is_some(), egui::Button::new("Delete"))
                .on_hover_text(
                    "Creates an undoable tombstone. Save validates project-wide region dependencies.",
                )
                .clicked()
                && let Some(record) = current.clone()
                && history.delete_ground_cover_definition(
                    catalog,
                    world_db::GroundCoverCatalogRecord::Preset(record),
                )
            {
                deleted_preset = true;
            }
        });
    }
    if let Some(record) = created_preset {
        state.selected_preset = Some(record.id);
        state.preset_draft = Some(record);
    } else if deleted_preset {
        state.selected_preset = None;
        state.preset_draft = None;
    }

    ui.separator();
    ui.strong("Card visual");
    let visual_label = state
        .selected_visual
        .and_then(|id| visuals.iter().find(|record| record.id == id))
        .map_or("Select visual", |record| record.display_name.as_str());
    egui::ComboBox::from_id_salt("ground_cover_catalog_visual")
        .width(ui.available_width())
        .selected_text(visual_label)
        .show_ui(ui, |ui| {
            for visual in &visuals {
                if ui
                    .selectable_label(
                        state.selected_visual == Some(visual.id),
                        &visual.display_name,
                    )
                    .clicked()
                {
                    state.selected_visual = Some(visual.id);
                    state.visual_draft = Some(visual.clone());
                    ui.close();
                }
            }
        });
    if state.visual_draft.as_ref().map(|record| record.id) != state.selected_visual {
        state.visual_draft = state
            .selected_visual
            .and_then(|id| catalog.current_visual(project, id));
    }
    let mut created_visual = None;
    let mut deleted_visual = false;
    if let Some(draft) = state.visual_draft.as_mut() {
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut draft.display_name);
        });
        let SourceGroundCoverVisualDefinition::CardCluster(card) = &mut draft.definition;
        ui.horizontal(|ui| {
            ui.label("Artwork");
            let built_in = card.procedural_recipe.is_none();
            if ui
                .selectable_label(built_in, "Built-in artwork")
                .on_hover_text(
                    "Select the frozen original silhouette masks; colors and physical card settings are unchanged",
                )
                .clicked()
            {
                card.procedural_recipe = None;
            }
            if ui
                .selectable_label(!built_in, "Generated artwork")
                .clicked()
                && card.procedural_recipe.is_none()
            {
                // Materializing the frozen recipe preserves the exact current meadow image while
                // making every parameter independently editable.
                card.procedural_recipe = Some(GroundCoverBladeRecipe::built_in_v1());
            }
        });
        if ui
            .small_button("Restore original meadow visual")
            .on_hover_text(
                "Restore the initial artwork, colors, card dimensions, flattening, and wind values. Use Apply visual to commit it as one undoable command.",
            )
            .clicked()
        {
            restore_original_meadow_visual(card);
        }
        if let Some(recipe) = card.procedural_recipe.as_mut() {
            egui::Grid::new("ground_cover_artwork_recipe_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Variants / blades");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut recipe.variant_count).range(1..=8));
                        ui.add(egui::DragValue::new(&mut recipe.blade_count).range(1..=128));
                    });
                    ui.end_row();
                    ui.label("Artwork seed");
                    ui.add(egui::DragValue::new(&mut recipe.seed).speed(1));
                    ui.end_row();
                    ui.label("Blade height min / max");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut recipe.minimum_blade_height)
                                .range(0.05..=1.0)
                                .speed(0.01),
                        );
                        ui.add(
                            egui::DragValue::new(&mut recipe.maximum_blade_height)
                                .range(0.05..=1.0)
                                .speed(0.01),
                        );
                    });
                    ui.end_row();
                    ui.label("Blade half-width min / max");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut recipe.minimum_blade_half_width)
                                .range(0.001..=0.1)
                                .speed(0.001),
                        );
                        ui.add(
                            egui::DragValue::new(&mut recipe.maximum_blade_half_width)
                                .range(0.001..=0.1)
                                .speed(0.001),
                        );
                    });
                    ui.end_row();
                    ui.label("Base jitter");
                    ui.add(
                        egui::DragValue::new(&mut recipe.base_jitter)
                            .range(0.0..=1.0)
                            .speed(0.01),
                    );
                    ui.end_row();
                    ui.label("Lean / curve / S-curve");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut recipe.maximum_lean)
                                .range(0.0..=0.5)
                                .speed(0.005),
                        );
                        ui.add(
                            egui::DragValue::new(&mut recipe.maximum_curve)
                                .range(0.0..=0.5)
                                .speed(0.005),
                        );
                        ui.add(
                            egui::DragValue::new(&mut recipe.maximum_s_curve)
                                .range(0.0..=0.25)
                                .speed(0.005),
                        );
                    });
                    ui.end_row();
                });
        } else {
            ui.weak("Frozen original meadow recipe: 4 variants × 34 blades.");
        }
        egui::Grid::new("ground_cover_visual_card_grid")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Bottom color");
                ui.color_edit_button_rgb(&mut card.bottom_color);
                ui.end_row();
                ui.label("Top color");
                ui.color_edit_button_rgb(&mut card.top_color);
                ui.end_row();
                ui.label("Card height min / max");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut card.minimum_card_height).range(0.01..=8.0));
                    ui.add(egui::DragValue::new(&mut card.maximum_card_height).range(0.01..=8.0));
                });
                ui.end_row();
                ui.label("Card width min / max");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut card.minimum_card_width).range(0.01..=4.0));
                    ui.add(egui::DragValue::new(&mut card.maximum_card_width).range(0.01..=4.0));
                });
                ui.end_row();
                ui.label("Flattened chance");
                ui.add(
                    egui::DragValue::new(&mut card.flattened_card_probability)
                        .range(0.0..=1.0)
                        .speed(0.01),
                );
                ui.end_row();
                ui.label("Max wind offset");
                ui.add(
                    egui::DragValue::new(&mut card.maximum_wind_displacement)
                        .range(0.0..=4.0)
                        .speed(0.01),
                );
                ui.end_row();
            });
        show_ground_cover_artwork_preview(
            ui,
            &mut state.artwork_preview_key,
            &mut state.artwork_preview_variant,
            &mut state.artwork_preview_texture,
            card,
        );
        let dimensions_valid = card.minimum_card_height > 0.0
            && card.maximum_card_height >= card.minimum_card_height
            && card.minimum_card_width > 0.0
            && card.maximum_card_width >= card.minimum_card_width;
        let artwork_valid = card
            .procedural_recipe
            .is_none_or(GroundCoverBladeRecipe::is_valid);
        let current = catalog.current_visual(project, draft.id);
        let can_apply = current.as_ref().is_some_and(|current| current != draft)
            && dimensions_valid
            && artwork_valid
            && !draft.display_name.trim().is_empty()
            && !catalog.saving()
            && !catalog.has_any_conflict();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(can_apply, egui::Button::new("Apply visual"))
                .clicked()
                && let Some(before) = current.clone()
            {
                let after = draft.clone();
                if history.update_ground_cover_visual(catalog, before, after.clone()) {
                    *draft = after;
                }
            }
            if ui.small_button("Reset").clicked()
                && let Some(current) = catalog.current_visual(project, draft.id)
            {
                *draft = current;
            }
            if ui.small_button("Duplicate").clicked()
                && let Some(source) = current.clone()
            {
                let mut duplicate = source;
                let id = Uuid::new_v4();
                duplicate.id = GroundCoverVisualId(*id.as_bytes());
                duplicate.key = duplicate_catalog_key(&duplicate.key, id);
                duplicate.display_name = format!("{} Copy", duplicate.display_name);
                duplicate.source_revision = 0;
                if history.create_ground_cover_visual(catalog, duplicate.clone()) {
                    created_visual = Some(duplicate);
                }
            }
            if ui
                .add_enabled(current.is_some(), egui::Button::new("Delete"))
                .on_hover_text(
                    "Creates an undoable tombstone. Save validates project-wide preset dependencies.",
                )
                .clicked()
                && let Some(record) = current.clone()
                && history.delete_ground_cover_definition(
                    catalog,
                    world_db::GroundCoverCatalogRecord::Visual(record),
                )
            {
                deleted_visual = true;
            }
        });
        if !dimensions_valid {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Maximum dimensions must be greater than or equal to minimum dimensions.",
            );
        }
        if !artwork_valid {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Artwork ranges must be ordered and remain inside their supported limits.",
            );
        }
    }
    if let Some(record) = created_visual {
        state.selected_visual = Some(record.id);
        state.visual_draft = Some(record);
    } else if deleted_visual {
        state.selected_visual = None;
        state.visual_draft = None;
    }

    ui.separator();
    ui.strong("Selected region");
    let selected_region = ground_cover_tool.selected_region;
    if state.selected_region != selected_region || state.region_revision != regions.edit_revision()
    {
        state.selected_region = selected_region;
        state.region_revision = regions.edit_revision();
        state.region_draft = selected_region.and_then(|id| regions.current(project, id));
    }
    let Some(draft) = state.region_draft.as_mut() else {
        ui.weak("Select a grass region in the World window to edit its assignment.");
        return;
    };
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut draft.display_name);
    });
    ui.checkbox(&mut draft.enabled, "Enabled");
    let selected = presets
        .iter()
        .find(|preset| preset.id == draft.preset)
        .map_or("Missing preset", |preset| preset.display_name.as_str());
    egui::ComboBox::from_id_salt("ground_cover_region_preset")
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for preset in &presets {
                ui.selectable_value(&mut draft.preset, preset.id, &preset.display_name);
            }
        });
    ui.horizontal(|ui| {
        ui.label("Density multiplier");
        ui.add(
            egui::DragValue::new(&mut draft.density_multiplier)
                .range(0.01..=20.0)
                .speed(0.01),
        );
    });
    let current = regions.current(project, draft.id);
    let can_apply = current.as_ref().is_some_and(|current| current != draft)
        && !draft.display_name.trim().is_empty()
        && draft.density_multiplier.is_finite()
        && draft.density_multiplier > 0.0
        && !regions.saving()
        && !regions.has_any_conflict();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(can_apply, egui::Button::new("Apply region"))
            .clicked()
            && let Some(before) = current
        {
            let after = draft.clone();
            if history.update_region(regions, before, after.clone()) {
                *draft = after;
            }
        }
        if ui.small_button("Reset").clicked()
            && let Some(current) = regions.current(project, draft.id)
        {
            *draft = current;
        }
    });
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
            diagnostic_row(ui, "Grass clusters", stats.ground_cover_clusters);
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
        "{} terrain page(s) · {} grass mask(s) · {} layer(s) · {} dense dirty · {} retained",
        dense_domains.terrain_record_count(),
        dense_domains.ground_cover_record_count(),
        dense_domains.ground_cover_layer_count(),
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

fn duplicate_catalog_key(source: &str, id: Uuid) -> String {
    let suffix = id.simple().to_string();
    format!("{source}-copy-{}", &suffix[..8])
}

fn show_ground_cover_artwork_preview(
    ui: &mut egui::Ui,
    preview_key: &mut Option<ArtworkPreviewKey>,
    preview_variant: &mut u8,
    preview_texture: &mut Option<egui::TextureHandle>,
    card: &SourceGroundCoverCardVisualRecord,
) {
    let recipe = card
        .procedural_recipe
        .unwrap_or_else(GroundCoverBladeRecipe::built_in_v1);
    if !recipe.is_valid() {
        return;
    }
    *preview_variant = (*preview_variant).min(recipe.variant_count - 1);
    ui.horizontal_wrapped(|ui| {
        ui.label("Mask preview");
        for variant in 0..recipe.variant_count {
            ui.selectable_value(preview_variant, variant, format!("{}", variant + 1));
        }
    });

    let key = ArtworkPreviewKey {
        recipe,
        bottom_color: card.bottom_color,
        top_color: card.top_color,
        variant: *preview_variant,
    };
    if *preview_key != Some(key) {
        let artwork = generate_ground_cover_card_artwork(recipe);
        let coverage = artwork
            .base_variant(*preview_variant)
            .expect("validated recipe produces the selected variant");
        let side = usize::from(artwork.resolution);
        let mut pixels = Vec::with_capacity(side * side);
        for y in 0..side {
            let height = 1.0 - y as f32 / (side - 1) as f32;
            let tint = [
                card.bottom_color[0] + (card.top_color[0] - card.bottom_color[0]) * height,
                card.bottom_color[1] + (card.top_color[1] - card.bottom_color[1]) * height,
                card.bottom_color[2] + (card.top_color[2] - card.bottom_color[2]) * height,
            ];
            for x in 0..side {
                let alpha = f32::from(coverage[y * side + x]) / 255.0;
                let background = if ((x / 16) + (y / 16)) % 2 == 0 {
                    0.075
                } else {
                    0.11
                };
                let channel = |value: f32| {
                    ((background * (1.0 - alpha) + value * alpha).clamp(0.0, 1.0) * 255.0).round()
                        as u8
                };
                pixels.push(egui::Color32::from_rgb(
                    channel(tint[0]),
                    channel(tint[1]),
                    channel(tint[2]),
                ));
            }
        }
        *preview_texture = Some(ui.ctx().load_texture(
            "ground-cover generated artwork preview",
            egui::ColorImage::new([side, side], pixels),
            egui::TextureOptions::LINEAR,
        ));
        *preview_key = Some(key);
    }
    if let Some(texture) = preview_texture {
        ui.image((texture.id(), egui::vec2(180.0, 180.0)));
    }
}

fn restore_original_meadow_visual(card: &mut SourceGroundCoverCardVisualRecord) {
    *card = SourceGroundCoverCardVisualRecord::original_meadow_v1();
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
            GROUND_COVER_WINDOW,
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

    #[test]
    fn original_meadow_restore_replaces_artwork_and_every_physical_visual_value() {
        let mut card = SourceGroundCoverCardVisualRecord::original_meadow_v1();
        card.procedural_recipe = Some(GroundCoverBladeRecipe {
            blade_count: 1,
            ..GroundCoverBladeRecipe::built_in_v1()
        });
        card.bottom_color = [1.0; 3];
        card.top_color = [0.0; 3];
        card.minimum_card_height = 0.1;
        card.maximum_card_height = 2.0;
        card.minimum_card_width = 0.1;
        card.maximum_card_width = 2.0;
        card.flattened_card_probability = 0.9;
        card.maximum_wind_displacement = 1.5;

        restore_original_meadow_visual(&mut card);

        assert_eq!(
            card,
            SourceGroundCoverCardVisualRecord::original_meadow_v1()
        );
    }
}
