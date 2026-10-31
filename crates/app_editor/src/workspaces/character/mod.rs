//! The Character workspace: a catalog character on a neutral stage, its clips on a timeline, its
//! face on sliders, and another character beside it to compare with.

use bevy::{
    camera::Exposure,
    core_pipeline::prepass::{DeferredPrepass, DepthPrepass},
    gizmos::config::GizmoConfigStore,
    prelude::*,
    render::view::Msaa,
};
use bevy_egui::{EguiPrimaryContextPass, egui};

use engine::{
    CharacterAppearance, CharacterPresentationCatalogSummary, CharacterPresentationPreview,
    CharacterPresentationPreviewPlugin, CharacterPreviewClip, CharacterPreviewClipRole,
    CharacterPreviewPlayhead, DEFAULT_CHARACTER_PRESENTATION_ID,
    load_character_presentation_catalog_summary,
};

mod face;
mod stage;

use super::{EditorWorkspace, character_workspace_active};
use crate::shell::{EditorUiFrame, EditorUiSet};

/// The character the workspace opens on: the one its face sliders and skin are made for.
const OPENING_PROFILE: &str = "presentations/female/mpfb_fitted";

/// The clips are sampled at 30 frames a second: a step moves one of them.
const STEP_SECONDS: f32 = 1.0 / 30.0;

const SPEEDS: std::ops::RangeInclusive<f32> = 0.1..=2.0;

#[derive(Component)]
pub(crate) struct CharacterWorkspaceCamera;

pub(crate) struct CharacterWorkspacePlugin;

impl Plugin for CharacterWorkspacePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(CharacterPresentationPreviewPlugin)
            .init_resource::<CharacterWorkspaceState>()
            .init_resource::<stage::Stage>()
            .init_resource::<stage::SavedLight>()
            .init_resource::<face::FaceEditor>()
            .add_systems(Startup, (setup_character_workspace, stage::setup))
            .add_systems(
                OnEnter(EditorWorkspace::Character),
                (enter_character_workspace, stage::enter),
            )
            .add_systems(
                OnExit(EditorWorkspace::Character),
                (exit_character_workspace, stage::exit),
            )
            .add_systems(
                Update,
                (
                    sync_previews,
                    face::sync_face,
                    measure_character,
                    stage::gather_meshes,
                    stage::apply,
                )
                    .chain()
                    .run_if(character_workspace_active),
            )
            .add_systems(
                PostUpdate,
                stage::draw_grid
                    .run_if(character_workspace_active)
                    .run_if(resource_exists::<GizmoConfigStore>),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (
                    character_panel,
                    face::face_panel,
                    transport_panel,
                    stage::viewport_panel,
                )
                    .chain()
                    .run_if(character_workspace_active)
                    .in_set(EditorUiSet::Workspace),
            );
    }
}

pub(crate) fn setup_character_workspace(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Msaa::Off,
        // Environment materials draw into a G-buffer, lit by the deferred lighting.
        DepthPrepass,
        DeferredPrepass,
        Exposure { ev100: 13.5 },
        stage::camera_bundle(),
        CharacterWorkspaceCamera,
        vegetation_render::VegetationViewDisabled,
        Name::new("Character workspace camera"),
    ));
    commands.spawn((
        Transform::default(),
        Visibility::Hidden,
        CharacterPresentationPreview::new(OPENING_PROFILE),
        CharacterAppearance::default(),
        PreviewActor,
        Name::new("Character workspace actor"),
    ));
    commands.spawn((
        Transform::from_xyz(-stage::COMPARE_ASIDE, 0.0, 0.0),
        Visibility::Hidden,
        CharacterPresentationPreview::new(DEFAULT_CHARACTER_PRESENTATION_ID),
        CompareActor,
        Name::new("Character workspace compared actor"),
    ));
}

#[derive(Component)]
struct PreviewActor;

#[derive(Component)]
struct CompareActor;

#[derive(Resource, Debug)]
pub(crate) struct CharacterWorkspaceState {
    playing: bool,
    speed: f32,
    catalog: CharacterPresentationCatalogSummary,
    profile: String,
    movement_context: String,
    clip_id: String,
    clip: CharacterPreviewClip,
    compare: Option<String>,
    /// Seconds to move both actors' clips to.
    seek: Option<f32>,
    playhead: CharacterPreviewPlayhead,
}

impl Default for CharacterWorkspaceState {
    fn default() -> Self {
        let catalog = load_character_presentation_catalog_summary()
            .unwrap_or_else(|error| panic!("invalid character presentation catalog: {error}"));
        let profile = catalog
            .profiles
            .iter()
            .find(|profile| profile.id == OPENING_PROFILE)
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
            playing: true,
            speed: 1.0,
            profile: profile.id.clone(),
            movement_context: context.context.clone(),
            clip_id: clip.id.clone(),
            clip: CharacterPreviewClip::Idle,
            catalog,
            compare: None,
            seek: Some(0.0),
            playhead: CharacterPreviewPlayhead::default(),
        }
    }
}

impl CharacterWorkspaceState {
    fn select_profile_defaults(&mut self) {
        let Some(profile) = self
            .catalog
            .profiles
            .iter()
            .find(|profile| profile.id == self.profile)
        else {
            return;
        };
        self.movement_context = profile.default_movement_context.clone();
        self.select_context_default_clip();
    }

    fn select_context_default_clip(&mut self) {
        let Some(context) = self.catalog.profiles.iter().find_map(|profile| {
            (profile.id == self.profile).then(|| {
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
            self.clip_id = clip.id.clone();
            self.clip = CharacterPreviewClip::Idle;
            self.seek = Some(0.0);
        }
    }

    fn select_clip(&mut self, id: String, role: CharacterPreviewClipRole) {
        self.clip_id = id;
        self.clip = match role {
            CharacterPreviewClipRole::Idle => CharacterPreviewClip::Idle,
            CharacterPreviewClipRole::Walk => CharacterPreviewClip::Walk,
            CharacterPreviewClipRole::Jog => CharacterPreviewClip::Jog,
        };
        self.seek = Some(0.0);
    }

    /// Stops on the frame `frames` from the playhead, round the clip's loop.
    fn step(&mut self, frames: f32) {
        self.playing = false;
        let duration = self.playhead.duration.max(STEP_SECONDS);
        self.seek = Some((self.playhead.seconds + frames * STEP_SECONDS).rem_euclid(duration));
    }

    fn profile_name(&self, id: &str) -> String {
        self.catalog
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .map_or_else(|| id.to_owned(), |profile| profile.name.clone())
    }
}

fn enter_character_workspace(
    mut state: ResMut<CharacterWorkspaceState>,
    mut actors: Query<&mut Visibility, With<PreviewActor>>,
) {
    state.seek = Some(0.0);

    for mut visibility in &mut actors {
        *visibility = Visibility::Inherited;
    }
}

/// Both characters' visibility.
type ActorVisibility<'w, 's> =
    Query<'w, 's, &'static mut Visibility, Or<(With<PreviewActor>, With<CompareActor>)>>;

fn exit_character_workspace(mut actors: ActorVisibility) {
    for mut visibility in &mut actors {
        *visibility = Visibility::Hidden;
    }
}

/// The character being worked on and where its clip is.
type MainPreview<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut CharacterPresentationPreview,
        Option<&'static CharacterPreviewPlayhead>,
    ),
    (With<PreviewActor>, Without<CompareActor>),
>;

/// The character beside it.
type ComparedPreview<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut CharacterPresentationPreview,
        &'static mut Visibility,
    ),
    (With<CompareActor>, Without<PreviewActor>),
>;

fn sync_previews(
    mut state: ResMut<CharacterWorkspaceState>,
    mut stage: ResMut<stage::Stage>,
    mut main: MainPreview,
    mut compared: ComparedPreview,
) {
    let seek = state.seek.take();

    for (mut preview, playhead) in &mut main {
        preview.set_profile(state.profile.clone());
        preview.set_clip(state.clip);
        preview.set_transport(state.playing, state.speed);

        if let Some(seconds) = seek {
            preview.seek(seconds);
        }

        if let Some(playhead) = playhead {
            state.playhead = *playhead;
        }
    }

    stage.comparing = state.compare.is_some();

    for (mut preview, mut visibility) in &mut compared {
        *visibility = if state.compare.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };

        if let Some(profile) = &state.compare {
            preview.set_profile(profile.clone());
        }

        preview.set_clip(state.clip);
        preview.set_transport(state.playing, state.speed);

        if let Some(seconds) = seek {
            preview.seek(seconds);
        }
    }
}

/// The framings stand back from a character by its own height.
fn measure_character(
    mut stage: ResMut<stage::Stage>,
    kits: Res<Assets<engine::CharacterKit>>,
    refs: Query<(&engine::CharacterKitRef, &ChildOf)>,
    actor: Query<Entity, With<PreviewActor>>,
) {
    for actor in &actor {
        let height = refs
            .iter()
            .find(|(_, parent)| parent.parent() == actor)
            .and_then(|(kit, _)| kits.get(kit.kit()))
            .map(engine::CharacterKit::height);

        if let Some(height) = height {
            stage.height = height;
        }
    }
}

/// The character, its clip and what it is compared with.
fn character_panel(mut frame: ResMut<EditorUiFrame>, mut state: ResMut<CharacterWorkspaceState>) {
    let Some(workspace_ui) = frame.0.as_mut() else {
        return;
    };

    egui::Panel::left("character_panel")
        .default_size(190.0)
        .min_size(130.0)
        .resizable(true)
        .show(workspace_ui, |ui| {
            ui.heading("Character");
            profile_row(ui, &mut state);
            context_row(ui, &mut state);
            ui.separator();
            clip_rows(ui, &mut state);
            ui.separator();
            compare_row(ui, &mut state);
        });
}

fn profile_row(ui: &mut egui::Ui, state: &mut CharacterWorkspaceState) {
    let profiles = state
        .catalog
        .profiles
        .iter()
        .map(|profile| {
            (
                profile.id.clone(),
                profile.name.clone(),
                profile.model_asset.clone(),
            )
        })
        .collect::<Vec<_>>();
    let selected = state.profile_name(&state.profile);
    let mut changed = false;
    let combo = egui::ComboBox::from_id_salt("character_profile")
        .selected_text(selected)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (id, name, _) in &profiles {
                changed |= ui
                    .selectable_value(&mut state.profile, id.clone(), name)
                    .changed();
            }
        });

    if let Some((_, _, asset)) = profiles.iter().find(|(id, ..)| *id == state.profile) {
        combo.response.on_hover_text(asset);
    }

    if changed {
        state.select_profile_defaults();
    }
}

/// Only a character with more than one way of moving has a choice here.
fn context_row(ui: &mut egui::Ui, state: &mut CharacterWorkspaceState) {
    let contexts = state
        .catalog
        .profiles
        .iter()
        .find(|profile| profile.id == state.profile)
        .map(|profile| {
            profile
                .movement_contexts
                .iter()
                .map(|context| (context.context.clone(), context.movement_set_name.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if contexts.len() < 2 {
        return;
    }

    let mut changed = false;
    egui::ComboBox::from_id_salt("character_context")
        .selected_text(
            contexts
                .iter()
                .find(|(context, _)| *context == state.movement_context)
                .map_or(state.movement_context.as_str(), |(_, name)| name.as_str()),
        )
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (context, name) in &contexts {
                changed |= ui
                    .selectable_value(&mut state.movement_context, context.clone(), name)
                    .changed();
            }
        });

    if changed {
        state.select_context_default_clip();
    }
}

fn clip_rows(ui: &mut egui::Ui, state: &mut CharacterWorkspaceState) {
    let clips = state
        .catalog
        .profiles
        .iter()
        .find(|profile| profile.id == state.profile)
        .and_then(|profile| {
            profile
                .movement_contexts
                .iter()
                .find(|context| context.context == state.movement_context)
        })
        .map(|context| context.clips.clone())
        .unwrap_or_default();

    for clip in clips {
        let selected = state.clip_id == clip.id;

        if ui.selectable_label(selected, &clip.name).clicked() && !selected {
            state.select_clip(clip.id, clip.role);
        }
    }
}

fn compare_row(ui: &mut egui::Ui, state: &mut CharacterWorkspaceState) {
    let others = state
        .catalog
        .profiles
        .iter()
        .filter(|profile| profile.id != state.profile)
        .map(|profile| (profile.id.clone(), profile.name.clone()))
        .collect::<Vec<_>>();
    ui.label("Compare with");
    egui::ComboBox::from_id_salt("character_compare")
        .selected_text(
            state
                .compare
                .as_deref()
                .map_or_else(|| "nobody".to_owned(), |id| state.profile_name(id)),
        )
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.compare, None, "nobody");

            for (id, name) in &others {
                ui.selectable_value(&mut state.compare, Some(id.clone()), name);
            }
        });
}

/// Play, step, scrub: the clip's timeline along the bottom of the window.
fn transport_panel(mut frame: ResMut<EditorUiFrame>, mut state: ResMut<CharacterWorkspaceState>) {
    let Some(workspace_ui) = frame.0.as_mut() else {
        return;
    };

    egui::Panel::bottom("character_transport").show(workspace_ui, |ui| {
        ui.horizontal(|ui| {
            if ui.button(if state.playing { "⏸" } else { "▶" }).clicked() {
                state.playing = !state.playing;
            }

            if ui.button("⏮").on_hover_text("Back to the start").clicked() {
                state.seek = Some(0.0);
            }

            if ui.button("◀").on_hover_text("One frame back").clicked() {
                state.step(-1.0);
            }

            if ui.button("▶|").on_hover_text("One frame on").clicked() {
                state.step(1.0);
            }

            // The timeline takes what the clock and the speed control leave.
            let speed_slider = 70.0;
            let others = 300.0;
            timeline(ui, &mut state, (ui.available_width() - others).max(60.0));
            ui.monospace(format!(
                "{:5.2} / {:.2} s",
                state.playhead.seconds, state.playhead.duration
            ));
            ui.label("speed");
            ui.spacing_mut().slider_width = speed_slider;
            ui.add(egui::Slider::new(&mut state.speed, SPEEDS).fixed_decimals(2));
        });
    });
}

fn timeline(ui: &mut egui::Ui, state: &mut CharacterWorkspaceState, width: f32) {
    let duration = state.playhead.duration.max(STEP_SECONDS);
    let mut seconds = state.playhead.seconds;
    ui.spacing_mut().slider_width = width;
    let response = ui.add(egui::Slider::new(&mut seconds, 0.0..=duration).show_value(false));

    if response.changed() {
        state.playing = false;
        state.seek = Some(seconds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_lights_are_its_own_and_the_world_gets_its_light_back() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_plugins(super::super::EditorWorkspacesPlugin)
            .init_resource::<stage::SavedLight>()
            .init_resource::<GlobalAmbientLight>()
            .add_systems(OnEnter(EditorWorkspace::Character), stage::enter)
            .add_systems(OnExit(EditorWorkspace::Character), stage::exit);
        let transform = Transform::from_xyz(0.0, -1.0, 1.0).looking_at(Vec3::ZERO, Vec3::Y);
        let sun = app
            .world_mut()
            .spawn((
                engine::WorldSun,
                transform,
                DirectionalLight {
                    illuminance: 0.0,
                    ..default()
                },
            ))
            .id();
        app.update();
        app.world_mut()
            .resource_mut::<NextState<EditorWorkspace>>()
            .set(EditorWorkspace::Character);
        app.update();
        app.world_mut()
            .get_mut::<DirectionalLight>(sun)
            .unwrap()
            .illuminance = 50_000.0;
        app.world_mut()
            .resource_mut::<NextState<EditorWorkspace>>()
            .set(EditorWorkspace::World);
        app.update();
        assert_eq!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .illuminance,
            0.0
        );
        assert_eq!(*app.world().get::<Transform>(sun).unwrap(), transform);
    }

    #[test]
    fn stepping_stops_on_a_frame_round_the_clips_loop() {
        let mut state = CharacterWorkspaceState {
            playhead: CharacterPreviewPlayhead {
                seconds: 0.0,
                duration: 1.0,
            },
            ..default()
        };
        state.step(-1.0);
        assert!(!state.playing);
        assert!((state.seek.unwrap() - (1.0 - STEP_SECONDS)).abs() < 1e-5);
    }

    #[test]
    fn framings_look_at_the_face_from_their_side() {
        let face = stage::framing_faces(stage::FACE);
        assert!(face.translation.z > 0.5);
        let profile = stage::framing_faces(stage::PROFILE);
        assert!(profile.translation.x > 0.5);
        assert!(profile.forward().x < -0.9);
    }
}
