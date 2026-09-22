//! Population placement, grouping and species-mixture controls.
use crate::vegetation_authoring::{
    VegetationAuthoringState,
    widgets::{drag_angle_degrees, drag_f32, drag_u16},
};
use bevy::prelude::*;
use bevy_egui::egui;
use vegetation::{
    GrowthPattern, VegetationCatalog, VegetationGroupingProfile, VoronoiClumpProfile,
};

pub(super) fn draw_population_editor(
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
