//! `--cloth-lab-video DIR`: films the player in a set of takes (walking, jogging, standing calm
//! and in the wind, from fixed bearings) and exits. Time steps exactly 1/60 s a frame however
//! long a frame takes, so the motion filmed is the motion played and every run is the same, and
//! each frame's screenshot is piped to ffmpeg, one MP4 per take and `all.mp4` of them in order.
//! `stats.txt` holds each take's shake per piece: with no frame-time noise, standing and walking
//! numbers compare across runs.
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

#[derive(Clone, Copy)]
enum Wind {
    /// The weather's own wind.
    World,
    Calm,
    /// Blowing this way relative to the player's facing at `WIND_MPS` (0 blows the way they
    /// face, so it comes from behind).
    Towards(f32),
}

struct Take {
    name: &'static str,
    bearing: f32,
    distance: f32,
    gait: LabGait,
    wind: Wind,
    seconds: f32,
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
    }
}

const TAKES: [Take; 8] = [
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
    take(
        "stand_calm_back",
        180.0,
        2.4,
        LabGait::Idle,
        Wind::Calm,
        4.0,
    ),
    take("stand_calm_side", 90.0, 2.4, LabGait::Idle, Wind::Calm, 3.0),
    take(
        "wind_from_left",
        180.0,
        2.8,
        LabGait::Idle,
        Wind::Towards(-90.0),
        4.0,
    ),
    take(
        "wind_from_front",
        90.0,
        2.8,
        LabGait::Idle,
        Wind::Towards(180.0),
        4.0,
    ),
];

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
        dressed: None,
        started: None,
        take: None,
        frames: 0,
        shake: Vec::new(),
        pass_us: (0.0, 0),
    })
    .add_systems(Update, film.after(drive_camera));
}

#[derive(Resource)]
struct Video {
    dir: PathBuf,
    encoder: Arc<Mutex<Encoder>>,
    /// Where the player stood when everyone was dressed, and when (stepped seconds).
    dressed: Option<(Vec3, f32)>,
    /// When the player had walked away from there: the takes' clock starts.
    started: Option<f32>,
    /// The take being set up or filmed.
    take: Option<usize>,
    /// Frames asked for in the current take.
    frames: u32,
    /// Per take and piece: summed shake (mm) of the cloth and of its animation, and frames.
    shake: Vec<(&'static str, &'static str, f32, f32, u32)>,
    pass_us: (f32, u32),
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
    for (k, take) in TAKES.iter().enumerate() {
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
        let last = TAKES.len() - 1;
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
    if video.take != Some(k) {
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
        Wind::Towards(towards) => Some((facing + towards, WIND_MPS)),
    };
    if settings.wind_override != wind {
        settings.wind_override = wind;
    }
    if !filming {
        return;
    }
    for piece in &stats.shake_by_piece {
        match video
            .shake
            .iter_mut()
            .find(|s| s.0 == take.name && s.1 == piece.name)
        {
            Some(sum) => {
                sum.2 += piece.cloth;
                sum.3 += piece.animation;
                sum.4 += 1;
            }
            None => video
                .shake
                .push((take.name, piece.name, piece.cloth, piece.animation, 1)),
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
    for (take, piece, cloth, animation, frames) in &video.shake {
        let n = (*frames).max(1) as f32;
        report.push_str(&format!(
            "{take} shake {piece} mm {:.3} (its animation {:.3})\n",
            cloth / n,
            animation / n
        ));
    }
    info!("CLOTH_LAB video stats:\n{report}");
    if let Err(error) = std::fs::write(video.dir.join("stats.txt"), report) {
        error!("CLOTH_LAB could not write stats: {error}");
    }
}
