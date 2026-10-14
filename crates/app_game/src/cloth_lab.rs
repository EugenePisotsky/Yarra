//! Cloth lab (`--cloth-lab`): the player and walking clones wear the engine's generated test
//! outfit ([`engine::TestOutfit`]): a cloak, a long skirt, a ponytail, a greatsword and a bow
//! on the back and a sword at the hip, all simulated ([`engine::ClothPlugin`]). The player plays
//! as usual; clones walk octagons around where they were placed. A [`panel`] chooses the pieces,
//! clones, gait and camera, tunes the simulation and shows what it costs.
//!
//! `--cloth-lab-shots DIR` instead walks the player too, saves close views from several sides
//! (one with the colliders drawn) and the cloth timings, then exits.
mod panel;

use crate::launch::{Args, LaunchOptions};
use crate::runtime_settings::{RuntimeSettings, RuntimeSettingsApply};
use bevy::prelude::*;
use engine::{
    ClothSettings, ClothStats, ClothWorn, GamePointerInputBlocked, GameplaySystems, MoveIntent,
    OutfitChoice, PlayerControlled, TestOutfit, WorldViewCamera,
};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LabGait {
    Idle,
    Walk,
    Jog,
    /// Two corners walking, two jogging.
    Mixed,
}

/// `--cloth-lab` and its options.
#[derive(Clone, Debug)]
pub(crate) struct ClothLabOptions {
    pub clones: usize,
    pub gait: LabGait,
    pub outfit: OutfitChoice,
    pub shots: Option<PathBuf>,
}

impl ClothLabOptions {
    pub(crate) fn parse(args: &Args) -> Result<Option<Self>, String> {
        if !args.has("--cloth-lab") {
            return Ok(None);
        }
        let outfit = match args.value("--cloth-lab-outfit")? {
            None | Some("all") => OutfitChoice::ALL,
            Some("none") => OutfitChoice::default(),
            Some(list) => {
                let mut choice = OutfitChoice::default();
                for name in list.split(',').map(str::trim) {
                    match name {
                        "cloak" => choice.cloak = true,
                        "skirt" => choice.skirt = true,
                        "ponytail" => choice.ponytail = true,
                        "greatsword" => choice.greatsword = true,
                        "bow" => choice.bow = true,
                        "sword" => choice.side_sword = true,
                        "over" => choice.weapons_over_cloak = true,
                        other => {
                            return Err(format!(
                                "--cloth-lab-outfit: unknown piece {other:?}; use all, none or \
                                 cloak,skirt,ponytail,greatsword,bow,sword,over"
                            ));
                        }
                    }
                }
                choice
            }
        };
        Ok(Some(Self {
            clones: args.number("--cloth-lab-clones", 3, 0..=63)?,
            gait: args.choice(
                "--cloth-lab-gait",
                "walk",
                &[
                    ("idle", LabGait::Idle),
                    ("walk", LabGait::Walk),
                    ("jog", LabGait::Jog),
                    ("mixed", LabGait::Mixed),
                ],
            )?,
            outfit,
            shots: args.path("--cloth-lab-shots"),
        }))
    }
}

/// A camera that keeps its bearing from the player's facing (0 looks at their face, 180 at
/// their back).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LabCamera {
    pub bearing: f32,
    pub distance: f32,
    pub pitch: f32,
}

#[derive(Resource)]
pub(crate) struct ClothLab {
    pub outfit: OutfitChoice,
    pub gait: LabGait,
    pub clones: usize,
    pub player_walks: bool,
    pub camera: Option<LabCamera>,
    spawned: Vec<Entity>,
    /// Smoothed player facing the lab camera follows, radians.
    facing: Option<f32>,
    log_timer: Timer,
}

/// Set by the panel while the pointer is over it, so the game ignores that pointer.
#[derive(Resource, Default)]
struct PanelHasPointer(bool);

/// Walks an octagon: each arrival turns 45° and heads on.
#[derive(Component)]
struct LabWalker {
    heading: Vec2,
    corner: u32,
    gait: Option<LabGait>,
}

impl LabWalker {
    fn new(heading: Vec2) -> Self {
        Self {
            heading,
            corner: 0,
            gait: None,
        }
    }
}

pub(crate) fn install(app: &mut App) -> Result<(), String> {
    let Some(options) = app.world().resource::<LaunchOptions>().cloth_lab.clone() else {
        return Ok(());
    };
    let shots = options.shots.clone();
    app.add_plugins(engine::ClothPlugin)
        .init_resource::<PanelHasPointer>()
        .insert_resource(ClothLab {
            outfit: options.outfit,
            gait: options.gait,
            clones: options.clones,
            player_walks: shots.is_some(),
            camera: shots.is_some().then_some(LabCamera {
                bearing: 180.0,
                distance: 3.4,
                pitch: 12.0,
            }),
            spawned: Vec::new(),
            facing: None,
            log_timer: Timer::from_seconds(5.0, TimerMode::Repeating),
        })
        .add_systems(
            Update,
            (dress, spawn_clones, walk, log_stats)
                .chain()
                .after(GameplaySystems::MoveIntent)
                .before(GameplaySystems::Movement),
        )
        .add_systems(
            Update,
            block_pointer
                .after(RuntimeSettingsApply)
                .before(GameplaySystems::CameraInput),
        )
        .add_systems(Update, drive_camera.after(GameplaySystems::CameraFollow));
    match shots {
        Some(dir) => {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("--cloth-lab-shots {}: {e}", dir.display()))?;
            // Controls stay unlocked: locked controls also stop every motor, and the walk is
            // the point.
            app.world_mut().resource_mut::<RuntimeSettings>().show_ui = false;
            app.insert_resource(Shots::new(dir))
                .add_systems(Update, shoot.after(drive_camera));
        }
        None => panel::install(app),
    }
    Ok(())
}

/// Lab actors wear the lab's outfit; the player is dressed as soon as it exists.
fn dress(
    mut commands: Commands,
    lab: Res<ClothLab>,
    players: Query<Entity, (With<PlayerControlled>, Without<TestOutfit>)>,
    mut worn: Query<&mut TestOutfit>,
) {
    for player in &players {
        commands.entity(player).insert(TestOutfit(lab.outfit));
    }
    if lab.is_changed() {
        for mut outfit in &mut worn {
            if outfit.0 != lab.outfit {
                outfit.0 = lab.outfit;
            }
        }
    }
}

/// Clones stand around the player once the player's outfit is on (its presentation and the
/// ground are ready by then): on a spiral, so any count spreads out.
type DressedPlayer<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static Transform, Has<LabWalker>),
    (With<PlayerControlled>, With<ClothWorn>),
>;

fn spawn_clones(mut commands: Commands, mut lab: ResMut<ClothLab>, player: DressedPlayer) {
    let Ok((player, origin, walking)) = player.single() else {
        return;
    };
    if lab.player_walks != walking {
        if lab.player_walks {
            commands.entity(player).insert(LabWalker::new(Vec2::Y));
        } else {
            commands.entity(player).remove::<LabWalker>();
        }
    }
    while lab.spawned.len() > lab.clones {
        let clone = lab.spawned.pop().expect("more clones than wanted");
        commands.entity(clone).despawn();
    }
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    while lab.spawned.len() < lab.clones {
        let k = lab.spawned.len() as f32 + 1.0;
        let angle = k * golden;
        let offset = Vec2::from_angle(angle) * (2.5 + 1.6 * k.sqrt());
        let position = origin.translation + Vec3::new(offset.x, 0.0, offset.y);
        let clone = commands
            .spawn((
                engine::standing_character(position, "Cloth lab clone"),
                TestOutfit(lab.outfit),
                LabWalker::new(Vec2::from_angle(angle * 1.7)),
            ))
            .id();
        lab.spawned.push(clone);
    }
}

fn walk(lab: Res<ClothLab>, mut walkers: Query<(&Transform, &mut MoveIntent, &mut LabWalker)>) {
    for (transform, mut intent, mut walker) in &mut walkers {
        if walker.gait != Some(lab.gait) {
            walker.gait = Some(lab.gait);
            intent.clear();
        }
        if lab.gait == LabGait::Idle || intent.destination().is_some() {
            continue;
        }
        let jog = match lab.gait {
            LabGait::Jog => true,
            LabGait::Mixed => walker.corner / 2 % 2 == 1,
            _ => false,
        };
        let side = if jog { 4.5 } else { 3.0 };
        walker.heading = Vec2::from_angle(std::f32::consts::FRAC_PI_4).rotate(walker.heading);
        walker.corner += 1;
        let target =
            transform.translation + Vec3::new(walker.heading.x, 0.0, walker.heading.y) * side;
        if jog {
            intent.jog_to(target);
        } else {
            intent.walk_to(target);
        }
    }
}

fn block_pointer(panel: Res<PanelHasPointer>, blocked: Option<ResMut<GamePointerInputBlocked>>) {
    if panel.0
        && let Some(mut blocked) = blocked
    {
        blocked.0 = true;
    }
}

fn drive_camera(
    time: Res<Time>,
    mut lab: ResMut<ClothLab>,
    player: Query<&Transform, (With<PlayerControlled>, Without<WorldViewCamera>)>,
    mut camera: Single<&mut Transform, With<WorldViewCamera>>,
) {
    let (Some(view), Ok(player)) = (lab.camera, player.single()) else {
        lab.facing = None;
        return;
    };
    let forward = player.rotation * Vec3::Z;
    let facing = forward.x.atan2(forward.z);
    let smoothed = match lab.facing {
        None => facing,
        Some(previous) => {
            let turn = (facing - previous + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            previous + turn * (time.delta_secs() * 2.5).min(1.0)
        }
    };
    lab.facing = Some(smoothed);
    let bearing = smoothed + view.bearing.to_radians();
    let pitch = view.pitch.to_radians();
    let focus = player.translation + Vec3::Y * 1.05;
    let direction = Vec3::new(
        bearing.sin() * pitch.cos(),
        pitch.sin(),
        bearing.cos() * pitch.cos(),
    );
    **camera =
        Transform::from_translation(focus + direction * view.distance).looking_at(focus, Vec3::Y);
}

fn log_stats(time: Res<Time<Real>>, mut lab: ResMut<ClothLab>, stats: Res<ClothStats>) {
    if lab.log_timer.tick(time.delta()).just_finished() && stats.characters > 0 {
        info!(
            "CLOTH_LAB characters={} simulated={} particles={} colliders={} pass_us={:.0} \
             mean_pass_us={:.0} work_us={:.0} worst_character_us={:.0}",
            stats.characters,
            stats.simulated,
            stats.particles,
            stats.colliders,
            stats.frame_us,
            stats.mean_frame_us,
            stats.work_us,
            stats.worst_character_us
        );
    }
}

/// One saved view: bearing from the player's facing, distance, and whether colliders show.
const SHOTS: [(&str, f32, f32, bool); 6] = [
    ("back", 180.0, 3.4, false),
    ("back_colliders", 180.0, 3.4, true),
    ("back_left", 135.0, 3.4, false),
    ("side", 90.0, 3.4, false),
    ("front", 25.0, 3.4, false),
    ("group", 160.0, 9.0, false),
];
/// Walking time before the first view, then between views.
const SETTLE_SECONDS: f32 = 8.0;
const SHOT_SECONDS: f32 = 2.5;

#[derive(Resource)]
struct Shots {
    dir: PathBuf,
    started: Option<f32>,
    taken: usize,
    frames: u32,
    pass_us: f32,
    worst_pass_us: f32,
    work_us: f32,
    frame_seconds: f32,
    characters: usize,
    particles: usize,
}

impl Shots {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            started: None,
            taken: 0,
            frames: 0,
            pass_us: 0.0,
            worst_pass_us: 0.0,
            work_us: 0.0,
            frame_seconds: 0.0,
            characters: 0,
            particles: 0,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn shoot(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut shots: ResMut<Shots>,
    mut lab: ResMut<ClothLab>,
    mut settings: ResMut<ClothSettings>,
    stats: Res<ClothStats>,
    worn: Query<(), With<ClothWorn>>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = time.elapsed_secs();
    let Some(started) = shots.started else {
        if lab.spawned.len() == lab.clones && worn.iter().count() == lab.clones + 1 {
            shots.started = Some(now);
            info!("CLOTH_LAB shots: every lab actor wears its outfit; walking");
        }
        return;
    };
    let elapsed = now - started;
    if elapsed > SETTLE_SECONDS * 0.5 {
        shots.frames += 1;
        shots.pass_us += stats.frame_us;
        shots.worst_pass_us = shots.worst_pass_us.max(stats.frame_us);
        shots.work_us += stats.work_us;
        shots.frame_seconds += time.delta_secs();
        shots.characters = stats.characters;
        shots.particles = stats.particles;
    }
    let Some(&(name, bearing, distance, colliders)) = SHOTS.get(shots.taken) else {
        if elapsed > SETTLE_SECONDS + SHOT_SECONDS * SHOTS.len() as f32 + 1.0 {
            let frames = shots.frames.max(1) as f32;
            let report = format!(
                "characters {}\nparticles {}\nframes {}\nmean frame ms {:.2}\n\
                 mean cloth pass us {:.0}\nworst cloth pass us {:.0}\n\
                 mean cloth work us (all cores) {:.0}\nmean work per character us {:.1}\n",
                shots.characters,
                shots.particles,
                shots.frames,
                shots.frame_seconds / frames * 1000.0,
                shots.pass_us / frames,
                shots.worst_pass_us,
                shots.work_us / frames,
                shots.work_us / frames / shots.characters.max(1) as f32,
            );
            info!("CLOTH_LAB shots done:\n{report}");
            if let Err(error) = std::fs::write(shots.dir.join("stats.txt"), report) {
                error!("CLOTH_LAB could not write stats: {error}");
            }
            exit.write(AppExit::Success);
        }
        return;
    };
    let due = SETTLE_SECONDS + SHOT_SECONDS * shots.taken as f32;
    // Frame the view first, take it once the camera and gizmos have settled on it.
    if elapsed > due - SHOT_SECONDS * 0.6 {
        lab.camera = Some(LabCamera {
            bearing,
            distance,
            pitch: 12.0,
        });
        settings.show_colliders = colliders;
    }
    if elapsed > due {
        use bevy::render::view::screenshot::{Screenshot, save_to_disk};
        let path = shots.dir.join(format!("{:02}_{name}.png", shots.taken));
        info!("CLOTH_LAB shot {}", path.display());
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        shots.taken += 1;
    }
}
