//! The interactive cloth lab panel (egui on the game's UI camera): pieces, clones and gait,
//! the lab camera, simulation settings, colliders, and what the cloth costs.
use super::{ClothLab, LabCamera, LabGait, PanelHasPointer};
use crate::game_render::GameUiCamera;
use crate::runtime_settings::RuntimeSettings;
use bevy::prelude::*;
use bevy_egui::{
    EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext, egui,
};
use engine::{ClothSettings, ClothStats};

pub(super) fn install(app: &mut App) {
    app.insert_resource(EguiGlobalSettings {
        auto_create_primary_context: false,
        ..default()
    })
    .add_plugins(EguiPlugin::default())
    .add_systems(Update, attach)
    .add_systems(EguiPrimaryContextPass, panel);
}

/// egui draws on the camera that renders to the window, not on the world camera's image.
fn attach(
    mut commands: Commands,
    cameras: Query<Entity, (With<GameUiCamera>, Without<PrimaryEguiContext>)>,
) {
    for camera in &cameras {
        commands.entity(camera).insert(PrimaryEguiContext);
    }
}

fn panel(
    mut contexts: EguiContexts,
    mut lab: ResMut<ClothLab>,
    mut settings: ResMut<ClothSettings>,
    stats: Res<ClothStats>,
    runtime: Res<RuntimeSettings>,
    mut pointer: ResMut<PanelHasPointer>,
) -> Result {
    let context = contexts.ctx_mut()?;
    if !runtime.show_ui {
        pointer.0 = false;
        return Ok(());
    }
    egui::Window::new("Cloth lab")
        .default_pos([16.0, 16.0])
        .default_width(340.0)
        .show(context, |ui| {
            ui.heading("Outfit");
            let mut outfit = lab.outfit;
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut outfit.cloak, "Cloak");
                ui.checkbox(&mut outfit.skirt, "Skirt");
                ui.checkbox(&mut outfit.ponytail, "Ponytail");
            });
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut outfit.greatsword, "Greatsword");
                ui.checkbox(&mut outfit.bow, "Bow");
                ui.checkbox(&mut outfit.side_sword, "Hip sword");
            });
            ui.checkbox(
                &mut outfit.weapons_over_cloak,
                "Back weapons strapped over the cloak",
            );
            if outfit != lab.outfit {
                lab.outfit = outfit;
            }

            ui.separator();
            let mut clones = lab.clones;
            ui.add(egui::Slider::new(&mut clones, 0..=63).text("Clones"));
            if clones != lab.clones {
                lab.clones = clones;
            }
            let mut gait = lab.gait;
            ui.horizontal(|ui| {
                ui.label("Clones");
                ui.radio_value(&mut gait, LabGait::Idle, "stand");
                ui.radio_value(&mut gait, LabGait::Walk, "walk");
                ui.radio_value(&mut gait, LabGait::Jog, "jog");
                ui.radio_value(&mut gait, LabGait::Mixed, "mixed");
            });
            if gait != lab.gait {
                lab.gait = gait;
            }
            let mut player_walks = lab.player_walks;
            ui.checkbox(&mut player_walks, "Player walks with them");
            if player_walks != lab.player_walks {
                lab.player_walks = player_walks;
            }

            ui.separator();
            let mut follow = lab.camera.is_some();
            ui.checkbox(&mut follow, "Lab camera (keeps its side of the player)");
            let mut view = lab.camera.unwrap_or(LabCamera {
                bearing: 180.0,
                distance: 3.4,
                pitch: 12.0,
            });
            ui.add_enabled_ui(follow, |ui| {
                ui.add(
                    egui::Slider::new(&mut view.bearing, -180.0..=180.0)
                        .suffix("°")
                        .text("From the face"),
                );
                ui.add(
                    egui::Slider::new(&mut view.distance, 1.5..=20.0)
                        .suffix(" m")
                        .text("Distance"),
                );
                ui.add(
                    egui::Slider::new(&mut view.pitch, -10.0..=70.0)
                        .suffix("°")
                        .text("Pitch"),
                );
            });
            let camera = follow.then_some(view);
            if camera != lab.camera {
                lab.camera = camera;
            }

            ui.separator();
            ui.heading("Simulation");
            let mut tuned = settings.clone();
            ui.checkbox(&mut tuned.enabled, "Simulate");
            ui.add(
                egui::Slider::new(&mut tuned.substep_hz, 60.0..=600.0)
                    .suffix(" /s")
                    .text("Substeps"),
            );
            let mut inertia = tuned.inertia.is_some();
            let mut share = tuned.inertia.unwrap_or(0.7);
            ui.horizontal(|ui| {
                ui.checkbox(&mut inertia, "Inertia");
                ui.add_enabled(inertia, egui::Slider::new(&mut share, 0.0..=1.0));
            });
            tuned.inertia = inertia.then_some(share);
            ui.add(
                egui::Slider::new(&mut tuned.wind_speed, 0.0..=20.0)
                    .suffix(" m/s")
                    .text("Wind at unit force"),
            );
            ui.add(egui::Slider::new(&mut tuned.gravity, 0.0..=20.0).text("Gravity"));
            ui.add(
                egui::Slider::new(&mut tuned.simulate_within, 5.0..=120.0)
                    .suffix(" m")
                    .text("Simulate within"),
            );
            ui.horizontal(|ui| {
                ui.checkbox(&mut tuned.show_colliders, "Colliders");
                ui.checkbox(&mut tuned.show_particles, "Particles");
            });
            if tuned != *settings {
                *settings = tuned;
            }

            ui.separator();
            ui.heading("Cost");
            ui.label(format!(
                "{} characters ({} simulated), {} particles, {} colliders",
                stats.characters, stats.simulated, stats.particles, stats.colliders
            ));
            ui.label(format!(
                "Cloth pass {:.3} ms (mean {:.3} ms)",
                stats.frame_us / 1000.0,
                stats.mean_frame_us / 1000.0
            ));
            let per = stats.work_us / stats.characters.max(1) as f32;
            ui.label(format!(
                "Work over all cores {:.3} ms; {per:.0} µs per character, worst {:.0} µs",
                stats.work_us / 1000.0,
                stats.worst_character_us
            ));
            ui.small("The player moves as usual (WASD, click). F1 has frame timings.");
        });
    pointer.0 = context.is_pointer_over_egui() || context.egui_wants_pointer_input();
    Ok(())
}
