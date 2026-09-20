//! World atmosphere authoring. Preview transport never mutates project source.
#[cfg(test)]
mod tests;
pub(crate) mod working;
use crate::{
    domain_editing::DenseDomainWorkingSets,
    editing::EditorHistory,
    preview::{EditorPreviewMode, PreviewModeState},
    project_store::{ProjectEditorStore, ProjectStoreUpdate},
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::{
        EditorUiFrame, EditorUiSet, EditorWindowDescriptor, EditorWindowId, EditorWindowRegistry,
    },
    workspaces::{EditorFramePacing, EditorWorkspace, FramePacingOwner, world_workspace_active},
};
use bevy::prelude::*;
use bevy_egui::{EguiPrimaryContextPass, egui};
use engine::{
    ApplyAtmosphere, AtmosphereOwner, AtmosphereState, WorldCatalog, WorldEnvironmentView,
    WorldOrigin,
};
use std::collections::BTreeMap;
use world::{
    WorldSpaceId,
    atmosphere::{AtmosphereProfile, PHASE_NAMES, PHASE_TIMES, evaluate},
};
pub(crate) const WINDOW: EditorWindowDescriptor = EditorWindowDescriptor {
    id: EditorWindowId("world.atmosphere"),
    workspace: EditorWorkspace::World,
    label: "Atmosphere",
    default_open: false,
};
pub(crate) struct AtmosphereAuthoringPlugin;
impl Plugin for AtmosphereAuthoringPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Preview>()
            .add_systems(
                Update,
                reconcile
                    .after(ProjectStoreUpdate)
                    .after(crate::journal::restore_loaded_journal),
            )
            .add_systems(PostUpdate, sync.before(ApplyAtmosphere))
            .add_systems(
                EguiPrimaryContextPass,
                draw.in_set(EditorUiSet::Workspace)
                    .run_if(world_workspace_active),
            );
    }
}
struct Controls {
    phase: f32,
    playing: bool,
    speed: f32,
    target: usize,
    published: bool,
    exposure: Option<f32>,
    cloud_seconds: f64,
    clouds_playing: bool,
    cloud_speed: f32,
}
impl Controls {
    fn new(p: &AtmosphereProfile) -> Self {
        Self {
            phase: p.initial_phase,
            playing: false,
            speed: 20.0,
            target: 1,
            published: false,
            exposure: None,
            cloud_seconds: 0.,
            clouds_playing: false,
            cloud_speed: 1.,
        }
    }
    fn advance(&mut self, seconds: f64, day_seconds: f32) {
        if self.playing {
            self.phase = (f64::from(self.phase) + seconds * f64::from(self.speed / day_seconds))
                .rem_euclid(1.) as f32;
        }
        if self.clouds_playing {
            self.cloud_seconds += seconds * f64::from(self.cloud_speed);
        }
    }
}
#[derive(Resource, Default)]
struct Preview {
    worlds: BTreeMap<WorldSpaceId, Controls>,
    gameplay: Option<(WorldSpaceId, AtmosphereProfile, f32)>,
}
fn reconcile(
    mut dense: ResMut<DenseDomainWorkingSets>,
    mut project: ResMut<ProjectEditorStore>,
    mut save: ResMut<EditorSaveCoordinator>,
) {
    if let Some(manifest) = project.manifest() {
        for source in &manifest.world_spaces {
            dense.atmospheres.pin(source);
        }
    }
    if let Some((id, result)) = project.atmosphere_completion.take() {
        if dense.atmospheres.saving == Some(id) {
            let committed = dense.atmospheres.complete(result);
            save.transaction_finished(committed);
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn sync(
    workspace: Res<State<EditorWorkspace>>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    dense: Res<DenseDomainWorkingSets>,
    mut preview: ResMut<Preview>,
    mut state: ResMut<AtmosphereState>,
    time: Res<Time>,
    mode: Res<PreviewModeState>,
    mut pacing: ResMut<EditorFramePacing>,
    mut cloud_clock: Option<ResMut<atmosphere::clouds::CloudClock>>,
) {
    pacing.release(FramePacingOwner::AtmospherePreview);
    if let Some(clock) = cloud_clock.as_deref_mut() {
        clock.playing = false;
    }
    if *workspace.get() != EditorWorkspace::World {
        state.owner = AtmosphereOwner::Study;
        for controls in preview.worlds.values_mut() {
            controls.playing = false;
            controls.clouds_playing = false;
        }
        preview.gameplay = None;
        return;
    }
    state.owner = AtmosphereOwner::Editor;
    let Some(id) = origin.space() else {
        return;
    };
    let Some(entry) = dense.atmospheres.entries.get(&id) else {
        return;
    };
    if mode.active() == Some(EditorPreviewMode::Gameplay) {
        if preview
            .gameplay
            .as_ref()
            .is_none_or(|(space, _, _)| *space != id)
        {
            preview.gameplay = Some((id, entry.current.clone(), entry.current.initial_phase));
            if let Some(clock) = cloud_clock.as_deref_mut() {
                clock.seconds = 0.;
            }
        }
        if let Some(clock) = cloud_clock.as_deref_mut() {
            clock.playing = true;
        }
        // Match the standalone game's fixed startup phase until game-clock ownership lands.
        let (_, profile, phase) = preview.gameplay.as_ref().unwrap();
        state.profile = profile.clone();
        state.phase = *phase;
        state.exposure_override = None;
        return;
    }
    preview.gameplay = None;
    let controls = preview
        .worlds
        .entry(id)
        .or_insert_with(|| Controls::new(&entry.current));
    let profile = if controls.published {
        catalog
            .world_space(id)
            .map(|s| &s.atmosphere)
            .unwrap_or(&entry.current)
    } else {
        &entry.current
    };
    controls.advance(time.delta_secs_f64(), profile.day_seconds);
    if controls.playing || controls.clouds_playing {
        pacing.request(FramePacingOwner::AtmospherePreview);
    }
    if let Some(clock) = cloud_clock.as_deref_mut() {
        clock.seconds = controls.cloud_seconds;
    }
    state.profile = profile.clone();
    state.phase = controls.phase;
    state.exposure_override = controls.exposure;
}

#[allow(clippy::too_many_arguments)]
fn draw(
    mut frame: ResMut<EditorUiFrame>,
    mut windows: ResMut<EditorWindowRegistry>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    mut dense: ResMut<DenseDomainWorkingSets>,
    mut preview: ResMut<Preview>,
    mut history: ResMut<EditorHistory>,
    publication: Res<RuntimePublicationState>,
    save: Res<EditorSaveCoordinator>,
    project: Res<ProjectEditorStore>,
    mode: Res<PreviewModeState>,
    mut cameras: Query<&mut WorldEnvironmentView>,
    mut cloud_quality: ResMut<engine::CloudQuality>,
) {
    let Some(root) = frame.0.as_mut() else {
        return;
    };
    if root.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        dense.atmospheres.cancel_gesture();
    }
    if !root.ctx().input(|i| i.pointer.any_down())
        && (!root.ctx().text_edit_focused()
            || root.ctx().input(|i| i.key_pressed(egui::Key::Enter)))
    {
        if let Some((id, before, after)) = dense.atmospheres.finish_gesture() {
            history.record_atmosphere(id, before, after);
        }
    }
    let mut open = windows.is_open(WINDOW.id);
    if !open {
        return;
    }
    let rect = root.available_rect_before_wrap();
    egui::Window::new("Atmosphere")
        .id(egui::Id::new(WINDOW.id.0))
        .open(&mut open)
        .default_width(380.0)
        .default_height(600.0)
        .default_pos(rect.left_top() + egui::vec2(325.0, 60.0))
        .constrain_to(rect)
        .vscroll(true)
        .show(root.ctx(), |ui| {
            let Some(id) = origin.space() else {
                ui.label("Waiting for world…");
                return;
            };
            let Some(entry) = dense.atmospheres.entries.get(&id) else {
                ui.label("Loading atmosphere…");
                return;
            };
            let before = entry.current.clone();
            let mut edited = before.clone();
            let saved = entry.base.clone();
            let controls = preview
                .worlds
                .entry(id)
                .or_insert_with(|| Controls::new(&before));
            ui.strong(catalog.world_space(id).map_or("World", |s| s.name.as_str()));
            ui.small("Changes preview on the landscape. Save & Publish updates the game.");
            if mode.active() != Some(EditorPreviewMode::Authoring) {
                ui.label("Return to Authoring preview to edit atmosphere.");
                return;
            }
            ui.separator();
            ui.strong("Preview · temporary");
            ui.horizontal(|ui| {
                ui.label("Cloud quality");
                for (quality, label) in [(engine::CloudQuality::Off, "Off"),
                    (engine::CloudQuality::Balanced, "Balanced"), (engine::CloudQuality::High, "High")] {
                    ui.selectable_value(&mut *cloud_quality, quality, label).on_hover_text(match quality {
                        engine::CloudQuality::Off => "Hide clouds and their direct shadows.",
                        engine::CloudQuality::Balanced => "Reuse a cached sky with a limited refresh rate to reduce GPU load.",
                        engine::CloudQuality::High => "Render clouds every frame. More detail and smoother fast previews, with higher GPU load.",
                    });
                }
            });
            ui.horizontal(|ui| {
                for i in [1, 2, 3, 0] {
                    if ui.button(PHASE_NAMES[i]).clicked() {
                        controls.phase = PHASE_TIMES[i];
                        controls.playing = false;
                    }
                }
            });
            preview_hour(ui, controls);
            ui.horizontal(|ui| {
                ui.checkbox(&mut controls.playing, "Play");
                ui.add(egui::Slider::new(&mut controls.speed, 1.0..=120.0).text("Speed ×"));
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut controls.clouds_playing, "Animate clouds");
                if ui.button("Reset clouds").clicked() { controls.cloud_seconds = 0.; }
            });
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut controls.cloud_speed, 1.0..=30.0)
                    .clamping(egui::SliderClamping::Edits).text("Cloud speed ×"));
                ui.label(format!("{:.1} s", controls.cloud_seconds));
            });
            ui.horizontal(|ui| {
                ui.selectable_value(&mut controls.published, false, "Edited");
                ui.selectable_value(&mut controls.published, true, "Published");
                if ui.button("Reset preview").clicked() {
                    *controls = Controls::new(&before);
                }
            });
            let mut locked = controls.exposure.is_some();
            if ui.checkbox(&mut locked, "Lock preview exposure").changed() {
                let profile = if controls.published {
                    catalog.world_space(id).map_or(&before, |s| &s.atmosphere)
                } else {
                    &before
                };
                controls.exposure = locked.then(|| evaluate(profile, controls.phase).exposure_ev100);
            }
            if let Some(ev) = &mut controls.exposure {
                ui.add(egui::Slider::new(ev, 0.0..=20.0).text("Preview EV100"));
            }
            for mut view in &mut cameras {
                if let Some(visibility) = view.visibility_override {
                    ui.horizontal(|ui| {
                        ui.label(format!("Bookmark visibility override: {visibility:.0} m"));
                        if ui.button("Clear").clicked() {
                            view.visibility_override = None;
                        }
                    });
                }
            }
            ui.separator();
            ui.strong("World settings · saved");
            let can_edit = !controls.published
                && !publication.active()
                && !save.active()
                && !project.save_in_flight()
                && !dense.saving()
                && !dense.has_any_conflict();
            ui.add_enabled_ui(can_edit, |ui| {
                ui.checkbox(&mut edited.outdoor, "Outdoor sky and celestial lighting");
                egui::CollapsingHeader::new("Clouds").default_open(true).show(ui, |ui| {
                    let c = &mut edited.clouds;
                    ui.horizontal(|ui| {
                        if ui.button("Clear").clicked() { c.enabled = false; }
                        if ui.button("Scattered").clicked() { *c = world::clouds::CloudSettings::scattered(); }
                        if ui.button("Overcast").clicked() { *c = world::clouds::CloudSettings::overcast(); }
                    });
                    ui.checkbox(&mut c.enabled, "Cloud layer");
                    ui.add(egui::Slider::new(&mut c.coverage,0.0..=1.0).text("Coverage"));
                    ui.add(egui::Slider::new(&mut c.density,0.0..=3.0).text("Density"));
                    ui.add(egui::Slider::new(&mut c.base_metres,300.0..=6000.0).text("Base · m"));
                    ui.add(egui::Slider::new(&mut c.thickness_metres,100.0..=3000.0).text("Thickness · m"));
                    ui.add(egui::Slider::new(&mut c.size_metres,300.0..=6000.0).text("Size · m"));
                    ui.add(egui::Slider::new(&mut c.erosion,0.0..=1.0).text("Edge detail"));
                    ui.add(egui::Slider::new(&mut c.wind_degrees,-180.0..=180.0).text("Wind heading °"));
                    ui.add(egui::Slider::new(&mut c.wind_metres_per_second,0.0..=100.0).text("Wind · m/s"));
                    ui.horizontal(|ui| { ui.label("Seed"); ui.add(egui::DragValue::new(&mut c.seed)); });
                    ui.small("Ground-view cloud layer. Use Animate clouds to preview wind independently of the day cycle.");
                });
                egui::CollapsingHeader::new("Sun and sky")
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Slider::new(&mut edited.azimuth_degrees, -180.0..=180.0)
                                .text("Sun path heading °"),
                        );
                        ui.add(
                            egui::Slider::new(&mut edited.maximum_elevation_degrees, 5.0..=85.0)
                                .text("Noon elevation °"),
                        );
                        ui.add(
                            egui::Slider::new(&mut edited.sun_diameter_degrees, 0.1..=5.0)
                                .text("Sun diameter °"),
                        );
                        ui.add(
                            egui::Slider::new(&mut edited.molecular_density, 0.1..=4.0)
                                .text("Air scattering"),
                        );
                    });
                egui::CollapsingHeader::new("Day palette")
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Editing:");
                            egui::ComboBox::from_id_salt("phase_target")
                                .selected_text(PHASE_NAMES[controls.target])
                                .show_ui(ui, |ui| {
                                    for (i, name) in PHASE_NAMES.iter().enumerate() {
                                        ui.selectable_value(&mut controls.target, i, *name);
                                    }
                                });
                        });
                        if ui.button("Preview this phase").clicked() {
                            controls.phase = PHASE_TIMES[controls.target];
                            controls.playing = false;
                        }
                        let p = &mut edited.phases[controls.target];
                        if controls.target != 0 {
                            color(ui, "Sun tint", &mut p.sun_srgb);
                            ui.add(
                                egui::Slider::new(&mut p.sun_lux, 0.0..=200_000.0).text("Sun · lux"),
                            );
                        } else {
                            ui.small("Moonlight is controlled in Night lighting below.");
                        }
                        color(ui, "Ambient tint", &mut p.ambient_srgb);
                        ui.add(
                            egui::Slider::new(&mut p.ambient_lux, 0.0..=20_000.0)
                                .text("Ambient strength"),
                        );
                    });
                egui::CollapsingHeader::new("Night lighting").show(ui, |ui| {
                    if ui.button("Preview night").clicked() {
                        controls.phase = PHASE_TIMES[0];
                        controls.playing = false;
                    }
                    ui.checkbox(&mut edited.night.enabled, "Moon light");
                    color(ui, "Moon tint", &mut edited.night.light_srgb);
                    ui.add(egui::Slider::new(&mut edited.night.illuminance_lux, 0.0..=20_000.0)
                        .text("Moon · lux"));
                    ui.add(egui::Slider::new(&mut edited.night.azimuth_degrees, -180.0..=180.0)
                        .text("Moon heading °"));
                    ui.add(egui::Slider::new(&mut edited.night.elevation_degrees, 5.0..=85.0)
                        .text("Moon elevation °"));
                    ui.add(egui::Slider::new(&mut edited.night.diameter_degrees, 0.1..=8.0)
                        .text("Moon diameter °"));
                    ui.add(egui::Slider::new(&mut edited.night.exposure_ev100, 0.0..=20.0)
                        .text("Night exposure · EV100"));
                    ui.small("Moonlight and exposure blend through twilight. Adjust shadow fill in the Night palette.");
                });
                egui::CollapsingHeader::new("Haze").show(ui, |ui| {
                    ui.add(
                        egui::Slider::new(&mut edited.visibility_metres, 50.0..=100_000.0)
                            .logarithmic(true)
                            .text("Visibility · m"),
                    );
                    color(ui, "Haze tint", &mut edited.haze_srgb);
                    ui.small("Near-ground aerosols. Weather presets will build on these controls.");
                });
                egui::CollapsingHeader::new("Presentation").show(ui, |ui| {
                    ui.add(
                        egui::Slider::new(&mut edited.exposure_ev100, 0.0..=20.0)
                            .text("Day exposure · EV100"),
                    );
                    ui.add(egui::Slider::new(&mut edited.bloom_intensity, 0.0..=1.0).text("Bloom"));
                });
                egui::CollapsingHeader::new("Startup and cycle").show(ui, |ui| {
                    let mut hour = edited.initial_phase * 24.0;
                    if ui
                        .add(egui::Slider::new(&mut hour, 0.0..=23.999).text("Initial time"))
                        .changed()
                    {
                        edited.initial_phase = hour / 24.0;
                    }
                    if ui.button("Use preview as startup").clicked() {
                        edited.initial_phase = controls.phase;
                    }
                    ui.add(
                        egui::Slider::new(&mut edited.day_seconds, 10.0..=604_800.0)
                            .logarithmic(true)
                            .text("Day length · seconds"),
                    );
                    ui.small(
                        "Game time stays fixed in this first slice. Preview Play tests the cycle.",
                    );
                });
                if before != saved && ui.button("Revert unsaved atmosphere edits").clicked() {
                    edited = saved;
                }
            });
            if edited != before && edited.validate().is_ok() {
                if dense.atmospheres.gesture.is_none() {
                    dense.atmospheres.gesture = Some((id, before));
                }
                dense.atmospheres.entries.get_mut(&id).unwrap().current = edited;
                dense.atmospheres.error = None;
            }
            if let Some(error) = &dense.atmospheres.error {
                ui.colored_label(egui::Color32::YELLOW, error);
            }
            if dense.atmospheres.conflict.is_some() {
                ui.horizontal(|ui| {
                    for (label, keep) in [("Keep my edits", true), ("Use database", false)] {
                        if ui.button(label).clicked() {
                            dense.atmospheres.resolve_conflict(keep);
                            history.clear();
                        }
                    }
                });
            }
            let dirty = dense.atmospheres.dirty_count();
            if dirty > 0 {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("{dirty} world atmosphere(s) unsaved"),
                );
            } else if catalog
                .world_space(id)
                .is_some_and(|s| s.atmosphere != dense.atmospheres.entries[&id].current)
            {
                ui.label("Saved · publish to update the game");
            }
        });
    windows.set_open(WINDOW.id, open);
}
fn preview_hour(ui: &mut egui::Ui, controls: &mut Controls) {
    let mut hour = controls.phase * 24.0;
    // Always-clamping also rounds every frame, marking an advancing clock as an
    // edit and immediately stopping Play. Only clamp/round actual user edits.
    if ui
        .add(
            egui::Slider::new(&mut hour, 0.0..=23.999)
                .clamping(egui::SliderClamping::Edits)
                .text("Hour")
                .fixed_decimals(2),
        )
        .changed()
    {
        controls.phase = hour / 24.0;
        controls.playing = false;
    }
}
fn color(ui: &mut egui::Ui, label: &str, value: &mut [f32; 3]) {
    ui.horizontal(|ui| {
        let mut c = value.map(|v| (v * 255.0).round() as u8);
        if ui.color_edit_button_srgb(&mut c).changed() {
            *value = c.map(|v| f32::from(v) / 255.0);
        }
        ui.label(label);
    });
}
