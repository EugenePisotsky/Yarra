//! Gameplay area authoring: named ground polygons the game's rules react to.
//!
//! The Areas tool draws and reshapes them in the viewport; the Areas window names them and
//! sets their height range. The whole set is one project record that saves with everything
//! else and reaches the game on publish without recooking any terrain.
#[cfg(test)]
mod tests;
mod viewport;
pub(crate) mod working;

use crate::{
    domain_editing::SourceWorkingSets,
    editing::EditorHistory,
    project_store::{ProjectEditorStore, ProjectStoreUpdate},
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::{
        EditorUiFrame, EditorUiSet, EditorWindowDescriptor, EditorWindowId, EditorWindowRegistry,
    },
    tools::{AREA_TOOL, EditorToolRegistry},
    workspaces::{EditorWorkspace, world::EditorCameraFocusRequest, world_workspace_active},
};
use bevy::prelude::*;
use bevy_egui::{EguiPrimaryContextPass, egui};
use engine::{WorldCatalog, WorldOrigin};
use std::sync::Arc;
use world::{GameplayArea, WorldPosition, WorldSpaceId};

pub(crate) const WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
    id: EditorWindowId("world.areas"),
    workspace: EditorWorkspace::World,
    label: "Areas",
    default_open: false,
};

pub(crate) struct AreaAuthoringPlugin;
impl Plugin for AreaAuthoringPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AreaToolState>()
            .add_systems(
                Update,
                (reconcile, end_gesture_away, viewport::input)
                    .chain()
                    .after(ProjectStoreUpdate)
                    .after(crate::journal::restore_loaded_journal)
                    .after(crate::workspaces::world::update_editor_camera)
                    .before(crate::workspaces::world::handle_editor_shortcuts)
                    .before(crate::saving::drive_editor_save),
            )
            .add_systems(PostUpdate, viewport::draw)
            .add_systems(
                EguiPrimaryContextPass,
                window
                    .in_set(EditorUiSet::Workspace)
                    .run_if(world_workspace_active),
            );
    }
}

#[derive(Resource, Default)]
pub(crate) struct AreaToolState {
    /// The world the selection and the shape being drawn belong to.
    space: Option<WorldSpaceId>,
    /// Name of the area being edited.
    selected: Option<String>,
    /// The corner of it picked last, for removal.
    corner: Option<usize>,
    /// Corners of an area being drawn, in world X/Z.
    draft: Option<Vec<[f64; 2]>>,
    /// The set before the gesture under way. A gesture becomes one undo step when it ends.
    gesture: Option<Arc<[GameplayArea]>>,
    /// The corner the pointer is dragging, and how far from the pointer it was grabbed.
    dragging: Option<(usize, [f64; 2])>,
    /// World X/Z under the pointer.
    hover: Option<[f64; 2]>,
    /// Lowest and highest ground under the selected area's corners, where terrain is loaded.
    ground: Option<[f32; 2]>,
    status: Option<String>,
}

impl AreaToolState {
    fn selected_index(&self, areas: &[GameplayArea]) -> Option<usize> {
        let name = self.selected.as_deref()?;
        areas.iter().position(|area| area.name == name)
    }

    /// Replaces the set as one undoable step.
    fn commit(
        &mut self,
        dense: &mut SourceWorkingSets,
        history: &mut EditorHistory,
        areas: Vec<GameplayArea>,
    ) -> bool {
        let before = dense.areas.areas().clone();
        match dense.areas.apply(areas.into()) {
            Ok(()) => {
                history.record_areas(before, dense.areas.areas().clone());
                self.status = None;
                true
            }
            Err(error) => {
                self.status = Some(error);
                false
            }
        }
    }

    /// Replaces the set as part of a gesture: a corner drag or a value being scrubbed.
    fn edit(&mut self, dense: &mut SourceWorkingSets, areas: Vec<GameplayArea>) -> bool {
        let before = dense.areas.areas().clone();
        match dense.areas.apply(areas.into()) {
            Ok(()) => {
                if self.gesture.is_none() {
                    self.gesture = Some(before);
                    dense.gesture_active = true;
                }
                self.status = None;
                true
            }
            Err(error) => {
                self.status = Some(error);
                false
            }
        }
    }

    /// Ends the gesture, recording it for undo or putting back what was there before it.
    fn finish(&mut self, dense: &mut SourceWorkingSets, history: &mut EditorHistory, cancel: bool) {
        self.dragging = None;
        let Some(before) = self.gesture.take() else {
            return;
        };
        dense.gesture_active = false;
        if !cancel {
            history.record_areas(before, dense.areas.areas().clone());
        } else if let Err(error) = dense.areas.apply(before) {
            self.status = Some(error);
        }
    }
}

/// The first of `area-1`, `area-2`, … that no area uses yet.
fn unused_name(areas: &[GameplayArea]) -> String {
    (1..)
        .map(|n| format!("area-{n}"))
        .find(|name| areas.iter().all(|area| &area.name != name))
        .expect("an unbounded sequence of names")
}

/// The set with one area changed.
fn with_area(
    areas: &[GameplayArea],
    index: usize,
    change: impl FnOnce(&mut GameplayArea),
) -> Vec<GameplayArea> {
    let mut areas = areas.to_vec();
    change(&mut areas[index]);
    areas
}

fn tool_active(tools: &EditorToolRegistry) -> bool {
    tools
        .active(EditorWorkspace::World)
        .is_some_and(|tool| tool.id == AREA_TOOL.id)
}

/// A gesture still under way when the World workspace is left ends there, as one undo
/// step. Its window is out of sight and cannot end it, and an open gesture holds up saving.
fn end_gesture_away(
    workspace: Res<State<EditorWorkspace>>,
    mut state: ResMut<AreaToolState>,
    mut dense: ResMut<SourceWorkingSets>,
    mut history: ResMut<EditorHistory>,
) {
    if *workspace.get() != EditorWorkspace::World && state.gesture.is_some() {
        state.finish(&mut dense, &mut history, false);
    }
}

fn reconcile(
    mut dense: ResMut<SourceWorkingSets>,
    mut project: ResMut<ProjectEditorStore>,
    mut save: ResMut<EditorSaveCoordinator>,
) {
    if let Some(record) = project.gameplay_areas() {
        dense.areas.pin(record);
    }
    if let Some((id, result)) = project.area_completion.take()
        && dense.areas.saving == Some(id)
    {
        let committed = dense.areas.complete(result);
        save.transaction_finished(committed);
    }
}

#[allow(clippy::too_many_arguments)]
fn window(
    mut frame: ResMut<EditorUiFrame>,
    mut windows: ResMut<EditorWindowRegistry>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    tools: Res<EditorToolRegistry>,
    mut dense: ResMut<SourceWorkingSets>,
    mut history: ResMut<EditorHistory>,
    mut state: ResMut<AreaToolState>,
    mut focus: ResMut<EditorCameraFocusRequest>,
    publication: Res<RuntimePublicationState>,
    save: Res<EditorSaveCoordinator>,
) {
    let Some(root) = frame.0.as_mut() else {
        return;
    };
    // A scrubbed or typed value is one undo step, taken when the pointer lifts or the field
    // is left.
    if state.dragging.is_none()
        && !root.ctx().input(|i| i.pointer.any_down())
        && (!root.ctx().text_edit_focused()
            || root.ctx().input(|i| i.key_pressed(egui::Key::Enter)))
    {
        state.finish(&mut dense, &mut history, false);
    }
    let mut open = windows.is_open(WINDOW.id);
    if !open {
        return;
    }
    let rect = root.available_rect_before_wrap();
    egui::Window::new("Areas")
        .id(egui::Id::new(WINDOW.id.0))
        .open(&mut open)
        .default_width(320.0)
        .default_height(420.0)
        .default_pos(rect.left_top() + egui::vec2(325.0, 60.0))
        .constrain_to(rect)
        .vscroll(true)
        .show(root.ctx(), |ui| {
            let Some(space) = origin.space() else {
                ui.label("Waiting for world…");
                return;
            };
            if !dense.areas.loaded() {
                ui.label("Loading gameplay areas…");
                return;
            }
            ui.small(
                "Named places the game's rules react to. Gameplay content refers to an area \
                 by its name. Save & Publish updates the game.",
            );
            if let Some(theirs) = dense.areas.conflict.as_ref().map(|c| c.areas.len()) {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("The project now holds {theirs} area(s) saved by someone else."),
                );
                ui.horizontal(|ui| {
                    if ui.button("Keep mine").clicked() {
                        dense.areas.resolve_conflict(true);
                    }
                    if ui.button("Take theirs").clicked() {
                        dense.areas.resolve_conflict(false);
                        history.clear();
                    }
                });
                return;
            }
            let busy = publication.active() || save.active() || dense.saving();
            ui.add_enabled_ui(!busy, |ui| {
                contents(
                    ui,
                    space,
                    &catalog,
                    tool_active(&tools),
                    &mut state,
                    &mut dense,
                    &mut history,
                    &mut focus,
                );
            });
            for message in [&state.status, &dense.areas.error].into_iter().flatten() {
                ui.colored_label(ui.visuals().error_fg_color, message);
            }
        });
    windows.set_open(WINDOW.id, open);
}

#[allow(clippy::too_many_arguments)]
fn contents(
    ui: &mut egui::Ui,
    space: WorldSpaceId,
    catalog: &WorldCatalog,
    tool_active: bool,
    state: &mut AreaToolState,
    dense: &mut SourceWorkingSets,
    history: &mut EditorHistory,
    focus: &mut EditorCameraFocusRequest,
) {
    let areas = dense.areas.areas().clone();
    ui.separator();
    if !tool_active {
        ui.label("Choose Areas in the World window to draw and reshape in the viewport.");
    } else if state.draft.is_some() {
        ui.label(
            "Click corners on the terrain. Click the first corner or press Enter to close \
             the shape. Backspace takes the last corner back; Esc cancels.",
        );
        if ui.button("Cancel drawing").clicked() {
            state.draft = None;
        }
    } else {
        if ui.button("Draw new area").clicked() {
            state.draft = Some(Vec::new());
            state.selected = None;
            state.corner = None;
        }
        ui.small(
            "Click an area to select it. Drag a corner to move it; click an edge to add a \
             corner there.",
        );
    }
    ui.separator();
    let elsewhere = areas.iter().filter(|area| area.space != space).count();
    egui::ScrollArea::vertical()
        .max_height(180.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for area in areas.iter().filter(|area| area.space == space) {
                let selected = state.selected.as_deref() == Some(area.name.as_str());
                if ui.selectable_label(selected, &area.name).clicked() && !selected {
                    state.selected = Some(area.name.clone());
                    state.corner = None;
                    state.draft = None;
                }
            }
        });
    if areas.len() == elsewhere {
        ui.weak("No areas in this world yet.");
    }
    if elsewhere > 0 {
        ui.weak(format!("{elsewhere} more in other worlds."));
    }
    let Some(index) = state.selected_index(&areas) else {
        return;
    };
    let area = &areas[index];
    ui.separator();
    // Text stays in egui's temporary state until applied: a half-typed name is not a name.
    let name_id = ui.make_persistent_id(("area-name", &area.name));
    let mut name = ui
        .data_mut(|d| d.get_temp::<String>(name_id))
        .unwrap_or_else(|| area.name.clone());
    let mut rename = false;
    ui.horizontal(|ui| {
        let field = ui.text_edit_singleline(&mut name);
        rename = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        rename |= ui.button("Rename").clicked();
    });
    ui.data_mut(|d| d.insert_temp(name_id, name.clone()));
    if rename && name != area.name {
        let renamed = with_area(&areas, index, |area| area.name = name.clone());
        if state.commit(dense, history, renamed) {
            state.selected = Some(name);
            ui.data_mut(|d| d.remove::<String>(name_id));
        }
        return;
    }
    ui.small("Lowercase letters, digits and _ - / . such as guard/gate_post.");

    ui.label(format!("{} corners", area.points.len()));
    let mut limited = area.height.is_some();
    if ui
        .checkbox(&mut limited, "Limit height")
        .on_hover_text(
            "Off: anyone above or below the shape is inside. On: only between two heights, \
             for places under a bridge or on one floor.",
        )
        .changed()
    {
        let height = limited.then(|| {
            let [low, high] = state.ground.unwrap_or([0., 0.]);
            [low.floor() - 2., high.ceil() + 4.]
        });
        let changed = with_area(&areas, index, |area| area.height = height);
        state.commit(dense, history, changed);
        return;
    }
    if let Some([mut low, mut high]) = area.height {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("From");
            changed |= ui
                .add(egui::DragValue::new(&mut low).speed(0.1).suffix(" m"))
                .changed();
            ui.label("to");
            changed |= ui
                .add(egui::DragValue::new(&mut high).speed(0.1).suffix(" m"))
                .changed();
        });
        if changed {
            let height = Some([low.min(high), high.max(low)]);
            let changed = with_area(&areas, index, |area| area.height = height);
            state.edit(dense, changed);
            return;
        }
    }
    ui.horizontal(|ui| {
        if ui.button("Go to").clicked() {
            let [x, z] = area.interior_point();
            let size = catalog
                .world_space(area.space)
                .map_or(world::DEFAULT_CELL_SIZE, |space| space.cell_size);
            let y = f64::from(state.ground.map_or(0., |[low, _]| low));
            focus.0 = Some(WorldPosition::from_world(area.space, [x, y, z], size));
        }
        let corner = state.corner.filter(|&c| c < area.points.len());
        if ui
            .add_enabled(
                corner.is_some() && area.points.len() > 3,
                egui::Button::new("Remove corner"),
            )
            .on_hover_text("Removes the corner clicked last. Delete does the same.")
            .clicked()
            && let Some(corner) = corner
        {
            let changed = with_area(&areas, index, |area| {
                area.points.remove(corner);
            });
            state.commit(dense, history, changed);
            state.corner = None;
        }
        if ui.button("Delete area").clicked() {
            let mut remaining = areas.to_vec();
            remaining.remove(index);
            if state.commit(dense, history, remaining) {
                state.selected = None;
                state.corner = None;
            }
        }
    });
}
