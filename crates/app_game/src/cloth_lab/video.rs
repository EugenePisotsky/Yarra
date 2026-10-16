//! `--cloth-lab-video DIR`: films the player in a set of takes (walking, jogging, standing calm
//! and in the wind, from fixed bearings) and exits. Time steps exactly 1/60 s a frame however
//! long a frame takes, so the motion filmed is the motion played and every run is the same, and
//! each frame's screenshot is piped to ffmpeg, one MP4 per take and `all.mp4` of them in order.
//! `stats.txt` holds each take's shake per piece (with no frame-time noise, standing and walking
//! numbers compare across runs) and, for draped garments, how many of their drawn vertices went
//! through something ([`engine::PieceShake::pierced`]), on average and at worst.
//! `CLOTH_LAB_TAKES=walk_back,gale_from_right` films only the takes named;
//! `CLOTH_LAB_ORBIT=72` circles the camera that many degrees a second through each take, to see
//! it from every side; `CLOTH_LAB_CLOCK=6` starts outfits' flutter clocks there
//! ([`ClothSettings::flutter_clock`]), filming another run of the same takes.
use super::{ClothLab, LabCamera, LabGait, drive_camera};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::time::TimeUpdateStrategy;
use engine::{ClothSettings, ClothStats, ClothWorn, PlayerControlled};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FPS: f64 = 60.0;
/// Before the first take, once the player is walking (it waits for the ground first).
const LEAD_SECONDS: f32 = 2.0;
/// How far the player walks before that.
const WALKED: f32 = 1.0;
/// Before each take: the camera, the gait and the wind change, then settle unfilmed.
const SETTLE_SECONDS: f32 = 1.5;
const WIND_MPS: f32 = 10.0;
/// A gale, strong enough to lift the cape off the back and across the weapons strapped over
/// it.
const GALE_MPS: f32 = 18.0;

#[derive(Clone, Copy)]
enum Wind {
    /// The weather's own wind.
    World,
    Calm,
    /// Blowing this way relative to the player's facing (0 blows the way they face, so it
    /// comes from behind), this fast.
    Towards(f32, f32),
}

struct Take {
    name: &'static str,
    bearing: f32,
    distance: f32,
    gait: LabGait,
    wind: Wind,
    seconds: f32,
    /// Take the cloak off and put it on again as the take is set up: garments start at rest,
    /// wherever things they are worn with are.
    redress: bool,
}

const fn take(
    name: &'static str,
    bearing: f32,
    distance: f32,
    gait: LabGait,
    wind: Wind,
    seconds: f32,
) -> Take {
    Take {
        name,
        bearing,
        distance,
        gait,
        wind,
        seconds,
        redress: false,
    }
}

const fn redress(name: &'static str, bearing: f32) -> Take {
    Take {
        redress: true,
        ..take(name, bearing, 2.6, LabGait::Idle, Wind::Calm, 3.0)
    }
}

const TAKES: [Take; 16] = [
    take("walk_back", 180.0, 2.8, LabGait::Walk, Wind::World, 6.0),
    take("walk_side", 90.0, 2.6, LabGait::Walk, Wind::World, 4.0),
    take(
        "walk_back_left",
        135.0,
        2.6,
        LabGait::Walk,
        Wind::World,
        4.0,
    ),
    take("jog_back", 180.0, 3.2, LabGait::Jog, Wind::World, 5.0),
    take("jog_side", 90.0, 3.0, LabGait::Jog, Wind::World, 4.0),
    take(
        "stand_calm_back",
        180.0,
        2.4,
        LabGait::Idle,
        Wind::Calm,
        4.0,
    ),
    take("stand_calm_side", 90.0, 2.4, LabGait::Idle, Wind::Calm, 3.0),
    redress("redress_back", 180.0),
    redress("redress_back_left", 140.0),
    take(
        "wind_from_left",
        180.0,
        2.8,
        LabGait::Idle,
        Wind::Towards(-90.0, WIND_MPS),
        4.0,
    ),
    take(
        "wind_from_front",
        90.0,
        2.8,
        LabGait::Idle,
        Wind::Towards(180.0, WIND_MPS),
        4.0,
    ),
    gale("gale_from_right", 90.0),
    gale("gale_from_left", -90.0),
    gale("gale_from_behind", 0.0),
    gale("gale_from_behind_left", -60.0),
    gale("gale_from_behind_right", 60.0),
];

/// A gale, the cloak put on again first so each run starts it at rest. The cape either catches
/// it and blows out or lies along it and hangs, by the gusts' timing: compare runs with other
/// `CLOTH_LAB_CLOCK`s, and their frames, not only the numbers.
const fn gale(name: &'static str, towards: f32) -> Take {
    Take {
        redress: true,
        ..take(
            name,
            180.0,
            3.0,
            LabGait::Idle,
            Wind::Towards(towards, GALE_MPS),
            5.0,
        )
    }
}

/// The takes to film, by index: those `CLOTH_LAB_TAKES` names, or all.
fn chosen_takes() -> Vec<usize> {
    let names = std::env::var("CLOTH_LAB_TAKES").unwrap_or_default();
    let names: Vec<&str> = names
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .collect();
    for name in &names {
        if !TAKES.iter().any(|t| t.name == *name) {
            warn!("CLOTH_LAB_TAKES: no take {name}");
        }
    }
    (0..TAKES.len())
        .filter(|&k| names.is_empty() || names.contains(&TAKES[k].name))
        .collect()
}

pub(super) fn install(app: &mut App, dir: PathBuf) {
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / FPS,
    )))
    .insert_resource(Video {
        encoder: Arc::new(Mutex::new(Encoder {
            dir: dir.clone(),
            stream: None,
            files: Vec::new(),
        })),
        dir,
        takes: chosen_takes(),
        orbit: std::env::var("CLOTH_LAB_ORBIT")
            .ok()
            .and_then(|o| o.parse().ok())
            .unwrap_or(0.0),
        dressed: None,
        started: None,
        take: None,
        redressing: false,
        frames: 0,
        shake: Vec::new(),
        rows: vec!["take,frame,piece,shake_mm,pierced,folds".into()],
        pass_us: (0.0, 0),
    })
    .add_systems(Update, film.after(drive_camera));
    let mut settings = app.world_mut().resource_mut::<ClothSettings>();
    settings.measure_pierce = true;
    if let Some(clock) = std::env::var("CLOTH_LAB_CLOCK")
        .ok()
        .and_then(|c| c.parse().ok())
    {
        settings.flutter_clock = clock;
    }
}

#[derive(Resource)]
struct Video {
    dir: PathBuf,
    encoder: Arc<Mutex<Encoder>>,
    /// Indices into [`TAKES`] of those filmed, in order.
    takes: Vec<usize>,
    /// Degrees a second the camera circles through each take.
    orbit: f32,
    /// Where the player stood when everyone was dressed, and when (stepped seconds).
    dressed: Option<(Vec3, f32)>,
    /// When the player had walked away from there: the takes' clock starts.
    started: Option<f32>,
    /// The take being set up or filmed.
    take: Option<usize>,
    /// Put the cloak back on next frame (see [`Take::redress`]).
    redressing: bool,
    /// Frames asked for in the current take.
    frames: u32,
    shake: Vec<TakeShake>,
    /// Each filmed frame's measures per piece, for `frames.csv`.
    rows: Vec<String>,
    pass_us: (f32, u32),
}

/// A piece's sums over a take: shake (mm) of the cloth and of its animation, pierced drawn
/// vertices, and frames; and the most vertices pierced in one frame.
struct TakeShake {
    take: &'static str,
    piece: &'static str,
    cloth: f32,
    animation: f32,
    pierced: f32,
    pierced_by: [f32; engine::PIERCED_BY.len()],
    worst: f32,
    folds: f32,
    worst_folds: f32,
    frames: u32,
}

/// The take's ffmpeg, fed frames in order however the screenshots arrive.
struct Encoder {
    dir: PathBuf,
    stream: Option<Stream>,
    files: Vec<PathBuf>,
}

struct Stream {
    take: usize,
    child: Child,
    stdin: Option<ChildStdin>,
    size: (u32, u32),
    next: u32,
    pending: BTreeMap<u32, Vec<u8>>,
}

impl Encoder {
    fn frame(&mut self, take: usize, index: u32, size: (u32, u32), rgba: Vec<u8>) {
        if self.stream.as_ref().is_some_and(|s| s.take != take) {
            self.finish();
        }
        if self.stream.is_none() {
            let path = self
                .dir
                .join(format!("{:02}_{}.mp4", take, TAKES[take].name));
            match start_ffmpeg(&path, size) {
                Ok(mut child) => {
                    self.files.push(path);
                    self.stream = Some(Stream {
                        take,
                        stdin: child.stdin.take(),
                        child,
                        size,
                        next: 0,
                        pending: BTreeMap::new(),
                    });
                }
                Err(error) => {
                    error!("CLOTH_LAB video: could not start ffmpeg: {error}");
                    return;
                }
            }
        }
        let stream = self.stream.as_mut().expect("started above");
        if size != stream.size {
            warn!(
                "CLOTH_LAB video: frame {index} is {size:?}, the take {:?}",
                stream.size
            );
            return;
        }
        stream.pending.insert(index, rgba);
        while let Some(frame) = stream.pending.remove(&stream.next) {
            if let Some(stdin) = &mut stream.stdin
                && let Err(error) = stdin.write_all(&frame)
            {
                error!("CLOTH_LAB video: ffmpeg stopped taking frames: {error}");
                stream.stdin = None;
            }
            stream.next += 1;
        }
    }

    /// Closes the current take's file.
    fn finish(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            if !stream.pending.is_empty() {
                warn!(
                    "CLOTH_LAB video: take {} lost {} frames",
                    TAKES[stream.take].name,
                    stream.pending.len()
                );
            }
            drop(stream.stdin.take());
            if let Err(error) = stream.child.wait() {
                error!("CLOTH_LAB video: ffmpeg: {error}");
            }
        }
    }

    fn received(&self, take: usize) -> u32 {
        self.stream
            .as_ref()
            .filter(|s| s.take == take)
            .map_or(0, |s| s.next + s.pending.len() as u32)
    }
}

fn start_ffmpeg(path: &Path, (width, height): (u32, u32)) -> std::io::Result<Child> {
    Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
        ])
        .args([
            "-s",
            &format!("{width}x{height}"),
            "-r",
            &FPS.to_string(),
            "-i",
            "-",
        ])
        .args(["-vf", "scale=1280:-2", "-c:v", "libx264", "-preset", "fast"])
        .args(["-crf", "20", "-pix_fmt", "yuv420p"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
}

#[allow(clippy::too_many_arguments)]
fn film(
    mut commands: Commands,
    time: Res<Time>,
    mut video: ResMut<Video>,
    mut lab: ResMut<ClothLab>,
    mut settings: ResMut<ClothSettings>,
    stats: Res<ClothStats>,
    worn: Query<(), With<ClothWorn>>,
    player: Query<&Transform, With<PlayerControlled>>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = time.elapsed_secs();
    let Some(started) = video.started else {
        let Ok(player) = player.single() else {
            return;
        };
        match video.dressed {
            None => {
                if lab.spawned.len() == lab.clones && worn.iter().count() == lab.clones + 1 {
                    video.dressed = Some((player.translation, now));
                    info!("CLOTH_LAB video: every lab actor wears its outfit");
                }
            }
            // The player waits for the ground under it before it walks.
            Some((origin, since)) => {
                if player.translation.distance(origin) > WALKED || now - since > 30.0 {
                    video.started = Some(now);
                }
            }
        }
        return;
    };
    // Where on the timeline this frame is: which take, and whether it is being filmed.
    let mut at = now - started - LEAD_SECONDS;
    let mut current = None;
    for &k in &video.takes {
        let take = &TAKES[k];
        if at < 0.0 {
            break;
        }
        if at < SETTLE_SECONDS + take.seconds {
            current = Some((k, at >= SETTLE_SECONDS));
            break;
        }
        at -= SETTLE_SECONDS + take.seconds;
    }
    let Some((k, filming)) = current else {
        if at < 0.0 {
            return;
        }
        // After the last take: wait for its frames, then write everything out.
        let last = video.takes.last().copied().unwrap_or(0);
        let mut encoder = video.encoder.lock().expect("encoder lock");
        if encoder.received(last) < video.frames && at < 5.0 {
            return;
        }
        encoder.finish();
        let files = std::mem::take(&mut encoder.files);
        drop(encoder);
        join_takes(&video.dir, &files);
        write_stats(&video);
        info!("CLOTH_LAB video done: {}", video.dir.display());
        exit.write(AppExit::Success);
        return;
    };
    let take = &TAKES[k];
    if video.redressing {
        video.redressing = false;
        lab.outfit.cloak = true;
    }
    if video.take != Some(k) {
        if take.redress && lab.outfit.cloak {
            lab.outfit.cloak = false;
            video.redressing = true;
        }
        video.take = Some(k);
        video.frames = 0;
        info!("CLOTH_LAB video take {} {}", k, take.name);
        lab.camera = Some(LabCamera {
            bearing: take.bearing,
            distance: take.distance,
            pitch: 12.0,
        });
        lab.gait = take.gait;
    }
    let facing = lab.facing.unwrap_or(0.0).to_degrees();
    let wind = match take.wind {
        Wind::World => None,
        Wind::Calm => Some((0.0, 0.0)),
        Wind::Towards(towards, speed) => Some((facing + towards, speed)),
    };
    if settings.wind_override != wind {
        settings.wind_override = wind;
    }
    if !filming {
        return;
    }
    let frame = video.frames;
    if video.orbit != 0.0
        && let Some(camera) = lab.camera.as_mut()
    {
        camera.bearing = take.bearing + video.orbit * frame as f32 / FPS as f32;
    }
    for piece in &stats.shake_by_piece {
        video.rows.push(format!(
            "{},{frame},{},{:.3},{},{}",
            take.name, piece.name, piece.cloth, piece.pierced, piece.folds
        ));
        match video
            .shake
            .iter_mut()
            .find(|s| s.take == take.name && s.piece == piece.name)
        {
            Some(sum) => {
                sum.cloth += piece.cloth;
                sum.animation += piece.animation;
                sum.pierced += piece.pierced;
                for (total, by) in sum.pierced_by.iter_mut().zip(piece.pierced_by) {
                    *total += by;
                }
                sum.worst = sum.worst.max(piece.pierced);
                sum.folds += piece.folds;
                sum.worst_folds = sum.worst_folds.max(piece.folds);
                sum.frames += 1;
            }
            None => video.shake.push(TakeShake {
                take: take.name,
                piece: piece.name,
                cloth: piece.cloth,
                animation: piece.animation,
                pierced: piece.pierced,
                pierced_by: piece.pierced_by,
                worst: piece.pierced,
                folds: piece.folds,
                worst_folds: piece.folds,
                frames: 1,
            }),
        }
    }
    video.pass_us.0 += stats.frame_us;
    video.pass_us.1 += 1;
    let index = video.frames;
    video.frames += 1;
    let encoder = video.encoder.clone();
    commands.spawn(Screenshot::primary_window()).observe(
        move |captured: On<ScreenshotCaptured>| {
            let image = captured.image.clone();
            let size = (image.width(), image.height());
            match image.try_into_dynamic() {
                Ok(image) => {
                    let rgba = image.to_rgba8().into_raw();
                    encoder
                        .lock()
                        .expect("encoder lock")
                        .frame(k, index, size, rgba);
                }
                Err(error) => error!("CLOTH_LAB video: frame {index}: {error:?}"),
            }
        },
    );
}

/// `all.mp4`: the takes one after another.
fn join_takes(dir: &Path, files: &[PathBuf]) {
    let list = dir.join("takes.txt");
    let text: String = files
        .iter()
        .map(|f| format!("file '{}'\n", f.display()))
        .collect();
    if let Err(error) = std::fs::write(&list, text) {
        error!("CLOTH_LAB video: {error}");
        return;
    }
    let joined = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&list)
        .args(["-c", "copy"])
        .arg(dir.join("all.mp4"))
        .status();
    if !joined.is_ok_and(|s| s.success()) {
        error!("CLOTH_LAB video: could not join the takes");
    }
    let _ = std::fs::remove_file(list);
}

fn write_stats(video: &Video) {
    let mut report = format!(
        "frame step ms {:.2}\nmean cloth pass us {:.0}\n",
        1000.0 / FPS,
        video.pass_us.0 / video.pass_us.1.max(1) as f32
    );
    for s in &video.shake {
        let n = s.frames.max(1) as f32;
        let by: Vec<String> = engine::PIERCED_BY
            .iter()
            .zip(s.pierced_by)
            .filter(|(_, count)| *count / n >= 0.05)
            .map(|(what, count)| format!("{what} {:.1}", count / n))
            .collect();
        report.push_str(&format!(
            "{} shake {} mm {:.3} (its animation {:.3}) pierced {:.1} (worst {:.0}{}{}) \
             folds {:.2} (worst {:.0})\n",
            s.take,
            s.piece,
            s.cloth / n,
            s.animation / n,
            s.pierced / n,
            s.worst,
            if by.is_empty() { "" } else { "; " },
            by.join(", "),
            s.folds / n,
            s.worst_folds
        ));
    }
    if let Err(error) = std::fs::write(video.dir.join("frames.csv"), video.rows.join("\n")) {
        error!("CLOTH_LAB could not write frames: {error}");
    }
    info!("CLOTH_LAB video stats:\n{report}");
    if let Err(error) = std::fs::write(video.dir.join("stats.txt"), report) {
        error!("CLOTH_LAB could not write stats: {error}");
    }
}
