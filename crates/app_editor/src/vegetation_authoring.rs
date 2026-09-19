//! Live vegetation catalog authoring for the World workspace.
//!
//! The editor owns a validated working catalog and persists it as a small global project-source
//! domain. Runtime publication remains explicit: Save updates the mutable project database, while
//! Save & Publish cooks and atomically adopts the immutable database read by the game.

use bevy::prelude::*;
use bevy_egui::{EguiPrimaryContextPass, egui};
use engine::{StreamedTerrainSurface, StreamedVegetationFieldPage, WorldOrigin};
use vegetation::{
    GrowthPattern, RibbonCurveProfile, TopologyProfile, VegetationCatalog, VegetationFieldPage,
    VegetationGroupingProfile, VegetationScene, VegetationSurfaceField, VoronoiClumpProfile,
};
use vegetation_render::{
    VegetationDebugMode, VegetationDebugScene, VegetationDebugSettings, VegetationDensityMode,
    VegetationDiagnostics, VegetationLighting, VegetationLightingMode, VegetationProfileMode,
    VegetationRenderPlugin,
};

use crate::{
    project_store::{ProjectEditorStore, VegetationSaveOutcome},
    saving::EditorSaveCoordinator,
    shell::{
        EditorUiFrame, EditorUiSet, EditorWindowDescriptor, EditorWindowId, EditorWindowRegistry,
    },
    tools::{EditorToolRegistry, VEGETATION_TOOL},
    workspaces::{EditorWorkspace, world_workspace_active},
};

pub(crate) const VEGETATION_WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
    id: EditorWindowId("world.vegetation"),
    workspace: EditorWorkspace::World,
    label: "Vegetation",
    default_open: false,
};

pub(crate) struct VegetationAuthoringPlugin;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct VegetationPreviewSync;

impl Plugin for VegetationAuthoringPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(VegetationRenderPlugin)
            .init_resource::<VegetationAuthoringState>()
            .insert_resource(
                VegetationDebugScene::new(VegetationScene {
                    catalog: vegetation::fixtures::reference_catalog(),
                    pages: Vec::new(),
                })
                .expect("the empty vegetation authoring scene is valid"),
            )
            .add_systems(
                Update,
                (adopt_project_catalog, sync_live_preview)
                    .chain()
                    .in_set(VegetationPreviewSync)
                    .after(engine::WorldStreamingSystems),
            )
            .add_systems(
                EguiPrimaryContextPass,
                vegetation_authoring_ui
                    .run_if(world_workspace_active)
                    .in_set(EditorUiSet::Workspace),
            );
    }
}

#[derive(Resource)]
pub(crate) struct VegetationAuthoringState {
    baseline: Option<VegetationCatalog>,
    working: Option<VegetationCatalog>,
    selected_population: usize,
    selected_species: usize,
    revision: u64,
    dirty: bool,
    preview_enabled: bool,
    curve_editor_scale: f32,
    validation_error: Option<String>,
    preview_error: Option<String>,
    save_request: Option<u64>,
    save_error: Option<String>,
    conflict_actual: Option<Option<VegetationCatalog>>,
}

impl Default for VegetationAuthoringState {
    fn default() -> Self {
        Self {
            baseline: None,
            working: None,
            selected_population: 0,
            selected_species: 0,
            revision: 1,
            dirty: false,
            preview_enabled: true,
            curve_editor_scale: 1.35,
            validation_error: None,
            preview_error: None,
            save_request: None,
            save_error: None,
            conflict_actual: None,
        }
    }
}

impl VegetationAuthoringState {
    pub(crate) fn study_source(&self) -> Option<(&VegetationCatalog, usize, u64)> {
        self.working
            .as_ref()
            .map(|catalog| (catalog, self.selected_population, self.revision))
    }

    /// Explicit replay import replaces the draft while retaining the project's save baseline.
    pub(crate) fn import_study_catalog(&mut self, catalog: VegetationCatalog, population: &str) {
        self.selected_population = catalog
            .populations
            .iter()
            .position(|p| p.key == population)
            .unwrap_or(0);
        self.selected_species = catalog
            .populations
            .get(self.selected_population)
            .and_then(|p| p.species.first())
            .and_then(|s| catalog.species.iter().position(|v| v.id == s.species))
            .unwrap_or(0);
        self.apply(catalog);
    }

    fn install(&mut self, catalog: VegetationCatalog) {
        self.selected_population = catalog
            .populations
            .iter()
            .position(|population| {
                matches!(population.grouping, VegetationGroupingProfile::Voronoi(_))
            })
            .unwrap_or(0);
        self.selected_species = catalog
            .populations
            .get(self.selected_population)
            .and_then(|population| population.species.first())
            .and_then(|choice| {
                catalog
                    .species
                    .iter()
                    .position(|species| species.id == choice.species)
            })
            .unwrap_or(0);
        self.baseline = Some(catalog.clone());
        self.working = Some(catalog);
        self.dirty = false;
        self.validation_error = None;
        self.preview_error = None;
        self.save_request = None;
        self.save_error = None;
        self.conflict_actual = None;
        self.bump_revision();
    }

    fn apply(&mut self, catalog: VegetationCatalog) {
        match catalog.validate() {
            Ok(()) => {
                self.working = Some(catalog);
                self.dirty = self.working != self.baseline;
                self.validation_error = None;
                self.bump_revision();
            }
            Err(error) => self.validation_error = Some(error.to_string()),
        }
    }

    fn reset(&mut self) {
        if let Some(baseline) = self.baseline.clone() {
            self.working = Some(baseline);
            self.dirty = false;
            self.validation_error = None;
            self.preview_error = None;
            self.save_error = None;
            self.conflict_actual = None;
            self.bump_revision();
        }
    }

    pub(crate) const fn dirty_count(&self) -> usize {
        self.dirty as usize
    }

    pub(crate) const fn saving(&self) -> bool {
        self.save_request.is_some()
    }

    pub(crate) const fn has_conflict(&self) -> bool {
        self.conflict_actual.is_some()
    }

    pub(crate) fn queue_save(&mut self, project: &mut ProjectEditorStore) -> bool {
        if !self.dirty || self.saving() || self.has_conflict() {
            return false;
        }
        let Some(replacement) = self.working.clone() else {
            return false;
        };
        let Some(request_id) = project.queue_vegetation_catalog(self.baseline.clone(), replacement)
        else {
            return false;
        };
        self.save_request = Some(request_id);
        self.save_error = None;
        true
    }

    fn finish_save(&mut self, request_id: u64, outcome: VegetationSaveOutcome) {
        if self.save_request != Some(request_id) {
            return;
        }
        self.save_request = None;
        match outcome {
            VegetationSaveOutcome::Committed(saved) => {
                self.baseline = Some(saved);
                self.dirty = self.working != self.baseline;
                self.save_error = None;
                self.conflict_actual = None;
            }
            VegetationSaveOutcome::Conflict { actual } => {
                self.save_error = Some(
                    "The project vegetation catalog changed after this draft was opened.".into(),
                );
                self.conflict_actual = Some(actual);
            }
            VegetationSaveOutcome::Failed(error) => {
                self.save_error = Some(format!("Vegetation save failed: {error}"));
            }
        }
    }

    fn reload_conflict(&mut self) {
        let Some(actual) = self.conflict_actual.take() else {
            return;
        };
        self.save_error = None;
        if let Some(actual) = actual {
            self.install(actual);
        } else {
            self.baseline = None;
            self.working = None;
            self.dirty = false;
            self.bump_revision();
        }
    }

    fn keep_draft_after_conflict(&mut self) {
        let Some(actual) = self.conflict_actual.take() else {
            return;
        };
        self.baseline = actual;
        self.dirty = self.working != self.baseline;
        self.save_error = None;
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
    }
}

pub(crate) fn process_vegetation_save_completion(
    mut project: ResMut<ProjectEditorStore>,
    mut state: ResMut<VegetationAuthoringState>,
    mut coordinator: ResMut<EditorSaveCoordinator>,
) {
    let Some(completion) = project.take_vegetation_save_completion() else {
        return;
    };
    let committed = matches!(&completion.outcome, VegetationSaveOutcome::Committed(_));
    state.finish_save(completion.request_id, completion.outcome);
    coordinator.transaction_finished(committed);
}

fn adopt_project_catalog(
    project: Res<ProjectEditorStore>,
    mut state: ResMut<VegetationAuthoringState>,
) {
    if state.baseline.is_some() {
        return;
    }
    let Some(source_catalog) = project.vegetation_catalog().cloned() else {
        return;
    };
    state.install(source_catalog);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewSignature {
    authoring_revision: u64,
    environment_revision: u64,
    enabled: bool,
    active_space: Option<world::WorldSpaceId>,
    origin_cell: world::CellCoord,
    pages: Vec<(Entity, i64, i32, i32, u8)>,
}

#[allow(clippy::too_many_arguments)]
fn sync_live_preview(
    runtime: Res<engine::WorldCatalog>,
    paint: Res<crate::environment_paint::EnvironmentPaintState>,
    workspace: Res<State<EditorWorkspace>>,
    environment: Res<crate::environment_paint::EnvironmentPreview>,
    origin: Res<WorldOrigin>,
    terrain_pages: Query<(Entity, &StreamedTerrainSurface)>,
    field_pages: Query<(Entity, &StreamedVegetationFieldPage)>,
    mut state: ResMut<VegetationAuthoringState>,
    mut scene: ResMut<VegetationDebugScene>,
    mut previous: Local<Option<PreviewSignature>>,
    render_origin: Option<ResMut<vegetation_render::VegetationRenderOrigin>>,
) {
    if matches!(
        *workspace.get(),
        EditorWorkspace::Vegetation | EditorWorkspace::Presets
    ) {
        if let Some(mut render_origin) = render_origin {
            render_origin.world_xz = [0.; 2];
        }
        *previous = None;
        return;
    }
    let tool_active = *workspace.get() == EditorWorkspace::World;
    let enabled =
        tool_active && state.preview_enabled && state.working.is_some() && !paint.coverage_visible;
    let active_space = origin.space();
    let fields = active_space.map_or_else(Vec::new, |space| {
        let mut fields = field_pages
            .iter()
            .filter(|(_, page)| page.key.space == space)
            .collect::<Vec<_>>();
        fields.sort_by_key(|(entity, page)| (page.key, *entity));
        fields
    });
    let mut page_signature = if enabled {
        fields
            .iter()
            .map(|(entity, page)| {
                (
                    *entity,
                    page.key.space.0,
                    page.key.cell.x,
                    page.key.cell.z,
                    page.key.lod,
                )
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if enabled {
        page_signature.extend(terrain_pages.iter().map(|(entity, page)| {
            (
                entity,
                page.key.space.0,
                page.key.cell.x,
                page.key.cell.z,
                page.key.lod | 0x80,
            )
        }));
        page_signature.sort();
    }
    let signature = PreviewSignature {
        authoring_revision: state.revision,
        environment_revision: environment.revision,
        enabled,
        active_space,
        origin_cell: origin.cell(),
        pages: page_signature,
    };
    if previous.as_ref() == Some(&signature) {
        return;
    }

    // The accepted catalog and fields switch together after a definition edit. While a new
    // catalog is compiling, keep rendering the previously accepted ground-and-grass products.
    let Some(catalog) = environment
        .catalog()
        .or_else(|| runtime.vegetation())
        .cloned()
    else {
        return;
    };
    let pages = if enabled {
        terrain_pages
            .iter()
            .filter(|(_, terrain)| Some(terrain.key.space) == active_space)
            .filter_map(|(_, terrain)| {
                let data = environment
                    .cell(terrain.key.space, terrain.key.cell)
                    .map(|cell| &cell.vegetation)
                    .or_else(|| {
                        if environment.catalog().is_some() {
                            return None;
                        }
                        fields
                            .iter()
                            .find(|(_, page)| page.key == terrain.key)
                            .map(|(_, page)| &page.data)
                    })?;
                let cell_origin = terrain.key.cell.origin(terrain.cell_size);
                let render_origin = origin.cell().origin(terrain.cell_size);
                let resolution = terrain.heightfield.resolution;
                let sample_count = usize::from(resolution).pow(2);
                Some(VegetationFieldPage::from_data(
                    [
                        (cell_origin[0] - render_origin[0]) as f32,
                        (cell_origin[1] - render_origin[1]) as f32,
                    ],
                    terrain.cell_size,
                    VegetationSurfaceField {
                        resolution,
                        heights: (0..sample_count)
                            .map(|index| {
                                terrain.heightfield.height_at(
                                    index % usize::from(resolution),
                                    index / usize::from(resolution),
                                )
                            })
                            .collect(),
                        normals_oct: terrain.heightfield.normals_oct.clone(),
                        validity: vec![u8::MAX; sample_count],
                    },
                    data.clone(),
                ))
            })
            .collect()
    } else {
        Vec::new()
    };
    match scene.replace(VegetationScene { catalog, pages }) {
        Ok(()) => {
            state.preview_error = None;
        }
        Err(error) => {
            state.preview_error = Some(format!("Live preview rejected the scene: {error}"));
        }
    }
    *previous = Some(signature);
}

fn vegetation_authoring_ui(
    mut frame: ResMut<EditorUiFrame>,
    mut windows: ResMut<EditorWindowRegistry>,
    mut tools: ResMut<EditorToolRegistry>,
    mut state: ResMut<VegetationAuthoringState>,
    diagnostics: Res<VegetationDiagnostics>,
    mut settings: ResMut<VegetationDebugSettings>,
    mut lighting: ResMut<VegetationLighting>,
    mut save: ResMut<EditorSaveCoordinator>,
    mut project: ResMut<ProjectEditorStore>,
) -> Result {
    if !windows.is_open(VEGETATION_WINDOW.id) {
        return Ok(());
    }
    let Some(viewport_ui) = frame.0.as_mut() else {
        return Ok(());
    };
    let context = viewport_ui.ctx().clone();
    let workspace_rect = viewport_ui.available_rect_before_wrap();
    let mut open = true;
    egui::Window::new("Vegetation")
        .id(egui::Id::new(VEGETATION_WINDOW.id.0))
        .open(&mut open)
        .default_pos([workspace_rect.right() - 770.0, workspace_rect.top() + 90.0])
        .default_size([420.0, 720.0])
        .constrain_to(workspace_rect)
        .resizable(true)
        .vscroll(true)
        .show(&context, |ui| {
            if ui.button("Canopy…").clicked() {
                windows.set_open(crate::canopy::CANOPY_WINDOW.id, true);
            }
            draw_vegetation_authoring(
                ui,
                &mut tools,
                &mut state,
                diagnostics.snapshot(),
                &mut settings,
                &mut lighting,
                &mut save,
                &mut project,
                true,
            );
        });
    windows.set_open(VEGETATION_WINDOW.id, open);
    Ok(())
}

pub(crate) fn draw_vegetation_authoring(
    ui: &mut egui::Ui,
    tools: &mut EditorToolRegistry,
    state: &mut VegetationAuthoringState,
    diagnostics: vegetation_render::VegetationDiagnosticsSnapshot,
    settings: &mut VegetationDebugSettings,
    lighting: &mut VegetationLighting,
    save: &mut EditorSaveCoordinator,
    project: &mut ProjectEditorStore,
    world_controls: bool,
) {
    let active = tools
        .active(EditorWorkspace::World)
        .is_some_and(|tool| tool.id == VEGETATION_TOOL.id);
    ui.horizontal(|ui| {
        if world_controls && ui.selectable_label(active, "Activate tool").clicked() {
            tools.set_active(EditorWorkspace::World, VEGETATION_TOOL.id);
        }
        if world_controls {
            ui.checkbox(&mut state.preview_enabled, "Live preview");
        }
        if ui
            .add_enabled(state.dirty, egui::Button::new("Revert draft"))
            .on_hover_text("Discard unsaved edits and restore the project catalog")
            .clicked()
        {
            state.reset();
        }
        let can_save = state.dirty
            && !state.saving()
            && !state.has_conflict()
            && !save.active()
            && !project.save_in_flight()
            && project.write_error().is_none();
        if ui
            .add_enabled(can_save, egui::Button::new("Save"))
            .on_hover_text("Persist this catalog in the project database")
            .clicked()
        {
            save.request_save();
        }
        let can_publish = !state.saving()
            && !state.has_conflict()
            && !save.active()
            && !project.save_in_flight()
            && project.write_error().is_none()
            && (state.dirty || project.source_epoch() > 0);
        let publish_label = if state.dirty {
            "Save & Publish"
        } else {
            "Publish"
        };
        if ui
            .add_enabled(can_publish, egui::Button::new(publish_label))
            .on_hover_text(
                "Persist pending edits, cook, and adopt the runtime database used by the game",
            )
            .clicked()
        {
            save.request_publish();
        }
    });
    ui.small("Edits preview immediately. Save writes the project catalog; Save & Publish also rebuilds the runtime database that the game loads automatically.");
    if state.dirty {
        ui.colored_label(egui::Color32::YELLOW, "Modified working draft");
    } else {
        ui.weak("Project catalog baseline");
    }
    if let Some(error) = &state.validation_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    if let Some(error) = &state.preview_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    if state.saving() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Saving vegetation catalog…");
        });
    }
    if let Some(error) = &state.save_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    if state.has_conflict() {
        ui.horizontal(|ui| {
            if ui.button("Reload project version").clicked() {
                state.reload_conflict();
            }
            if ui.button("Keep draft and retry").clicked() {
                state.keep_draft_after_conflict();
            }
        });
    }

    let Some(mut candidate) = state.working.clone() else {
        ui.separator();
        ui.weak("Waiting for the project vegetation catalog…");
        return;
    };
    ui.separator();
    let mut changed = false;
    changed |= draw_population_editor(ui, state, &mut candidate);
    ui.separator();
    changed |= draw_species_editor(ui, state, &mut candidate);
    if changed {
        state.apply(candidate);
    }

    ui.separator();
    draw_preview_controls(ui, settings, lighting);
    draw_diagnostics(ui, diagnostics);
}

fn draw_population_editor(
    ui: &mut egui::Ui,
    state: &mut VegetationAuthoringState,
    catalog: &mut VegetationCatalog,
) -> bool {
    ui.heading("Population");
    if catalog.populations.is_empty() {
        ui.weak("No populations in the catalog.");
        return false;
    }
    state.selected_population = state
        .selected_population
        .min(catalog.populations.len().saturating_sub(1));
    let selected_key = catalog.populations[state.selected_population].key.clone();
    egui::ComboBox::from_id_salt("vegetation_population")
        .width(ui.available_width())
        .selected_text(selected_key)
        .show_ui(ui, |ui| {
            for (index, population) in catalog.populations.iter().enumerate() {
                if ui
                    .selectable_label(index == state.selected_population, &population.key)
                    .clicked()
                {
                    state.selected_population = index;
                    if let Some(species) = population.species.first().and_then(|choice| {
                        catalog
                            .species
                            .iter()
                            .position(|species| species.id == choice.species)
                    }) {
                        state.selected_species = species;
                    }
                    ui.close();
                }
            }
        });

    let population = &mut catalog.populations[state.selected_population];
    let mut changed = false;
    let maximum_density = match population.growth {
        GrowthPattern::Uniform { .. } => 512.0,
        GrowthPattern::ParentChild {
            parent_spacing,
            children_per_parent,
            ..
        } => (f32::from(children_per_parent) / parent_spacing.powi(2)).min(512.0),
    };
    changed |= drag_f32(
        ui,
        "Maximum density / m²",
        &mut population.density_per_square_meter,
        0.1,
        0.0001..=maximum_density,
    );
    ui.small("This is the maximum deterministic root budget before coverage and clump retention.");

    egui::CollapsingHeader::new("Root placement")
        .default_open(true)
        .show(ui, |ui| match &mut population.growth {
            GrowthPattern::Uniform { jitter } => {
                ui.label("Stratified roots");
                changed |= drag_f32(ui, "Cell jitter", jitter, 0.01, 0.0..=1.0);
            }
            GrowthPattern::ParentChild {
                parent_spacing,
                children_per_parent,
                radius,
                parent_jitter,
            } => {
                ui.label("Explicit parent/child roots");
                let maximum_spacing = (f32::from(*children_per_parent)
                    / population.density_per_square_meter)
                    .sqrt()
                    .clamp(0.05, 64.0);
                let spacing_changed = drag_f32(
                    ui,
                    "Parent spacing",
                    parent_spacing,
                    0.05,
                    0.05..=maximum_spacing,
                );
                if spacing_changed {
                    *radius = radius.min(*parent_spacing * 2.0);
                    changed = true;
                }
                let minimum_children = (population.density_per_square_meter
                    * parent_spacing.powi(2))
                .ceil()
                .clamp(1.0, 256.0) as u16;
                changed |= drag_u16(
                    ui,
                    "Children / parent",
                    children_per_parent,
                    minimum_children..=256,
                );
                changed |= drag_f32(
                    ui,
                    "Child radius",
                    radius,
                    0.02,
                    0.0..=(*parent_spacing * 2.0),
                );
                changed |= drag_f32(ui, "Parent jitter", parent_jitter, 0.01, 0.0..=1.0);
                ui.small(format!(
                    "Maximum supported density {:.2} / m²",
                    f32::from(*children_per_parent) / parent_spacing.powi(2)
                ));
            }
        });

    egui::CollapsingHeader::new("Grouping")
        .default_open(true)
        .show(ui, |ui| {
            let current_kind = GroupingKind::from(population.grouping);
            let mut requested_kind = current_kind;
            egui::ComboBox::from_id_salt("vegetation_grouping_kind")
                .selected_text(current_kind.label())
                .show_ui(ui, |ui| {
                    for kind in GroupingKind::ALL {
                        ui.selectable_value(&mut requested_kind, kind, kind.label());
                    }
                });
            if requested_kind != current_kind {
                apply_grouping_kind(population, requested_kind);
                changed = true;
            }
            match &mut population.grouping {
                VegetationGroupingProfile::None => {
                    ui.weak("No shared spatial grouping signal.");
                }
                VegetationGroupingProfile::Parent => {
                    ui.small("The explicit growth parent supplies the shared group sample.");
                }
                VegetationGroupingProfile::Voronoi(profile) => {
                    changed |= draw_voronoi_profile(ui, profile);
                    ui.small(format!(
                        "Expected maximum roots / clump: {:.1}",
                        population.density_per_square_meter * profile.spacing.powi(2)
                    ));
                }
            }
        });

    egui::CollapsingHeader::new("Rest orientation")
        .default_open(true)
        .show(ui, |ui| {
            let orientation = &mut population.orientation;
            changed |= drag_f32(
                ui,
                "Shared direction",
                &mut orientation.shared_group_weight,
                0.02,
                -4.0..=4.0,
            );
            changed |= drag_f32(
                ui,
                "Radial direction",
                &mut orientation.radial_weight,
                0.02,
                -4.0..=4.0,
            );
            changed |= drag_f32(
                ui,
                "Tangential direction",
                &mut orientation.tangential_weight,
                0.02,
                -4.0..=4.0,
            );
            changed |= drag_f32(
                ui,
                "Independent random",
                &mut orientation.random_weight,
                0.02,
                0.0..=4.0,
            );
            changed |= drag_f32(
                ui,
                "Flow field",
                &mut orientation.flow_weight,
                0.02,
                -4.0..=4.0,
            );
            changed |= drag_angle_degrees(
                ui,
                "Angular jitter",
                &mut orientation.angular_jitter_radians,
                0.0..=180.0,
            );
        });
    changed
}

fn draw_voronoi_profile(ui: &mut egui::Ui, profile: &mut VoronoiClumpProfile) -> bool {
    let mut changed = false;
    changed |= drag_f32(ui, "Clump spacing", &mut profile.spacing, 0.02, 0.05..=64.0);
    changed |= drag_f32(
        ui,
        "Feature jitter",
        &mut profile.feature_jitter,
        0.01,
        0.0..=1.0,
    );
    changed |= drag_f32(
        ui,
        "Boundary softness",
        &mut profile.boundary_softness,
        0.01,
        0.0..=1.0,
    );
    changed |= drag_f32(
        ui,
        "Root attraction",
        &mut profile.root_attraction,
        0.01,
        0.0..=1.0,
    );
    changed |= drag_f32(
        ui,
        "Center retention",
        &mut profile.center_retention,
        0.01,
        0.0..=1.0,
    );
    changed |= drag_f32(
        ui,
        "Edge retention",
        &mut profile.edge_retention,
        0.01,
        0.0..=1.0,
    );
    changed |= drag_f32(
        ui,
        "Retention falloff",
        &mut profile.retention_falloff,
        0.02,
        0.1..=8.0,
    );
    changed |= drag_f32(
        ui,
        "Clump density variation",
        &mut profile.density_variation,
        0.01,
        0.0..=1.0,
    );
    changed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupingKind {
    None,
    Parent,
    Voronoi,
}

impl GroupingKind {
    const ALL: [Self; 3] = [Self::None, Self::Parent, Self::Voronoi];

    const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Parent => "Explicit parent",
            Self::Voronoi => "Analytic Voronoi",
        }
    }
}

impl From<VegetationGroupingProfile> for GroupingKind {
    fn from(value: VegetationGroupingProfile) -> Self {
        match value {
            VegetationGroupingProfile::None => Self::None,
            VegetationGroupingProfile::Parent => Self::Parent,
            VegetationGroupingProfile::Voronoi(_) => Self::Voronoi,
        }
    }
}

fn apply_grouping_kind(population: &mut vegetation::VegetationPopulation, kind: GroupingKind) {
    match kind {
        GroupingKind::None => {
            population.grouping = VegetationGroupingProfile::None;
            population.orientation.shared_group_weight = 0.0;
            population.orientation.radial_weight = 0.0;
            population.orientation.tangential_weight = 0.0;
            if population.orientation.random_weight + population.orientation.flow_weight.abs()
                <= f32::EPSILON
            {
                population.orientation.random_weight = 1.0;
            }
        }
        GroupingKind::Parent => {
            if !matches!(population.growth, GrowthPattern::ParentChild { .. }) {
                let parent_spacing = (256.0 / population.density_per_square_meter)
                    .sqrt()
                    .min(1.5);
                let children_per_parent =
                    (population.density_per_square_meter * parent_spacing * parent_spacing)
                        .ceil()
                        .clamp(1.0, 256.0) as u16;
                population.growth = GrowthPattern::ParentChild {
                    parent_spacing,
                    children_per_parent,
                    radius: parent_spacing * 0.75,
                    parent_jitter: 0.75,
                };
            }
            population.grouping = VegetationGroupingProfile::Parent;
            population.orientation.shared_group_weight = 0.5;
        }
        GroupingKind::Voronoi => {
            if !matches!(population.growth, GrowthPattern::Uniform { .. }) {
                population.growth = GrowthPattern::Uniform { jitter: 0.85 };
            }
            population.grouping = VegetationGroupingProfile::Voronoi(VoronoiClumpProfile {
                spacing: 1.75,
                feature_jitter: 0.85,
                boundary_softness: 0.18,
                root_attraction: 0.2,
                center_retention: 1.0,
                edge_retention: 0.82,
                retention_falloff: 1.25,
                density_variation: 0.12,
            });
            population.orientation.shared_group_weight = 0.6;
            population.orientation.radial_weight = 0.8;
        }
    }
}

/// Focused palette view over the same validated draft used by the full inspector.
pub(crate) fn draw_population_colors(ui: &mut egui::Ui, state: &mut VegetationAuthoringState) {
    let Some(mut candidate) = state.working.clone() else {
        ui.weak("Waiting for the vegetation catalog…");
        return;
    };
    let Some(population) = candidate.populations.get(state.selected_population) else {
        ui.weak("Select a grass population in the inspector.");
        return;
    };
    ui.strong(&population.key);
    ui.small("Each species blends from its root color to its tip color. Clump variation changes the tint between groups.");
    let mut changed = false;
    for (index, species) in candidate.species.iter_mut().enumerate() {
        if !population
            .species
            .iter()
            .any(|choice| choice.species == species.id && choice.weight > 0.0)
        {
            continue;
        }
        ui.push_id(index, |ui| {
            ui.group(|ui| {
                ui.label(&species.key);
                changed |= draw_species_colors(ui, &mut species.material);
            });
        });
    }
    if changed {
        state.apply(candidate);
    }
    if let Some(error) = &state.validation_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
}

fn draw_species_colors(
    ui: &mut egui::Ui,
    material: &mut vegetation::VegetationMaterialProfile,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Root color");
        changed |= ui.color_edit_button_rgb(&mut material.root_color).changed();
        ui.label("Tip color");
        changed |= ui.color_edit_button_rgb(&mut material.tip_color).changed();
    });
    changed |= ui
        .add(
            egui::Slider::new(&mut material.clump_color_variation, 0.0..=1.0)
                .text("Clump variation"),
        )
        .changed();
    changed
}

fn draw_species_editor(
    ui: &mut egui::Ui,
    state: &mut VegetationAuthoringState,
    catalog: &mut VegetationCatalog,
) -> bool {
    ui.heading("Species");
    if catalog.species.is_empty() {
        ui.weak("No species in the catalog.");
        return false;
    }
    state.selected_species = state
        .selected_species
        .min(catalog.species.len().saturating_sub(1));
    let selected_key = catalog.species[state.selected_species].key.clone();
    egui::ComboBox::from_id_salt("vegetation_species")
        .width(ui.available_width())
        .selected_text(selected_key)
        .show_ui(ui, |ui| {
            for (index, species) in catalog.species.iter().enumerate() {
                if ui
                    .selectable_label(index == state.selected_species, &species.key)
                    .clicked()
                {
                    state.selected_species = index;
                    ui.close();
                }
            }
        });

    let species = &mut catalog.species[state.selected_species];
    let mut changed = false;
    egui::CollapsingHeader::new("Envelope")
        .default_open(true)
        .show(ui, |ui| {
            let minimum_height_changed = drag_f32(
                ui,
                "Minimum height",
                &mut species.bounds.minimum_height,
                0.01,
                0.01..=species.bounds.maximum_height,
            );
            let maximum_height_changed = drag_f32(
                ui,
                "Maximum height",
                &mut species.bounds.maximum_height,
                0.01,
                species.bounds.minimum_height..=16.0,
            );
            changed |= minimum_height_changed | maximum_height_changed;
            if (minimum_height_changed || maximum_height_changed)
                && species.height.pair_below_height > 0.0
            {
                species.height.pair_below_height = species
                    .height
                    .pair_below_height
                    .clamp(species.bounds.minimum_height, species.bounds.maximum_height);
            }
            changed |= drag_f32(
                ui,
                "Minimum half-width",
                &mut species.bounds.minimum_half_width,
                0.001,
                0.0001..=species.bounds.maximum_half_width,
            );
            changed |= drag_f32(
                ui,
                "Maximum half-width",
                &mut species.bounds.maximum_half_width,
                0.001,
                species.bounds.minimum_half_width..=4.0,
            );
            changed |= drag_f32(
                ui,
                "Horizontal reach",
                &mut species.bounds.maximum_horizontal_reach,
                0.01,
                0.0..=16.0,
            );
        });

    egui::CollapsingHeader::new("Height & topology allocation")
        .default_open(true)
        .show(ui, |ui| {
            changed |= drag_f32(
                ui,
                "Height distribution bias",
                &mut species.height.distribution_bias,
                0.01,
                -1.0..=1.0,
            );
            ui.weak("-1 favors short blades, 0 is neutral, and +1 favors tall blades without changing the envelope.");

            match &mut species.topology {
                TopologyProfile::Ribbon(profile) => {
                    let mut pairing_enabled = species.height.pair_below_height > 0.0;
                    if ui
                        .checkbox(&mut pairing_enabled, "Pair short blades")
                        .changed()
                    {
                        changed = true;
                        if pairing_enabled {
                            species.height.pair_below_height =
                                (species.bounds.minimum_height + species.bounds.maximum_height)
                                    * 0.5;
                            profile.blades_per_render_unit = 2;
                        } else {
                            species.height.pair_below_height = 0.0;
                            profile.blades_per_render_unit = 1;
                        }
                    }
                    if pairing_enabled {
                        changed |= drag_f32(
                            ui,
                            "Pair below height",
                            &mut species.height.pair_below_height,
                            0.01,
                            species.bounds.minimum_height..=species.bounds.maximum_height,
                        );
                        profile.blades_per_render_unit = 2;
                        ui.weak("Roots at or below this stable world-space height divide the fixed vertex budget between two blades. Taller roots spend it on one better-sampled curve.");
                    }
                }
                TopologyProfile::BroadLeafCluster(_) => {
                    ui.weak("Broad-leaf clusters keep their authored two-leaf topology; short-grass ribbon pairing does not apply.");
                }
            }

            if matches!(species.topology, TopologyProfile::Ribbon(_)) {
                let split_fraction = species.expected_split_topology_fraction();
                ui.monospace(format!(
                    "Estimated topology mix: {:.0}% single / {:.0}% paired · {:.2} blades/root",
                    (1.0 - split_fraction) * 100.0,
                    split_fraction * 100.0,
                    1.0 + split_fraction,
                ));
            }
        });

    egui::CollapsingHeader::new("Shape")
        .default_open(true)
        .show(ui, |ui| match &mut species.topology {
            TopologyProfile::Ribbon(profile) => {
                ui.label("Cubic Bezier ribbon");
                changed |= drag_f32(
                    ui,
                    "Vertex distribution",
                    &mut profile.longitudinal_power,
                    0.02,
                    0.2..=4.0,
                );
                ui.weak("Higher values move samples toward the root; paired blades use a bend-focused sample pattern.");
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("View scale");
                    ui.add(
                        egui::Slider::new(&mut state.curve_editor_scale, 1.0..=3.0)
                            .show_value(true),
                    );
                });
                ui.weak("Lower values zoom in for precise control-point placement. Curve coordinates are normalized by blade height; Envelope controls real-world scale.");
                let may_pair = species.height.pair_below_height > 0.0
                    || profile.blades_per_render_unit > 1;
                let preview_sections = if may_pair {
                    profile.high_section_count.clamp(2, 4)
                } else {
                    profile.high_section_count
                };
                if may_pair {
                    ui.weak("Markers show the paired main blade. Its companion is 80% as long with a gentler, three-section curve. Both share a full-width base and a facing direction.");
                }
                let maximum_tip_tilt = profile.curve_variant_b.tip_tilt_radians;
                changed |= draw_ribbon_curve_editor(
                    ui,
                    "Minimum silhouette",
                    "minimum",
                    &mut profile.curve_variant_a,
                    0.0..=maximum_tip_tilt,
                    preview_sections,
                    may_pair,
                    profile.longitudinal_power,
                    state.curve_editor_scale,
                );
                let minimum_tip_tilt = profile.curve_variant_a.tip_tilt_radians;
                changed |= draw_ribbon_curve_editor(
                    ui,
                    "Maximum silhouette",
                    "maximum",
                    &mut profile.curve_variant_b,
                    minimum_tip_tilt..=1.55,
                    preview_sections,
                    may_pair,
                    profile.longitudinal_power,
                    state.curve_editor_scale,
                );
                ui.weak("Drag 1 and 2 to shape the two handles. Drag T along the unit arc to place the tip. Each blade interpolates one complete minimum/maximum silhouette.");
                changed |= drag_f32(
                    ui,
                    "Lateral curve",
                    &mut profile.maximum_lateral_curve,
                    0.01,
                    0.0..=1.0,
                );
                changed |= drag_angle_degrees(
                    ui,
                    "Maximum view opening",
                    &mut profile.maximum_view_opening_radians,
                    0.0..=45.0,
                );
                ui.weak(
                    "Maximum tangent-axis silhouette opening toward the camera. It is zero for an already face-on ribbon and grows continuously toward edge-on views; 0 disables it. Start around 15-20 degrees.",
                );
                if profile.blades_per_render_unit == 2 {
                    changed |= drag_angle_degrees(
                        ui,
                        "Pair spread",
                        &mut profile.pair_spread_radians,
                        0.0..=180.0,
                    );
                }
            }
            TopologyProfile::BroadLeafCluster(profile) => {
                ui.label("Broad two-leaf cluster");
                changed |= drag_f32(
                    ui,
                    "Crown radius",
                    &mut profile.crown_radius,
                    0.01,
                    0.0..=2.0,
                );
                changed |= drag_f32(
                    ui,
                    "Minimum droop",
                    &mut profile.minimum_droop,
                    0.01,
                    0.0..=profile.maximum_droop,
                );
                changed |= drag_f32(
                    ui,
                    "Maximum droop",
                    &mut profile.maximum_droop,
                    0.01,
                    profile.minimum_droop..=2.0,
                );
                changed |= drag_f32(ui, "Camber", &mut profile.maximum_camber, 0.01, 0.0..=1.0);
            }
        });

    egui::CollapsingHeader::new("Group coherence")
        .default_open(true)
        .show(ui, |ui| {
            let response = &mut species.group_response;
            changed |= drag_f32(
                ui,
                "Height coherence",
                &mut response.height_coherence,
                0.01,
                0.0..=1.0,
            );
            changed |= drag_f32(
                ui,
                "Silhouette coherence",
                &mut response.silhouette_coherence,
                0.01,
                0.0..=1.0,
            );
            changed |= drag_f32(
                ui,
                "Lateral coherence",
                &mut response.lateral_curve_coherence,
                0.01,
                0.0..=1.0,
            );
        });

    egui::CollapsingHeader::new("Material & lighting")
        .default_open(true)
        .show(ui, |ui| {
            changed |= draw_species_colors(ui, &mut species.material);
            ui.weak(
                "Colors are authored per species and interpolated from the crowded root to the tip.",
            );
            changed |= drag_f32(
                ui,
                "Perceptual roughness",
                &mut species.material.perceptual_roughness,
                0.01,
                0.0..=1.0,
            );
            changed |= drag_f32(
                ui,
                "Transmission",
                &mut species.material.transmission,
                0.01,
                0.0..=1.0,
            );
            changed |= drag_f32(
                ui,
                "Root AO",
                &mut species.material.root_ao,
                0.01,
                0.0..=1.0,
            );
            changed |= drag_f32(
                ui,
                "Tip AO",
                &mut species.material.tip_ao,
                0.01,
                0.0..=1.0,
            );
            ui.weak(
                "AO also shapes the minimum ambient level under received shadows, so dense roots remain darker than exposed tips.",
            );
            changed |= drag_f32(
                ui,
                "Normal rounding",
                &mut species.material.normal_rounding,
                0.01,
                0.0..=1.0,
            );
            ui.weak(
                "Controls the analytic cross-section used for lighting without adding geometry. Zero is flat; existing values around 0.4 are already nearly cylindrical. It changes lighting, not silhouette.",
            );
        });
    changed
}

fn draw_ribbon_curve_editor(
    ui: &mut egui::Ui,
    label: &str,
    id_salt: &'static str,
    curve: &mut RibbonCurveProfile,
    tip_tilt_range: std::ops::RangeInclusive<f32>,
    high_section_count: u8,
    paired_main: bool,
    longitudinal_power: f32,
    chart_extent: f32,
) -> bool {
    const EDITOR_HEIGHT: f32 = 230.0;
    const CURVE_SAMPLES: usize = 64;

    let mut changed = false;
    ui.label(label);
    let desired_size = egui::vec2(ui.available_width().max(180.0), EDITOR_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let canvas = CurveCanvas::new(rect.shrink(10.0), chart_extent.clamp(0.6, 3.0));
    let original_controls = ribbon_preview_controls(*curve);

    for control_index in 1..=3 {
        let point = canvas.to_screen(original_controls[control_index]);
        let control_name = match control_index {
            1 => "Root handle",
            2 => "Tip handle",
            _ => "Tip",
        };
        let response = ui
            .interact(
                egui::Rect::from_center_size(point, egui::vec2(22.0, 22.0)),
                ui.id()
                    .with(("ribbon_curve_control", id_salt, control_index)),
                egui::Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Grab)
            .on_hover_text(control_name);
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            update_ribbon_curve_control(
                curve,
                control_index,
                canvas.from_screen(pointer),
                &tip_tilt_range,
            );
            changed = true;
        }
    }

    let controls = ribbon_preview_controls(*curve);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::from_black_alpha(52));
    draw_curve_grid(&painter, canvas);

    let unit_arc: Vec<_> = (0..=32)
        .map(|index| {
            let angle = 1.55 * index as f32 / 32.0;
            canvas.to_screen([angle.sin(), angle.cos()])
        })
        .collect();
    painter.add(egui::Shape::line(
        unit_arc,
        egui::Stroke::new(1.0, egui::Color32::from_gray(64)),
    ));
    painter.line_segment(
        [canvas.to_screen(controls[0]), canvas.to_screen(controls[1])],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(76, 156, 176)),
    );
    painter.line_segment(
        [canvas.to_screen(controls[2]), canvas.to_screen(controls[3])],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(188, 145, 58)),
    );

    let mut sampled = Vec::with_capacity(CURVE_SAMPLES + 1);
    for index in 0..=CURVE_SAMPLES {
        let t = index as f32 / CURVE_SAMPLES as f32;
        sampled.push(cubic_preview_point(controls, t));
    }
    painter.add(egui::Shape::line(
        sampled
            .iter()
            .copied()
            .map(|point| canvas.to_screen(point))
            .collect(),
        egui::Stroke::new(2.5, egui::Color32::from_rgb(116, 210, 126)),
    ));

    let sections = u32::from(high_section_count.max(1));
    for row in 0..=sections {
        let linear_t = if paired_main && sections == 4 {
            [0.0_f32, 0.215, 0.5, 0.70, 1.0][row as usize]
        } else if paired_main {
            let middle = sections.div_ceil(2).max(1);
            if row <= middle {
                0.5 * row as f32 / middle as f32
            } else {
                0.5 + 0.5 * (row - middle) as f32 / (sections - middle) as f32
            }
        } else {
            row as f32 / sections as f32
        };
        let t = linear_t.powf(longitudinal_power.max(0.2));
        painter.circle_filled(
            canvas.to_screen(cubic_preview_point(controls, t)),
            2.5,
            egui::Color32::WHITE,
        );
    }

    let point_colors = [
        egui::Color32::from_gray(110),
        egui::Color32::from_rgb(94, 200, 228),
        egui::Color32::from_rgb(238, 190, 78),
        egui::Color32::WHITE,
    ];
    let point_labels = ["R", "1", "2", "T"];
    for index in 0..4 {
        let screen = canvas.to_screen(controls[index]);
        painter.circle_filled(screen, 6.0, point_colors[index]);
        painter.text(
            screen + egui::vec2(8.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            point_labels[index],
            egui::FontId::monospace(10.0),
            point_colors[index],
        );
    }
    ui.small(format!(
        "1 ({:.3}, {:.3})   2 ({:.3}, {:.3})   T ({:.3}, {:.3})   tip {:.1}°",
        controls[1][0],
        controls[1][1],
        controls[2][0],
        controls[2][1],
        controls[3][0],
        controls[3][1],
        curve.tip_tilt_radians.to_degrees(),
    ));
    changed
}

#[derive(Clone, Copy)]
struct CurveCanvas {
    origin: egui::Pos2,
    pixels_per_unit: f32,
    rect: egui::Rect,
}

impl CurveCanvas {
    fn new(rect: egui::Rect, extent: f32) -> Self {
        let pixels_per_unit = (rect.width() / (extent * 2.0))
            .min(rect.height() / (extent * 1.5))
            .max(1.0);
        let view_height = extent * 1.5 * pixels_per_unit;
        let top = rect.center().y - view_height * 0.5;
        Self {
            origin: egui::pos2(rect.center().x, top + extent * 1.25 * pixels_per_unit),
            pixels_per_unit,
            rect,
        }
    }

    fn to_screen(self, point: [f32; 2]) -> egui::Pos2 {
        egui::pos2(
            self.origin.x + point[0] * self.pixels_per_unit,
            self.origin.y - point[1] * self.pixels_per_unit,
        )
    }

    fn from_screen(self, point: egui::Pos2) -> [f32; 2] {
        [
            (point.x - self.origin.x) / self.pixels_per_unit,
            (self.origin.y - point.y) / self.pixels_per_unit,
        ]
    }
}

fn draw_curve_grid(painter: &egui::Painter, canvas: CurveCanvas) {
    let ground_y = canvas.to_screen([0.0, 0.0]).y;
    painter.line_segment(
        [
            egui::pos2(canvas.rect.left(), ground_y),
            egui::pos2(canvas.rect.right(), ground_y),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_gray(72)),
    );
    painter.line_segment(
        [
            egui::pos2(canvas.origin.x, canvas.rect.top()),
            egui::pos2(canvas.origin.x, canvas.rect.bottom()),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_gray(52)),
    );
}

fn update_ribbon_curve_control(
    curve: &mut RibbonCurveProfile,
    control_index: usize,
    point: [f32; 2],
    tip_tilt_range: &std::ops::RangeInclusive<f32>,
) {
    match control_index {
        1 => update_polar_handle(
            point,
            -1.55..=1.55,
            &mut curve.root_tangent_radians,
            &mut curve.root_handle_length,
        ),
        2 => {
            let controls = ribbon_preview_controls(*curve);
            update_polar_handle(
                [controls[3][0] - point[0], controls[3][1] - point[1]],
                -1.55..=3.05,
                &mut curve.tip_tangent_radians,
                &mut curve.tip_handle_length,
            );
        }
        3 => {
            if point[0].hypot(point[1]) > 1e-4 {
                curve.tip_tilt_radians = point[0]
                    .atan2(point[1])
                    .clamp(*tip_tilt_range.start(), *tip_tilt_range.end());
            }
        }
        _ => {}
    }
}

fn update_polar_handle(
    vector: [f32; 2],
    angle_range: std::ops::RangeInclusive<f32>,
    angle: &mut f32,
    length: &mut f32,
) {
    let requested_length = vector[0].hypot(vector[1]);
    if requested_length > 1e-4 {
        *angle = vector[0]
            .atan2(vector[1])
            .clamp(*angle_range.start(), *angle_range.end());
    }
    *length = requested_length.clamp(0.02, 1.5);
}

fn ribbon_preview_controls(curve: RibbonCurveProfile) -> [[f32; 2]; 4] {
    let p0 = [0.0, 0.0];
    let p3 = [curve.tip_tilt_radians.sin(), curve.tip_tilt_radians.cos()];
    let p1 = [
        curve.root_tangent_radians.sin() * curve.root_handle_length,
        curve.root_tangent_radians.cos() * curve.root_handle_length,
    ];
    let p2 = [
        p3[0] - curve.tip_tangent_radians.sin() * curve.tip_handle_length,
        p3[1] - curve.tip_tangent_radians.cos() * curve.tip_handle_length,
    ];
    [p0, p1, p2, p3]
}

fn cubic_preview_point(points: [[f32; 2]; 4], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let weights = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    [
        points
            .iter()
            .zip(weights)
            .map(|(point, weight)| point[0] * weight)
            .sum(),
        points
            .iter()
            .zip(weights)
            .map(|(point, weight)| point[1] * weight)
            .sum(),
    ]
}

fn draw_preview_controls(
    ui: &mut egui::Ui,
    settings: &mut VegetationDebugSettings,
    lighting: &mut VegetationLighting,
) {
    ui.heading("Preview");
    egui::Grid::new("vegetation_preview_controls")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Visualization");
            egui::ComboBox::from_id_salt("vegetation_debug_mode")
                .selected_text(settings.mode.label())
                .show_ui(ui, |ui| {
                    for mode in [
                        VegetationDebugMode::ProceduralGeometry,
                        VegetationDebugMode::AcceptedSpecies,
                        VegetationDebugMode::ParentLinks,
                        VegetationDebugMode::CandidateOutcomes,
                        VegetationDebugMode::GroupStructure,
                    ] {
                        ui.selectable_value(&mut settings.mode, mode, mode.label());
                    }
                });
            ui.end_row();
            ui.label("Workload");
            egui::ComboBox::from_id_salt("vegetation_profile_mode")
                .selected_text(settings.profile_mode.label())
                .show_ui(ui, |ui| {
                    for mode in [
                        VegetationProfileMode::Full,
                        VegetationProfileMode::DrawFrozen,
                        VegetationProfileMode::ComputeOnly,
                        VegetationProfileMode::ScheduleOnly,
                    ] {
                        ui.selectable_value(&mut settings.profile_mode, mode, mode.label());
                    }
                });
            ui.end_row();
            ui.label("LOD density");
            egui::ComboBox::from_id_salt("vegetation_density_mode")
                .selected_text(settings.density_mode.label())
                .show_ui(ui, |ui| {
                    for mode in [
                        VegetationDensityMode::Balanced,
                        VegetationDensityMode::FullReference,
                        VegetationDensityMode::Authored,
                    ] {
                        ui.selectable_value(&mut settings.density_mode, mode, mode.label());
                    }
                });
            ui.end_row();
            ui.label("Far grass coverage");
            ui.checkbox(
                &mut settings.far_width_compensation,
                "Width compensation",
            )
            .on_hover_text(
                "Widens retained low-LOD and distant subpixel ribbons; authored taper and LOD fade-outs remain intact",
            );
            ui.end_row();
        });
    egui::CollapsingHeader::new("Environment lighting preview")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Lighting model");
                egui::ComboBox::from_id_salt("vegetation_lighting_mode")
                    .selected_text(settings.lighting_mode.label())
                    .show_ui(ui, |ui| {
                        for mode in [
                            VegetationLightingMode::RoundedGloss,
                            VegetationLightingMode::Legacy,
                            VegetationLightingMode::UnlitDiagnostic,
                            VegetationLightingMode::VertexOnlyDiagnostic,
                        ] {
                            ui.selectable_value(&mut settings.lighting_mode, mode, mode.label());
                        }
                    });
            });
            let _ = drag_f32(
                ui,
                "Diffuse strength",
                &mut lighting.diffuse_strength,
                0.01,
                0.0..=2.0,
            );
            let _ = drag_f32(
                ui,
                "Specular strength",
                &mut lighting.specular_strength,
                0.01,
                0.0..=2.0,
            );
            let _ = drag_f32(
                ui,
                "Transmission strength",
                &mut lighting.transmission_strength,
                0.01,
                0.0..=2.0,
            );
            let _ = drag_f32(
                ui,
                "Received shadow strength",
                &mut lighting.received_shadow_strength,
                0.01,
                0.0..=1.0,
            );
            ui.weak(
                "These are live renderer preview controls. Species colors and material values above are saved with the catalog.",
            );
        });
}

fn draw_diagnostics(
    ui: &mut egui::Ui,
    diagnostics: vegetation_render::VegetationDiagnosticsSnapshot,
) {
    ui.heading("Live budget");
    let emitted = diagnostics.emitted_instances.iter().sum::<u32>();
    let dropped = diagnostics.capacity_dropped_instances.iter().sum::<u32>();
    egui::Grid::new("vegetation_authoring_diagnostics")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            diagnostic_row(ui, "Resident pages", diagnostics.source_pages);
            diagnostic_row(ui, "Work items", diagnostics.source_work_items);
            diagnostic_row(
                ui,
                "Candidate evaluations",
                diagnostics.candidate_evaluations,
            );
            diagnostic_row(ui, "Emitted units", emitted);
            diagnostic_row(ui, "Capacity drops", dropped);
            ui.label("Bin capacities");
            ui.monospace(format!(
                "{}/{}/{}/{}",
                diagnostics.topology_instance_capacities[0],
                diagnostics.topology_instance_capacities[1],
                diagnostics.topology_instance_capacities[2],
                diagnostics.topology_instance_capacities[3],
            ));
            ui.end_row();
            ui.label("Submitted indices");
            ui.monospace(diagnostics.submitted_indices.to_string());
            ui.end_row();
            ui.label("Source upload");
            ui.monospace(format!(
                "{:.2} MiB",
                diagnostics.source_uploaded_bytes as f64 / (1024.0 * 1024.0)
            ));
            ui.end_row();
        });
    if dropped == 0 {
        ui.colored_label(egui::Color32::LIGHT_GREEN, "Fixed instance budget: OK");
    } else {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            "Fixed instance budget violated; do not judge this preset yet.",
        );
    }
}

fn diagnostic_row(ui: &mut egui::Ui, label: &str, value: u32) {
    ui.label(label);
    ui.monospace(value.to_string());
    ui.end_row();
}

fn drag_f32(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    speed: f64,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    let mut changed = false;
    egui::Grid::new(("vegetation_f32", label))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label(label);
            changed = ui
                .add(egui::DragValue::new(value).speed(speed).range(range))
                .changed();
            ui.end_row();
        });
    changed
}

fn drag_u16(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u16,
    range: std::ops::RangeInclusive<u16>,
) -> bool {
    let mut changed = false;
    egui::Grid::new(("vegetation_u16", label))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label(label);
            changed = ui
                .add(egui::DragValue::new(value).speed(1).range(range))
                .changed();
            ui.end_row();
        });
    changed
}

fn drag_angle_degrees(
    ui: &mut egui::Ui,
    label: &str,
    radians: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    let mut degrees = radians.to_degrees();
    let changed = drag_f32(ui, label, &mut degrees, 0.25, range);
    if changed {
        *radians = degrees.to_radians();
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_draft_validates_before_replacing_the_live_catalog() {
        let catalog = vegetation::fixtures::reference_catalog();
        let mut state = VegetationAuthoringState::default();
        state.install(catalog.clone());
        let mut invalid = catalog.clone();
        invalid.populations[0].density_per_square_meter = -1.0;
        state.apply(invalid);
        assert_eq!(state.working, Some(catalog));
        assert!(state.validation_error.is_some());
        assert!(!state.dirty);
    }

    #[test]
    fn reset_restores_the_project_baseline() {
        let catalog = vegetation::fixtures::reference_catalog();
        let mut state = VegetationAuthoringState::default();
        state.install(catalog.clone());
        let mut changed = catalog.clone();
        changed.populations[0].density_per_square_meter *= 0.9;
        state.apply(changed);
        assert!(state.dirty);
        state.reset();
        assert_eq!(state.working, Some(catalog));
        assert!(!state.dirty);
    }

    #[test]
    fn study_import_keeps_project_baseline_and_existing_save_request() {
        let catalog = vegetation::fixtures::reference_catalog();
        let mut state = VegetationAuthoringState::default();
        state.install(catalog.clone());
        state.save_request = Some(17);
        let mut study = catalog.clone();
        study.populations[3].density_per_square_meter = 25.0;
        state.import_study_catalog(study.clone(), "short_split_fill");
        assert_eq!(state.baseline, Some(catalog));
        assert_eq!(state.working, Some(study));
        assert_eq!(state.selected_population, 3);
        assert_eq!(state.save_request, Some(17));
        assert!(state.dirty);
    }

    #[test]
    fn curve_editor_updates_root_and_tip_handles_in_normalized_space() {
        let mut curve = RibbonCurveProfile {
            tip_tilt_radians: 0.6,
            root_tangent_radians: 0.0,
            tip_tangent_radians: 0.0,
            root_handle_length: 0.3,
            tip_handle_length: 0.2,
        };
        update_ribbon_curve_control(&mut curve, 1, [0.3, 0.4], &(0.0..=1.55));
        assert!((curve.root_handle_length - 0.5).abs() < 1e-5);
        assert!((curve.root_tangent_radians - 0.3_f32.atan2(0.4)).abs() < 1e-5);

        let tip = ribbon_preview_controls(curve)[3];
        update_ribbon_curve_control(&mut curve, 2, [tip[0] - 0.4, tip[1] - 0.3], &(0.0..=1.55));
        assert!((curve.tip_handle_length - 0.5).abs() < 1e-5);
        assert!((curve.tip_tangent_radians - 0.4_f32.atan2(0.3)).abs() < 1e-5);
    }

    #[test]
    fn curve_editor_constrains_tip_to_the_authored_variant_range() {
        let mut curve = RibbonCurveProfile {
            tip_tilt_radians: 0.6,
            root_tangent_radians: 0.0,
            tip_tangent_radians: 0.0,
            root_handle_length: 0.3,
            tip_handle_length: 0.2,
        };
        update_ribbon_curve_control(&mut curve, 3, [1.0, 0.0], &(0.2..=0.9));
        assert!((curve.tip_tilt_radians - 0.9).abs() < 1e-5);
        let controls = ribbon_preview_controls(curve);
        assert!((controls[3][0].hypot(controls[3][1]) - 1.0).abs() < 1e-5);
    }
}
