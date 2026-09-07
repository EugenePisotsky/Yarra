//! Reproducible low-camera movement from log9, opt-in and separate from the UI baseline.
use bevy::{diagnostic::FrameCount, prelude::*};
use engine::{GameInputSystems, WorldViewCamera};

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
            "low-walk" | "ground-low" | "ground-overhead" | "ground-walk"
        ),
        "expected --render-repro low-walk, ground-low, ground-overhead or ground-walk"
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
    app.add_systems(Update, move_camera.after(GameInputSystems))
        .add_systems(PostUpdate, synchronize_wind);
    warn!(
        "RENDER_REPRO name={name} version=4 warmup_frames=300 path_frames=600 msaa=4 prepass=false; ground scenes use native resolution, no UI, no grass; low-walk retains 75% composite"
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
    if frames.is_some() || screenshot.is_some() {
        assert!(
            frames.is_none_or(|v| v >= 900),
            "allow at least 900 frames for warmup/screenshot"
        );
        app.insert_resource(ReproOutput { frames, screenshot })
            .add_systems(Update, output);
    }
}

#[derive(Resource)]
struct ReproView(String);
#[derive(Resource)]
struct ReproOutput {
    frames: Option<u32>,
    screenshot: Option<String>,
}

fn output(
    mut commands: Commands,
    frame: Res<FrameCount>,
    config: Res<ReproOutput>,
    mut exit: MessageWriter<AppExit>,
) {
    if frame.0 == 600
        && let Some(path) = &config.screenshot
    {
        use bevy::render::view::screenshot::{Screenshot, save_to_disk};
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()));
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
    view: Res<ReproView>,
    mut camera: Single<&mut Transform, With<WorldViewCamera>>,
) {
    **camera = match view.0.as_str() {
        "ground-low" => pose(0),
        "ground-overhead" => Transform::from_xyz(-12.0, 18.0, 16.0).looking_at(Vec3::ZERO, Vec3::Y),
        _ => pose(frame.0),
    };
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
}

fn synchronize_wind(frame: Res<FrameCount>, mut wind: ResMut<vegetation_render::VegetationWind>) {
    // Run after Update's normal wind advance. Capture frame 600 now has identical wind
    // geometry even when startup compilation takes a different amount of wall-clock time.
    wind.set_phase_seconds(frame.0 as f32 / if cfg!(target_os = "ios") { 60.0 } else { 120.0 });
}
