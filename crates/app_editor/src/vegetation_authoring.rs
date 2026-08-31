//! Live vegetation catalog authoring for the World workspace.
//!
//! This first slice deliberately owns a session draft rather than mutating the immutable runtime
//! catalog. The resource and tool boundaries are permanent: command history, project persistence,
//! field painting, and assemblage editing can be added without moving the live preview or parameter
//! model back into the World UI module.

use bevy::prelude::*;
use bevy_egui::{EguiPrimaryContextPass, egui};
use engine::{StreamedTerrainSurface, StreamedVegetationFieldPage, WorldOrigin};
use vegetation::{
    GrowthPattern, TopologyProfile, VegetationCatalog, VegetationFieldPage,
    VegetationGroupingProfile, VegetationScene, VegetationSurfaceField, VoronoiClumpProfile,
};
use vegetation_render::{
    VegetationDebugMode, VegetationDebugScene, VegetationDebugSettings, VegetationDiagnostics,
    VegetationProfileMode, VegetationRenderPlugin,
};

use crate::{
    project_store::ProjectEditorStore,
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
            .add_systems(Update, (adopt_project_catalog, sync_live_preview).chain())
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
    validation_error: Option<String>,
    preview_error: Option<String>,
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
            validation_error: None,
            preview_error: None,
        }
    }
}

impl VegetationAuthoringState {
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
            self.bump_revision();
        }
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
    }
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
    enabled: bool,
    active_space: Option<world::WorldSpaceId>,
    origin_cell: world::CellCoord,
    pages: Vec<(Entity, i64, i32, i32, u8)>,
}

fn sync_live_preview(
    workspace: Res<State<EditorWorkspace>>,
    tools: Res<EditorToolRegistry>,
    origin: Res<WorldOrigin>,
    terrain_pages: Query<(Entity, &StreamedTerrainSurface)>,
    field_pages: Query<(Entity, &StreamedVegetationFieldPage)>,
    mut state: ResMut<VegetationAuthoringState>,
    mut scene: ResMut<VegetationDebugScene>,
    mut previous: Local<Option<PreviewSignature>>,
) {
    let tool_active = *workspace.get() == EditorWorkspace::World
        && tools
            .active(EditorWorkspace::World)
            .is_some_and(|tool| tool.id == VEGETATION_TOOL.id);
    let enabled = tool_active && state.preview_enabled && state.working.is_some();
    let active_space = origin.space();
    let mut fields = active_space.map_or_else(Vec::new, |space| {
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
        enabled,
        active_space,
        origin_cell: origin.cell(),
        pages: page_signature,
    };
    if previous.as_ref() == Some(&signature) {
        return;
    }

    let Some(catalog) = state.working.clone() else {
        return;
    };
    let pages = if enabled {
        fields
            .drain(..)
            .filter_map(|(_, fields)| {
                let terrain = terrain_pages
                    .iter()
                    .map(|(_, terrain)| terrain)
                    .find(|terrain| {
                        terrain.key.space == fields.key.space
                            && terrain.key.cell == fields.key.cell
                            && terrain.key.lod == fields.key.lod
                    })?;
                let cell_origin = fields.key.cell.origin(fields.cell_size);
                let render_origin = origin.cell().origin(fields.cell_size);
                let resolution = terrain.heightfield.resolution;
                let sample_count = usize::from(resolution).pow(2);
                Some(VegetationFieldPage::from_data(
                    [
                        (cell_origin[0] - render_origin[0]) as f32,
                        (cell_origin[1] - render_origin[1]) as f32,
                    ],
                    fields.cell_size,
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
                    fields.data.clone(),
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
            draw_vegetation_authoring(
                ui,
                &mut tools,
                &mut state,
                diagnostics.snapshot(),
                &mut settings,
            );
        });
    windows.set_open(VEGETATION_WINDOW.id, open);
    Ok(())
}

fn draw_vegetation_authoring(
    ui: &mut egui::Ui,
    tools: &mut EditorToolRegistry,
    state: &mut VegetationAuthoringState,
    diagnostics: vegetation_render::VegetationDiagnosticsSnapshot,
    settings: &mut VegetationDebugSettings,
) {
    let active = tools
        .active(EditorWorkspace::World)
        .is_some_and(|tool| tool.id == VEGETATION_TOOL.id);
    ui.horizontal(|ui| {
        if ui.selectable_label(active, "Activate tool").clicked() {
            tools.set_active(EditorWorkspace::World, VEGETATION_TOOL.id);
        }
        ui.checkbox(&mut state.preview_enabled, "Live preview");
        if ui
            .add_enabled(state.dirty, egui::Button::new("Reset session draft"))
            .clicked()
        {
            state.reset();
        }
    });
    ui.small("Edits update the production V2 renderer immediately. Persistence and undo are the next source-command layer; this draft is session-local.");
    if state.dirty {
        ui.colored_label(egui::Color32::YELLOW, "Modified session draft");
    } else {
        ui.weak("Project catalog baseline");
    }
    if let Some(error) = &state.validation_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    if let Some(error) = &state.preview_error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
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
    draw_preview_controls(ui, settings);
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
            changed |= drag_f32(
                ui,
                "Minimum height",
                &mut species.bounds.minimum_height,
                0.01,
                0.01..=species.bounds.maximum_height,
            );
            changed |= drag_f32(
                ui,
                "Maximum height",
                &mut species.bounds.maximum_height,
                0.01,
                species.bounds.minimum_height..=16.0,
            );
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

    egui::CollapsingHeader::new("Shape")
        .default_open(true)
        .show(ui, |ui| match &mut species.topology {
            TopologyProfile::Ribbon(profile) => {
                ui.label("Cubic Bezier ribbon");
                changed |= drag_f32(
                    ui,
                    "Longitudinal power",
                    &mut profile.longitudinal_power,
                    0.02,
                    0.2..=4.0,
                );
                changed |= drag_angle_degrees(
                    ui,
                    "Minimum tilt",
                    &mut profile.minimum_tilt_radians,
                    0.0..=profile.maximum_tilt_radians.to_degrees(),
                );
                changed |= drag_angle_degrees(
                    ui,
                    "Maximum tilt",
                    &mut profile.maximum_tilt_radians,
                    profile.minimum_tilt_radians.to_degrees()..=88.8,
                );
                changed |= drag_f32(
                    ui,
                    "Minimum bend",
                    &mut profile.minimum_bend,
                    0.01,
                    0.0..=profile.maximum_bend,
                );
                changed |= drag_f32(
                    ui,
                    "Maximum bend",
                    &mut profile.maximum_bend,
                    0.01,
                    profile.minimum_bend..=2.0,
                );
                changed |= drag_f32(
                    ui,
                    "Lateral curve",
                    &mut profile.maximum_lateral_curve,
                    0.01,
                    0.0..=1.0,
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
                "Tilt coherence",
                &mut response.tilt_coherence,
                0.01,
                0.0..=1.0,
            );
            changed |= drag_f32(
                ui,
                "Bend coherence",
                &mut response.bend_coherence,
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
    changed
}

fn draw_preview_controls(ui: &mut egui::Ui, settings: &mut VegetationDebugSettings) {
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
}
