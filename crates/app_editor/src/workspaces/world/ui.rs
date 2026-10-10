//! World toolbar and floating-window composition. Visibility does not activate authoring tools.
//! Each window is its own system, drawn after the toolbar while it is open.
use crate::{
    domain_editing::SourceWorkingSets,
    editing::{EditorHistory, EditorObjectWorkingSet, EditorSelection, TransformInspectorDraft},
    environment_paint::{EnvironmentLayerBrowser, EnvironmentPaintState, EnvironmentPreview},
    journal::EditorJournalStatus,
    listings::ProjectListings,
    project_store::ProjectEditorStore,
    publication::RuntimePublicationState,
    road_authoring::RoadToolState,
    saving::EditorSaveCoordinator,
    shell::{EditorUiFrame, EditorWindowDescriptor, EditorWindowId, EditorWindowRegistry},
    tools::EditorToolRegistry,
    vegetation_authoring::VegetationAuthoringState,
    workspaces::{
        EditorWorkspace,
        presets::PresetAuthoringState,
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
    diagnostic::DiagnosticsStore, ecs::system::SystemParam,
    gizmos::transform_gizmo::TransformGizmoSettings, prelude::*,
};
use bevy_egui::egui;
use engine::{ActiveWorldSpace, StreamingStats, WorldCatalog, WorldOrigin, WorldViewpoint};

pub(crate) use toolbar::world_toolbar;

mod assets;
mod diagnostics;
mod hierarchy;
mod inspector;
mod navigator;
mod toolbar;

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

const MARGIN: f32 = 12.0;

/// Where a window first opens within the workspace area, and its default size.
struct Layout {
    position: fn(egui::Rect) -> [f32; 2],
    size: [f32; 2],
    scroll: bool,
}

/// Shows a World window while it is open in the registry; its close button updates the registry.
fn show_window(
    frame: &mut EditorUiFrame,
    windows: &mut EditorWindowRegistry,
    descriptor: EditorWindowDescriptor,
    layout: Layout,
    contents: impl FnOnce(&mut egui::Ui, &mut EditorWindowRegistry),
) {
    let Some(viewport_ui) = frame.0.as_mut() else {
        return;
    };
    if !windows.is_open(descriptor.id) {
        return;
    }
    let context = viewport_ui.ctx().clone();
    let area = viewport_ui.available_rect_before_wrap();
    let mut open = true;
    egui::Window::new(descriptor.label)
        .id(egui::Id::new(descriptor.id.0))
        .open(&mut open)
        .default_pos((layout.position)(area))
        .default_size(layout.size)
        .constrain_to(area)
        .resizable(true)
        .vscroll(layout.scroll)
        .show(&context, |ui| contents(ui, windows));
    windows.set_open(descriptor.id, open);
}

#[allow(clippy::too_many_arguments)] // Bevy system parameters.
pub(crate) fn hierarchy_window(
    mut frame: ResMut<EditorUiFrame>,
    mut windows: ResMut<EditorWindowRegistry>,
    catalog: Res<WorldCatalog>,
    project: Res<ProjectEditorStore>,
    mut selection: ResMut<EditorSelection>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut active_space: ResMut<ActiveWorldSpace>,
    mut tools: ResMut<EditorToolRegistry>,
    mut ui_state: ResMut<WorldWorkspaceUiState>,
) {
    let layout = Layout {
        position: |area| [area.left() + MARGIN, area.top() + MARGIN],
        size: [285.0, 560.0],
        scroll: false,
    };
    show_window(
        &mut frame,
        &mut windows,
        WORLD_WINDOW,
        layout,
        |ui, windows| {
            draw_world_hierarchy(
                ui,
                &catalog,
                &project,
                &mut selection,
                &mut objects,
                &mut active_space,
                &mut tools,
                &mut ui_state,
                windows,
            );
        },
    );
}

#[derive(SystemParam)]
pub(crate) struct InspectorResources<'w> {
    catalog: Res<'w, WorldCatalog>,
    viewpoint: Res<'w, WorldViewpoint>,
    origin: Res<'w, WorldOrigin>,
    tools: Res<'w, EditorToolRegistry>,
    gizmo_settings: Res<'w, TransformGizmoSettings>,
    environment_preview: Res<'w, EnvironmentPreview>,
    vegetation: Res<'w, VegetationAuthoringState>,
    save: Res<'w, EditorSaveCoordinator>,
    publication: Res<'w, RuntimePublicationState>,
    presets: Res<'w, PresetAuthoringState>,
    project: Res<'w, ProjectEditorStore>,
    dense_domains: ResMut<'w, SourceWorkingSets>,
    selection: ResMut<'w, EditorSelection>,
    objects: ResMut<'w, EditorObjectWorkingSet>,
    history: ResMut<'w, EditorHistory>,
    transform_draft: ResMut<'w, TransformInspectorDraft>,
    focus_request: ResMut<'w, EditorCameraFocusRequest>,
    roads: ResMut<'w, RoadToolState>,
    paint: ResMut<'w, EnvironmentPaintState>,
    layer_browser: ResMut<'w, EnvironmentLayerBrowser>,
}

pub(crate) fn inspector_window(
    mut frame: ResMut<EditorUiFrame>,
    mut windows: ResMut<EditorWindowRegistry>,
    resources: InspectorResources,
) {
    let InspectorResources {
        catalog,
        viewpoint,
        origin,
        tools,
        gizmo_settings,
        environment_preview,
        vegetation,
        save,
        publication,
        presets,
        project,
        mut dense_domains,
        mut selection,
        mut objects,
        mut history,
        mut transform_draft,
        mut focus_request,
        mut roads,
        mut paint,
        mut layer_browser,
    } = resources;
    let layout = Layout {
        position: |area| [area.right() - 332.0 - MARGIN, area.top() + MARGIN],
        size: [332.0, 560.0],
        scroll: true,
    };
    show_window(
        &mut frame,
        &mut windows,
        INSPECTOR_WINDOW,
        layout,
        |ui, _| {
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
                vegetation.working_catalog().map(|(catalog, _, _)| catalog),
                save.active()
                    || publication.active()
                    || project.save_in_flight()
                    || presets.dirty(),
            );
        },
    );
}

#[allow(clippy::too_many_arguments)] // Bevy system parameters.
pub(crate) fn assets_window(
    mut frame: ResMut<EditorUiFrame>,
    mut windows: ResMut<EditorWindowRegistry>,
    viewpoint: Res<WorldViewpoint>,
    mut listings: ResMut<ProjectListings>,
    mut project: ResMut<ProjectEditorStore>,
    mut selection: ResMut<EditorSelection>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut history: ResMut<EditorHistory>,
    mut object_palette: ResMut<EditorObjectPalette>,
    mut transform_draft: ResMut<TransformInspectorDraft>,
    mut tools: ResMut<EditorToolRegistry>,
) {
    let layout = Layout {
        position: |area| [area.left() + 315.0, area.top() + MARGIN],
        size: [430.0, 480.0],
        scroll: false,
    };
    show_window(&mut frame, &mut windows, ASSETS_WINDOW, layout, |ui, _| {
        draw_asset_browser(
            ui,
            &viewpoint,
            &mut listings,
            &mut project,
            &mut selection,
            &mut objects,
            &mut history,
            &mut object_palette,
            &mut transform_draft,
            &mut tools,
        );
    });
}

pub(crate) fn navigator_window(
    mut frame: ResMut<EditorUiFrame>,
    mut windows: ResMut<EditorWindowRegistry>,
    mut listings: ResMut<ProjectListings>,
    mut selection: ResMut<EditorSelection>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut transform_draft: ResMut<TransformInspectorDraft>,
    mut tools: ResMut<EditorToolRegistry>,
) {
    let layout = Layout {
        position: |area| [area.left() + 315.0, area.top() + 70.0],
        size: [390.0, 440.0],
        scroll: false,
    };
    show_window(
        &mut frame,
        &mut windows,
        NAVIGATOR_WINDOW,
        layout,
        |ui, _| {
            draw_navigator(
                ui,
                &mut listings,
                &mut selection,
                &mut objects,
                &mut transform_draft,
                &mut tools,
            );
        },
    );
}

#[allow(clippy::too_many_arguments)] // Bevy system parameters.
pub(crate) fn diagnostics_window(
    mut frame: ResMut<EditorUiFrame>,
    mut windows: ResMut<EditorWindowRegistry>,
    diagnostics: Res<DiagnosticsStore>,
    catalog: Res<WorldCatalog>,
    viewpoint: Res<WorldViewpoint>,
    origin: Res<WorldOrigin>,
    stats: Res<StreamingStats>,
    dense_domains: Res<SourceWorkingSets>,
    journal: Res<EditorJournalStatus>,
    listings: Res<ProjectListings>,
    publication: Res<RuntimePublicationState>,
    project: Res<ProjectEditorStore>,
    objects: Res<EditorObjectWorkingSet>,
    history: Res<EditorHistory>,
    tools: Res<EditorToolRegistry>,
) {
    let layout = Layout {
        position: |area| [area.center().x - 220.0, area.bottom() - 430.0],
        size: [440.0, 410.0],
        scroll: true,
    };
    show_window(
        &mut frame,
        &mut windows,
        DIAGNOSTICS_WINDOW,
        layout,
        |ui, _| {
            draw_world_diagnostics(
                ui,
                &diagnostics,
                &catalog,
                &viewpoint,
                &origin,
                &stats,
                &dense_domains,
                &journal,
                &listings,
                &publication,
                &project,
                &objects,
                &history,
                &tools,
            );
        },
    );
}

/// A preset or road style the inspector asked to edit opens the Presets workspace.
pub(crate) fn open_requested_presets(
    frame: Res<EditorUiFrame>,
    mut paint: ResMut<EnvironmentPaintState>,
    mut roads: ResMut<RoadToolState>,
    mut presets: ResMut<PresetAuthoringState>,
    mut next_workspace: ResMut<NextState<EditorWorkspace>>,
) {
    if frame.0.is_none() {
        return;
    }
    if let Some((space, preset)) = paint.preset_request.take() {
        presets.open(space, preset);
        next_workspace.set(EditorWorkspace::Presets);
    }
    if let Some((space, style)) = roads.style_request.take() {
        presets.open_road(space, style);
        next_workspace.set(EditorWorkspace::Presets);
    }
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
