//! Opt-in profiling controls. Normal game presentation is unchanged.
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::{
    prelude::*,
    window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode},
};

#[derive(Resource, Clone)]
pub(crate) struct ProfileSettings {
    /// None follows the normal game's world-resolution scale and surface aspect ratio.
    pub size: Option<UVec2>,
    /// Exact physical window size for comparable Retina GPU measurements.
    surface: Option<UVec2>,
    pub bloom: bool,
    pub temporal_bypass: bool,
    pub msaa: Msaa,
    pub grass: bool,
    fps: u32,
    warmup: f64,
    seconds: f64,
    fullscreen: bool,
    diagnostic: bool,
    /// Keep gameplay event-loop/VSync behavior while recording timed samples.
    native_pacing: bool,
}

impl ProfileSettings {
    fn parse(args: &[String]) -> Result<Option<Self>, String> {
        let value = |key: &str| -> Result<Option<&str>, String> {
            args.iter()
                .position(|a| a == key)
                .map(|i| {
                    args.get(i + 1)
                        .map(String::as_str)
                        .filter(|v| !v.starts_with("--"))
                        .ok_or_else(|| format!("{key} requires a value"))
                })
                .transpose()
        };
        let diagnostic = args.iter().any(|a| a == "--profile-diagnostic");
        let seconds = value("--profile-seconds")?;
        if diagnostic && seconds.is_some() {
            return Err("Diagnostic captures cannot be combined with timed power runs".into());
        }
        if !diagnostic && seconds.is_none() {
            return Ok(None);
        }
        if diagnostic
            && !args
                .iter()
                .any(|a| matches!(a.as_str(), "--metal-capture" | "--render-frames"))
        {
            return Err(
                "Diagnostic presentation requires --metal-capture or --render-frames".into(),
            );
        }
        if !diagnostic
            && args.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "--render-frames" | "--render-snapshot" | "--metal-capture"
                )
            })
        {
            return Err(
                "Timed power runs cannot include frame-limited output, screenshots or GPU capture"
                    .into(),
            );
        }
        let duration = |s: &str, low: f64, high: f64| -> Result<f64, String> {
            let n = s
                .parse::<f64>()
                .map_err(|_| format!("Invalid duration: {s}"))?;
            if n.is_finite() && (low..=high).contains(&n) {
                Ok(n)
            } else {
                Err(format!("Duration must be between {low} and {high} seconds"))
            }
        };
        let size = value("--profile-size")?.unwrap_or("2560x1440");
        let size = if size == "game" {
            None
        } else {
            let (w, h) = size
                .split_once('x')
                .ok_or("Expected game or WIDTHxHEIGHT")?;
            let width = w.parse::<u32>().map_err(|_| "Invalid width")?;
            let height = h.parse::<u32>().map_err(|_| "Invalid height")?;
            if !(64..=8192).contains(&width) || !(64..=8192).contains(&height) {
                return Err("Profile dimensions must be in 64..8192".into());
            }
            Some(UVec2::new(width, height))
        };
        let surface = value("--profile-surface")?
            .map(|s| {
                let (w, h) = s
                    .split_once('x')
                    .ok_or("Expected WIDTHxHEIGHT for profile surface")?;
                let w = w.parse::<u32>().map_err(|_| "Invalid surface width")?;
                let h = h.parse::<u32>().map_err(|_| "Invalid surface height")?;
                if !(64..=8192).contains(&w) || !(64..=8192).contains(&h) {
                    return Err("Profile surface dimensions must be in 64..8192");
                }
                Ok(UVec2::new(w, h))
            })
            .transpose()?;
        let bloom = match value("--profile-bloom")?.unwrap_or("on") {
            "on" => true,
            "off" => false,
            _ => return Err("Profile bloom must be on or off".into()),
        };
        // Preserve old direct profiling commands; the runner explicitly selects its new default.
        let fullscreen = match value("--profile-window")?.unwrap_or("windowed") {
            "fullscreen" => true,
            "windowed" => false,
            _ => return Err("Profile window must be fullscreen or windowed".into()),
        };
        let fps = value("--profile-fps")?
            .unwrap_or("60")
            .parse::<u32>()
            .map_err(|_| "Invalid fps")?;
        if fps != 0 && !(15..=240).contains(&fps) {
            return Err("FPS must be 0 (uncapped) or 15..240".into());
        }
        let msaa = match value("--profile-msaa")?.unwrap_or("4") {
            "1" => Msaa::Off,
            "2" => Msaa::Sample2,
            "4" => Msaa::Sample4,
            _ => return Err("MSAA must be 1, 2 or 4".into()),
        };
        let grass = match value("--profile-grass")?.unwrap_or("full") {
            "full" => true,
            "off" => false,
            _ => return Err("Profile grass must be full or off".into()),
        };
        Ok(Some(Self {
            size,
            surface,
            bloom,
            temporal_bypass: args.iter().any(|a| a == "--profile-temporal-bypass"),
            msaa,
            grass,
            fps,
            warmup: duration(value("--profile-warmup")?.unwrap_or("15"), 1.0, 600.0)?,
            seconds: duration(seconds.unwrap_or("2"), 2.0, 3600.0)?,
            fullscreen,
            diagnostic,
            native_pacing: args.iter().any(|a| a == "--profile-native-pacing"),
        }))
    }

    pub fn resolution_scale(&self) -> f32 {
        if self.size.is_some() {
            1.0
        } else {
            crate::game_render::GameRenderSettings::default().resolution_scale
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct ProfileClock {
    pub route_seconds: f64,
    started: Option<f64>,
    last_sample: Option<f64>,
    last_tick: Option<f64>,
    intervals: Vec<f64>,
    total_frames: u64,
    focused: bool,
    finished: bool,
}

impl ProfileClock {
    pub fn reference_frame(&self) -> u32 {
        300 + (self.route_seconds * 120.0).round() as u32
    }
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn install(app: &mut App) {
    let args: Vec<_> = std::env::args().collect();
    let Some(settings) = ProfileSettings::parse(&args).unwrap_or_else(|e| panic!("{e}")) else {
        return;
    };
    let diagnostic = settings.diagnostic;
    if !settings.native_pacing {
        crate::frame_pacing::set_fps(app, settings.fps);
    }
    // A timed/diagnostic profile describes one launch rate, not a changing UI cap.
    app.world_mut()
        .resource_mut::<crate::frame_pacing::FramePacing>()
        .profile_locked = true;
    app.insert_resource(settings).add_systems(Startup, setup);
    // Captures use the existing deterministic frame-based camera/wind, not a timed run.
    // No measure_start/sample/complete events are emitted for diagnostic presentation.
    if !diagnostic {
        app.init_resource::<ProfileClock>()
            .add_systems(PreUpdate, tick);
    }
}

fn setup(settings: Res<ProfileSettings>, mut window: Single<&mut Window, With<PrimaryWindow>>) {
    if settings.fullscreen {
        window.mode = WindowMode::BorderlessFullscreen(MonitorSelection::Primary);
    } else {
        window.mode = WindowMode::Windowed;
        if let Some(surface) = settings.surface {
            window
                .resolution
                .set_physical_resolution(surface.x, surface.y);
        } else {
            let size = settings.size.unwrap_or(UVec2::new(1280, 720));
            window
                .resolution
                .set(720.0 * size.x as f32 / size.y as f32, 720.0);
        }
    }
    window.resizable = false;
    if settings.fps == 0 && !settings.native_pacing {
        window.present_mode = PresentMode::AutoNoVsync;
    }
    let pacing = if settings.native_pacing {
        "native"
    } else {
        "profile"
    };
    let size = settings
        .size
        .map_or_else(|| "game".into(), |s| format!("{}x{}", s.x, s.y));
    let presentation = if settings.fullscreen {
        "fullscreen"
    } else {
        "windowed"
    };
    if settings.diagnostic {
        warn!(
            "GRASS_PROFILE event=diagnostic_config unix_ms={} size={size} window={presentation} resolution_scale={} fps={} msaa={} grass={} pacing={pacing} clock=frame-based",
            unix_ms(),
            settings.resolution_scale(),
            settings.fps,
            settings.msaa.samples(),
            settings.grass
        );
        return;
    }
    warn!(
        "GRASS_PROFILE event=config unix_ms={} size={size} window={presentation} resolution_scale={} fps={} warmup_s={} seconds={} msaa={} grass={} pacing={pacing} clock=real-time",
        unix_ms(),
        settings.resolution_scale(),
        settings.fps,
        settings.warmup,
        settings.seconds,
        settings.msaa.samples(),
        settings.grass
    );
}

fn tick(
    settings: Res<ProfileSettings>,
    time: Res<Time<Real>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut clock: ResMut<ProfileClock>,
    mut exit: MessageWriter<AppExit>,
) {
    if clock.finished {
        return;
    }
    let now = time.elapsed_secs_f64();
    if now < settings.warmup {
        return;
    }
    let Some(start) = clock.started else {
        clock.started = Some(now);
        clock.last_tick = Some(now);
        clock.last_sample = Some(now);
        clock.focused = window.focused;
        warn!(
            "GRASS_PROFILE event=measure_start unix_ms={} elapsed_s={now:.6} focused={}",
            unix_ms(),
            window.focused
        );
        return;
    };
    clock.route_seconds = now - start;
    if let Some(last) = clock.last_tick.replace(now) {
        clock.intervals.push((now - last) * 1000.0);
    }
    clock.total_frames += 1;
    clock.focused &= window.focused;
    let finished = clock.route_seconds >= settings.seconds;
    if now - clock.last_sample.unwrap_or(now) >= 1.0 || finished {
        let span = now - clock.last_sample.unwrap_or(now);
        let count = clock.intervals.len();
        let max = clock.intervals.iter().copied().fold(0.0_f64, f64::max);
        clock.intervals.sort_by(f64::total_cmp);
        let quantile = |q: f64| {
            clock
                .intervals
                .get(((count.saturating_sub(1)) as f64 * q) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        let p95 = quantile(0.95);
        let p99 = quantile(0.99);
        let threshold = if settings.fps == 0 {
            f64::INFINITY
        } else {
            1500.0 / f64::from(settings.fps)
        };
        let late = clock.intervals.iter().filter(|&&v| v > threshold).count();
        warn!(
            "GRASS_PROFILE event=sample unix_ms={} elapsed_s={now:.6} route_s={:.6} window_s={span:.6} frames={count} update_fps={:.3} update_p95_ms={p95:.3} update_p99_ms={p99:.3} update_max_ms={max:.3} late_updates={late} focused={}",
            unix_ms(),
            clock.route_seconds,
            count as f64 / span,
            clock.focused
        );
        clock.last_sample = Some(now);
        clock.intervals.clear();
        clock.focused = window.focused;
    }
    if finished {
        warn!(
            "GRASS_PROFILE event=complete unix_ms={} elapsed_s={now:.6} measured_s={:.6} frames={}",
            unix_ms(),
            clock.route_seconds,
            clock.total_frames
        );
        clock.finished = true;
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(s: &str) -> Result<Option<ProfileSettings>, String> {
        ProfileSettings::parse(&s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
    }
    #[test]
    fn profiling_is_opt_in_and_rejects_invalid_or_contaminated_runs() {
        assert!(parse("game").unwrap().is_none());
        for args in [
            "--profile-seconds NaN",
            "--profile-seconds -1",
            "--profile-seconds 10 --profile-fps 1",
            "--profile-seconds 10 --profile-size 0x1440",
            "--profile-seconds 10 --profile-surface 0x1440",
            "--profile-seconds 10 --profile-surface 3456",
            "--profile-seconds 10 --profile-bloom invalid",
            "--profile-seconds 10 --profile-window maximized",
            "--profile-seconds 10 --render-snapshot file.png",
            "--profile-seconds 10 --metal-capture /tmp/frame.gputrace",
            "--profile-seconds 10 --profile-diagnostic --render-frames 900",
            "--profile-diagnostic",
        ] {
            assert!(parse(args).is_err(), "{args}");
        }
        let p = parse("--profile-seconds 10 --profile-fps 120 --profile-grass off")
            .unwrap()
            .unwrap();
        assert_eq!(p.size, Some(UVec2::new(2560, 1440)));
        assert!(!p.fullscreen);
        assert!(!p.grass);
        assert!(!p.native_pacing);
        let p = parse("--profile-seconds 10 --profile-native-pacing")
            .unwrap()
            .unwrap();
        assert!(p.native_pacing);
    }

    #[test]
    fn diagnostic_presentation_is_separate_from_timed_measurement() {
        let p = parse("--profile-diagnostic --metal-capture /tmp/frame.gputrace --profile-size game --profile-window fullscreen --profile-fps 120")
            .unwrap().unwrap();
        assert!(p.diagnostic);
        assert!(p.fullscreen);
        assert_eq!(p.size, None);
    }
    #[test]
    fn fullscreen_game_size_uses_normal_resolution_scale() {
        let p = parse("--profile-seconds 10 --profile-size game --profile-window fullscreen")
            .unwrap()
            .unwrap();
        assert!(p.fullscreen);
        assert_eq!(p.size, None);
        assert_eq!(p.resolution_scale(), 0.5);
    }
    #[test]
    fn physical_surface_does_not_override_scene_resolution() {
        let p = parse("--profile-seconds 10 --profile-size game --profile-surface 3456x1942 --profile-bloom off --profile-temporal-bypass")
            .unwrap().unwrap();
        assert_eq!(p.surface, Some(UVec2::new(3456, 1942)));
        assert_eq!(p.size, None);
        assert_eq!(p.resolution_scale(), 0.5);
        assert!(!p.bloom);
        assert!(p.temporal_bypass);
        let default = parse("--profile-seconds 10").unwrap().unwrap();
        assert!(default.bloom);
        assert!(!default.temporal_bypass);
        assert!(default.surface.is_none());
    }
    #[test]
    fn route_position_depends_on_elapsed_time_not_update_count() {
        let mut clock = ProfileClock::default();
        for fps in [60, 120] {
            clock.route_seconds = (2 * fps) as f64 / fps as f64;
            assert_eq!(clock.reference_frame(), 540);
        }
    }
}
