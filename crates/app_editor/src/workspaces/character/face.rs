//! The face panel: the character's face sliders, read from its kit, and the faces kept as files
//! (`engine::saved_faces`).

use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use bevy_egui::egui;
use engine::{
    CharacterAppearance, CharacterKit, CharacterKitRef, FaceSlider, NO_HAIR, SkinLayers, SkinLook,
    SkinPaint, saved_faces,
};

use super::PreviewActor;
use crate::shell::EditorUiFrame;

/// The share of each slider's range a random face reaches: sliders, details.
const RANDOM_REACH: f32 = 0.6;

const RANDOM_DETAIL_REACH: f32 = 0.3;

#[derive(Resource)]
pub(super) struct FaceEditor {
    appearance: CharacterAppearance,
    name: String,
    status: String,
    random: u64,
}

impl Default for FaceEditor {
    fn default() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |time| time.as_nanos() as u64);
        Self {
            appearance: CharacterAppearance::default(),
            name: String::new(),
            status: String::new(),
            random: seed | 1,
        }
    }
}

impl FaceEditor {
    /// A number in 0..1 (xorshift).
    fn next_random(&mut self) -> f32 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        (self.random >> 40) as f32 / (1u64 << 24) as f32
    }

    /// A face around the model's own: every slider somewhere in the middle of its range, each
    /// choice either left out or one of its options.
    fn randomize(&mut self, sliders: &[FaceSlider]) {
        let mut face = CharacterAppearance {
            look: self.appearance.look.clone(),
            ..default()
        };

        for slider in sliders.iter().filter(|slider| slider.choice.is_none()) {
            let reach = if slider.detail {
                RANDOM_DETAIL_REACH
            } else {
                RANDOM_REACH
            };
            let [low, high] = slider.range;
            let value = (low + (high - low) * self.next_random()) * reach;
            face.sliders.insert(slider.label.clone(), value);
        }

        for (_, options) in choices(sliders) {
            if self.next_random() < 0.5 {
                let pick =
                    options[(self.next_random() * options.len() as f32) as usize % options.len()];
                let value = pick.range[1] * self.next_random() * RANDOM_REACH;
                face.sliders.insert(pick.label.clone(), value);
            }
        }

        self.appearance = face;
    }
}

pub(super) fn sync_face(
    editor: Res<FaceEditor>,
    mut actor: Single<&mut CharacterAppearance, With<PreviewActor>>,
) {
    actor.set_if_neq(editor.appearance.clone());
}

fn preview_kit<'a>(
    actor: Entity,
    refs: &Query<(&CharacterKitRef, &ChildOf)>,
    kits: &'a Assets<CharacterKit>,
) -> Option<&'a CharacterKit> {
    refs.iter()
        .find(|(_, parent)| parent.parent() == actor)
        .and_then(|(kit, _)| kits.get(kit.kit()))
}

pub(super) fn face_panel(
    mut frame: ResMut<EditorUiFrame>,
    mut editor: ResMut<FaceEditor>,
    kits: Res<Assets<CharacterKit>>,
    refs: Query<(&CharacterKitRef, &ChildOf)>,
    actor: Single<Entity, With<PreviewActor>>,
) {
    let Some(workspace_ui) = frame.0.as_mut() else {
        return;
    };
    let kit = preview_kit(*actor, &refs, &kits);

    egui::Panel::right("face_panel")
        .default_size(320.0)
        .min_size(220.0)
        .resizable(true)
        .show(workspace_ui, |ui| {
            ui.heading("Face");

            let Some(kit) = kit else {
                ui.weak(
                    "This profile's model has no kit: faces need the kit export_glb.py writes.",
                );
                return;
            };

            file_rows(ui, &mut editor, kit.sliders());
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some(layers) = kit.skin_layers() {
                    look_groups(ui, &mut editor.appearance.look, layers);
                }

                slider_groups(ui, &mut editor.appearance, kit.sliders());
            });
        });
}

fn file_rows(ui: &mut egui::Ui, editor: &mut FaceEditor, sliders: &[FaceSlider]) {
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut editor.name);

        if ui.button("Save").clicked() {
            editor.status = match editor.appearance.save(&editor.name) {
                Ok(path) => format!("Saved {}", path.display()),
                Err(error) => error,
            };
        }
    });
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("face_load")
            .selected_text("Load…")
            .show_ui(ui, |ui| {
                for name in saved_faces() {
                    if ui.selectable_label(false, &name).clicked() {
                        match CharacterAppearance::load(&name) {
                            Ok(face) => {
                                editor.appearance = face;
                                editor.status = format!("Loaded {name}");
                                editor.name = name;
                            }
                            Err(error) => editor.status = error,
                        }
                    }
                }
            });

        if ui.button("Reset").clicked() {
            editor.appearance = CharacterAppearance::default();
        }

        if ui.button("Randomize").clicked() {
            editor.randomize(sliders);
        }
    });

    if !editor.status.is_empty() {
        ui.weak(&editor.status);
    }
}

/// What the skin paints: the brow and its colour, and makeup.
fn look_groups(ui: &mut egui::Ui, look: &mut SkinLook, layers: &SkinLayers) {
    egui::CollapsingHeader::new("brow style").show(ui, |ui| {
        let chosen = layers
            .brows
            .iter()
            .find(|brow| brow.label == look.brow)
            .or_else(|| layers.brows.first());
        egui::ComboBox::from_id_salt("brow_shape")
            .selected_text(chosen.map_or("none", |brow| brow.label.as_str()))
            .show_ui(ui, |ui| {
                for brow in &layers.brows {
                    ui.selectable_value(&mut look.brow, brow.label.clone(), &brow.label);
                }
            });
        paint_row(ui, &mut look.brow_paint, "colour");
    });
    egui::CollapsingHeader::new("hair").show(ui, |ui| {
        let chosen = if look.hair_style == NO_HAIR {
            NO_HAIR
        } else {
            layers
                .hairs
                .iter()
                .find(|hair| hair.label == look.hair_style)
                .or_else(|| layers.hairs.first())
                .map_or(NO_HAIR, |hair| hair.label.as_str())
        };
        egui::ComboBox::from_id_salt("hair_style")
            .selected_text(chosen)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut look.hair_style, NO_HAIR.to_owned(), NO_HAIR);

                for hair in &layers.hairs {
                    ui.selectable_value(&mut look.hair_style, hair.label.clone(), &hair.label);
                }
            });
        ui.horizontal(|ui| {
            ui.color_edit_button_rgb(&mut look.hair.color);
            ui.label("colour");
        });
    });
    egui::CollapsingHeader::new("makeup").show(ui, |ui| {
        paint_row(ui, &mut look.eyeshadow, "eyeshadow");
        paint_row(ui, &mut look.liner, "liner");
        paint_row(ui, &mut look.blush, "blush");
        paint_row(ui, &mut look.lipstick, "lipstick");
    });
}

fn paint_row(ui: &mut egui::Ui, paint: &mut SkinPaint, text: &str) {
    ui.horizontal(|ui| {
        ui.color_edit_button_rgb(&mut paint.color);
        ui.add(egui::Slider::new(&mut paint.strength, 0.0..=1.0).text(text));
    });
}

fn slider_groups(ui: &mut egui::Ui, face: &mut CharacterAppearance, sliders: &[FaceSlider]) {
    let mut groups: Vec<&str> = Vec::new();

    for slider in sliders {
        if !groups.contains(&slider.group.as_str()) {
            groups.push(&slider.group);
        }
    }

    for group in groups {
        let members = sliders
            .iter()
            .filter(|slider| slider.group == group)
            .collect::<Vec<_>>();
        egui::CollapsingHeader::new(group).show(ui, |ui| {
            for (choice, options) in choices_of(&members) {
                choice_rows(ui, face, choice, &options);
            }

            for slider in members.iter().filter(|s| s.choice.is_none() && !s.detail) {
                slider_row(ui, face, slider, &slider.label);
            }

            let details = members
                .iter()
                .filter(|s| s.choice.is_none() && s.detail)
                .collect::<Vec<_>>();

            if !details.is_empty() {
                egui::CollapsingHeader::new("details")
                    .id_salt((group, "details"))
                    .show(ui, |ui| {
                        for slider in details {
                            slider_row(ui, face, slider, &slider.label);
                        }
                    });
            }
        });
    }
}

/// The choices among the sliders, each with its options, in the sliders' order.
fn choices(sliders: &[FaceSlider]) -> Vec<(&str, Vec<&FaceSlider>)> {
    choices_of(&sliders.iter().collect::<Vec<_>>())
}

fn choices_of<'a>(sliders: &[&'a FaceSlider]) -> Vec<(&'a str, Vec<&'a FaceSlider>)> {
    let mut out: Vec<(&str, Vec<&FaceSlider>)> = Vec::new();

    for slider in sliders {
        let Some(choice) = slider.choice.as_deref() else {
            continue;
        };

        match out.iter_mut().find(|(name, _)| *name == choice) {
            Some((_, options)) => options.push(slider),
            None => out.push((choice, vec![slider])),
        }
    }

    out
}

/// A choice: which of its options (or none), and that option's strength.
fn choice_rows(
    ui: &mut egui::Ui,
    face: &mut CharacterAppearance,
    choice: &str,
    options: &[&FaceSlider],
) {
    let current = options
        .iter()
        .find(|option| face.sliders.get(&option.label).is_some_and(|v| *v != 0.0))
        .map(|option| option.label.clone());
    let mut selected = current.clone();
    egui::ComboBox::from_label(choice)
        .selected_text(selected.as_deref().unwrap_or("none"))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut selected, None, "none");

            for option in options {
                ui.selectable_value(&mut selected, Some(option.label.clone()), &option.label);
            }
        });

    if selected != current {
        let strength = current
            .as_ref()
            .and_then(|label| face.sliders.get(label).copied())
            .unwrap_or(0.0);

        for option in options {
            face.sliders.remove(&option.label);
        }

        if let Some(option) = options.iter().find(|o| Some(&o.label) == selected.as_ref()) {
            let strength = if strength > 0.0 {
                strength.min(option.range[1])
            } else {
                option.range[1] / 2.0
            };
            face.sliders.insert(option.label.clone(), strength);
        }
    }

    if let Some(option) = options.iter().find(|o| Some(&o.label) == selected.as_ref()) {
        slider_row(ui, face, option, "strength");
    }
}

fn slider_row(ui: &mut egui::Ui, face: &mut CharacterAppearance, slider: &FaceSlider, text: &str) {
    let mut value = face.sliders.get(&slider.label).copied().unwrap_or(0.0);
    let [low, high] = slider.range;

    if ui
        .add(egui::Slider::new(&mut value, low..=high).text(text))
        .changed()
    {
        if value == 0.0 {
            face.sliders.remove(&slider.label);
        } else {
            face.sliders.insert(slider.label.clone(), value);
        }
    }
}
