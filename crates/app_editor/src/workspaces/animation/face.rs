//! The face panel: the preview character's face sliders, read from its kit, and the faces kept
//! as files (`engine::saved_faces`).

use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use bevy_egui::egui;
use engine::{CharacterAppearance, CharacterKit, CharacterKitRef, FaceSlider, saved_faces};

use super::{AnimationPreviewActor, AnimationWorkspaceCamera};
use crate::shell::EditorUiFrame;

/// The face framings: how far from the face the camera stands and how far below the top of the
/// head it looks, metres.
const FACE_DISTANCE: f32 = 0.55;

const FACE_DROP: f32 = 0.15;

/// The share of each slider's range a random face reaches: sliders, details.
const RANDOM_REACH: f32 = 0.6;

const RANDOM_DETAIL_REACH: f32 = 0.3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Framing {
    #[default]
    Body,
    Face,
    ThreeQuarter,
    Profile,
}

impl Framing {
    const ALL: [Self; 4] = [Self::Body, Self::Face, Self::ThreeQuarter, Self::Profile];

    fn label(self) -> &'static str {
        match self {
            Self::Body => "Body",
            Self::Face => "Face",
            Self::ThreeQuarter => "¾",
            Self::Profile => "Profile",
        }
    }

    /// The camera for this framing of a character standing at the origin facing +Z.
    pub(super) fn view(self, height: f32) -> Transform {
        let turn: f32 = match self {
            Self::Body => return Transform::from_xyz(0.0, 1.4, 4.0).looking_at(Vec3::Y, Vec3::Y),
            Self::Face => 0.0,
            Self::ThreeQuarter => 35.0,
            Self::Profile => 90.0,
        };
        let face = Vec3::new(0.0, height - FACE_DROP, 0.0);
        let direction = Quat::from_rotation_y(turn.to_radians()) * Vec3::Z;
        Transform::from_translation(face + direction * FACE_DISTANCE).looking_at(face, Vec3::Y)
    }
}

#[derive(Resource)]
pub(super) struct FaceEditor {
    appearance: CharacterAppearance,
    name: String,
    status: String,
    framing: Framing,
    framed: Option<Framing>,
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
            framing: Framing::Body,
            framed: None,
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
        let mut face = CharacterAppearance::default();

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
    mut actor: Single<&mut CharacterAppearance, With<AnimationPreviewActor>>,
) {
    actor.set_if_neq(editor.appearance.clone());
}

pub(super) fn frame_camera(
    mut editor: ResMut<FaceEditor>,
    kits: Res<Assets<CharacterKit>>,
    refs: Query<(&CharacterKitRef, &ChildOf)>,
    actor: Single<Entity, With<AnimationPreviewActor>>,
    mut camera: Single<&mut Transform, With<AnimationWorkspaceCamera>>,
) {
    if editor.framed == Some(editor.framing) {
        return;
    }

    // The body framing stands back from any character; the others wait for the kit's height.
    let height = match preview_kit(*actor, &refs, &kits) {
        Some(kit) => kit.height(),
        None if editor.framing == Framing::Body => 0.0,
        None => return,
    };

    **camera = editor.framing.view(height);
    editor.framed = Some(editor.framing);
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

pub(super) fn face_panel_ui(
    mut frame: ResMut<EditorUiFrame>,
    mut editor: ResMut<FaceEditor>,
    kits: Res<Assets<CharacterKit>>,
    refs: Query<(&CharacterKitRef, &ChildOf)>,
    actor: Single<Entity, With<AnimationPreviewActor>>,
) {
    let Some(workspace_ui) = frame.0.as_mut() else {
        return;
    };
    let kit = preview_kit(*actor, &refs, &kits);

    egui::Panel::right("face_panel")
        .default_size(360.0)
        .resizable(true)
        .show(workspace_ui, |ui| {
            ui.heading("Face");
            framing_row(ui, &mut editor);

            let Some(kit) = kit else {
                ui.weak(
                    "This profile's model has no kit: faces need the kit export_glb.py writes.",
                );
                return;
            };

            file_rows(ui, &mut editor, kit.sliders());
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                slider_groups(ui, &mut editor.appearance, kit.sliders());
            });
        });
}

fn framing_row(ui: &mut egui::Ui, editor: &mut FaceEditor) {
    ui.horizontal(|ui| {
        for framing in Framing::ALL {
            ui.selectable_value(&mut editor.framing, framing, framing.label());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framings_look_at_the_face_from_their_side() {
        let height = 1.75;
        let face = Framing::Face.view(height);
        assert!((face.translation.z - FACE_DISTANCE).abs() < 1e-5);
        let profile = Framing::Profile.view(height);
        assert!((profile.translation.x - FACE_DISTANCE).abs() < 1e-5);
        assert!(profile.forward().x < -0.99);
    }
}
