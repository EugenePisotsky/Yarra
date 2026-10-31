//! The character probe (`--character-probe PROFILE`): a character catalog profile stands ahead of
//! the player at the start, facing the camera and playing its idle, so a character can be judged
//! in the game's own light next to the one the player plays.
//! `content/characters/scripts/export_glb.py` makes one from an MPFB preset. With
//! `--character-face NAME` it wears a kept face (`engine::CharacterAppearance::load`). With
//! `--character-portrait DIR` the camera leaves the player for fixed views of the probe, each
//! saved to DIR, and the game exits: the same views of the same character after every change.
use std::path::PathBuf;

use crate::launch::LaunchOptions;
use crate::runtime_settings::RuntimeSettings;

use bevy::{
    ecs::system::SystemParam,
    prelude::*,
    render::view::screenshot::{Screenshot, save_to_disk},
};
use engine::{
    CharacterAppearance, GameplaySystems, WorldStartAdopted, WorldStartView, WorldSun,
    WorldViewCamera,
};
use world::WorldViewBookmark;

/// How far ahead of the start's focus the model stands and how far to the camera's right,
/// metres: off the line of sight, so the player does not hide it.
const AHEAD_M: f32 = 2.4;

const ASIDE_M: f32 = 0.9;

/// How long a portrait's probe waits to stand, for the clock to have placed the sun it faces.
const PORTRAIT_SUN_SECONDS: f32 = 5.0;

/// How long the world gets to load around the probe, then each view to settle: the temporal
/// upscaler and eye adaptation need a moment on a new view.
const PORTRAIT_LOAD_SECONDS: f32 = 30.0;

const PORTRAIT_VIEW_SECONDS: f32 = 3.0;

/// How long after its last view the game runs, for the last screenshot to reach the disk.
const PORTRAIT_FINISH_SECONDS: f32 = 2.0;

/// How far from straight at the sun a portrait's character faces, degrees: the sun on one side
/// of the face, as a portrait's key light is.
const PORTRAIT_SUN_TURN: f32 = 35.0;

/// A portrait's views: its file, the camera's turn round the character from its front, degrees
/// (positive towards the lit side), its aim above the head joint and its distance, metres.
const PORTRAIT_VIEWS: [(&str, f32, f32, f32); 8] = [
    ("face_front", 0.0, 0.02, 0.62),
    ("face_three_quarter", 35.0, 0.02, 0.62),
    ("face_shadow_side", -40.0, 0.02, 0.62),
    ("face_profile", 90.0, 0.02, 0.62),
    ("eyes_close", 30.0, 0.04, 0.28),
    ("mouth_close", 35.0, -0.05, 0.30),
    ("upper_body", 20.0, -0.20, 1.50),
    ("full_body", 0.0, -0.75, 3.60),
];

#[derive(Resource)]
struct ProbeProfile {
    profile: String,
    face: CharacterAppearance,
}

#[derive(Component)]
struct CharacterProbe;

pub(crate) fn install(app: &mut App) {
    let options = app.world().resource::<LaunchOptions>();
    let Some(profile) = options.character_probe.clone() else {
        return;
    };
    let face = match options.character_face.as_deref() {
        Some(name) => CharacterAppearance::load(name)
            .unwrap_or_else(|error| panic!("--character-face: {error}")),
        None => CharacterAppearance::default(),
    };
    let portrait = options.character_portrait.clone();
    app.insert_resource(ProbeProfile { profile, face })
        .add_systems(Update, place_probe);

    if let Some(folder) = portrait {
        {
            let mut settings = app.world_mut().resource_mut::<RuntimeSettings>();
            settings.controls_locked = true;
            settings.show_ui = false;
        }

        app.insert_resource(Portrait {
            folder,
            view: 0,
            view_seconds: 0.0,
            waited: 0.0,
            finished: 0.0,
        })
        .add_systems(Update, take_portrait.after(GameplaySystems::CameraFollow));
    }
}

#[derive(Resource)]
struct Portrait {
    folder: PathBuf,
    view: usize,
    view_seconds: f32,
    waited: f32,
    finished: f32,
}

/// What a portrait's probe stands by: the sun, once the clock has placed it.
#[derive(SystemParam)]
struct PortraitStand<'w, 's> {
    portrait: Option<Res<'w, Portrait>>,
    sun: Query<'w, 's, &'static GlobalTransform, With<WorldSun>>,
    time: Res<'w, Time<Real>>,
}

impl PortraitStand<'_, '_> {
    fn waiting(&self) -> bool {
        self.portrait.is_some() && self.time.elapsed_secs() < PORTRAIT_SUN_SECONDS
    }

    /// With the sun on one side of the face.
    fn facing(&self) -> Option<Quat> {
        self.portrait.as_ref()?;
        let towards_sun = self.sun.single().ok()?.back();
        let yaw = towards_sun.x.atan2(towards_sun.z) - PORTRAIT_SUN_TURN.to_radians();
        Some(Quat::from_rotation_y(yaw))
    }
}

/// Holds the camera on the current view after the game has placed it, and saves each view once it
/// has settled. The views frame the head joint: the idle moves it.
fn take_portrait(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut portrait: ResMut<Portrait>,
    joints: Query<(&Name, &GlobalTransform)>,
    probe: Single<&GlobalTransform, With<CharacterProbe>>,
    mut camera: Single<&mut Transform, With<WorldViewCamera>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some((_, head)) = joints.iter().find(|(name, _)| name.as_str() == "head") else {
        return;
    };
    portrait.waited += time.delta_secs();

    if portrait.waited < PORTRAIT_LOAD_SECONDS {
        return;
    }

    let Some(&(name, turn, rise, distance)) = PORTRAIT_VIEWS.get(portrait.view) else {
        portrait.finished += time.delta_secs();

        if portrait.finished >= PORTRAIT_FINISH_SECONDS {
            exit.write(AppExit::Success);
        }

        return;
    };
    let target = head.translation() + Vec3::Y * rise;
    let side = probe.rotation() * Quat::from_rotation_y(turn.to_radians()) * Vec3::Z;
    **camera = Transform::from_translation(target + side * distance).looking_at(target, Vec3::Y);
    portrait.view_seconds += time.delta_secs();

    if portrait.view_seconds >= PORTRAIT_VIEW_SECONDS {
        let path = portrait.folder.join(format!("{name}.png"));
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        portrait.view += 1;
        portrait.view_seconds = 0.0;
    }
}

/// Stands the character at the start view, or again at the one the world adopts: its motor keeps
/// the facing it was spawned with, so a moved probe is a new one.
fn place_probe(
    mut commands: Commands,
    profile: Res<ProbeProfile>,
    start: Res<WorldStartView>,
    mut adopted: MessageReader<WorldStartAdopted>,
    probe: Query<Entity, With<CharacterProbe>>,
    portrait: PortraitStand,
) {
    if portrait.waiting() {
        return;
    }

    let view = match adopted.read().last() {
        Some(WorldStartAdopted(view)) => view,
        None if probe.is_empty() => match start.0.as_ref() {
            Some(view) => view,
            None => return,
        },
        None => return,
    };

    for entity in &probe {
        commands.entity(entity).despawn();
    }

    let mut place = stand(view);

    if let Some(rotation) = portrait.facing() {
        place.rotation = rotation;
    }

    commands.spawn((
        engine::standing_character(place, profile.profile.clone(), "Character probe"),
        profile.face.clone(),
        CharacterProbe,
    ));
}

/// Ahead of the focus along the camera's view and to its right, the model's front (glTF's +Z)
/// turned towards the camera.
fn stand(view: &WorldViewBookmark) -> Transform {
    let yaw = view.yaw_degrees.to_radians();
    let towards_camera = Vec3::new(yaw.sin(), 0.0, yaw.cos());
    let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    let focus = Vec3::from_array(view.position);
    Transform::from_translation(focus - towards_camera * AHEAD_M + right * ASIDE_M)
        .with_rotation(Quat::from_rotation_y(yaw))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(yaw_degrees: f32) -> WorldViewBookmark {
        WorldViewBookmark {
            position: [10.0, 5.0, -3.0],
            yaw_degrees,
            pitch_degrees: 10.0,
            distance: 4.0,
            fog_visibility: 1000.0,
            route: Vec::new(),
        }
    }

    #[test]
    fn the_model_stands_ahead_of_the_focus_and_faces_the_camera() {
        for yaw in [0.0, 45.0, 135.0, -90.0] {
            let view = view(yaw);
            let stand = stand(&view);
            let camera = engine::orbit_transform(
                Vec3::from_array(view.position),
                yaw.to_radians(),
                0.2,
                4.0,
            );
            let focus = Vec3::from_array(view.position);
            let ahead = (stand.translation - focus)
                .dot((focus - camera.translation).with_y(0.0).normalize());
            let facing = (stand.rotation * Vec3::Z).dot(
                (camera.translation - stand.translation)
                    .with_y(0.0)
                    .normalize(),
            );

            assert!((ahead - AHEAD_M).abs() < 1e-3, "yaw {yaw}: {ahead} m ahead");
            assert!(facing > 0.9, "yaw {yaw}: faces the camera by {facing}");
        }
    }
}
