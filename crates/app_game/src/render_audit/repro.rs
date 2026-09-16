//! Reproducible low-camera movement from log9, opt-in and separate from the UI baseline.
use bevy::{diagnostic::FrameCount, prelude::*};
use engine::{ActiveWorldSpace, GameInputSystems, WorldViewCamera};

use super::{AuditRenderPath, AuditSettings};

pub(super) fn install(app: &mut App) {
    let mut args = std::env::args();
    if !args.any(|arg| arg == "--render-repro") {
        return;
    }
    let name = args.next().expect("--render-repro requires a scene");
    assert!(
        matches!(
            name.as_str(),
            "low-walk"
                | "grass-close"
                | "grass-away"
                | "grass-follow"
                | "grass-follow-far"
                | "grass-zoom"
                | "grass-top-down"
                | "grass-overhead"
                | "grass-stream"
                | "ground-low"
                | "ground-overhead"
                | "ground-walk"
                | "ground-stream"
        ),
        "expected --render-repro low-walk, grass-close, grass-away, grass-zoom, grass-top-down, grass-overhead, grass-stream, ground-low, ground-overhead, ground-walk or ground-stream"
    );
    let ground = name.starts_with("ground-");
    // Same 75%/4x/no-prepass setup as log9. Leave both optimization switches independent.
    *app.world_mut().resource_mut::<AuditSettings>() = AuditSettings {
        scene: if ground {
            super::Scene::Ground
        } else {
            super::Scene::Current
        },
        render_path: if ground {
            AuditRenderPath::Direct
        } else {
            AuditRenderPath::Composite
        },
        scale_index: if ground { 0 } else { 1 },
        msaa: Msaa::Sample4,
        prepass: false,
        controls_locked: true,
        counters: std::env::args_os().any(|arg| arg == "--grass-counters"),
        show_ui: !ground,
        ..default()
    };
    app.insert_resource(ReproView(name.clone()));
    crate::profile::install(app);
    if let Some(profile) = app
        .world()
        .get_resource::<crate::profile::ProfileSettings>()
        .cloned()
    {
        let mut settings = app.world_mut().resource_mut::<AuditSettings>();
        settings.render_path = AuditRenderPath::Composite;
        settings.scale_index = if profile.size.is_some() { 0 } else { 1 };
        settings.msaa = profile.msaa;
        settings.grass = if profile.grass {
            vegetation_render::VegetationProfileMode::Full
        } else {
            vegetation_render::VegetationProfileMode::Disabled
        };
    }
    app.add_systems(Update, move_camera.after(GameInputSystems))
        .add_systems(PostUpdate, synchronize_wind);
    let path_frames = if name.ends_with("-stream") {
        6000
    } else if name == "grass-zoom" {
        1200
    } else {
        600
    };
    warn!(
        "RENDER_REPRO name={name} version=8 warmup_frames=300 path_frames={path_frames} msaa=4 prepass=false; ground scenes use native resolution, no UI, no grass; low-walk retains 75% composite"
    );
    let value = |flag: &str| {
        let mut args = std::env::args();
        args.find(|arg| arg == flag).and_then(|_| args.next())
    };
    let frames = value("--render-frames").map(|v| {
        v.parse::<u32>()
            .expect("--render-frames requires an integer")
    });
    let screenshot = value("--render-snapshot");
    let screenshot_frames = value("--render-snapshot-frames")
        .map(|value| {
            value
                .split(',')
                .map(|frame| {
                    frame
                        .parse::<u32>()
                        .expect("--render-snapshot-frames requires comma-separated frame numbers")
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![600]);
    assert!(
        screenshot_frames
            .iter()
            .all(|&frame| frame >= 300 && frames.is_none_or(|end| frame < end)),
        "snapshot frames must follow warmup and precede the exit frame"
    );
    if frames.is_some() || screenshot.is_some() {
        assert!(
            frames.is_none_or(|v| v >= 900),
            "allow at least 900 frames for warmup/screenshot"
        );
        app.insert_resource(ReproOutput {
            frames,
            screenshot,
            screenshot_frames,
        })
        .add_systems(Update, output);
    }
}

#[derive(Resource)]
struct ReproView(String);
#[derive(Resource)]
struct ReproOutput {
    frames: Option<u32>,
    screenshot: Option<String>,
    screenshot_frames: Vec<u32>,
}

fn output(
    mut commands: Commands,
    frame: Res<FrameCount>,
    config: Res<ReproOutput>,
    mut exit: MessageWriter<AppExit>,
) {
    if config.screenshot_frames.contains(&frame.0)
        && let Some(path) = &config.screenshot
    {
        use bevy::render::view::screenshot::{Screenshot, save_to_disk};
        let path = if config.screenshot_frames.len() > 1 {
            let path = std::path::Path::new(path);
            path.with_file_name(format!(
                "{}-{:06}.{}",
                path.file_stem().unwrap().to_string_lossy(),
                frame.0,
                path.extension().unwrap_or_default().to_string_lossy()
            ))
        } else {
            std::path::PathBuf::from(path)
        };
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if config.frames.is_some_and(|end| frame.0 >= end) {
        exit.write(AppExit::Success);
    }
}

fn pose(frame: u32) -> Transform {
    // Warm up the actual close view first. Walk ~10 m forward and back so all source
    // pages stay in the resident shell around the idle actor, without teleports.
    let phase = (frame.saturating_sub(300) % 600) as f32 / 300.0;
    let progress = if phase <= 1.0 { phase } else { 2.0 - phase };
    Transform::from_translation(
        Vec3::new(-2.876, 1.730, 2.384) + Vec3::new(5.389, 0.0, -8.279) * progress,
    )
    .with_rotation(Quat::from_xyzw(-0.0851, -0.4410, -0.0420, 0.8925).normalize())
}

fn move_camera(
    frame: Res<FrameCount>,
    profile: Option<Res<crate::profile::ProfileClock>>,
    view: Res<ReproView>,
    mut camera: Single<&mut Transform, With<WorldViewCamera>>,
    mut active_space: ResMut<ActiveWorldSpace>,
) {
    let frame = profile.as_ref().map_or(frame.0, |p| p.reference_frame());
    **camera = match view.0.as_str() {
        "ground-low" => pose(0),
        "grass-close" | "grass-away" => {
            // Match the minimum-distance third-person rig, including its elevated focus.
            // The opposite orbit exposes body lighting without the strong sun reflection.
            let pitch = 10.0_f32.to_radians();
            let facing = if view.0 == "grass-away" { -1.0 } else { 1.0 };
            Transform::from_xyz(0.0, 0.9 + 4.0 * pitch.sin(), facing * 4.0 * pitch.cos())
                .looking_at(Vec3::new(0.0, 0.9, 0.0), Vec3::Y)
        }
        "grass-follow" | "grass-follow-far" => {
            let distance = if view.0 == "grass-follow" { 9.7 } else { 17.6 };
            let pitch = (10.0_f32 + 45.0 * ((distance - 4.0) / 20.0)).to_radians();
            let focus = Vec3::new(0.0, 0.9, 0.0);
            Transform::from_xyz(0.0, 0.9 + distance * pitch.sin(), distance * pitch.cos())
                .looking_at(focus, Vec3::Y)
        }
        "grass-zoom" => zoom_pose(frame),
        // Separate vertical inspection from the oblique gameplay/overhead views. NEG_Z
        // avoids a collinear look/up basis while preserving +X toward screen right.
        "grass-top-down" => Transform::from_xyz(0.0, 18.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z),
        "ground-overhead" | "grass-overhead" => {
            Transform::from_xyz(-12.0, 18.0, 16.0).looking_at(Vec3::ZERO, Vec3::Y)
        }
        "ground-stream" | "grass-stream" => stream_pose(frame),
        _ => pose(frame),
    };
    if view.0.ends_with("-stream")
        && let Some(space) = active_space.current()
    {
        // Drive the actual residency focus too. Camera-only ground-walk stays in
        // the original preload ring and cannot validate page streaming.
        let position = camera.translation;
        active_space.request(space, [position.x, 0.0, position.z]);
    }
}

fn zoom_pose(frame: u32) -> Transform {
    // Same distance/pitch relationship as the gameplay rig; a fixed focus separates zoom
    // artifacts from streaming. Return to exactly the initial pose after one cycle.
    let phase = (frame.saturating_sub(300) % 1200) as f32 / 600.0;
    let t = if phase <= 1.0 { phase } else { 2.0 - phase };
    let distance = 17.6 + (7.76 - 17.6) * t;
    let zoom = (distance - 4.0) / 20.0;
    let pitch = (10.0 + 45.0 * zoom).to_radians();
    let yaw = 45.0_f32.to_radians();
    let focus = Vec3::new(0.0, 0.9, 0.0);
    Transform::from_translation(
        focus
            + Vec3::new(
                yaw.sin() * distance * pitch.cos(),
                distance * pitch.sin(),
                yaw.cos() * distance * pitch.cos(),
            ),
    )
    .looking_at(focus, Vec3::Y)
}

fn stream_pose(frame: u32) -> Transform {
    let phase = frame.saturating_sub(300).min(6000) as f32 / 3000.0;
    let progress = if phase <= 1.0 { phase } else { 2.0 - phase };
    let mut camera = pose(0);
    camera.translation += Vec3::new(5.389, 0.0, -8.279).normalize() * (128.0 * progress);
    camera
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_walk_warms_up_then_moves_without_teleporting() {
        assert_eq!(pose(0), pose(300));
        assert_eq!(pose(300), pose(900));
        assert!((pose(600).translation - Vec3::new(2.513, 1.730, -5.895)).length() < 0.0001);
        for frame in 301..=1500 {
            assert!((pose(frame).translation - pose(frame - 1).translation).length() < 0.034);
            assert_eq!(pose(frame).translation.y, 1.730);
            assert!(pose(frame).rotation.is_normalized());
        }
    }

    #[test]
    fn stream_route_crosses_pages_and_returns_without_jumps() {
        assert_eq!(stream_pose(0), stream_pose(300));
        assert_eq!(stream_pose(300), stream_pose(6300));
        assert!(
            (stream_pose(3300)
                .translation
                .distance(stream_pose(300).translation)
                - 128.0)
                .abs()
                < 0.0001
        );
        for frame in 301..=6600 {
            assert!(
                stream_pose(frame)
                    .translation
                    .distance(stream_pose(frame - 1).translation)
                    < 0.044
            );
        }
    }
}

fn synchronize_wind(
    frame: Res<FrameCount>,
    profile: Option<Res<crate::profile::ProfileClock>>,
    view: Res<ReproView>,
    mut wind: ResMut<vegetation_render::VegetationWind>,
) {
    if matches!(
        view.0.as_str(),
        "grass-zoom" | "grass-top-down" | "grass-follow" | "grass-follow-far"
    ) {
        wind.set_phase_seconds(0.0);
        return;
    }
    // Run after Update's normal wind advance. Capture frame 600 now has identical wind
    // geometry even when startup compilation takes a different amount of wall-clock time.
    wind.set_phase_seconds(profile.as_ref().map_or_else(
        || frame.0 as f32 / if cfg!(target_os = "ios") { 60.0 } else { 120.0 },
        |p| 2.5 + p.route_seconds as f32,
    ));
}
