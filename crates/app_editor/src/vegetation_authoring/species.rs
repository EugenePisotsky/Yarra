//! Species shape/material controls and the focused palette view.
use crate::vegetation_authoring::{
    VegetationAuthoringState,
    curve::draw_ribbon_curve_editor,
    widgets::{drag_angle_degrees, drag_f32},
};
use bevy::prelude::*;
use bevy_egui::egui;
use vegetation::{TopologyProfile, VegetationCatalog};

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

pub(super) fn draw_species_editor(
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
