use bevy::{gizmos::config::GizmoConfigStore, prelude::*, render::view::Msaa};
use bevy_egui::{EguiPrimaryContextPass, egui};
use engine::{
    CharacterPresentationCatalogSummary, CharacterPresentationPreview,
    CharacterPresentationPreviewPlugin, CharacterPreviewClip, CharacterPreviewClipRole,
    DEFAULT_CHARACTER_PRESENTATION_ID, load_character_presentation_catalog_summary,
};

use super::{EditorWorkspace, animation_workspace_active};
use crate::shell::{EditorUiFrame, EditorUiSet};

#[derive(Component)]
pub(crate) struct AnimationWorkspaceCamera;

pub(crate) struct AnimationWorkspacePlugin;

impl Plugin for AnimationWorkspacePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(CharacterPresentationPreviewPlugin)
            .init_resource::<AnimationWorkspaceState>()
            .init_resource::<AnimationPreviewSyncState>()
            .add_systems(Startup, setup_animation_workspace)
            .add_systems(
                OnEnter(EditorWorkspace::Animation),
                enter_animation_workspace,
            )
            .add_systems(OnExit(EditorWorkspace::Animation), exit_animation_workspace)
            .add_systems(
                Update,
                (advance_animation_transport, sync_animation_preview)
                    .chain()
                    .run_if(animation_workspace_active),
            )
            .add_systems(
                PostUpdate,
                draw_animation_stage
                    .run_if(animation_workspace_active)
                    .run_if(resource_exists::<GizmoConfigStore>),
            )
            .add_systems(
                EguiPrimaryContextPass,
                animation_workspace_ui
                    .run_if(animation_workspace_active)
                    .in_set(EditorUiSet::Workspace),
            );
    }
}

pub(crate) fn setup_animation_workspace(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Msaa::Off,
        Camera {
            is_active: false,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.075, 0.08, 0.09)),
            ..default()
        },
        Transform::from_xyz(0.0, 1.4, 4.0).looking_at(Vec3::Y, Vec3::Y),
        AnimationWorkspaceCamera,
        vegetation_render::VegetationViewDisabled,
        Name::new("Animation workspace camera"),
    ));
    commands.spawn((
        Transform::default(),
        Visibility::Hidden,
        CharacterPresentationPreview::new(DEFAULT_CHARACTER_PRESENTATION_ID),
        AnimationPreviewActor,
        Name::new("Animation workspace presentation actor"),
    ));
}

#[derive(Component)]
struct AnimationPreviewActor;

#[derive(Resource, Default)]
struct AnimationPreviewSyncState {
    restart_generation: u64,
}

#[derive(Resource, Debug)]
pub(crate) struct AnimationWorkspaceState {
    active: bool,
    generation: u64,
    playing: bool,
    time_seconds: f32,
    speed: f32,
    catalog: CharacterPresentationCatalogSummary,
    presentation_profile: String,
    movement_context: String,
    selected_clip_id: String,
    selected_clip: CharacterPreviewClip,
    restart_generation: u64,
    crouch_set: Option<String>,
    additional_bank: Option<String>,
    equipment_combat_set: Option<String>,
}

impl Default for AnimationWorkspaceState {
    fn default() -> Self {
        let catalog = load_character_presentation_catalog_summary()
            .unwrap_or_else(|error| panic!("invalid character presentation catalog: {error}"));
        let profile = catalog
            .profiles
            .iter()
            .find(|profile| profile.id == DEFAULT_CHARACTER_PRESENTATION_ID)
            .or_else(|| catalog.profiles.first())
            .expect("validated character catalog contains a presentation profile");
        let context = profile
            .movement_contexts
            .iter()
            .find(|context| context.context == profile.default_movement_context)
            .expect("validated default movement context exists");
        let clip = context
            .clips
            .iter()
            .find(|clip| clip.role == CharacterPreviewClipRole::Idle)
            .expect("validated movement set contains Idle");
        Self {
            active: false,
            generation: 0,
            playing: false,
            time_seconds: 0.0,
            speed: 1.0,
            presentation_profile: profile.id.clone(),
            movement_context: context.context.clone(),
            selected_clip_id: clip.id.clone(),
            selected_clip: CharacterPreviewClip::Idle,
            catalog,
            restart_generation: 0,
            crouch_set: None,
            additional_bank: None,
            equipment_combat_set: None,
        }
    }
}

impl AnimationWorkspaceState {
    fn enter(&mut self) {
        self.active = true;
        self.generation = self.generation.wrapping_add(1).max(1);
        self.playing = false;
        self.time_seconds = 0.0;
        self.restart_generation = self.restart_generation.wrapping_add(1).max(1);
    }

    fn exit(&mut self) {
        self.active = false;
        self.playing = false;
        self.time_seconds = 0.0;
    }
}

impl AnimationWorkspaceState {
    fn select_profile_defaults(&mut self) {
        let Some(profile) = self
            .catalog
            .profiles
            .iter()
            .find(|profile| profile.id == self.presentation_profile)
        else {
            return;
        };
        self.movement_context = profile.default_movement_context.clone();
        self.select_context_default_clip();
    }

    fn select_context_default_clip(&mut self) {
        let Some(context) = self.catalog.profiles.iter().find_map(|profile| {
            (profile.id == self.presentation_profile).then(|| {
                profile
                    .movement_contexts
                    .iter()
                    .find(|context| context.context == self.movement_context)
            })?
        }) else {
            return;
        };
        if let Some(clip) = context
            .clips
            .iter()
            .find(|clip| clip.role == CharacterPreviewClipRole::Idle)
        {
            self.selected_clip_id = clip.id.clone();
            self.selected_clip = CharacterPreviewClip::Idle;
            self.restart();
        }
    }

    fn select_clip(&mut self, id: String, role: CharacterPreviewClipRole) {
        self.selected_clip_id = id;
        self.selected_clip = match role {
            CharacterPreviewClipRole::Idle => CharacterPreviewClip::Idle,
            CharacterPreviewClipRole::Walk => CharacterPreviewClip::Walk,
            CharacterPreviewClipRole::Jog => CharacterPreviewClip::Jog,
        };
        self.restart();
    }

    fn restart(&mut self) {
        self.time_seconds = 0.0;
        self.restart_generation = self.restart_generation.wrapping_add(1).max(1);
    }
}

fn enter_animation_workspace(
    mut state: ResMut<AnimationWorkspaceState>,
    mut actors: Query<&mut Visibility, With<AnimationPreviewActor>>,
) {
    state.enter();
    for mut visibility in &mut actors {
        *visibility = Visibility::Inherited;
    }
}

fn exit_animation_workspace(
    mut state: ResMut<AnimationWorkspaceState>,
    mut actors: Query<&mut Visibility, With<AnimationPreviewActor>>,
) {
    state.exit();
    for mut visibility in &mut actors {
        *visibility = Visibility::Hidden;
    }
}

fn advance_animation_transport(time: Res<Time>, mut state: ResMut<AnimationWorkspaceState>) {
    if state.active && state.playing {
        state.time_seconds += time.delta_secs() * state.speed;
    }
}

fn sync_animation_preview(
    state: Res<AnimationWorkspaceState>,
    mut sync: ResMut<AnimationPreviewSyncState>,
    mut preview: Single<&mut CharacterPresentationPreview, With<AnimationPreviewActor>>,
) {
    preview.set_profile(state.presentation_profile.clone());
    preview.set_clip(state.selected_clip);
    preview.set_transport(state.playing, state.speed);
    if sync.restart_generation != state.restart_generation {
        preview.restart();
        sync.restart_generation = state.restart_generation;
    }
}

fn draw_animation_stage(mut gizmos: Gizmos) {
    let color = Color::srgba(0.28, 0.32, 0.38, 0.72);
    for coordinate in -10..=10 {
        let coordinate = coordinate as f32 * 0.5;
        gizmos.line(
            Vec3::new(coordinate, 0.0, -5.0),
            Vec3::new(coordinate, 0.0, 5.0),
            color,
        );
        gizmos.line(
            Vec3::new(-5.0, 0.0, coordinate),
            Vec3::new(5.0, 0.0, coordinate),
            color,
        );
    }
}

fn animation_workspace_ui(
    mut frame: ResMut<EditorUiFrame>,
    mut state: ResMut<AnimationWorkspaceState>,
) {
    let Some(viewport_ui) = frame.0.as_mut() else {
        return;
    };
    render_animation_workspace(viewport_ui, &mut state);
}

fn render_animation_workspace(workspace_ui: &mut egui::Ui, state: &mut AnimationWorkspaceState) {
    egui::Panel::left("animation_catalog_panel")
        .default_size(340.0)
        .resizable(true)
        .show(workspace_ui, |ui| {
            ui.heading("Animation workspace");
            ui.small(format!(
                "Catalog-backed preview session {} · {:.2}s",
                state.generation, state.time_seconds
            ));
            ui.add_space(8.0);

            let profiles = state
                .catalog
                .profiles
                .iter()
                .map(|profile| (profile.id.clone(), profile.name.clone()))
                .collect::<Vec<_>>();
            let mut profile_changed = false;
            egui::ComboBox::from_label("Presentation profile")
                .selected_text(
                    profiles
                        .iter()
                        .find(|(id, _)| id == &state.presentation_profile)
                        .map_or(state.presentation_profile.as_str(), |(_, name)| name.as_str()),
                )
                .show_ui(ui, |ui| {
                    for (id, name) in &profiles {
                        profile_changed |= ui
                            .selectable_value(&mut state.presentation_profile, id.clone(), name)
                            .changed();
                    }
                });
            if profile_changed {
                state.select_profile_defaults();
            }

            let contexts = state
                .catalog
                .profiles
                .iter()
                .find(|profile| profile.id == state.presentation_profile)
                .map(|profile| {
                    profile
                        .movement_contexts
                        .iter()
                        .map(|context| {
                            (
                                context.context.clone(),
                                context.movement_set_name.clone(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut context_changed = false;
            egui::ComboBox::from_label("Movement context")
                .selected_text(
                    contexts
                        .iter()
                        .find(|(context, _)| context == &state.movement_context)
                        .map_or(state.movement_context.as_str(), |(_, name)| name.as_str()),
                )
                .show_ui(ui, |ui| {
                    for (context, name) in &contexts {
                        context_changed |= ui
                            .selectable_value(&mut state.movement_context, context.clone(), name)
                            .changed();
                    }
                });
            if context_changed {
                state.select_context_default_clip();
            }

            ui.separator();
            ui.strong("Clips");
            let clips = state
                .catalog
                .profiles
                .iter()
                .find(|profile| profile.id == state.presentation_profile)
                .and_then(|profile| {
                    profile
                        .movement_contexts
                        .iter()
                        .find(|context| context.context == state.movement_context)
                })
                .map(|context| context.clips.clone())
                .unwrap_or_default();
            for clip in clips {
                let selected = state.selected_clip_id == clip.id;
                if ui
                    .selectable_label(
                        selected,
                        format!("{} · {:?} · {}", clip.name, clip.role, clip.playback),
                    )
                    .clicked()
                    && !selected
                {
                    state.select_clip(clip.id, clip.role);
                }
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button(if state.playing { "Pause" } else { "Play" })
                    .clicked()
                {
                    state.playing = !state.playing;
                }
                if ui.button("Restart").clicked() {
                    state.restart();
                }
            });
            ui.add(egui::Slider::new(&mut state.speed, 0.1..=2.0).text("Playback speed"));

            ui.separator();
            let profile = state
                .catalog
                .profiles
                .iter()
                .find(|profile| profile.id == state.presentation_profile);
            egui::Grid::new("animation_presentation_contract")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Profile ID");
                    ui.monospace(&state.presentation_profile);
                    ui.end_row();
                    ui.label("Model");
                    ui.monospace(profile.map_or("missing", |profile| profile.model_name.as_str()));
                    ui.end_row();
                    ui.label("Model asset");
                    ui.monospace(profile.map_or("missing", |profile| profile.model_asset.as_str()));
                    ui.end_row();
                    ui.label("Movement context");
                    ui.monospace(&state.movement_context);
                    ui.end_row();
                    ui.label("Stable clip");
                    ui.monospace(&state.selected_clip_id);
                    ui.end_row();
                    ui.label("Crouch set");
                    ui.monospace(state.crouch_set.as_deref().unwrap_or("not assigned"));
                    ui.end_row();
                    ui.label("Additional bank");
                    ui.monospace(state.additional_bank.as_deref().unwrap_or("not assigned"));
                    ui.end_row();
                    ui.label("Equipment combat set");
                    ui.monospace(
                        state
                            .equipment_combat_set
                            .as_deref()
                            .unwrap_or("not assigned"),
                    );
                    ui.end_row();
                });
            ui.weak(
                "The preview uses the runtime presentation resolver and real GLTF clips. Reserved crouch, additional-bank, and equipment-set slots remain explicit catalog extensions.",
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_workspace_preserves_presentation_choice_but_resets_transport() {
        let mut state = AnimationWorkspaceState {
            presentation_profile: "presentations/male/custom".into(),
            ..default()
        };
        state.enter();
        state.playing = true;
        state.time_seconds = 4.0;
        state.exit();
        assert!(!state.active);
        assert!(!state.playing);
        assert_eq!(state.time_seconds, 0.0);
        assert_eq!(state.presentation_profile, "presentations/male/custom");
    }
}
