//! Validated game launch configuration. Process arguments are read only by main.
mod flags;

use bevy::prelude::*;
pub(crate) use flags::Args;
use std::{ffi::OsString, path::PathBuf};
use vegetation_render::VegetationDensityMode;

#[derive(Resource, Clone, Default)]
pub(crate) struct LaunchOptions {
    pub help: bool,
    pub mode: RunMode,
    pub world_db: Option<PathBuf>,
    pub start_view: Option<PathBuf>,
    pub story: Option<PathBuf>,
    pub fps: u32,
    pub upscaler: upscaling::UpscaleMethod,
    pub clouds: engine::CloudQuality,
    pub density: VegetationDensityMode,
    pub weather: engine::WeatherStart,
    /// Day phase to start at instead of the authored time, 0..1.
    pub time: Option<f32>,
    /// Whether the time of day passes.
    pub day_clock: bool,
    pub canopy_path: Option<PathBuf>,
    pub diagnostics: DiagnosticsMode,
    pub panel_open: bool,
    pub audit_log: bool,
    #[cfg_attr(not(target_os = "ios"), allow(dead_code))]
    pub console: bool,
    pub timing_log: bool,
    pub gpu_detail: bool,
    pub gpu_off: bool,
    pub metalfx_timing: bool,
    pub counters: bool,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub input_trace: bool,
    pub debug_world_switch: bool,
    /// How many LODs coarser than the drawn one instanced trees cast shadows from.
    pub tree_shadow_lod: usize,
    pub timer_pacing: bool,
    pub terrain_near_off: bool,
    pub vertex_reference: bool,
    pub placement_reference: bool,
    pub candidate_reference: bool,
    pub prepared_blades: Option<u64>,
    pub msaa_store_reference: bool,
    pub temporal_standard_output: bool,
    pub metal_capture: Option<PathBuf>,
    pub profile: Option<crate::profile::ProfileSettings>,
    pub repro: Option<ReproOptions>,
    /// Render scale at launch (one of `RESOLUTION_SCALES`).
    pub resolution_scale: Option<f32>,
    pub lod_lab: Option<LodLabOptions>,
    pub look_capture: Option<crate::look_capture::LookCaptureOptions>,
}

/// `--lod-lab`: one tree's representations compared in place ([`crate::lod_lab`]).
#[derive(Clone, Debug)]
pub(crate) struct LodLabOptions {
    /// Catalog key (`pack/asset`) of the tree under study.
    pub asset: String,
    /// Neighbours around it, as `(asset, count, spacing in metres)`.
    pub stand: Option<(String, usize, f32)>,
    /// Captures every switch and the fixed distances there, then exits.
    pub capture: Option<PathBuf>,
    /// Saves one window screenshot (panel included) once the trees have drawn, then exits.
    pub screenshot: Option<PathBuf>,
    /// Camera bearings from the sun's, in degrees: 0 has the sun behind the camera.
    pub yaws: Vec<f32>,
    pub pitch: f32,
    /// The tree's scale: smaller trees switch nearer.
    pub scale: f32,
    pub distances: Vec<f32>,
    pub settle: u32,
}

/// Who holds the camera and decides when the run ends, chosen once from the flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RunMode {
    /// The player plays; a Metal capture may still record a frame.
    #[default]
    Play,
    /// A scripted camera route, optionally timed by a profile.
    Repro,
    /// A timed or diagnostic profile from the start view.
    Profile,
    LodLab,
    LookCapture,
    /// The demo-world streaming regression.
    Smoke,
}

/// Startup composition; Full preserves the existing instrumentation by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DiagnosticsMode {
    Off,
    Panel,
    #[default]
    Full,
}

#[derive(Clone)]
pub(crate) struct ReproOptions {
    pub route: crate::repro::Route,
    pub prepass: bool,
    pub hide_ui: bool,
    pub frame_clock: bool,
    pub temporal_view: upscaling::temporal::TemporalDebug,
    pub frames: Option<u32>,
    pub snapshot: Option<PathBuf>,
    pub snapshot_frames: Vec<u32>,
}

impl LaunchOptions {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let args = Args::collect(args)?;
        if args.has("--help") {
            return Ok(Self {
                help: true,
                ..default()
            });
        }
        let diagnostics = args.choice(
            "--diagnostics",
            "full",
            &[
                ("off", DiagnosticsMode::Off),
                ("panel", DiagnosticsMode::Panel),
                ("full", DiagnosticsMode::Full),
            ],
        )?;
        args.check_needs(diagnostics)?;
        let mode = RunMode::of(&args)?;
        if args.has("--gpu-timing-off") && args.has("--gpu-timing-detail") {
            return Err("GPU timing off conflicts with detailed GPU timing".into());
        }
        if args.has("--trace-camera-input") && !cfg!(target_os = "macos") {
            return Err("Camera input tracing requires macOS".into());
        }
        let resolution_scale = resolution_scale(&args)?;
        let upscaler = args.choice(
            "--upscaler",
            "auto",
            &[
                ("auto", upscaling::UpscaleMethod::Auto),
                ("linear", upscaling::UpscaleMethod::Linear),
                ("metalfx-spatial", upscaling::UpscaleMethod::MetalFxSpatial),
                (
                    "metalfx-temporal",
                    upscaling::UpscaleMethod::MetalFxTemporal,
                ),
            ],
        )?;
        let profile = crate::profile::ProfileSettings::parse(&args, resolution_scale)?;
        if profile.is_some() && args.has("--fps") && !args.has("--profile-native-pacing") {
            return Err(
                "Use --profile-fps for a profile, or --profile-native-pacing to preserve --fps"
                    .into(),
            );
        }
        if args.has("--profile-temporal-bypass")
            && upscaler != upscaling::UpscaleMethod::MetalFxTemporal
        {
            return Err("--profile-temporal-bypass requires --upscaler metalfx-temporal".into());
        }
        let repro = ReproOptions::parse(&args)?;
        // Measurements and regressions must not change weather or time unless asked explicitly.
        let measured = mode != RunMode::Play || args.has("--metal-capture");
        Ok(Self {
            help: false,
            mode,
            world_db: args.path("--world-db"),
            start_view: args.path("--start-view"),
            story: args.path("--story"),
            fps: fps(&args)?,
            upscaler,
            clouds: args.choice(
                "--cloud-quality",
                "balanced",
                &[
                    ("off", engine::CloudQuality::Off),
                    ("balanced", engine::CloudQuality::Balanced),
                    ("high", engine::CloudQuality::High),
                ],
            )?,
            density: args.choice(
                "--grass-density",
                "balanced",
                &[
                    ("balanced", VegetationDensityMode::Balanced),
                    ("full", VegetationDensityMode::FullReference),
                    ("authored", VegetationDensityMode::Authored),
                ],
            )?,
            weather: weather(&args)?.unwrap_or(if measured {
                engine::WeatherStart::Authored
            } else {
                engine::WeatherStart::Automatic
            }),
            time: time(&args)?,
            day_clock: args.choice(
                "--day-clock",
                if measured { "off" } else { "on" },
                &[("on", true), ("off", false)],
            )?,
            canopy_path: args.path("--canopy-look"),
            diagnostics,
            panel_open: args.has("--performance-open") || args.has("--render-audit"),
            audit_log: args.has("--render-audit") || repro.is_some(),
            console: args.has("--render-console") || repro.is_some(),
            timing_log: args.has("--timing-log"),
            gpu_detail: args.has("--gpu-timing-detail"),
            gpu_off: args.has("--gpu-timing-off"),
            metalfx_timing: args.has("--metalfx-timing-log"),
            counters: args.has("--grass-counters"),
            input_trace: args.has("--trace-camera-input"),
            debug_world_switch: args.has("--debug-world-switch"),
            tree_shadow_lod: args.number("--tree-shadow-lod", 0, 0..=2)?,
            timer_pacing: args.has("--frame-pacing-timer"),
            terrain_near_off: args.has("--terrain-near-off"),
            vertex_reference: args.has("--grass-vertex-reference"),
            placement_reference: args.has("--grass-placement-reference"),
            candidate_reference: args.has("--grass-candidate-reference"),
            prepared_blades: args
                .has("--grass-prepared-blades")
                .then(|| args.number("--grass-prepared-blades", 0, 32768..=524288))
                .transpose()?,
            msaa_store_reference: args.has("--msaa-store-reference"),
            temporal_standard_output: args.has("--temporal-standard-output"),
            metal_capture: metal_capture(&args)?,
            profile,
            repro,
            resolution_scale,
            lod_lab: LodLabOptions::parse(&args)?,
            look_capture: look_capture(&args)?,
        })
    }

    pub fn canopy_path(&self) -> PathBuf {
        self.canopy_path
            .clone()
            .unwrap_or_else(vegetation::CanopyShading::saved_path)
    }

    pub fn help() -> String {
        flags::help()
    }
}

impl RunMode {
    /// One owner at most for the camera and the end of the run; a profile times a repro's
    /// route but nothing else's.
    fn of(args: &Args) -> Result<Self, String> {
        let owners: Vec<_> = [
            ("--render-repro", Self::Repro),
            ("--lod-lab", Self::LodLab),
            ("--look-capture", Self::LookCapture),
            ("--streaming-smoke", Self::Smoke),
        ]
        .into_iter()
        .filter(|(flag, _)| args.has(flag))
        .collect();
        let profile = args.has("--profile-seconds") || args.has("--profile-diagnostic");
        match owners.as_slice() {
            [] if profile => Ok(Self::Profile),
            [] => Ok(Self::Play),
            [(_, Self::Repro)] => Ok(Self::Repro),
            [(flag, mode)] => {
                if profile {
                    return Err(format!(
                        "{flag} holds the camera and ends the run; it cannot share a profile"
                    ));
                }
                if args.has("--metal-capture") {
                    return Err(format!(
                        "{flag} holds the camera and ends the run; it cannot share a Metal capture"
                    ));
                }
                Ok(*mode)
            }
            [(first, _), (second, _), ..] => Err(format!(
                "{first} and {second} both hold the camera; choose one"
            )),
        }
    }
}

impl DiagnosticsMode {
    fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Panel => "panel",
            Self::Full => "full",
        }
    }
}

// Following a 120 Hz display kept the GPU busy whenever it had headroom, and loud.
fn fps(args: &Args) -> Result<u32, String> {
    args.value("--fps")?
        .unwrap_or("60")
        .parse::<u32>()
        .ok()
        .filter(|&fps| fps == 0 || (15..=240).contains(&fps))
        .ok_or_else(|| "--fps requires 0 or 15..240".into())
}

fn resolution_scale(args: &Args) -> Result<Option<f32>, String> {
    args.value("--resolution-scale")?
        .map(|v| match v {
            "1" | "1.0" => Ok(1.0),
            "0.75" => Ok(0.75),
            "0.5" => Ok(0.5),
            "0.33" => Ok(1.0 / 3.0),
            _ => Err("--resolution-scale requires 1, 0.75, 0.5 or 0.33".into()),
        })
        .transpose()
}

fn weather(args: &Args) -> Result<Option<engine::WeatherStart>, String> {
    args.value("--weather")?
        .map(|name| match name {
            "auto" => Ok(engine::WeatherStart::Automatic),
            "authored" => Ok(engine::WeatherStart::Authored),
            _ => engine::WeatherKind::ALL
                .into_iter()
                .find(|kind| kind.label().eq_ignore_ascii_case(name))
                .map(engine::WeatherStart::Manual)
                .ok_or_else(|| {
                    "--weather requires auto, authored, clear, scattered, overcast, rain or storm"
                        .to_string()
                }),
        })
        .transpose()
}

fn time(args: &Args) -> Result<Option<f32>, String> {
    args.value("--time")?
        .map(|v| {
            v.split_once(':')
                .and_then(|(h, m)| Some((h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
                .filter(|&(h, m)| h < 24 && m < 60 && v.len() <= 5)
                .map(|(h, m)| (h * 60 + m) as f32 / 1440.0)
                .ok_or_else(|| "--time requires HH:MM".into())
        })
        .transpose()
}

fn metal_capture(args: &Args) -> Result<Option<PathBuf>, String> {
    let Some(capture) = args.path("--metal-capture") else {
        return Ok(None);
    };
    if !cfg!(target_vendor = "apple") {
        return Err("Metal capture requires an Apple device".into());
    }
    if capture.extension().and_then(|s| s.to_str()) != Some("gputrace") {
        return Err("Metal capture requires a .gputrace path".into());
    }
    if !(capture.is_absolute() || cfg!(target_os = "ios") && capture.components().count() == 1) {
        return Err("Metal capture requires an absolute path (or a plain filename on iOS)".into());
    }
    if capture.is_absolute() && capture.exists() {
        return Err("Metal capture output already exists".into());
    }
    Ok(Some(capture))
}

fn look_capture(args: &Args) -> Result<Option<crate::look_capture::LookCaptureOptions>, String> {
    let Some(dir) = args.path("--look-capture") else {
        return Ok(None);
    };
    if !args.has("--start-view") {
        return Err("--look-capture shows the --start-view".into());
    }
    Ok(Some(crate::look_capture::LookCaptureOptions {
        dir,
        variants: crate::look_capture::parse_variants(
            args.value("--look-variants")?
                .ok_or("--look-capture needs --look-variants")?,
        )?,
        settle: args.number("--look-settle", 25.0, 5.0..=600.0)?,
    }))
}

impl ReproOptions {
    fn parse(args: &Args) -> Result<Option<Self>, String> {
        let Some(name) = args.value("--render-repro")? else {
            return Ok(None);
        };
        let route = crate::repro::Route::parse(name)?;
        if route.needs_start_view() && !args.has("--start-view") {
            return Err(format!("{name} requires --start-view"));
        }
        let frames = args
            .has("--render-frames")
            .then(|| args.number("--render-frames", 0, 900..=u32::MAX))
            .transpose()?;
        let snapshot_frames = args
            .value("--render-snapshot-frames")?
            .unwrap_or("600")
            .split(',')
            .map(|v| {
                v.parse::<u32>()
                    .map_err(|_| "Snapshot frames must be comma-separated integers")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if snapshot_frames
            .iter()
            .any(|&n| n < 300 || frames.is_some_and(|end| n >= end))
        {
            return Err("Snapshot frames must follow warmup (>=300) and precede exit".into());
        }
        Ok(Some(Self {
            route,
            prepass: args.has("--render-prepass"),
            hide_ui: args.has("--render-ui-off"),
            frame_clock: args.has("--render-frame-clock"),
            temporal_view: args.choice(
                "--render-temporal-view",
                "off",
                &[
                    ("off", upscaling::temporal::TemporalDebug::Off),
                    ("motion", upscaling::temporal::TemporalDebug::Motion),
                    ("depth", upscaling::temporal::TemporalDebug::Depth),
                ],
            )?,
            frames,
            snapshot: args.path("--render-snapshot"),
            snapshot_frames,
        }))
    }
}

impl LodLabOptions {
    fn parse(args: &Args) -> Result<Option<Self>, String> {
        let Some(asset) = args.value("--lod-lab")? else {
            return Ok(None);
        };
        if !args.has("--start-view") {
            return Err("--lod-lab places the tree at the --start-view focus".into());
        }
        if args.has("--lod-lab-capture") && args.has("--lod-lab-screenshot") {
            return Err(
                "--lod-lab-screenshot shows the interactive lab; drop --lod-lab-capture".into(),
            );
        }
        let distances = args.list(
            "--lod-lab-distances",
            &[10., 25., 50., 100., 200., 400., 800.],
        )?;
        if distances.iter().any(|d| !(2.0..=4000.0).contains(d)) {
            return Err("--lod-lab-distances must lie within 2..4000 metres".into());
        }
        Ok(Some(Self {
            asset: asset.to_owned(),
            stand: args
                .value("--lod-lab-stand")?
                .map(|stand| -> Result<_, String> {
                    Ok((
                        stand.to_owned(),
                        args.number("--lod-lab-stand-count", 24, 1..=400)?,
                        args.number("--lod-lab-spacing", 6.0, 1.0..=50.0)?,
                    ))
                })
                .transpose()?,
            capture: args.path("--lod-lab-capture"),
            screenshot: args.path("--lod-lab-screenshot"),
            yaws: args.list("--lod-lab-yaws", &[0., 90., 180.])?,
            pitch: args.number("--lod-lab-pitch", 3.0, -10.0..=80.0)?,
            scale: args.number("--lod-lab-scale", 1.0, 0.25..=4.0)?,
            distances,
            settle: args.number("--lod-lab-settle", 30, 1..=600)?,
        }))
    }
}

#[cfg(test)]
mod tests;
