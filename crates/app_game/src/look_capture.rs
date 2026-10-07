//! Look captures (`--look-capture DIR`): the start view as played, once per presentation
//! variant (exposure, tonemapper, the strength of sun and sky light), so the variants can be
//! compared side by side (`tools/look_sheet.py`). The world loads once and only the
//! presentation changes between frames: the camera stays put, wind and weather stand still.
use crate::runtime_settings::RuntimeSettings;
use bevy::{
    core_pipeline::tonemapping::Tonemapping,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    window::{MonitorSelection, PrimaryWindow, WindowMode},
};
use engine::{AtmosphereState, FogTuning, ForestSkyOcclusion, WorldSun, WorldViewCamera};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// Frames and seconds each variant draws before its screenshot. Eye adaptation takes that long
/// to settle, also when a variant turns it off (the correction eases back to none).
const VARIANT_FRAMES: u32 = 30;
const VARIANT_SECONDS: f64 = 5.0;
/// Frames to wait past the settle time for terrain to reach full quality.
const QUALITY_FRAMES: u32 = 600;
const TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// One presentation to capture; `None` and 1.0 keep the game's own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LookVariant {
    /// Shown in the sheet; `row|column` places it in a grid.
    pub(crate) name: String,
    pub(crate) ev100: Option<f32>,
    pub(crate) tonemapping: Option<Tonemapping>,
    /// Scales the sky (ambient) light of every lighting phase.
    pub(crate) ambient: f32,
    /// Scales the sun of every lighting phase.
    pub(crate) sun: f32,
    /// Share of sky light the crowns above hold back ([`ForestSkyOcclusion`]).
    pub(crate) canopy: Option<f32>,
    /// Eye adaptation; the game's setting when `None`.
    pub(crate) auto_exposure: Option<bool>,
    /// Ground haze and valley mist; the game's setting when `None`.
    pub(crate) fog: Option<bool>,
    /// Time of day as a phase of the day, 0..1; the world's start time when `None`.
    pub(crate) phase: Option<f32>,
    /// Scales of haze and mist extinction and of mist depth ([`FogTuning`]).
    pub(crate) tuning: FogTuning,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LookCaptureOptions {
    pub(crate) dir: PathBuf,
    pub(crate) variants: Vec<LookVariant>,
    /// Seconds the world draws before the first capture.
    pub(crate) settle: f32,
}

pub(crate) const TONEMAPPERS: [(&str, Tonemapping); 8] = [
    ("tony", Tonemapping::TonyMcMapface),
    ("agx", Tonemapping::AgX),
    ("neutral", Tonemapping::KhronosPbrNeutral),
    ("filmic", Tonemapping::BlenderFilmic),
    ("aces", Tonemapping::AcesFitted),
    ("boring", Tonemapping::SomewhatBoringDisplayTransform),
    ("reinhard", Tonemapping::ReinhardLuminance),
    ("none", Tonemapping::None),
];

/// `NAME[:key=value,...]` separated by `;`, keys `ev`, `tone`, `ambient`, `sun`, `canopy`,
/// `auto`, `fog`, `phase`, `haze`, `mist` and `depth`.
pub(crate) fn parse_variants(spec: &str) -> Result<Vec<LookVariant>, String> {
    let variants = spec
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|item| {
            let (name, settings) = item.split_once(':').unwrap_or((item, ""));
            let mut variant = LookVariant {
                name: name.trim().to_owned(),
                ev100: None,
                tonemapping: None,
                ambient: 1.0,
                sun: 1.0,
                canopy: None,
                auto_exposure: None,
                fog: None,
                phase: None,
                tuning: FogTuning::default(),
            };
            for setting in settings.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let (key, value) = setting.split_once('=').ok_or_else(|| {
                    format!("look variant {name:?}: {setting:?} is not key=value")
                })?;
                let number = |range: std::ops::RangeInclusive<f32>| {
                    value
                        .parse::<f32>()
                        .ok()
                        .filter(|v| range.contains(v))
                        .ok_or_else(|| {
                            format!(
                                "look variant {name:?}: {key} requires {}..{}",
                                range.start(),
                                range.end()
                            )
                        })
                };
                match key.trim() {
                    "ev" => variant.ev100 = Some(number(4.0..=18.0)?),
                    "ambient" => variant.ambient = number(0.05..=20.0)?,
                    "sun" => variant.sun = number(0.05..=20.0)?,
                    "canopy" => variant.canopy = Some(number(0.0..=1.0)?),
                    "auto" | "fog" => {
                        let on = match value {
                            "1" | "on" => true,
                            "0" | "off" => false,
                            _ => {
                                return Err(format!(
                                    "look variant {name:?}: {key} requires on or off"
                                ));
                            }
                        };
                        if key.trim() == "auto" {
                            variant.auto_exposure = Some(on);
                        } else {
                            variant.fog = Some(on);
                        }
                    }
                    "phase" => variant.phase = Some(number(0.0..=0.999)?),
                    "haze" => variant.tuning.haze = number(0.0..=20.0)?,
                    "mist" => variant.tuning.mist = number(0.0..=20.0)?,
                    "depth" => variant.tuning.mist_depth = number(0.1..=10.0)?,
                    "tone" => {
                        variant.tonemapping = Some(
                            TONEMAPPERS
                                .iter()
                                .find(|(label, _)| *label == value)
                                .map(|(_, tone)| *tone)
                                .ok_or_else(|| {
                                    format!(
                                        "look variant {name:?}: tone requires {}",
                                        TONEMAPPERS.map(|(label, _)| label).join(", ")
                                    )
                                })?,
                        );
                    }
                    other => return Err(format!("look variant {name:?}: unknown key {other:?}")),
                }
            }
            if variant.name.is_empty() {
                return Err("look variants need names".into());
            }
            Ok(variant)
        })
        .collect::<Result<Vec<_>, String>>()?;
    if variants.is_empty() {
        return Err("--look-variants lists no variants".into());
    }
    Ok(variants)
}

#[derive(Resource)]
struct Capture {
    options: LookCaptureOptions,
    started: Instant,
    /// Frames since the world began drawing, until the first variant.
    warm: u32,
    /// The variant drawing, frames it has drawn and when it started.
    current: Option<(usize, u32, f64)>,
    awaiting: Arc<AtomicBool>,
    saving: Arc<AtomicUsize>,
    failed: Arc<AtomicBool>,
    frames: Vec<Value>,
    done: bool,
}

pub(crate) fn install(app: &mut App) {
    let Some(options) = app
        .world()
        .resource::<crate::launch::LaunchOptions>()
        .look_capture
        .clone()
    else {
        return;
    };
    {
        let mut settings = app.world_mut().resource_mut::<RuntimeSettings>();
        settings.controls_locked = true;
        settings.show_ui = false;
    }
    app.insert_resource(Capture {
        options,
        started: Instant::now(),
        warm: 0,
        current: None,
        awaiting: Arc::default(),
        saving: Arc::default(),
        failed: Arc::default(),
        frames: Vec::new(),
        done: false,
    })
    .add_systems(Startup, fullscreen)
    .add_systems(Update, run)
    .add_systems(
        PostUpdate,
        (
            (freeze_wind.before(engine::TreeWindSystems), present).before(engine::ApplyAtmosphere),
            scale_lights.after(engine::ApplyAtmosphere),
        ),
    );
}

/// Captures show the game as played: fullscreen at the window's native resolution.
fn fullscreen(mut window: Single<&mut Window, With<PrimaryWindow>>) {
    window.mode = WindowMode::BorderlessFullscreen(MonitorSelection::Primary);
}

/// Still wind, so variants differ only in their presentation.
fn freeze_wind(mut wind: ResMut<vegetation_render::VegetationWind>) {
    wind.set_phase_seconds(0.0);
}

/// Holds the current variant's exposure and tonemapper, every frame in case anything else
/// writes them.
#[allow(clippy::too_many_arguments)]
fn present(
    mut commands: Commands,
    capture: Res<Capture>,
    mut atmosphere: ResMut<AtmosphereState>,
    mut occlusion: ResMut<ForestSkyOcclusion>,
    (mut presentation, mut tuning): (ResMut<engine::AtmospherePresentation>, ResMut<FogTuning>),
    settings: Res<RuntimeSettings>,
    cameras: Query<(Entity, Option<&Tonemapping>), With<WorldViewCamera>>,
) {
    let Some((index, ..)) = capture.current else {
        return;
    };
    let variant = &capture.options.variants[index];
    atmosphere.exposure_override = variant.ev100;
    let phase = variant.phase.unwrap_or(atmosphere.profile.initial_phase);
    if atmosphere.phase != phase {
        atmosphere.phase = phase;
    }
    let fog = variant.fog.unwrap_or(settings.fog);
    if presentation.low_air != fog {
        presentation.low_air = fog;
    }
    tuning.set_if_neq(variant.tuning);
    occlusion.set_if_neq(ForestSkyOcclusion(
        variant.canopy.unwrap_or(ForestSkyOcclusion::default().0),
    ));
    let adapt = variant.auto_exposure.unwrap_or(settings.auto_exposure);
    if presentation.auto_exposure != adapt {
        presentation.auto_exposure = adapt;
    }
    let tone = variant.tonemapping.unwrap_or(engine::WORLD_TONEMAPPING);
    for (entity, current) in &cameras {
        if current != Some(&tone) {
            commands.entity(entity).insert(tone);
        }
    }
}

/// Scales the sun and sky light the atmosphere just set from the world's profile (which the
/// game restores whenever the state's copy differs, so variants cannot edit it).
fn scale_lights(
    capture: Res<Capture>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut sun: Query<&mut DirectionalLight, With<WorldSun>>,
) {
    let Some((index, ..)) = capture.current else {
        return;
    };
    let variant = &capture.options.variants[index];
    ambient.brightness *= variant.ambient;
    for mut light in &mut sun {
        light.illuminance *= variant.sun;
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    mut commands: Commands,
    mut capture: ResMut<Capture>,
    atmosphere: Res<AtmosphereState>,
    terrain: Res<engine::TerrainLodStats>,
    window: Single<&Window, With<PrimaryWindow>>,
    sun: Option<Single<&GlobalTransform, With<WorldSun>>>,
    time: Res<Time<Real>>,
    settings: Res<RuntimeSettings>,
    mut exit: MessageWriter<AppExit>,
) {
    if capture.started.elapsed() > TIMEOUT || capture.failed.load(Ordering::Acquire) {
        error!("LOOK capture failed or timed out");
        exit.write(AppExit::error());
        return;
    }
    if capture.done {
        if !capture.awaiting.load(Ordering::Acquire) && capture.saving.load(Ordering::Acquire) == 0
        {
            exit.write(AppExit::Success);
        }
        return;
    }
    if capture.awaiting.load(Ordering::Acquire) {
        return;
    }
    let now = time.elapsed_secs_f64();
    let Some((index, drawn, started)) = capture.current else {
        // Wait for the world to settle at full terrain quality, then start.
        capture.warm += 1;
        let settled = time.elapsed_secs() >= capture.options.settle;
        let quality = !terrain.quality_pending || capture.warm > QUALITY_FRAMES;
        if settled && quality {
            if let Err(e) = std::fs::create_dir_all(&capture.options.dir) {
                error!("LOOK cannot create {}: {e}", capture.options.dir.display());
                exit.write(AppExit::error());
                return;
            }
            info!(
                "LOOK capturing {} variants into {}",
                capture.options.variants.len(),
                capture.options.dir.display()
            );
            capture.current = Some((0, 0, now));
        }
        return;
    };
    let adapting = capture.options.variants[index]
        .auto_exposure
        .unwrap_or(settings.auto_exposure);
    if drawn < VARIANT_FRAMES || now - started < VARIANT_SECONDS {
        capture.current = Some((index, drawn.min(VARIANT_FRAMES - 1) + 1, started));
        return;
    }
    if drawn > VARIANT_FRAMES {
        // Its screenshot is taken: the next variant may draw.
        if index + 1 < capture.options.variants.len() {
            capture.current = Some((index + 1, 0, now));
        } else {
            capture.done = true;
        }
        return;
    }
    capture.current = Some((index, drawn + 1, started));
    let variant = capture.options.variants[index].clone();
    let file = format!(
        "{index:02}-{}.png",
        variant
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '-'
            })
            .collect::<String>()
    );
    capture.frames.push(json!({
        "file": file,
        "name": variant.name,
        "ev100": variant.ev100.unwrap_or(atmosphere.profile.exposure_ev100),
        "tonemapping": format!("{:?}", variant.tonemapping.unwrap_or(engine::WORLD_TONEMAPPING)),
        "ambient": variant.ambient,
        "sun": variant.sun,
        "canopy": variant.canopy.unwrap_or(ForestSkyOcclusion::default().0),
        "auto_exposure": adapting,
        "fog": variant.fog.unwrap_or(settings.fog),
        "phase": variant.phase.unwrap_or(atmosphere.profile.initial_phase),
        "haze": variant.tuning.haze,
        "mist": variant.tuning.mist,
        "depth": variant.tuning.mist_depth,
    }));
    let path = capture.options.dir.join(&file);
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
                    .and_then(|image| image.to_rgb8().save(&path).map_err(|e| e.to_string()));
                if let Err(e) = result {
                    error!("LOOK cannot save {}: {e}", path.display());
                    failed.store(true, Ordering::Release);
                }
                saving.fetch_sub(1, Ordering::AcqRel);
            });
        },
    );
    if index + 1 < capture.options.variants.len() {
        return;
    }
    let phases = &atmosphere.profile.phases;
    let to_sun = sun.map(|s| s.back());
    let manifest = json!({
        "window": {
            "physical": [window.physical_width(), window.physical_height()],
            "scale_factor": window.scale_factor(),
        },
        "authored": {
            "ev100": atmosphere.profile.exposure_ev100,
            "phases": phases.iter().map(|p| json!({"sun_lux": p.sun_lux, "ambient_lux": p.ambient_lux})).collect::<Vec<_>>(),
        },
        "sun_elevation_degrees": to_sun.map(|d| d.y.asin().to_degrees()),
        "frames": capture.frames,
    });
    let manifest_path = capture.options.dir.join("manifest.json");
    if let Err(e) = std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    ) {
        error!("LOOK cannot write {}: {e}", manifest_path.display());
        exit.write(AppExit::error());
        return;
    }
    info!(
        "LOOK wrote {} frames and {}",
        capture.frames.len(),
        manifest_path.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_parse_names_and_settings() {
        let variants =
            parse_variants("tony|ev13; agx|ev14:tone=agx,ev=14 ;bright:ambient=2,sun=0.8").unwrap();
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0].name, "tony|ev13");
        assert_eq!(variants[0].ev100, None);
        assert_eq!(variants[1].tonemapping, Some(Tonemapping::AgX));
        assert_eq!(variants[1].ev100, Some(14.0));
        assert_eq!((variants[2].ambient, variants[2].sun), (2.0, 0.8));
        let fog = parse_variants("dawn:phase=0.27,fog=off,mist=2,depth=0.5").unwrap();
        assert_eq!(fog[0].phase, Some(0.27));
        assert_eq!(fog[0].fog, Some(false));
        assert_eq!((fog[0].tuning.mist, fog[0].tuning.mist_depth), (2.0, 0.5));
        assert!(parse_variants("x:fog=maybe").is_err());
        assert!(parse_variants("x:tone=sepia").is_err());
        assert!(parse_variants("x:ev=40").is_err());
        assert!(parse_variants(";").is_err());
    }
}
