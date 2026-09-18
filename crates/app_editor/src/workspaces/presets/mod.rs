//! Shared environment preset authoring, isolated from map-layer overrides and camera state.
pub(crate) mod collection;
mod fixture;
mod road_styles;
#[cfg(test)]
mod tests;
mod ui;
mod viewport;
mod visual_changes;
use super::EditorWorkspace;
use crate::{
    domain_editing::DenseDomainWorkingSets, project_store::ProjectEditorStore, shell::EditorUiSet,
};
use bevy::prelude::*;
use bevy_egui::{EguiPrimaryContextPass, egui};
use environment::{PresetId, PresetLibrary};
use fixture::Footprint;
use world::{TerrainSurfaceId, WorldSpaceId};

pub(crate) use viewport::PresetWorkspaceCamera;
pub(crate) struct PresetWorkspacePlugin;
impl Plugin for PresetWorkspacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PresetAuthoringState>()
            .init_resource::<viewport::PreviewState>()
            .add_systems(Startup, viewport::setup)
            .add_systems(OnEnter(EditorWorkspace::Presets), viewport::enter)
            .add_systems(OnExit(EditorWorkspace::Presets), viewport::leave)
            .add_systems(
                Update,
                viewport::update.run_if(in_state(EditorWorkspace::Presets)),
            )
            .add_systems(
                EguiPrimaryContextPass,
                ui::draw
                    .in_set(EditorUiSet::Workspace)
                    .run_if(in_state(EditorWorkspace::Presets)),
            );
    }
}
#[derive(Clone)]
struct Draft {
    base: PresetLibrary,
    library: PresetLibrary,
}
impl Draft {
    fn dirty(&self) -> bool {
        self.base != self.library
    }
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Environment,
    Roads,
}
#[derive(Resource)]
pub(crate) struct PresetAuthoringState {
    mode: Mode,
    styles: road_styles::Styles,
    requested: Option<(WorldSpaceId, Option<PresetId>)>,
    space: Option<WorldSpaceId>,
    selected: Option<PresetId>,
    draft: Option<Draft>,
    search: String,
    navigation: Vec<PresetId>,
    revision: u64,
    size: u16,
    footprint: Footprint,
    base: Option<TerrainSurfaceId>,
    underlay: Option<PresetId>,
    yaw: f32,
    pitch: f32,
    distance: f32,
    focus_height: f32,
    fit_distance: f32,
    playing: bool,
    phase: f32,
    message: Option<String>,
    texture: egui::TextureId,
    image: Handle<Image>,
}
impl Default for PresetAuthoringState {
    fn default() -> Self {
        Self {
            mode: Mode::Environment,
            styles: default(),
            requested: None,
            space: None,
            selected: None,
            draft: None,
            search: String::new(),
            navigation: vec![],
            revision: 0,
            size: 8,
            footprint: Footprint::Patch,
            base: None,
            underlay: None,
            yaw: 35.0,
            pitch: 40.0,
            distance: 11.0,
            focus_height: 0.25,
            fit_distance: 11.0,
            playing: false,
            phase: 0.0,
            message: None,
            texture: egui::TextureId::default(),
            image: default(),
        }
    }
}
impl PresetAuthoringState {
    pub(crate) fn open(&mut self, space: WorldSpaceId, preset: Option<PresetId>) {
        self.mode = Mode::Environment;
        self.requested = Some((space, preset));
    }
    pub(crate) fn open_road(
        &mut self,
        space: WorldSpaceId,
        style: Option<environment::roads::RoadProfileId>,
    ) {
        self.mode = Mode::Roads;
        self.requested = Some((space, None));
        self.styles.request(style);
        self.bump();
    }
    pub(crate) fn dirty(&self) -> bool {
        self.draft.as_ref().is_some_and(Draft::dirty) || self.styles.dirty()
    }
    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
    }
    fn refresh(&mut self, dense: &DenseDomainWorkingSets, project: &ProjectEditorStore) {
        if self.styles.refresh(&dense.roads) {
            self.bump();
        }
        let Some(library) = dense.presets() else {
            return;
        };
        if !self.styles.reference_initialized
            && let Some(d) = &self.styles.draft
        {
            self.styles.underlay = library.presets.iter().find_map(|p| match &p.kind {
                environment::PresetKind::Foliage(f) if f.channel == d.value.vegetation_channel => {
                    Some(p.id)
                }
                _ => None,
            });
            self.styles.reference_initialized = true;
            self.bump();
        }
        if self
            .draft
            .as_ref()
            .is_none_or(|d| !d.dirty() && d.base != *library)
        {
            let visual_changed = self
                .draft
                .as_ref()
                .is_none_or(|d| !visual_changes::same_library(&d.library, library));
            self.draft = Some(Draft {
                base: library.clone(),
                library: library.clone(),
            });
            if visual_changed {
                self.bump();
            }
        }
        if let Some((space, preset)) = self.requested.take() {
            if self.space != Some(space) {
                self.base = dense.definition(space).map(|d| d.base_surface);
            }
            self.space = Some(space);
            if let Some(preset) = preset {
                self.select(preset);
            }
            self.navigation.clear();
        }
        if self.space.is_none() {
            self.space = project.manifest().map(|m| m.default_world_space);
        }
        if self.base.is_none() {
            self.base = self
                .space
                .and_then(|s| dense.definition(s))
                .map(|d| d.base_surface);
        }
        if self.selected.is_some_and(|id| {
            self.draft
                .as_ref()
                .is_some_and(|d| d.library.get(id).is_none())
        }) {
            self.selected = None;
            self.navigation.clear();
        }
        if self.selected.is_none()
            && let Some(p) = library.presets.first()
        {
            self.select(p.id);
        }
    }
    fn select(&mut self, id: PresetId) {
        if self.selected != Some(id) {
            if let Some(old) = self.selected {
                self.navigation.push(old);
                if self.navigation.len() > 32 {
                    self.navigation.remove(0);
                }
            }
            self.selected = Some(id);
            self.underlay = None;
            self.bump();
        }
    }
    fn preview_underlay(&self) -> Option<PresetId> {
        if self.mode == Mode::Roads {
            self.styles.underlay
        } else {
            self.underlay
        }
    }
    fn camera(&self) -> Transform {
        let yaw = self.yaw.to_radians();
        let pitch = self.pitch.to_radians();
        let target = Vec3::new(
            f32::from(self.size) / 2.0,
            self.focus_height,
            f32::from(self.size) / 2.0,
        );
        Transform::from_translation(
            target
                + Vec3::new(
                    yaw.sin() * pitch.cos(),
                    pitch.sin(),
                    yaw.cos() * pitch.cos(),
                ) * self.distance,
        )
        .looking_at(target, Vec3::Y)
    }
}
