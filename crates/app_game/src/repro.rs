//! Reproducible camera routes for profiles and captures, opt-in and separate from the UI
//! baseline.
use bevy::{diagnostic::FrameCount, prelude::*};
use engine::{ActiveWorldSpace, GameplaySystems, WorldViewCamera};

use crate::game_render::scale_index;
use crate::runtime_settings::{RuntimeSettings, Scene};

/// A repeatable camera route, named on the command line by [`Route::name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    LowWalk,
    GrassClose,
    GrassAway,
    GrassFollow,
    GrassFollowFar,
    GrassZoom,
    GrassTopDown,
    GrassOverhead,
    GrassStream,
    GrassSoak,
    Landscape,
    LandscapeTurn,
    LandscapeDescent,
    /// The player walks the start view's route with the normal follow camera.
    ActorWalk,
}

impl Route {
    pub(crate) const ALL: [Self; 14] = [
        Self::LowWalk,
        Self::GrassClose,
        Self::GrassAway,
        Self::GrassFollow,
        Self::GrassFollowFar,
        Self::GrassZoom,
        Self::GrassTopDown,
        Self::GrassOverhead,
        Self::GrassStream,
        Self::GrassSoak,
        Self::Landscape,
        Self::LandscapeTurn,
        Self::LandscapeDescent,
        Self::ActorWalk,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::LowWalk => "low-walk",
            Self::GrassClose => "grass-close",
            Self::GrassAway => "grass-away",
            Self::GrassFollow => "grass-follow",
            Self::GrassFollowFar => "grass-follow-far",
            Self::GrassZoom => "grass-zoom",
            Self::GrassTopDown => "grass-top-down",
            Self::GrassOverhead => "grass-overhead",
            Self::GrassStream => "grass-stream",
            Self::GrassSoak => "grass-soak",
            Self::Landscape => "landscape",
            Self::LandscapeTurn => "landscape-turn",
            Self::LandscapeDescent => "landscape-descent",
            Self::ActorWalk => "actor-walk",
        }
    }

    pub(crate) fn parse(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|route| route.name() == name)
            .ok_or_else(|| {
                let names: Vec<_> = Self::ALL.iter().map(|route| route.name()).collect();
                format!("Unknown repro {name:?}; expected {}", names.join(", "))
            })
    }

    fn landscape(self) -> bool {
        matches!(
            self,
            Self::Landscape | Self::LandscapeTurn | Self::LandscapeDescent
        )
    }

    /// Landscape routes start at the bookmark; the actor walks its route.
    pub(crate) fn needs_start_view(self) -> bool {
        self.landscape() || self == Self::ActorWalk
    }

    /// Routes that move the residency focus, not only the camera.
    fn streams(self) -> bool {
        matches!(self, Self::GrassStream | Self::GrassSoak)
    }

    fn path_frames(self) -> u32 {
        match self {
            Self::GrassStream => 6000,
            Self::GrassSoak => 1800,
            Self::GrassZoom | Self::LandscapeTurn => 1200,
            _ => 600,
        }
    }
}

pub(crate) fn install(app: &mut App) {
    let options = app
        .world()
        .resource::<crate::launch::LaunchOptions>()
        .clone();
    let Some(repro) = options.repro else {
        return;
    };
    let route = repro.route;
    // 4× MSAA and no prepass unless asked for.
    *app.world_mut().resource_mut::<RuntimeSettings>() = RuntimeSettings {
        scene: Scene::Current,
        // 75% unless `--resolution-scale` names another of the game's scales.
        scale_index: options.resolution_scale.and_then(scale_index).unwrap_or(1),
        msaa: Msaa::Sample4,
        prepass: repro.prepass,
        // A scripted walk needs the character to move.
        controls_locked: route != Route::ActorWalk,
        counters: options.counters,
        show_ui: !repro.hide_ui,
        temporal_debug: repro.temporal_view,
        ..app.world().resource::<RuntimeSettings>().clone()
    };
    if route == Route::ActorWalk {
        let view = app
            .world()
            .resource::<engine::WorldStartView>()
            .0
            .clone()
            .expect("actor-walk requires --start-view with a route");
        app.insert_resource(engine::PlayerRoute::new(view.route).with_speed(ACTOR_WALK_SPEED_MPS))
            .add_systems(Update, log_actor_walk);
    }
    app.insert_resource(ReproView(route))
        .insert_resource(ReproClock {
            frames: repro.frame_clock,
        });
    app.add_systems(Update, move_camera.after(GameplaySystems::CameraFollow))
        .add_systems(PostUpdate, synchronize_wind.before(engine::TreeWindSystems));
    let settings = app.world().resource::<RuntimeSettings>();
    warn!(
        "RENDER_REPRO name={} version=9 warmup_frames=300 path_frames={} prepass={} ui={}",
        route.name(),
        route.path_frames(),
        settings.prepass,
        settings.show_ui
    );
    let frames = repro.frames;
    let screenshot = repro.snapshot;
    let screenshot_frames = repro.snapshot_frames;
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

/// About 70 km/h: kilometres of island in a couple of minutes.
const ACTOR_WALK_SPEED_MPS: f32 = 20.;

#[derive(Resource)]
struct ReproView(Route);
/// Whether routes advance by frame count (at 60 fps) instead of elapsed time.
#[derive(Resource)]
struct ReproClock {
    frames: bool,
}
#[derive(Resource)]
struct ReproOutput {
    frames: Option<u32>,
    screenshot: Option<std::path::PathBuf>,
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

/// Where the walk has reached, every two seconds of frames at 60 FPS.
fn log_actor_walk(
    frame: Res<FrameCount>,
    viewpoint: Res<engine::WorldViewpoint>,
    catalog: Res<engine::WorldCatalog>,
    route: Option<Res<engine::PlayerRoute>>,
) {
    if !frame.0.is_multiple_of(120) {
        return;
    }
    let Some(position) = viewpoint.position() else {
        return;
    };
    let size = catalog
        .world_space(position.space)
        .map_or(world::DEFAULT_CELL_SIZE, |s| s.cell_size);
    let origin = position.cell.origin(size);
    warn!(
        "ACTOR_WALK frame={} x={:.0} z={:.0} remaining={}",
        frame.0,
        origin[0] + f64::from(position.local[0]),
        origin[1] + f64::from(position.local[2]),
        route.map_or(0, |r| r.remaining())
    );
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

#[allow(clippy::too_many_arguments)] // The route's clocks, camera and world placement.
fn move_camera(
    frame: Res<FrameCount>,
    time: Res<Time<Real>>,
    profile: Option<Res<crate::profile::ProfileClock>>,
    view: Res<ReproView>,
    clock: Res<ReproClock>,
    mut camera: Single<&mut Transform, With<WorldViewCamera>>,
    mut active_space: ResMut<ActiveWorldSpace>,
    start_view: Res<engine::WorldStartView>,
    origin: Res<engine::WorldOrigin>,
    catalog: Res<engine::WorldCatalog>,
    viewpoint: Res<engine::WorldViewpoint>,
) {
    if view.0 == Route::ActorWalk {
        // Follow the walking player while looking left and right, as a player does.
        let (Some(bookmark), Some(position)) = (start_view.0.as_ref(), viewpoint.position()) else {
            return;
        };
        let size = catalog
            .world_space(position.space)
            .map_or(world::DEFAULT_CELL_SIZE, |s| s.cell_size);
        let offset = origin.cell().origin(size);
        let at = position.cell.origin(size);
        let relative = Vec3::new(
            (at[0] + f64::from(position.local[0]) - offset[0]) as f32,
            position.local[1],
            (at[1] + f64::from(position.local[2]) - offset[1]) as f32,
        );
        // Hold the heading while the cover grows to the full patch budget, then look behind
        // to one side and back every 20 s: most of the cover is replaced each time.
        let mut look = bookmark.clone();
        let t = time.elapsed_secs();
        if t > 60. && (((t - 60.) / 20.) as u32).is_multiple_of(2) {
            look.yaw_degrees += 180.;
        }
        **camera = engine::WorldStartView::camera_at(&look, relative);
        return;
    }
    if view.0.landscape() {
        let bookmark = start_view.0.as_ref().unwrap();
        let elapsed = profile.as_ref().map_or_else(
            || {
                let seconds = if clock.frames {
                    frame.0 as f32 / 60.
                } else {
                    time.elapsed_secs()
                };
                (seconds - 5.).max(0.)
            },
            |p| p.route_seconds as f32,
        );
        let position = Vec3::from_array(if view.0 == Route::LandscapeDescent {
            bookmark.route_position(elapsed * 6.)
        } else {
            bookmark.position
        });
        let cell_size = origin
            .space()
            .and_then(|s| catalog.world_space(s))
            .map_or(world::DEFAULT_CELL_SIZE, |s| s.cell_size);
        let offset = origin.cell().origin(cell_size);
        **camera = engine::WorldStartView::camera_at(
            bookmark,
            position - Vec3::new(offset[0] as f32, 0., offset[1] as f32),
        );
        if view.0 == Route::LandscapeTurn {
            // Turn in place, then dwell well beyond source residency's cooling period.
            // At 60 FPS each heading lasts ten seconds; no camera translation/rebase.
            let frame = profile.as_ref().map_or(frame.0, |p| p.reference_frame());
            if (frame / 600) % 2 == 1 {
                camera.rotation = Quat::from_rotation_y(135_f32.to_radians()) * camera.rotation;
            }
        }
        if let Some(space) = active_space.current() {
            active_space.request(space, position.to_array());
        }
        return;
    }
    let frame = profile.as_ref().map_or(frame.0, |p| p.reference_frame());
    **camera = match view.0 {
        Route::GrassClose | Route::GrassAway => {
            // Match the minimum-distance third-person rig, including its elevated focus.
            // The opposite orbit exposes body lighting without the strong sun reflection.
            let pitch = 10.0_f32.to_radians();
            let facing = if view.0 == Route::GrassAway {
                -1.0
            } else {
                1.0
            };
            Transform::from_xyz(0.0, 0.9 + 4.0 * pitch.sin(), facing * 4.0 * pitch.cos())
                .looking_at(Vec3::new(0.0, 0.9, 0.0), Vec3::Y)
        }
        Route::GrassFollow | Route::GrassFollowFar => {
            let distance = if view.0 == Route::GrassFollow {
                9.7
            } else {
                17.6
            };
            let pitch = (10.0_f32 + 45.0 * ((distance - 4.0) / 20.0)).to_radians();
            let focus = Vec3::new(0.0, 0.9, 0.0);
            Transform::from_xyz(0.0, 0.9 + distance * pitch.sin(), distance * pitch.cos())
                .looking_at(focus, Vec3::Y)
        }
        Route::GrassZoom => zoom_pose(frame),
        Route::GrassSoak => {
            let mut camera = zoom_pose(300);
            camera.translation += soak_focus(frame);
            camera
        }
        // Separate vertical inspection from the oblique gameplay/overhead views. NEG_Z
        // avoids a collinear look/up basis while preserving +X toward screen right.
        Route::GrassTopDown => {
            Transform::from_xyz(0.0, 18.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z)
        }
        Route::GrassOverhead => {
            Transform::from_xyz(-12.0, 18.0, 16.0).looking_at(Vec3::ZERO, Vec3::Y)
        }
        Route::GrassStream => stream_pose(frame),
        _ => pose(frame),
    };
    if view.0.streams()
        && let Some(space) = active_space.current()
    {
        // Drive the actual residency focus too. A camera-only route stays in the original
        // preload ring and cannot validate page streaming.
        let position = if view.0 == Route::GrassSoak {
            soak_focus(frame)
        } else {
            camera.translation
        };
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

fn soak_focus(frame: u32) -> Vec3 {
    // Keep traversing the authored meadow, rather than finishing a one-way
    // stream route or spending most of a long run above empty ground. One 30 s
    // loop crosses cell boundaries at walking speed and returns continuously.
    let phase = (frame.saturating_sub(300) % 1800) as f32 / 1800.0;
    let angle = phase * std::f32::consts::TAU;
    Vec3::new(12.0 * angle.sin(), 0.0, 12.0 * (1.0 - angle.cos()))
}

fn synchronize_wind(
    frame: Res<FrameCount>,
    profile: Option<Res<crate::profile::ProfileClock>>,
    view: Res<ReproView>,
    mut wind: ResMut<vegetation_render::VegetationWind>,
) {
    if matches!(
        view.0,
        Route::GrassZoom | Route::GrassTopDown | Route::GrassFollow | Route::GrassFollowFar
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

    #[test]
    fn soak_route_keeps_moving_across_repeated_cycles() {
        assert_eq!(soak_focus(0), Vec3::ZERO);
        assert_eq!(soak_focus(300), soak_focus(2100));
        assert_eq!(soak_focus(2100), soak_focus(3900));
        assert!(soak_focus(1200).z > 23.9);
        for frame in 301..=5700 {
            let distance = soak_focus(frame).distance(soak_focus(frame - 1));
            assert!((0.04..0.043).contains(&distance));
        }
    }
}
