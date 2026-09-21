//! Validated game launch configuration. Process arguments are read only by main.
use bevy::prelude::*;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Resource, Clone, Default)]
pub(crate) struct LaunchOptions {
    pub help: bool,
    pub world_db: Option<PathBuf>,
    pub start_view: Option<PathBuf>,
    pub fps: u32,
    pub upscaler: upscaling::UpscaleMethod,
    pub clouds: engine::CloudQuality,
    pub density: vegetation_render::VegetationDensityMode,
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
    pub streaming_smoke: bool,
    pub debug_world_switch: bool,
    pub timer_pacing: bool,
    pub terrain_legacy: bool,
    pub terrain_reference: bool,
    pub terrain_procedural: bool,
    pub terrain_universal: bool,
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
}

/// Startup composition; Full preserves the existing instrumentation by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DiagnosticsMode {
    Off,
    Panel,
    #[default]
    Full,
}

#[derive(Clone)]
pub(crate) struct ReproOptions {
    pub name: String,
    pub prepass: bool,
    pub hide_ui: bool,
    pub frames: Option<u32>,
    pub snapshot: Option<PathBuf>,
    pub snapshot_frames: Vec<u32>,
}

// One registry drives accepted spelling, value requirements and --help.
const FLAGS: &[(&str, bool, &str)] = &[
    (
        "--help",
        false,
        "Show launch options without starting the renderer",
    ),
    ("--world-db", true, "FILE: cooked runtime database"),
    ("--start-view", true, "FILE: logical camera bookmark"),
    (
        "--fps",
        true,
        "0 or 15..240: gameplay cap; 0 follows display",
    ),
    (
        "--upscaler",
        true,
        "auto | linear | metalfx-spatial | metalfx-temporal",
    ),
    ("--cloud-quality", true, "off | balanced | high"),
    ("--grass-density", true, "balanced | full | authored"),
    (
        "--canopy-look",
        true,
        "FILE: canopy appearance, captured by F1",
    ),
    (
        "--diagnostics",
        true,
        "off | panel | full (default): omit diagnostics, F1/frame stats only, or F1 + CPU/GPU timings",
    ),
    ("--performance-open", false, "Open F1 at startup"),
    (
        "--render-audit",
        false,
        "Open F1 and enable structured audit logs",
    ),
    (
        "--render-console",
        false,
        "Also send iOS audit output to stderr",
    ),
    ("--timing-log", false, "Log CPU/render timing summaries"),
    (
        "--gpu-timing-detail",
        false,
        "Enable detailed GPU pass probes",
    ),
    (
        "--gpu-timing-off",
        false,
        "Disable GPU timestamp instrumentation",
    ),
    (
        "--metalfx-timing-log",
        false,
        "Log native Temporal command-buffer timing",
    ),
    (
        "--grass-counters",
        false,
        "Enable optional GPU grass statistics",
    ),
    (
        "--trace-camera-input",
        false,
        "macOS: bounded native input trace",
    ),
    (
        "--streaming-smoke",
        false,
        "Run demo-world traversal/residency regression and exit",
    ),
    (
        "--debug-world-switch",
        false,
        "Enable the demo Tab world-space switch",
    ),
    (
        "--frame-pacing-timer",
        false,
        "Compare timer pacing with native display pacing",
    ),
    (
        "--terrain-legacy",
        false,
        "Use the legacy nearby world renderer",
    ),
    (
        "--terrain-reference",
        false,
        "Use reference terrain material preparation",
    ),
    (
        "--terrain-procedural",
        false,
        "Disable stochastic lookup cache",
    ),
    (
        "--terrain-prepared-universal",
        false,
        "Prefer portable prepared textures over native ASTC",
    ),
    ("--terrain-near-off", false, "Disable hierarchy near detail"),
    (
        "--grass-vertex-reference",
        false,
        "Disable prepared blade deformation",
    ),
    (
        "--grass-placement-reference",
        false,
        "Disable early candidate rejection",
    ),
    (
        "--grass-candidate-reference",
        false,
        "Disable source acceptance cache",
    ),
    (
        "--grass-prepared-blades",
        true,
        "32768..524288: preparation capacity experiment",
    ),
    (
        "--msaa-store-reference",
        false,
        "Preserve multisample color for comparison",
    ),
    (
        "--temporal-standard-output",
        false,
        "Compare standard Temporal tone-map output",
    ),
    (
        "--metal-capture",
        true,
        "PATH.gputrace: one native Apple GPU capture",
    ),
    (
        "--render-repro",
        true,
        "NAME: repeatable route (see docs/PERFORMANCE.md)",
    ),
    (
        "--render-frames",
        true,
        "N >= 900: stop repro at this frame",
    ),
    (
        "--render-snapshot",
        true,
        "PATH: screenshot output for a repro",
    ),
    (
        "--render-snapshot-frames",
        true,
        "N,N: capture frames after warmup and before exit",
    ),
    ("--render-prepass", false, "Enable depth prepass in a repro"),
    ("--render-ui-off", false, "Hide UI during a repro"),
    (
        "--profile-seconds",
        true,
        "2..3600: timed measurement duration",
    ),
    ("--profile-warmup", true, "1..600: warmup seconds"),
    (
        "--profile-size",
        true,
        "game | WIDTHxHEIGHT: internal pixels",
    ),
    (
        "--profile-surface",
        true,
        "WIDTHxHEIGHT: physical window pixels",
    ),
    ("--profile-window", true, "fullscreen | windowed"),
    (
        "--profile-fps",
        true,
        "0 or 15..240: profile deadline; 0 uncapped unless native pacing",
    ),
    (
        "--profile-native-pacing",
        false,
        "Keep gameplay pacing; profile FPS is the reference deadline",
    ),
    ("--profile-msaa", true, "1 | 2 | 4"),
    ("--profile-grass", true, "full | off"),
    ("--profile-bloom", true, "on | off"),
    (
        "--profile-temporal-bypass",
        false,
        "Bypass Temporal reconstruction for attribution",
    ),
    (
        "--profile-diagnostic",
        false,
        "Finite capture presentation; requires capture/frame limit",
    ),
];

impl LaunchOptions {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut values = BTreeMap::new();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let name = arg.to_str().ok_or("Option names must be UTF-8")?;
            let name = if name == "-h" { "--help" } else { name };
            let Some(&(key, takes_value, _)) = FLAGS.iter().find(|f| f.0 == name) else {
                let reason = match name {
                    "--grass-field-baseline" => {
                        "The previous-catalog experiment was removed; use the published catalog."
                    }
                    "--grass-bands" => {
                        "The standalone blade-band experiment controls were removed."
                    }
                    "--vegetation-v2-debug" => "Use F1 Advanced for terrain page gizmos.",
                    "--frame-pacing-display-only" => {
                        "The intermediate display-only pacing experiment was removed."
                    }
                    "--terrain-lod" | "--terrain-prepared" => {
                        "This path is already the default; omit the obsolete switch."
                    }
                    _ => "Use --help to list supported options.",
                };
                return Err(format!("Unknown or retired option {name:?}. {reason}"));
            };
            let value = if takes_value {
                let value = args
                    .next()
                    .ok_or_else(|| format!("{key} requires a value"))?;
                if value.to_string_lossy().starts_with("--") || value.is_empty() {
                    return Err(format!("{key} requires a value"));
                }
                value
            } else {
                OsString::new()
            };
            if values.insert(key, value).is_some() {
                return Err(format!("Duplicate option {key}"));
            }
        }
        if values.contains_key("--help") {
            return Ok(Self {
                help: true,
                ..default()
            });
        }
        let has = |key| values.contains_key(key);
        let path = |key| values.get(key).map(PathBuf::from);
        let value = |key| -> Result<Option<&str>, String> {
            values
                .get(key)
                .map(|s| {
                    s.to_str()
                        .ok_or_else(|| format!("{key} requires a UTF-8 value"))
                })
                .transpose()
        };
        let fps = value("--fps")?
            .unwrap_or("0")
            .parse::<u32>()
            .map_err(|_| "--fps requires 0 or 15..240")?;
        if fps != 0 && !(15..=240).contains(&fps) {
            return Err("--fps requires 0 or 15..240".into());
        }
        let upscaler = match value("--upscaler")?.unwrap_or("auto") {
            "auto" => upscaling::UpscaleMethod::Auto,
            "linear" => upscaling::UpscaleMethod::Linear,
            "metalfx-spatial" => upscaling::UpscaleMethod::MetalFxSpatial,
            "metalfx-temporal" => upscaling::UpscaleMethod::MetalFxTemporal,
            _ => {
                return Err(
                    "--upscaler requires auto, linear, metalfx-spatial or metalfx-temporal".into(),
                );
            }
        };
        let clouds = match value("--cloud-quality")?.unwrap_or("balanced") {
            "off" => engine::CloudQuality::Off,
            "balanced" => engine::CloudQuality::Balanced,
            "high" => engine::CloudQuality::High,
            _ => return Err("--cloud-quality requires off, balanced or high".into()),
        };
        let density = match value("--grass-density")?.unwrap_or("balanced") {
            "balanced" => vegetation_render::VegetationDensityMode::Balanced,
            "full" => vegetation_render::VegetationDensityMode::FullReference,
            "authored" => vegetation_render::VegetationDensityMode::Authored,
            _ => return Err("--grass-density requires balanced, full or authored".into()),
        };
        let diagnostics = match value("--diagnostics")?.unwrap_or("full") {
            "off" => DiagnosticsMode::Off,
            "panel" => DiagnosticsMode::Panel,
            "full" => DiagnosticsMode::Full,
            _ => return Err("--diagnostics requires off, panel or full".into()),
        };
        if diagnostics == DiagnosticsMode::Off {
            for flag in [
                "--performance-open",
                "--render-audit",
                "--grass-counters",
                "--metalfx-timing-log",
                "--trace-camera-input",
            ] {
                if has(flag) {
                    return Err(format!("{flag} conflicts with --diagnostics off"));
                }
            }
        }
        if diagnostics != DiagnosticsMode::Full {
            for flag in ["--timing-log", "--gpu-timing-detail", "--gpu-timing-off"] {
                if has(flag) {
                    return Err(format!("{flag} requires --diagnostics full"));
                }
            }
        }
        let prepared_blades = value("--grass-prepared-blades")?
            .map(|v| {
                v.parse::<u64>()
                    .ok()
                    .filter(|n| (32768..=524288).contains(n))
                    .ok_or("--grass-prepared-blades requires 32768..524288")
            })
            .transpose()?;
        let profile_args: Vec<String> = values
            .iter()
            .flat_map(|(k, v)| {
                let mut pair = vec![k.to_string()];
                if !v.is_empty() {
                    pair.push(v.to_string_lossy().into_owned());
                }
                pair
            })
            .collect();
        let profile = crate::profile::ProfileSettings::parse(&profile_args)?;
        if profile.is_none() && values.keys().any(|k| k.starts_with("--profile-")) {
            return Err("Profile options require --profile-seconds or --profile-diagnostic".into());
        }
        if profile.is_some() && has("--fps") && !has("--profile-native-pacing") {
            return Err(
                "Use --profile-fps for a profile, or --profile-native-pacing to preserve --fps"
                    .into(),
            );
        }
        if has("--gpu-timing-off") && has("--gpu-timing-detail") {
            return Err("GPU timing off conflicts with detailed GPU timing".into());
        }
        if has("--profile-temporal-bypass") && upscaler != upscaling::UpscaleMethod::MetalFxTemporal
        {
            return Err("--profile-temporal-bypass requires --upscaler metalfx-temporal".into());
        }
        let repro = if let Some(name) = value("--render-repro")? {
            if !crate::repro::NAMES.contains(&name) {
                return Err(format!(
                    "Unknown repro {name:?}; expected {}",
                    crate::repro::NAMES.join(", ")
                ));
            }
            if name.starts_with("landscape") && !has("--start-view") {
                return Err("Landscape repro requires --start-view".into());
            }
            let frames = value("--render-frames")?
                .map(|v| {
                    v.parse::<u32>()
                        .map_err(|_| "--render-frames requires an integer")
                })
                .transpose()?;
            if frames.is_some_and(|n| n < 900) {
                return Err("--render-frames must be at least 900".into());
            }
            let snapshot_frames = value("--render-snapshot-frames")?
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
            if has("--render-snapshot-frames") && !has("--render-snapshot") {
                return Err("Snapshot frames require --render-snapshot".into());
            }
            Some(ReproOptions {
                name: name.into(),
                prepass: has("--render-prepass"),
                hide_ui: has("--render-ui-off"),
                frames,
                snapshot: path("--render-snapshot"),
                snapshot_frames,
            })
        } else {
            if [
                "--render-frames",
                "--render-snapshot",
                "--render-snapshot-frames",
                "--render-prepass",
                "--render-ui-off",
            ]
            .iter()
            .any(|k| has(k))
            {
                return Err(
                    "Frame/snapshot/prepass/UI repro options require --render-repro".into(),
                );
            }
            None
        };
        if has("--streaming-smoke")
            && (profile.is_some() || repro.is_some() || has("--metal-capture"))
        {
            return Err("Streaming smoke cannot share control/exit ownership with a profile, repro or capture".into());
        }
        if has("--trace-camera-input") && !cfg!(target_os = "macos") {
            return Err("Camera input tracing requires macOS".into());
        }
        if let Some(capture) = path("--metal-capture") {
            if !cfg!(target_vendor = "apple") {
                return Err("Metal capture requires an Apple device".into());
            }
            if capture.extension().and_then(|s| s.to_str()) != Some("gputrace") {
                return Err("Metal capture requires a .gputrace path".into());
            }
            if !(capture.is_absolute()
                || cfg!(target_os = "ios") && capture.components().count() == 1)
            {
                return Err(
                    "Metal capture requires an absolute path (or a plain filename on iOS)".into(),
                );
            }
            if capture.is_absolute() && capture.exists() {
                return Err("Metal capture output already exists".into());
            }
        }
        Ok(Self {
            help: false,
            world_db: path("--world-db"),
            start_view: path("--start-view"),
            fps,
            upscaler,
            clouds,
            density,
            canopy_path: path("--canopy-look"),
            diagnostics,
            panel_open: has("--performance-open") || has("--render-audit"),
            audit_log: has("--render-audit") || repro.is_some(),
            console: has("--render-console") || repro.is_some(),
            timing_log: has("--timing-log"),
            gpu_detail: has("--gpu-timing-detail"),
            gpu_off: has("--gpu-timing-off"),
            metalfx_timing: has("--metalfx-timing-log"),
            counters: has("--grass-counters"),
            input_trace: has("--trace-camera-input"),
            streaming_smoke: has("--streaming-smoke"),
            debug_world_switch: has("--debug-world-switch"),
            timer_pacing: has("--frame-pacing-timer"),
            terrain_legacy: has("--terrain-legacy"),
            terrain_reference: has("--terrain-reference"),
            terrain_procedural: has("--terrain-procedural"),
            terrain_universal: has("--terrain-prepared-universal"),
            terrain_near_off: has("--terrain-near-off"),
            vertex_reference: has("--grass-vertex-reference"),
            placement_reference: has("--grass-placement-reference"),
            candidate_reference: has("--grass-candidate-reference"),
            prepared_blades,
            msaa_store_reference: has("--msaa-store-reference"),
            temporal_standard_output: has("--temporal-standard-output"),
            metal_capture: path("--metal-capture"),
            profile,
            repro,
        })
    }

    pub fn canopy_path(&self) -> PathBuf {
        self.canopy_path.clone().unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/vegetation/canopy-look.ron")
        })
    }

    pub fn help() -> String {
        let mut help = String::from("Yarra game\nUsage: yarra-app-game [OPTIONS]\n\n");
        for (name, value, description) in FLAGS {
            help.push_str(&format!(
                "  {name}{}\n      {description}\n",
                if *value { " VALUE" } else { "" }
            ));
        }
        help.push_str("\nPrecedence: normal defaults, launch options, repro preset, profile presentation.\nProfile FPS owns pacing unless --profile-native-pacing is set.\nF1 Reset restores the effective launch configuration.\n");
        help
    }
}

#[cfg(test)]
mod tests;
