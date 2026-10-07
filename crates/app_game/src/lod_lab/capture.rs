//! Automated LOD lab captures (`--lod-lab-capture DIR`). Once every tree has drawn for a few
//! seconds, from each bearing: at 0.8–1.2× each switch between representations, the one on
//! each side forced and the game's own choice; at each fixed distance, the game's choice;
//! and with each, a frame with the tree hidden, which isolates the tree and its shadow. From
//! the first bearing, a strip through each switch in 1.25% steps of the game's choice, and
//! the dissolve itself: the camera steps across each switch and back, and frames are taken
//! as fast as they come until the fade ends. Then the wind where the impostor takes over and
//! farther out: the last mesh LOD and the impostor in turn, a few frames apart, for a few
//! seconds. Every
//! other frame waits until the tree is not fading. Frames are cropped to the tree and its
//! shadow and listed in `manifest.json` for `tools/lod_lab_sheet.py`.
use super::{LabView, LodLab, label, switches};
use crate::game_render::RESOLUTION_SCALES;
use crate::runtime_settings::RuntimeSettings;
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    window::{MonitorSelection, PrimaryWindow, WindowMode},
};
use engine::{LabRepresentation, LabTree, WorldSun, WorldViewCamera};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// Frames every tree draws before the first capture: shadows, the forest shadow map and
/// terrain around the camera settle.
const WARMUP_FRAMES: u32 = 240;
const TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Wind samples of each of the last mesh LOD and the impostor, and frames between samples,
/// at these shares of the impostor's switch distance.
const SWAY_SAMPLES: usize = 30;
const SWAY_FACTORS: [f32; 2] = [1.06, 1.75];
const SWAY_FRAMES: u32 = 8;

#[derive(Clone, Debug)]
struct Pose {
    view: LabView,
    representation: LabRepresentation,
    /// `switch`, `fixed`, `strip`, `fade` or `sway`.
    kind: &'static str,
    /// `before`, `after`, `auto`, `hidden`, for fades `out` and `back`, for sway `mesh` and
    /// `impostor`.
    role: &'static str,
    switch: Option<usize>,
    factor: Option<f32>,
    settle: u32,
    /// Captured repeatedly from the moment the camera arrives until the fade ends.
    sequence: bool,
}

#[derive(Resource)]
struct Capture {
    dir: PathBuf,
    poses: Vec<Pose>,
    index: usize,
    waited: u32,
    warm: u32,
    awaiting: Arc<AtomicBool>,
    saving: Arc<AtomicUsize>,
    failed: Arc<AtomicBool>,
    header: Value,
    frames: Vec<Value>,
    started: Instant,
    /// For a sequence pose: when the camera arrived, and frames taken since the fade ended.
    sequence: Option<(f64, u32)>,
}

pub(super) fn install(app: &mut App, dir: PathBuf) {
    app.insert_resource(Capture {
        dir,
        poses: Vec::new(),
        index: 0,
        waited: 0,
        warm: 0,
        awaiting: Arc::default(),
        saving: Arc::default(),
        failed: Arc::default(),
        header: Value::Null,
        frames: Vec::new(),
        started: Instant::now(),
        sequence: None,
    })
    .add_systems(Startup, fullscreen)
    .add_systems(Update, run.after(super::LabCameraSet));
}

/// Captures show the game as played: fullscreen at the window's native resolution.
fn fullscreen(mut window: Single<&mut Window, With<PrimaryWindow>>) {
    window.mode = WindowMode::BorderlessFullscreen(MonitorSelection::Primary);
}

fn plan(tree: &LabTree, options: &crate::launch::LodLabOptions) -> Vec<Pose> {
    let switches = switches(tree);
    let pitch = options.pitch.to_radians();
    let settle = options.settle;
    let again = (settle / 2).max(8);
    let mut poses = Vec::new();
    let mut previous_bearing = None;
    for &bearing in &options.yaws {
        type Row = (f32, &'static str, Option<usize>, Option<f32>);
        let mut rows: Vec<(Row, Vec<(&'static str, LabRepresentation)>)> = Vec::new();
        for (j, &switch) in switches.iter().enumerate() {
            for factor in [0.8, 0.9, 1.0, 1.1, 1.2] {
                rows.push((
                    (switch * factor, "switch", Some(j), Some(factor)),
                    vec![
                        ("hidden", LabRepresentation::Hidden),
                        ("before", tree.bands[j].representation),
                        ("after", tree.bands[j + 1].representation),
                        ("auto", LabRepresentation::Auto),
                    ],
                ));
            }
        }
        for &distance in &options.distances {
            rows.push((
                (distance, "fixed", None, None),
                vec![
                    ("hidden", LabRepresentation::Hidden),
                    ("auto", LabRepresentation::Auto),
                ],
            ));
        }
        rows.sort_by(|a, b| a.0.0.total_cmp(&b.0.0));
        for ((distance, kind, switch, factor), roles) in rows {
            for (k, (role, representation)) in roles.into_iter().enumerate() {
                let turned = previous_bearing != Some(bearing);
                previous_bearing = Some(bearing);
                poses.push(Pose {
                    view: LabView {
                        bearing: bearing.to_radians(),
                        pitch,
                        distance,
                    },
                    representation,
                    kind,
                    role,
                    switch,
                    factor,
                    settle: if turned {
                        settle * 2
                    } else if k == 0 {
                        settle
                    } else {
                        again
                    },
                    sequence: false,
                });
            }
        }
    }
    for (j, &switch) in switches.iter().enumerate() {
        for k in 0..=24 {
            let factor = 0.85 + 0.3 * k as f32 / 24.0;
            poses.push(Pose {
                view: LabView {
                    bearing: options.yaws[0].to_radians(),
                    pitch,
                    distance: switch * factor,
                },
                representation: LabRepresentation::Auto,
                kind: "strip",
                role: "auto",
                switch: Some(j),
                factor: Some(factor),
                settle: if k == 0 { settle * 2 } else { again },
                sequence: false,
            });
        }
    }
    // The dissolve over time: steady on one side, then frames while it fades to the other,
    // then back; the ground without the tree at both, to measure the shadow frame by frame.
    for (j, &switch) in switches.iter().enumerate() {
        for (factor, role, representation, sequence) in [
            (1.06, "hidden", LabRepresentation::Hidden, false),
            (0.94, "hidden", LabRepresentation::Hidden, false),
            (0.94, "before", LabRepresentation::Auto, false),
            (1.06, "out", LabRepresentation::Auto, true),
            (0.94, "back", LabRepresentation::Auto, true),
        ] {
            poses.push(Pose {
                view: LabView {
                    bearing: options.yaws[0].to_radians(),
                    pitch,
                    distance: switch * factor,
                },
                representation,
                kind: "fade",
                role,
                switch: Some(j),
                factor: Some(factor),
                settle: if sequence { 0 } else { settle * 2 },
                sequence,
            });
        }
    }
    // Wind just past the impostor switch and farther out: the last mesh LOD and the impostor
    // in turn, so the sheet compares how far their tops sway at nearly the same moment.
    let last = tree.bands.len().saturating_sub(1);
    if last > 0 && tree.bands[last].representation == LabRepresentation::Impostor {
        let j = last - 1;
        for factor in SWAY_FACTORS {
            let pose = |representation, role, settle| Pose {
                view: LabView {
                    bearing: options.yaws[0].to_radians(),
                    pitch,
                    distance: switches[j] * factor,
                },
                representation,
                kind: "sway",
                role,
                switch: Some(j),
                factor: Some(factor),
                settle,
                sequence: false,
            };
            poses.push(pose(LabRepresentation::Hidden, "hidden", settle * 2));
            for _ in 0..SWAY_SAMPLES {
                poses.push(pose(tree.bands[j].representation, "mesh", SWAY_FRAMES));
                poses.push(pose(LabRepresentation::Impostor, "impostor", 3));
            }
        }
    }
    poses
}

/// The window pixels holding the tree (its bounds) and its shadow on level ground, with a
/// margin, and the tree's own box within them.
fn crop(
    camera: &Camera,
    camera_transform: &GlobalTransform,
    window: &Window,
    root: Vec3,
    bounds: [f32; 3],
    scale: f32,
    to_sun: Vec3,
) -> Option<([u32; 4], [f32; 4])> {
    let half = Vec3::new(bounds[0], 0.0, bounds[2]) * 0.5 * scale;
    let height = bounds[1] * scale;
    let corners: Vec<Vec3> = [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
        .into_iter()
        .flat_map(|(x, z)| {
            let c = root + Vec3::new(half.x * x, 0.0, half.z * z);
            [c, c + Vec3::Y * height]
        })
        .collect();
    let shadow: Vec<Vec3> = if to_sun.y > 0.05 {
        corners
            .iter()
            .filter(|c| c.y > root.y + 0.01)
            .map(|&c| c - to_sun * ((c.y - root.y) / to_sun.y).min(2.0 * height))
            .collect()
    } else {
        Vec::new()
    };
    let factor = window.scale_factor();
    let project = |p: Vec3| {
        camera
            .world_to_viewport(camera_transform, p)
            .ok()
            .map(|v| v * factor)
    };
    let tree: Vec<Vec2> = corners.iter().filter_map(|&p| project(p)).collect();
    if tree.len() < corners.len() {
        return None;
    }
    let all: Vec<Vec2> = tree
        .iter()
        .copied()
        .chain(shadow.iter().filter_map(|&p| project(p)))
        .collect();
    let extent = |points: &[Vec2]| {
        points.iter().fold(
            (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
            |(lo, hi), p| (lo.min(*p), hi.max(*p)),
        )
    };
    let (lo, hi) = extent(&all);
    let margin = ((hi - lo).max_element() * 0.1).max(24.0);
    let size = Vec2::new(
        window.physical_width() as f32,
        window.physical_height() as f32,
    );
    let lo = (lo - margin).max(Vec2::ZERO).floor();
    let hi = (hi + margin).min(size).ceil();
    if hi.x - lo.x < 2.0 || hi.y - lo.y < 2.0 {
        return None;
    }
    let (tree_lo, tree_hi) = extent(&tree);
    Some((
        [
            lo.x as u32,
            lo.y as u32,
            (hi.x - lo.x) as u32,
            (hi.y - lo.y) as u32,
        ],
        [
            tree_lo.x - lo.x,
            tree_lo.y - lo.y,
            tree_hi.x - tree_lo.x,
            tree_hi.y - tree_lo.y,
        ],
    ))
}

fn band_json(tree: &LabTree) -> Value {
    let range = |r: &std::ops::Range<f32>| {
        if r.end >= f32::MAX {
            Value::Null
        } else {
            json!([r.start, r.end])
        }
    };
    tree.bands
        .iter()
        .map(|b| {
            json!({
                "representation": label(b.representation),
                "fade_in": range(&b.fade_in),
                "fade_out": range(&b.fade_out),
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn run(
    mut commands: Commands,
    mut capture: ResMut<Capture>,
    mut lab: ResMut<LodLab>,
    mut trees: Query<(&LabTree, &mut LabRepresentation, &GlobalTransform)>,
    camera: Single<(&Camera, &GlobalTransform), With<WorldViewCamera>>,
    window: Single<&Window, With<PrimaryWindow>>,
    sun: Single<&GlobalTransform, With<WorldSun>>,
    terrain: Res<engine::TerrainLodStats>,
    settings: Res<RuntimeSettings>,
    time: Res<Time<Real>>,
    mut exit: MessageWriter<AppExit>,
) {
    if capture.started.elapsed() > TIMEOUT || capture.failed.load(Ordering::Acquire) {
        error!("LOD_LAB capture failed or timed out");
        exit.write(AppExit::error());
        return;
    }
    let Some((entity, root)) = lab.tree else {
        return;
    };
    if !trees.iter().all(|(tree, ..)| tree.ready) {
        return;
    }
    let Ok((tree, mut representation, transform)) = trees.get_mut(entity) else {
        return;
    };
    let scale = transform.to_scale_rotation_translation().0.y;
    if capture.poses.is_empty() {
        capture.warm += 1;
        if capture.warm < WARMUP_FRAMES {
            return;
        }
        if tree.bands.is_empty() {
            error!("LOD_LAB: no LOD bands; the LOD projection is unknown");
            exit.write(AppExit::error());
            return;
        }
        capture.poses = plan(tree, &lab.options);
        if let Err(e) = std::fs::create_dir_all(&capture.dir) {
            error!("LOD_LAB cannot create {}: {e}", capture.dir.display());
            exit.write(AppExit::error());
            return;
        }
        let to_sun = sun.back();
        capture.header = json!({
            "asset": tree.asset.key,
            "scale": scale,
            "height": tree.asset.bounds[1] * scale,
            "bounds": tree.asset.bounds,
            "bands": band_json(tree),
            "switches": switches(tree),
            "pixels_per_metre": tree.pixels_per_metre,
            "impostor_view": tree.impostor_view.map(|(cell, radius)| json!({"cell": cell, "radius": radius})),
            "window": {
                "logical": [window.width(), window.height()],
                "physical": [window.physical_width(), window.physical_height()],
                "scale_factor": window.scale_factor(),
            },
            "render": {
                "resolution_scale": RESOLUTION_SCALES[settings.scale_index],
                "msaa": format!("{:?}", settings.msaa),
                "upscaler": format!("{:?}", settings.upscaler),
            },
            "sun": {
                "direction_to_sun": [to_sun.x, to_sun.y, to_sun.z],
                "elevation_degrees": to_sun.y.asin().to_degrees(),
            },
            "stand": lab.options.stand.as_ref().map(|(asset, count, spacing)| json!({"asset": asset, "count": count, "spacing": spacing})),
            "yaws": lab.options.yaws,
            "pitch": lab.options.pitch,
            "settle": lab.options.settle,
        });
        info!(
            "LOD_LAB capturing {} poses of {} into {}",
            capture.poses.len(),
            tree.asset.key,
            capture.dir.display()
        );
        return;
    }
    if capture.awaiting.load(Ordering::Acquire) {
        return;
    }
    if capture.index == capture.poses.len() {
        if capture.saving.load(Ordering::Acquire) == 0 {
            let manifest = json!({"header": capture.header, "frames": capture.frames});
            let path = capture.dir.join("manifest.json");
            match std::fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()) {
                Ok(()) => {
                    info!(
                        "LOD_LAB wrote {} frames and {}",
                        capture.frames.len(),
                        path.display()
                    );
                    exit.write(AppExit::Success);
                }
                Err(e) => {
                    error!("LOD_LAB cannot write {}: {e}", path.display());
                    exit.write(AppExit::error());
                }
            }
        }
        return;
    }
    let pose = capture.poses[capture.index].clone();
    let now = time.elapsed_secs_f64();
    if capture.waited == 0 {
        lab.view = pose.view;
        lab.wind = pose.kind == "sway";
        representation.set_if_neq(pose.representation);
        capture.waited = 1;
        capture.sequence = pose.sequence.then_some((now, 0));
        return;
    }
    let (camera, camera_transform) = *camera;
    let placed = (camera_transform.translation().distance(root) - pose.view.distance).abs()
        <= 0.002 * pose.view.distance + 0.01;
    if !placed
        || (!pose.sequence
            && (capture.waited < pose.settle
                || (tree.fading && capture.waited < pose.settle + 600)
                || (terrain.quality_pending && capture.waited < pose.settle + 300)))
    {
        capture.waited += 1;
        return;
    }
    let index = capture.index;
    let elapsed = capture.sequence.map(|(start, _)| now - start);
    match capture.sequence.as_mut() {
        // Keep taking frames of this pose until the fade has ended for two of them.
        Some((_, after)) if *after < 2 => {
            if !tree.fading {
                *after += 1;
            }
            capture.waited += 1;
        }
        _ => {
            capture.index += 1;
            capture.waited = 0;
            capture.sequence = None;
        }
    }
    let d = pose.view.distance;
    let height = tree.asset.bounds[1] * scale;
    let logical = tree.pixels_per_metre * height / d;
    let drawn: Vec<String> = tree
        .bands
        .iter()
        .filter(|b| d >= b.fade_in.start && d <= b.fade_out.end)
        .map(|b| label(b.representation))
        .collect();
    let texels = tree.impostor_view.map(|(cell, radius)| {
        cell as f32 * d / (2.0 * radius * scale * tree.pixels_per_metre * window.scale_factor())
    });
    let Some((rect, tree_box)) = crop(
        camera,
        camera_transform,
        &window,
        root,
        tree.asset.bounds,
        scale,
        sun.back().into(),
    ) else {
        warn!("LOD_LAB pose {index} at {d:.1} m has the tree off screen; skipped");
        return;
    };
    let file = format!(
        "{index:04}-{}-b{:03.0}-{d:07.1}m-{}{}.png",
        pose.kind,
        pose.view.bearing.to_degrees().rem_euclid(360.0),
        pose.role,
        elapsed.map_or(String::new(), |t| format!("-{:04.0}ms", t * 1000.0))
    );
    capture.frames.push(json!({
        "file": file,
        "kind": pose.kind,
        "role": pose.role,
        "representation": label(pose.representation),
        "bearing": pose.view.bearing.to_degrees(),
        "pitch": pose.view.pitch.to_degrees(),
        "distance": d,
        "switch": pose.switch,
        "factor": pose.factor,
        "drawn": drawn,
        "crop": rect,
        "tree_box": tree_box,
        "height_px": logical,
        "height_px_physical": logical * window.scale_factor(),
        "impostor_texels_per_pixel": texels,
        "fading": tree.fading,
        "t": elapsed,
        "clock": now,
    }));
    let path = capture.dir.join(file);
    let (awaiting, saving, failed) = (
        capture.awaiting.clone(),
        capture.saving.clone(),
        capture.failed.clone(),
    );
    awaiting.store(true, Ordering::Release);
    commands.spawn(Screenshot::primary_window()).observe(
        move |captured: On<ScreenshotCaptured>| {
            let image = captured.image.clone();
            saving.fetch_add(1, Ordering::AcqRel);
            awaiting.store(false, Ordering::Release);
            let (saving, failed, path) = (saving.clone(), failed.clone(), path.clone());
            std::thread::spawn(move || {
                let result = image
                    .try_into_dynamic()
                    .map_err(|e| e.to_string())
                    .and_then(|image| {
                        let [x, y, w, h] = rect;
                        let x = x.min(image.width().saturating_sub(1));
                        let y = y.min(image.height().saturating_sub(1));
                        let w = w.min(image.width() - x);
                        let h = h.min(image.height() - y);
                        image
                            .crop_imm(x, y, w, h)
                            .to_rgb8()
                            .save(&path)
                            .map_err(|e| e.to_string())
                    });
                if let Err(e) = result {
                    error!("LOD_LAB cannot save {}: {e}", path.display());
                    failed.store(true, Ordering::Release);
                }
                saving.fetch_sub(1, Ordering::AcqRel);
            });
        },
    );
}
