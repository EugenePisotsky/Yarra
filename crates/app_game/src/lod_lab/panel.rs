//! The interactive LOD lab panel (egui on the game's UI camera): distance, bearing and pitch,
//! what the tree draws, the sun and shadows, and what the game would draw there.
use super::{LodLab, label, switches};
use crate::game_render::GameUiCamera;
use crate::runtime_settings::RuntimeSettings;
use bevy::{prelude::*, window::PrimaryWindow};
use bevy_egui::{
    EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext, egui,
};
use engine::{AtmosphereState, LabRepresentation, LabTree, WorldSun};

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

#[allow(clippy::too_many_arguments)]
fn panel(
    mut contexts: EguiContexts,
    mut lab: ResMut<LodLab>,
    mut trees: Query<(&LabTree, &mut LabRepresentation, &GlobalTransform)>,
    mut atmosphere: ResMut<AtmosphereState>,
    mut settings: ResMut<RuntimeSettings>,
    sun: Single<&GlobalTransform, With<WorldSun>>,
    window: Single<&Window, With<PrimaryWindow>>,
) -> Result {
    let context = contexts.ctx_mut()?;
    egui::Window::new("LOD lab")
        .default_pos([16.0, 16.0])
        .default_width(380.0)
        .show(context, |ui| {
            let Some((entity, _)) = lab.tree else {
                ui.label(format!("Placing {}...", lab.subject.key));
                return;
            };
            let Ok((tree, mut representation, transform)) = trees.get_mut(entity) else {
                return;
            };
            ui.heading(&tree.asset.key);
            if !tree.ready {
                ui.label("Loading...");
            }
            let mut view = lab.view;
            ui.add(
                egui::Slider::new(&mut view.distance, 2.0..=2000.0)
                    .logarithmic(true)
                    .suffix(" m")
                    .text("Distance"),
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("Switches:");
                for (j, switch) in switches(tree).into_iter().enumerate() {
                    let text = format!(
                        "{} to {} at {switch:.0} m",
                        label(tree.bands[j].representation),
                        label(tree.bands[j + 1].representation)
                    );
                    if ui.button(text).clicked() {
                        view.distance = switch;
                    }
                }
            });
            let mut bearing = view.bearing.to_degrees();
            ui.add(
                egui::Slider::new(&mut bearing, -180.0..=180.0)
                    .suffix("°")
                    .text("Bearing from sun"),
            );
            view.bearing = bearing.to_radians();
            let mut pitch = view.pitch.to_degrees();
            ui.add(egui::Slider::new(&mut pitch, -5.0..=70.0).suffix("°").text("Pitch"));
            view.pitch = pitch.to_radians();
            lab.view = view;

            ui.separator();
            let mut chosen = *representation;
            ui.horizontal_wrapped(|ui| {
                ui.label("Draw:");
                ui.radio_value(&mut chosen, LabRepresentation::Auto, "game");
                for lod in 0..tree.asset.meshes() {
                    ui.radio_value(&mut chosen, LabRepresentation::Mesh(lod), format!("LOD{lod}"));
                }
                if tree.asset.impostor().is_some() {
                    ui.radio_value(&mut chosen, LabRepresentation::Impostor, "impostor");
                }
                ui.radio_value(&mut chosen, LabRepresentation::Hidden, "hidden");
            });
            if chosen != *representation {
                lab.blink = None;
                if chosen != LabRepresentation::Auto && chosen != lab.forced[1] {
                    lab.forced = [lab.forced[1], chosen];
                }
                *representation = chosen;
            }
            let mut blinking = lab.blink.is_some();
            if ui
                .checkbox(
                    &mut blinking,
                    format!(
                        "Blink {} ↔ {}",
                        label(lab.forced[0]),
                        label(lab.forced[1])
                    ),
                )
                .changed()
            {
                lab.blink = blinking.then(|| Timer::from_seconds(0.5, TimerMode::Repeating));
            }
            ui.checkbox(&mut lab.wind, "Wind");

            ui.separator();
            let to_sun = atmosphere
                .direction_override
                .unwrap_or_else(|| sun.back().into());
            let mut elevation = to_sun.y.clamp(-1.0, 1.0).asin().to_degrees();
            let mut azimuth = to_sun.x.atan2(to_sun.z).to_degrees();
            let mut overridden = atmosphere.direction_override.is_some();
            ui.checkbox(&mut overridden, "Set the sun");
            ui.add_enabled_ui(overridden, |ui| {
                ui.add(egui::Slider::new(&mut elevation, 2.0..=89.0).suffix("°").text("Elevation"));
                ui.add(egui::Slider::new(&mut azimuth, -180.0..=180.0).suffix("°").text("Azimuth"));
            });
            let direction = {
                let (e, a) = (elevation.to_radians(), azimuth.to_radians());
                Vec3::new(a.sin() * e.cos(), e.sin(), a.cos() * e.cos())
            };
            let wanted = overridden.then_some(direction);
            if atmosphere.direction_override != wanted {
                atmosphere.direction_override = wanted;
            }
            let mut shadows = settings.shadows;
            ui.horizontal(|ui| {
                ui.label("Shadows:");
                ui.radio_value(&mut shadows, 0, "soft");
                ui.radio_value(&mut shadows, 1, "hardware 2×2");
                ui.radio_value(&mut shadows, 2, "off");
            });
            if shadows != settings.shadows {
                settings.shadows = shadows;
            }

            ui.separator();
            let scale = transform.to_scale_rotation_translation().0.y;
            let d = lab.view.distance;
            let logical = tree.pixels_per_metre * tree.asset.bounds[1] * scale / d;
            let drawn: Vec<String> = tree
                .bands
                .iter()
                .filter(|b| d >= b.fade_in.start && d <= b.fade_out.end)
                .map(|b| label(b.representation))
                .collect();
            ui.label(format!(
                "On screen {logical:.0} px tall ({:.0} physical)",
                logical * window.scale_factor()
            ));
            ui.label(format!("The game draws {} here", drawn.join(" + ")));
            if let Some((cell, radius)) = tree.impostor_view {
                let texels = cell as f32 * d
                    / (2.0 * radius * scale * tree.pixels_per_metre * window.scale_factor());
                ui.label(format!(
                    "Impostor {texels:.2} texels per screen pixel{}",
                    if texels < 1.0 { " (magnified)" } else { "" }
                ));
            }
            ui.small("Keys: Up/Down distance, Left/Right bearing, PageUp/PageDown pitch, F next switch, 0 game, 1-9 LOD, I impostor, H hide, B blink");
        });
    Ok(())
}
