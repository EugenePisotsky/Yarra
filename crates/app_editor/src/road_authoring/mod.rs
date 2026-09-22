//! Road source gestures share the editor history, save transaction and environment preview.
pub(crate) mod commands;
mod junctions;
mod loader;
mod viewport;
pub(crate) mod working;
use crate::{
    domain_editing::DenseDomainWorkingSets,
    editing::EditorHistory,
    project_store::ProjectEditorStore,
    tools::{EditorToolRegistry, ROAD_TOOL},
    workspaces::EditorWorkspace,
};
use bevy::prelude::*;
use bevy_egui::egui;
use environment::{PresetKind, roads::*};
use working::RoadChange;
use world::{CellCoord, WorldSpaceId};
use world_db::{RoadRecordKey, RoadSourceRecord};

pub(crate) struct RoadAuthoringPlugin;
impl Plugin for RoadAuthoringPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoadToolState>()
            .init_resource::<loader::RoadLoader>()
            .add_systems(
                Update,
                (loader::update, viewport::input)
                    .chain()
                    .after(crate::project_store::ProjectStoreUpdate)
                    .after(crate::domain_editing::reconcile_dense_working_sets)
                    .after(crate::workspaces::world::update_editor_camera)
                    .before(crate::environment_paint::preview::receive_preview)
                    .before(crate::workspaces::world::handle_editor_shortcuts)
                    .before(crate::saving::drive_editor_save),
            )
            .add_systems(PostUpdate, viewport::draw);
    }
}
#[derive(Resource, Default)]
pub(crate) struct RoadToolState {
    space: Option<WorldSpaceId>,
    pub(super) road: Option<RoadId>,
    pub(super) knot: Option<RoadKnotId>,
    pub(super) span: Option<RoadSpanId>,
    style: Option<RoadProfileId>,
    pub(crate) style_request: Option<(WorldSpaceId, Option<RoadProfileId>)>,
    pub(super) creating: bool,
    pub(super) first: Option<RoadPoint>,
    pub(super) extending: bool,
    pub(super) branching: bool,
    pub(super) connecting: bool,
    pub(super) hover: Option<RoadPoint>,
    drag: Option<viewport::Drag>,
    pub(super) status: Option<String>,
    pub(crate) load_error: Option<String>,
    pub(crate) ready: bool,
    loaded_space: Option<WorldSpaceId>,
    ready_window: Option<(WorldSpaceId, RoadCellBounds)>,
    pub(super) refresh: u64,
}
impl RoadToolState {
    fn can_create_at(&self, space: WorldSpaceId, cell: CellCoord) -> bool {
        self.ready
            && self
                .ready_window
                .is_some_and(|(loaded, bounds)| loaded == space && bounds.contains(cell))
    }
    fn discovery_status(&self, space: WorldSpaceId) -> &'static str {
        if self.loaded_space == Some(space) {
            "Nearby roads · 5 × 5 cells around the editing focus"
        } else {
            "Loading nearby road controls…"
        }
    }
    fn commit(
        &mut self,
        dense: &mut DenseDomainWorkingSets,
        history: &mut EditorHistory,
        changes: Result<Vec<RoadChange>, String>,
    ) -> bool {
        let result = changes.and_then(|changes| {
            let before = dense.roads.changes(changes.iter().map(|c| c.key));
            dense.roads.apply(&changes)?;
            let after = dense.roads.changes(changes.iter().map(|c| c.key));
            history.record_roads(before, after);
            Ok(())
        });
        match result {
            Ok(()) => {
                self.status = None;
                true
            }
            Err(e) => {
                self.status = Some(e);
                false
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn inspector(
    ui: &mut egui::Ui,
    state: &mut RoadToolState,
    dense: &mut DenseDomainWorkingSets,
    history: &mut EditorHistory,
    space: Option<WorldSpaceId>,
    preview: &crate::environment_paint::EnvironmentPreview,
    busy: bool,
) {
    ui.heading("Roads");
    ui.weak("Click a curve or point to select it. Drag points and cyan handles to shape the road. Esc cancels a drag.");
    if let Some(e) = &state.load_error {
        ui.colored_label(egui::Color32::YELLOW, e);
    }
    if let Some(e) = &state.status {
        ui.colored_label(egui::Color32::YELLOW, e);
    }
    if let Some(e) = &dense.roads.error {
        ui.colored_label(egui::Color32::YELLOW, e);
    }
    if let Some(e) = &preview.error {
        ui.colored_label(egui::Color32::YELLOW, format!("Preview: {e}"));
        ui.weak("Last valid terrain and grass remain visible.");
    }
    if dense.roads.conflict {
        if ui
            .button("Discard road edits and reload (clears undo)")
            .clicked()
        {
            dense.roads.discard();
            history.clear();
            state.refresh = state.refresh.wrapping_add(1);
        }
        return;
    }
    let Some(space) = space else { return };
    let Some(size) = dense.definition(space).map(|d| d.cell_size) else {
        ui.weak("Loading environment…");
        return;
    };
    ui.add(egui::Label::new(egui::RichText::new(state.discovery_status(space)).weak()).truncate());
    let disabled = busy || dense.saving() || dense.gesture_active || dense.has_any_conflict();
    ui.collapsing("History and recovery", |ui| {
        if ui
            .add_enabled(!disabled, egui::Button::new("Clear undo history"))
            .on_hover_text("Release all editor undo/redo steps. Saved and unsaved content is kept.")
            .clicked()
        {
            history.clear();
            state.refresh = state.refresh.wrapping_add(1);
        }
        if ui
            .add_enabled(!disabled, egui::Button::new("Retry road query"))
            .clicked()
        {
            state.refresh = state.refresh.wrapping_add(1);
        }
    });
    ui.add_enabled_ui(!disabled, |ui| {
        let styles = dense.roads.entries.values().filter_map(|e| match &e.current {
            Some(RoadSourceRecord::Profile(p)) if dense.presets().and_then(|l| l.get(p.ground)).is_some_and(|g| matches!(&g.kind, PresetKind::Ground(g) if g.surfaces.iter().all(|s| dense.definition(space).unwrap().surfaces.contains(&s.surface)))) => Some(p.clone()),
            _ => None,
        }).collect::<Vec<_>>();
        if !styles.iter().any(|p| Some(p.id) == state.style) { state.style = styles.first().map(|p| p.id); }
        ui.label("Style for new roads");
        egui::ComboBox::from_id_salt("road-style").selected_text(styles.iter().find(|p| Some(p.id) == state.style).map_or("Create a style…", |p| p.name.as_str())).show_ui(ui, |ui| {
            for p in &styles { ui.selectable_value(&mut state.style, Some(p.id), &p.name); }
        });
        if ui.button("Road style library…").clicked() { state.style_request = Some((space, state.style)); }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    state.ready && state.style.is_some(),
                    egui::Button::new("New cart road"),
                )
                .clicked()
            {
                state.branching = false;
                state.connecting = false;
                state.creating = true;
                state.extending = false;
                state.first = None;
                state.status = Some("Click the start and end on the ground".into());
            }
            if (state.creating || state.extending || state.branching || state.connecting) && ui.button("Cancel").clicked() {
                state.creating = false;
                state.extending = false;
                state.branching = false;
                state.connecting = false;
                state.first = None;
                state.status = None;
            }
        });
        ui.separator();
        ui.label("Nearby roads");
        let roads = dense
            .roads
            .entries
            .values()
            .filter_map(|e| match &e.current {
                Some(RoadSourceRecord::Road(r)) if r.space == space => Some(r.road.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        egui::ScrollArea::vertical()
            .id_salt("road-list")
            .max_height(130.0)
            .show(ui, |ui| {
                for r in &roads {
                    if ui
                        .selectable_label(state.road == Some(r.id), &r.name)
                        .clicked()
                    {
                        state.road = Some(r.id);
                        state.knot = None;
                        state.span = None;
                    }
                }
            });
        let Some(id) = state.road else { return };
        let Some(RoadSourceRecord::Road(source)) =
            dense.roads.get(RoadRecordKey::Road(id)).cloned()
        else {
            return;
        };
        let mut next = source.clone();
        ui.separator();
        // Text remains in egui's temporary state until explicitly applied.
        let name_id = ui.make_persistent_id(("road-name", id.0));
        let mut name = ui
            .data_mut(|d| d.get_temp::<String>(name_id))
            .unwrap_or_else(|| source.road.name.clone());
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut name);
            if ui.button("Rename").clicked() {
                next.road.name = name.trim().to_owned();
            }
        });
        ui.data_mut(|d| d.insert_temp(name_id, name));
        ui.label("Selected road style");
        egui::ComboBox::from_id_salt("selected-road-style").selected_text(styles.iter().find(|p| p.id == source.road.profile).map_or("Unavailable style", |p| p.name.as_str())).show_ui(ui, |ui| {
            for p in &styles { ui.selectable_value(&mut next.road.profile, p.id, &p.name); }
        });
        if ui.button("Edit this shared style…").clicked() { state.style_request = Some((space, source.road.profile.into())); }
        ui.checkbox(&mut next.road.enabled, "Enabled");
        egui::ComboBox::from_id_salt("road-direction")
            .selected_text(format!("{:?}", next.road.direction))
            .show_ui(ui, |ui| {
                for (v, label) in [
                    (TravelDirection::Bidirectional, "Two-way"),
                    (TravelDirection::Forward, "Forward"),
                    (TravelDirection::Reverse, "Reverse"),
                ] {
                    ui.selectable_value(&mut next.road.direction, v, label);
                }
            });
        ui.weak("Direction is source metadata; NPC routing is not connected yet.");
        if next != source {
            let changes = next.road.validate().map_err(|e| e.to_string()).and_then(|_| {
                if next.road.profile != source.road.profile {
                    if !state.ready { return Err("Wait for road dependencies to load".into()); }
                    let p = styles.iter().find(|p| p.id == next.road.profile).ok_or("Missing style")?;
                    validate_route_style(&dense.roads, id, p)?;
                }
                Ok(())
            }).map(|_| {
                vec![RoadChange {
                    key: RoadRecordKey::Road(id),
                    record: Some(RoadSourceRecord::Road(next)),
                }]
            });
            state.commit(dense, history, changes);
        }
        if let Some(knot) = state.knot {
            ui.label("Selected point — drag the orange width handle to adjust shoulders.");
            if ui.button("Extend from this endpoint").clicked() {
                state.branching = false;
                state.connecting = false;
                state.extending = true;
                state.first = None;
                state.creating = false;
                state.status = Some("Click the next endpoint on the ground".into());
            }
            junctions::inspector(ui, state, dense, history, knot, size);
            if !junctions::ready(&dense.roads, knot) {
                ui.weak("Loading adjoining curves…");
            }
        }
        if let Some(span) = state.span {
            ui.horizontal(|ui| {
                if ui.button("Split curve").clicked() {
                    let changes = commands::split(&dense.roads, span, size);
                    state.commit(dense, history, changes);
                }
                if ui.button("Remove section").clicked() {
                    let changes = commands::delete_span(&dense.roads, span);
                    if state.commit(dense, history, changes) {
                        state.span = None;
                        state.knot = None;
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests;

/// Includes the saved minimum over the whole route and unsaved local knot changes.
fn validate_route_style(
    w: &working::RoadWorkingSet,
    route: RoadId,
    p: &CartTrackProfile,
) -> Result<(), String> {
    for e in w.entries.values() {
        if let Some(RoadSourceRecord::Junction(j)) = &e.current
            && j.junction.profile != p.id
            && j.junction.knots.iter().any(|k| matches!(w.get(RoadRecordKey::Knot(*k)), Some(RoadSourceRecord::Knot(k)) if k.road == route))
        {
            return Err("Disconnect this road from its junction before assigning a different style".into());
        }
    }
    let saved = w.route_minimum_widths.get(&route).copied().flatten();
    let local = w.entries.values().filter_map(|e| match &e.current {
        Some(RoadSourceRecord::Knot(k)) if k.road == route => Some(k.knot.width),
        _ => None,
    });
    if saved
        .into_iter()
        .chain(local)
        .any(|v| v < p.minimum_width())
    {
        return Err(format!(
            "Style needs a {:.2} m corridor. Widen and save this road before assigning it.",
            p.minimum_width()
        ));
    }
    Ok(())
}
