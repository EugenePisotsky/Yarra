//! Catalog inspector, preview controls and budget diagnostics.
use crate::{
    project_store::ProjectEditorStore,
    saving::EditorSaveCoordinator,
    shell::{EditorUiFrame, EditorWindowRegistry},
    tools::{EditorToolRegistry, VEGETATION_TOOL},
    vegetation_authoring::{
        VEGETATION_WINDOW, VegetationAuthoringState, population::draw_population_editor,
        species::draw_species_editor, widgets::drag_f32,
    },
    workspaces::EditorWorkspace,
};
use bevy::prelude::*;
use bevy_egui::egui;
use vegetation_render::{
    VegetationDebugMode, VegetationDebugSettings, VegetationDensityMode, VegetationDiagnostics,
    VegetationLighting, VegetationLightingMode, VegetationProfileMode,
};

pub(super) fn vegetation_authoring_ui(
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
