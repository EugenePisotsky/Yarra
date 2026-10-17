//! Cloth lab (`--cloth-lab`): the player and walking clones wear the engine's generated test
//! outfit ([`engine::TestOutfit`]): a cloak, a long skirt, a ponytail, a greatsword and a bow
//! on the back and a sword at the hip, all simulated ([`engine::ClothPlugin`]). The player plays
//! as usual; clones walk octagons around where they were placed. A [`panel`] chooses the pieces,
//! clones, gait and camera, tunes the simulation and shows what it costs.
//!
//! `--cloth-lab-shots DIR` instead walks the player too, saves close views from several sides
//! (one with the colliders drawn) and the cloth timings, then exits; `--cloth-lab-video DIR`
//! films takes instead ([`video`]).
//!
//! The cloak and the skirt are those draped in Blender (`tools/cloth/author_test_garments.py`)
//! when their files are there, else the generated ones; `--cloth-lab-cape` and
//! `--cloth-lab-skirt` choose. The cloak and the skirt wear
//! fabrics made by `tools/cloth/import_fabric.py` when those are there, else plain colours.
mod panel;
mod video;

use crate::launch::{Args, LaunchOptions};
use crate::runtime_settings::{RuntimeSettings, RuntimeSettingsApply};
use bevy::prelude::*;
use engine::{
    ClothSettings, ClothStats, ClothWorn, GamePointerInputBlocked, GameplaySystems, MoveIntent,
    OutfitChoice, PlayerControlled, TestOutfit, WorldViewCamera,
};
use std::path::PathBuf;
use std::sync::Arc;

/// The draped test cape and skirt, under the asset root.
const DRAPED_CAPE: &str = "local/characters/female_main/test_cape.cloth.json";
const DRAPED_SKIRT: &str = "local/characters/female_main/test_skirt.cloth.json";
/// Fabrics for the cloak and the skirt, asset folders.
const CAPE_FABRIC: &str = "local/fabrics/denim";
const SKIRT_FABRIC: &str = "local/fabrics/gray_twill";

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
    pub video: Option<PathBuf>,
    pub cape: LabGarment,
    pub skirt: LabGarment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LabGarment {
    /// The draped one if its file is there, else the generated one.
    Draped,
    Generated,
    File(PathBuf),
}

impl LabGarment {
    fn parse(args: &Args, flag: &str) -> Result<Self, String> {
        Ok(match args.value(flag)? {
            None | Some("draped") => Self::Draped,
            Some("generated") => Self::Generated,
            Some(path) => Self::File(path.into()),
        })
    }

    /// The draped garment chosen, if any: `default` under the asset root unless given a file.
    fn load(&self, flag: &str, default: PathBuf) -> Result<Option<Arc<engine::Garment>>, String> {
        let path = match self {
            Self::Generated => return Ok(None),
            Self::Draped => default,
            Self::File(path) => path.clone(),
        };
        match engine::Garment::load(&path) {
            Ok(garment) => {
                info!("CLOTH_LAB draped {}", path.display());
                Ok(Some(Arc::new(garment)))
            }
            Err(error) if *self == Self::Draped => {
                warn!("CLOTH_LAB the generated one instead of a draped one: {error}");
                Ok(None)
            }
            Err(error) => Err(format!("{flag} {error}")),
        }
    }
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
                        other => {
                            return Err(format!(
                                "--cloth-lab-outfit: unknown piece {other:?}; use all, none or \
                                 cloak,skirt,ponytail,greatsword,bow,sword"
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
            video: args.path("--cloth-lab-video"),
            cape: LabGarment::parse(args, "--cloth-lab-cape")?,
            skirt: LabGarment::parse(args, "--cloth-lab-skirt")?,
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
    /// Looks the other way from where it stands, the player out of view.
    pub away: bool,
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

/// Walks an octagon, turning 45° at each corner without stopping.
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
    let video = options.video.clone();
    let unattended = shots.is_some() || video.is_some();
    app.add_plugins(engine::ClothPlugin)
        .init_resource::<PanelHasPointer>()
        .insert_resource(ClothLab {
            outfit: options.outfit,
            gait: options.gait,
            clones: options.clones,
            player_walks: unattended,
            camera: unattended.then_some(LabCamera {
                bearing: 180.0,
                distance: 3.4,
                pitch: 12.0,
                away: false,
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
    let root = crate::resolve_asset_root();
    let mut garments = engine::ClothGarments {
        cloak: options
            .cape
            .load("--cloth-lab-cape", root.join(DRAPED_CAPE))?,
        skirt: options
            .skirt
            .load("--cloth-lab-skirt", root.join(DRAPED_SKIRT))?,
        ..default()
    };
    for (fabric, folder) in [
        (&mut garments.cloak_fabric, CAPE_FABRIC),
        (&mut garments.skirt_fabric, SKIRT_FABRIC),
    ] {
        match engine::Fabric::load(&root, folder) {
            Ok(loaded) => *fabric = Some(loaded),
            Err(error) => info!("CLOTH_LAB a plain colour instead of a fabric: {error}"),
        }
    }
    app.insert_resource(garments);
    if !unattended {
        panel::install(app);
        return Ok(());
    }
    // Controls stay unlocked: locked controls also stop every motor, and the walk is the point.
    app.world_mut().resource_mut::<RuntimeSettings>().show_ui = false;
    // Unattended tuning runs, e.g. CLOTH_LAB_TUNE="damping=2,lift=0.5,flutter=0,
    // inertia=0.6,substeps=240,wind=0,contact=0.6,self=1,draw=1,full_within=20,distant_hz=120,
    // view=20" (`draw` shows colliders and particles; `view` is the out-of-view margin in
    // degrees, negative to simulate characters wherever they are).
    if let Ok(tune) = std::env::var("CLOTH_LAB_TUNE") {
        let mut settings = engine::ClothSettings::default();
        for pair in tune.split(',') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            let value: f32 = value
                .parse()
                .map_err(|_| format!("CLOTH_LAB_TUNE {pair}"))?;
            match key {
                "damping" => settings.damping = value,
                "lift" => settings.lift = value,
                "flutter" => settings.flutter = value,
                "inertia" => settings.inertia = Some(value),
                "substeps" => settings.substep_hz = value,
                "wind" => settings.wind_override = Some((0.0, value)),
                "contact" => settings.contact = value,
                "self" => settings.self_collide = value > 0.5,
                "full_within" => settings.full_rate_within = value,
                "distant_hz" => settings.distant_substep_hz = value,
                "view" => settings.out_of_view = (value >= 0.0).then_some(value),
                "draw" => {
                    settings.show_colliders = value > 0.5;
                    settings.show_particles = value > 0.5;
                }
                _ => return Err(format!("CLOTH_LAB_TUNE: unknown {key}")),
            }
        }
        app.insert_resource(settings);
    }
    for (flag, dir) in [("--cloth-lab-shots", &shots), ("--cloth-lab-video", &video)] {
        if let Some(dir) = dir {
            std::fs::create_dir_all(dir).map_err(|e| format!("{flag} {}: {e}", dir.display()))?;
        }
    }
    if let Some(dir) = video {
        video::install(app, dir);
    } else if let Some(dir) = shots {
        app.insert_resource(Shots::new(dir))
            .add_systems(Update, shoot.after(drive_camera));
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
        if lab.gait == LabGait::Idle {
            continue;
        }
        let jog = match lab.gait {
            LabGait::Jog => true,
            LabGait::Mixed => walker.corner / 2 % 2 == 1,
            _ => false,
        };
        // The next corner before reaching this one, so walkers turn without stopping.
        let ahead = if jog { 2.5 } else { 1.6 };
        if let Some(corner) = intent.destination() {
            let left = Vec2::new(
                corner.x - transform.translation.x,
                corner.z - transform.translation.z,
            );
            if left.length() > ahead {
                continue;
            }
        }
        let side = if jog { 4.5 } else { 3.0 };
        walker.heading = Vec2::from_angle(std::f32::consts::FRAC_PI_4).rotate(walker.heading);
        walker.corner += 1;
        let from = intent.destination().unwrap_or(transform.translation);
        let target = from + Vec3::new(walker.heading.x, 0.0, walker.heading.y) * side;
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
    let at = focus + direction * view.distance;
    let look = if view.away { 2.0 * at - focus } else { focus };
    **camera = Transform::from_translation(at).looking_at(look, Vec3::Y);
}

fn log_stats(time: Res<Time<Real>>, mut lab: ResMut<ClothLab>, stats: Res<ClothStats>) {
    if lab.log_timer.tick(time.delta()).just_finished() && stats.characters > 0 {
        info!(
            "CLOTH_LAB characters={} simulated={} distant={} out_of_view={} \
             particles={} colliders={} pass_us={:.0} mean_pass_us={:.0} work_us={:.0} \
             worst_character_us={:.0} wind_mps={:.1}",
            stats.characters,
            stats.simulated,
            stats.distant,
            stats.out_of_view,
            stats.particles,
            stats.colliders,
            stats.frame_us,
            stats.mean_frame_us,
            stats.work_us,
            stats.worst_character_us,
            stats.wind.length()
        );
    }
}

/// One saved view.
struct Shot {
    name: &'static str,
    /// The camera's bearing from the player's facing (0 looks at their face).
    bearing: f32,
    distance: f32,
    colliders: bool,
    /// Everyone stands, and the wind blows this way relative to the player's facing at
    /// `WIND_MPS` (0 blows the way they face, so it comes from behind).
    wind: Option<f32>,
}

const fn shot(name: &'static str, bearing: f32, distance: f32) -> Shot {
    Shot {
        name,
        bearing,
        distance,
        colliders: false,
        wind: None,
    }
}

const SHOTS: [Shot; 8] = [
    shot("back", 180.0, 3.4),
    Shot {
        colliders: true,
        ..shot("back_colliders", 180.0, 3.4)
    },
    shot("back_left", 135.0, 3.4),
    shot("side", 90.0, 3.4),
    shot("front", 25.0, 3.4),
    shot("group", 160.0, 9.0),
    Shot {
        wind: Some(180.0),
        ..shot("wind_from_front", 90.0, 3.6)
    },
    Shot {
        wind: Some(-90.0),
        ..shot("wind_from_left", 180.0, 3.6)
    },
];
const WIND_MPS: f32 = 10.0;
/// Walking time before the first view, then between views.
const SETTLE_SECONDS: f32 = 14.0;
/// Walking jitter is measured over this window, before any view is framed or captured.
const MEASURE_SECONDS: (f32, f32) = (5.0, 11.5);
const SHOT_SECONDS: f32 = 3.0;

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
    /// Summed shake (mm) and frames, walking and standing in the wind.
    shake_walking: (f32, u32),
    shake_standing: (f32, u32),
    /// Summed shake (mm) per kind of piece, and frames, walking.
    shake_by_piece: Vec<(engine::PieceShake, u32)>,
    /// Steady frames in a row.
    steady: u32,
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
            shake_walking: (0.0, 0),
            shake_standing: (0.0, 0),
            shake_by_piece: Vec::new(),
            steady: 0,
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
    if elapsed > MEASURE_SECONDS.0 {
        shots.frames += 1;
        shots.pass_us += stats.frame_us;
        shots.worst_pass_us = shots.worst_pass_us.max(stats.frame_us);
        shots.work_us += stats.work_us;
        shots.frame_seconds += time.delta_secs();
        shots.characters = stats.characters;
        shots.particles = stats.particles;
        // Jitter only over steady frames: a hitch (a screenshot, streaming) shows as jitter in
        // everything for the next three.
        let ms = time.delta_secs() * 1000.0;
        shots.steady = if (13.0..20.0).contains(&ms) {
            shots.steady + 1
        } else {
            0
        };
        let walking = (MEASURE_SECONDS.0..MEASURE_SECONDS.1).contains(&elapsed);
        let standing = lab.gait == LabGait::Idle;
        if shots.steady >= 4 && (walking || standing) {
            let phase = if standing {
                &mut shots.shake_standing
            } else {
                &mut shots.shake_walking
            };
            phase.0 += stats.shake_mm;
            phase.1 += 1;
        }
        if shots.steady >= 4 && walking {
            for piece in &stats.shake_by_piece {
                match shots
                    .shake_by_piece
                    .iter_mut()
                    .find(|(p, _)| p.name == piece.name)
                {
                    Some((sum, n)) => {
                        sum.cloth += piece.cloth;
                        sum.animation += piece.animation;
                        *n += 1;
                    }
                    None => shots.shake_by_piece.push((*piece, 1)),
                }
            }
        }
    }
    let Some(view) = SHOTS.get(shots.taken) else {
        if elapsed > SETTLE_SECONDS + SHOT_SECONDS * SHOTS.len() as f32 + 1.0 {
            let frames = shots.frames.max(1) as f32;
            let report = format!(
                "characters {}\nparticles {}\nframes {}\nmean frame ms {:.2}\n\
                 mean cloth pass us {:.0}\nworst cloth pass us {:.0}\n\
                 mean cloth work us (all cores) {:.0}\nmean work per character us {:.1}\n\
                 shake walking mm {:.3}\nshake standing in wind mm {:.3}\n",
                shots.characters,
                shots.particles,
                shots.frames,
                shots.frame_seconds / frames * 1000.0,
                shots.pass_us / frames,
                shots.worst_pass_us,
                shots.work_us / frames,
                shots.work_us / frames / shots.characters.max(1) as f32,
                shots.shake_walking.0 / shots.shake_walking.1.max(1) as f32,
                shots.shake_standing.0 / shots.shake_standing.1.max(1) as f32,
            );
            let mut report = report;
            for (piece, n) in &shots.shake_by_piece {
                let n = (*n).max(1) as f32;
                report.push_str(&format!(
                    "shake {} mm {:.3} (its animation {:.3})\n",
                    piece.name,
                    piece.cloth / n,
                    piece.animation / n
                ));
            }
            info!("CLOTH_LAB shots done:\n{report}");
            if let Err(error) = std::fs::write(shots.dir.join("stats.txt"), report) {
                error!("CLOTH_LAB could not write stats: {error}");
            }
            exit.write(AppExit::Success);
        }
        return;
    };
    let due = SETTLE_SECONDS + SHOT_SECONDS * shots.taken as f32;
    // Frame the view first, take it once the camera, gizmos and wind have settled on it.
    if elapsed > due - SHOT_SECONDS * 0.8 {
        lab.camera = Some(LabCamera {
            bearing: view.bearing,
            distance: view.distance,
            pitch: 12.0,
            away: false,
        });
        settings.show_colliders = view.colliders;
        let facing = lab.facing.unwrap_or(0.0).to_degrees();
        settings.wind_override = view.wind.map(|towards| (facing + towards, WIND_MPS));
        if view.wind.is_some() && lab.gait != LabGait::Idle {
            lab.gait = LabGait::Idle;
        }
    }
    if elapsed > due {
        use bevy::render::view::screenshot::{Screenshot, save_to_disk};
        let path = shots
            .dir
            .join(format!("{:02}_{}.png", shots.taken, view.name));
        info!("CLOTH_LAB shot {}", path.display());
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        shots.taken += 1;
    }
}
