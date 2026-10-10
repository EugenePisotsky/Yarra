//! The launch option registry: spelling, value syntax, help group and requirement of every
//! flag in one table, which also generates `--help` and the launch table in docs/PERFORMANCE.md.
use super::DiagnosticsMode;
use std::{collections::BTreeMap, ffi::OsString, fmt::Display, ops::RangeInclusive, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {
    Help,
    Scene,
    Diagnostics,
    Repro,
    Captures,
    Profile,
    Look,
    LodLab,
    GrassReference,
    Reference,
    Input,
}

impl Group {
    const ALL: [Self; 11] = [
        Self::Help,
        Self::Scene,
        Self::Diagnostics,
        Self::Repro,
        Self::Captures,
        Self::Profile,
        Self::Look,
        Self::LodLab,
        Self::GrassReference,
        Self::Reference,
        Self::Input,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Help => "Help",
            Self::Scene => "Scene and presentation",
            Self::Diagnostics => "Panel and logging",
            Self::Repro => "Scripted routes",
            Self::Captures => "Captures and checks",
            Self::Profile => "Timed profile",
            Self::Look => "Look captures",
            Self::LodLab => "LOD lab",
            Self::GrassReference => "Grass references",
            Self::Reference => "Terrain, output and pacing references",
            Self::Input => "Input and demo controls",
        }
    }
}

/// What a flag needs besides itself.
#[derive(Clone, Copy)]
pub(crate) enum Needs {
    Nothing,
    Flag(&'static str),
    /// A timed or diagnostic profile.
    Profile,
    /// At least this diagnostics composition.
    Diagnostics(DiagnosticsMode),
}

pub(crate) struct Flag {
    name: &'static str,
    /// Value syntax; empty for a switch.
    value: &'static str,
    group: Group,
    needs: Needs,
    help: &'static str,
}

const fn flag(name: &'static str, value: &'static str, group: Group, help: &'static str) -> Flag {
    Flag {
        name,
        value,
        group,
        needs: Needs::Nothing,
        help,
    }
}

impl Flag {
    const fn needs(self, needs: Needs) -> Self {
        Self { needs, ..self }
    }

    fn usage(&self) -> String {
        if self.value.is_empty() {
            self.name.into()
        } else {
            format!("{} {}", self.name, self.value)
        }
    }
}

const REPRO: Needs = Needs::Flag("--render-repro");
const PANEL: Needs = Needs::Diagnostics(DiagnosticsMode::Panel);
const FULL: Needs = Needs::Diagnostics(DiagnosticsMode::Full);
const LAB: Needs = Needs::Flag("--lod-lab");
const LOOK: Needs = Needs::Flag("--look-capture");
use Group::{
    Captures, Diagnostics, GrassReference, Help, Input, LodLab, Look, Profile, Reference, Repro,
    Scene,
};

const FLAGS: &[Flag] = &[
    flag("--help", "", Help, "Show launch options without starting the renderer"),
    flag("--world-db", "FILE", Scene, "Cooked runtime database"),
    flag("--start-view", "FILE", Scene, "Logical camera bookmark"),
    flag(
        "--story",
        "DIR",
        Scene,
        "Authored gameplay project; adds a guard and gate near the start",
    ),
    flag(
        "--fps",
        "0|15..240",
        Scene,
        "Gameplay cap (default 60); 0 follows the display",
    ),
    flag(
        "--resolution-scale",
        "1|0.75|0.5|0.33",
        Scene,
        "Render scale at launch (F1 changes it later); in a profile, the scale of --profile-size game",
    ),
    flag(
        "--time",
        "HH:MM",
        Scene,
        "Start at this time of day instead of the authored one",
    ),
    flag(
        "--day-clock",
        "on|off",
        Scene,
        "Let the time of day pass; profiles, repros and captures default to off",
    ),
    flag(
        "--weather",
        "auto|authored|clear|scattered|overcast|rain|storm",
        Scene,
        "Start weather; profiles, repros and captures default to authored",
    ),
    flag(
        "--upscaler",
        "auto|linear|metalfx-spatial|metalfx-temporal",
        Scene,
        "World upscaler (default auto)",
    ),
    flag(
        "--cloud-quality",
        "off|balanced|high",
        Scene,
        "Clouds (default balanced)",
    ),
    flag(
        "--grass-density",
        "balanced|full|authored",
        Scene,
        "Grass density (default balanced)",
    ),
    flag(
        "--tree-shadow-lod",
        "0..2",
        Scene,
        "LOD steps coarser that instanced trees cast shadows from (default 0)",
    ),
    flag(
        "--canopy-look",
        "FILE",
        Scene,
        "Canopy appearance, captured by F1",
    ),
    flag(
        "--diagnostics",
        "off|panel|full",
        Diagnostics,
        "Omit diagnostics, F1/frame stats only, or F1 + CPU/GPU timings (default full)",
    ),
    flag("--performance-open", "", Diagnostics, "Open F1 at startup").needs(PANEL),
    flag(
        "--render-audit",
        "",
        Diagnostics,
        "Open F1 and enable structured audit logs",
    )
    .needs(PANEL),
    flag(
        "--render-console",
        "",
        Diagnostics,
        "Also send iOS audit output to stderr",
    ),
    flag(
        "--timing-log",
        "",
        Diagnostics,
        "Log CPU/render timing summaries",
    )
    .needs(FULL),
    flag(
        "--gpu-timing-detail",
        "",
        Diagnostics,
        "Enable detailed GPU pass probes",
    )
    .needs(FULL),
    flag(
        "--gpu-timing-off",
        "",
        Diagnostics,
        "Disable GPU timestamp instrumentation",
    )
    .needs(FULL),
    flag(
        "--metalfx-timing-log",
        "",
        Diagnostics,
        "Log native Temporal command-buffer timing",
    )
    .needs(PANEL),
    flag(
        "--grass-counters",
        "",
        Diagnostics,
        "Enable optional GPU grass statistics",
    )
    .needs(PANEL),
    flag(
        "--render-repro",
        "NAME",
        Repro,
        "Repeatable camera route (names in docs/PERFORMANCE.md)",
    ),
    flag(
        "--render-frames",
        "N",
        Repro,
        "Stop the route at this frame (at least 900)",
    )
    .needs(REPRO),
    flag("--render-snapshot", "PATH", Repro, "Screenshot output").needs(REPRO),
    flag(
        "--render-snapshot-frames",
        "N,N",
        Repro,
        "Screenshot frames after warmup and before exit (default 600)",
    )
    .needs(Needs::Flag("--render-snapshot")),
    flag("--render-prepass", "", Repro, "Enable the depth prepass").needs(REPRO),
    flag(
        "--render-frame-clock",
        "",
        Repro,
        "Advance the route by frame count at 60 fps, so captures match between builds",
    )
    .needs(REPRO),
    flag(
        "--render-temporal-view",
        "motion|depth",
        Repro,
        "Show MetalFX Temporal's motion or depth input instead of the image",
    )
    .needs(REPRO),
    flag("--render-ui-off", "", Repro, "Hide the UI").needs(REPRO),
    flag(
        "--metal-capture",
        "PATH.gputrace",
        Captures,
        "One native Apple GPU capture",
    ),
    flag(
        "--streaming-smoke",
        "",
        Captures,
        "Run the demo-world traversal/residency regression and exit",
    ),
    flag(
        "--profile-seconds",
        "2..3600",
        Profile,
        "Timed measurement duration",
    ),
    flag(
        "--profile-warmup",
        "1..600",
        Profile,
        "Warmup seconds (default 15)",
    )
    .needs(Needs::Profile),
    flag(
        "--profile-size",
        "game|WIDTHxHEIGHT",
        Profile,
        "Internal pixels (default 2560x1440)",
    )
    .needs(Needs::Profile),
    flag(
        "--profile-surface",
        "WIDTHxHEIGHT",
        Profile,
        "Physical window pixels",
    )
    .needs(Needs::Profile),
    flag(
        "--profile-window",
        "fullscreen|windowed",
        Profile,
        "Window mode (default windowed)",
    )
    .needs(Needs::Profile),
    flag(
        "--profile-fps",
        "0|15..240",
        Profile,
        "Profile deadline; 0 uncapped unless native pacing",
    )
    .needs(Needs::Profile),
    flag(
        "--profile-native-pacing",
        "",
        Profile,
        "Keep gameplay pacing; profile FPS is the reference deadline",
    )
    .needs(Needs::Profile),
    flag("--profile-msaa", "1|2|4", Profile, "MSAA samples (default 4)").needs(Needs::Profile),
    flag("--profile-grass", "full|off", Profile, "Grass").needs(Needs::Profile),
    flag("--profile-objects", "on|off", Profile, "Object draws").needs(Needs::Profile),
    flag("--profile-terrain", "on|off", Profile, "Terrain draws").needs(Needs::Profile),
    flag("--profile-bloom", "on|off", Profile, "Bloom").needs(Needs::Profile),
    flag(
        "--profile-auto-exposure",
        "on|off",
        Profile,
        "Auto exposure",
    )
    .needs(Needs::Profile),
    flag(
        "--profile-fog",
        "on|off",
        Profile,
        "Ground haze and valley mist",
    )
    .needs(Needs::Profile),
    flag(
        "--profile-particles",
        "on|off",
        Profile,
        "Ambient particles",
    )
    .needs(Needs::Profile),
    flag(
        "--profile-shafts",
        "on|off",
        Profile,
        "Light shafts and sun rays",
    )
    .needs(Needs::Profile),
    flag(
        "--profile-temporal-bypass",
        "",
        Profile,
        "Bypass Temporal reconstruction for attribution",
    )
    .needs(Needs::Profile),
    flag(
        "--profile-diagnostic",
        "",
        Profile,
        "Finite capture presentation; requires --metal-capture or --render-frames",
    ),
    flag(
        "--look-capture",
        "DIR",
        Look,
        "Screenshot the start view once per --look-variants presentation, then exit",
    ),
    flag(
        "--look-variants",
        "LIST",
        Look,
        "NAME[:ev=EV100,tone=tony|agx|neutral|filmic|aces|boring|reinhard|none,ambient=SCALE,sun=SCALE,canopy=0..1,auto=on|off];...",
    )
    .needs(LOOK),
    flag(
        "--look-settle",
        "SECONDS",
        Look,
        "World drawing time before the first look capture (default 25)",
    )
    .needs(LOOK),
    flag(
        "--lod-lab",
        "PACK/ASSET",
        LodLab,
        "Study one tree's LODs at the --start-view focus",
    ),
    flag(
        "--lod-lab-stand",
        "ASSET",
        LodLab,
        "Surround the tree with a stand of this asset",
    )
    .needs(LAB),
    flag(
        "--lod-lab-stand-count",
        "N",
        LodLab,
        "Trees in the stand (default 24)",
    )
    .needs(Needs::Flag("--lod-lab-stand")),
    flag(
        "--lod-lab-spacing",
        "METRES",
        LodLab,
        "Stand spacing (default 6)",
    )
    .needs(Needs::Flag("--lod-lab-stand")),
    flag(
        "--lod-lab-capture",
        "DIR",
        LodLab,
        "Capture every LOD switch from both sides and fixed distances, then exit",
    )
    .needs(LAB),
    flag(
        "--lod-lab-screenshot",
        "FILE",
        LodLab,
        "Save the window (panel included) once the trees have drawn, then exit",
    )
    .needs(LAB),
    flag(
        "--lod-lab-yaws",
        "DEGREES,...",
        LodLab,
        "Camera bearings from the sun's; 0 has the sun behind the camera (default 0,90,180)",
    )
    .needs(LAB),
    flag(
        "--lod-lab-scale",
        "0.25..4",
        LodLab,
        "The tree's scale (default 1); smaller trees switch nearer",
    )
    .needs(LAB),
    flag(
        "--lod-lab-pitch",
        "DEGREES",
        LodLab,
        "Camera elevation above the tree's root (default 3)",
    )
    .needs(LAB),
    flag(
        "--lod-lab-distances",
        "METRES,...",
        LodLab,
        "Fixed capture distances (default 10,25,50,100,200,400,800)",
    )
    .needs(LAB),
    flag(
        "--lod-lab-settle",
        "FRAMES",
        LodLab,
        "Frames drawn before each capture (default 30)",
    )
    .needs(LAB),
    flag(
        "--grass-vertex-reference",
        "",
        GrassReference,
        "Disable prepared blade deformation",
    ),
    flag(
        "--grass-placement-reference",
        "",
        GrassReference,
        "Disable early candidate rejection",
    ),
    flag(
        "--grass-candidate-reference",
        "",
        GrassReference,
        "Disable the source acceptance cache",
    ),
    flag(
        "--grass-prepared-blades",
        "32768..524288",
        GrassReference,
        "Blade preparation capacity",
    ),
    flag(
        "--terrain-legacy",
        "",
        Reference,
        "Use the legacy nearby world renderer",
    ),
    flag(
        "--terrain-reference",
        "",
        Reference,
        "Use reference terrain material preparation",
    ),
    flag(
        "--terrain-procedural",
        "",
        Reference,
        "Disable the stochastic lookup cache",
    ),
    flag(
        "--terrain-prepared-universal",
        "",
        Reference,
        "Prefer portable prepared textures over native ASTC",
    ),
    flag("--terrain-near-off", "", Reference, "Disable hierarchy near detail"),
    flag(
        "--msaa-store-reference",
        "",
        Reference,
        "Preserve multisample color for comparison",
    ),
    flag(
        "--temporal-standard-output",
        "",
        Reference,
        "Compare standard Temporal tone-map output",
    ),
    flag(
        "--frame-pacing-timer",
        "",
        Reference,
        "Compare timer pacing with native display pacing",
    ),
    flag(
        "--trace-camera-input",
        "",
        Input,
        "macOS: bounded native input trace",
    )
    .needs(PANEL),
    flag(
        "--debug-world-switch",
        "",
        Input,
        "Enable the demo Tab world-space switch",
    ),
];

/// Launch arguments matched against the registry: each flag at most once, with its value.
pub(crate) struct Args(BTreeMap<&'static str, OsString>);

impl Args {
    pub(crate) fn collect(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut values = BTreeMap::new();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let name = arg.to_str().ok_or("Option names must be UTF-8")?;
            let name = if name == "-h" { "--help" } else { name };
            let Some(flag) = FLAGS.iter().find(|f| f.name == name) else {
                return Err(format!(
                    "Unknown option {name:?}. Use --help to list supported options."
                ));
            };
            let value = if flag.value.is_empty() {
                OsString::new()
            } else {
                args.next()
                    .filter(|v| !v.is_empty() && !v.to_string_lossy().starts_with("--"))
                    .ok_or_else(|| format!("{} requires a value", flag.name))?
            };
            if values.insert(flag.name, value).is_some() {
                return Err(format!("Duplicate option {}", flag.name));
            }
        }
        Ok(Self(values))
    }

    /// Every flag given has what it needs.
    pub(crate) fn check_needs(&self, diagnostics: DiagnosticsMode) -> Result<(), String> {
        for flag in FLAGS.iter().filter(|f| self.has(f.name)) {
            let missing = match flag.needs {
                Needs::Nothing => None,
                Needs::Flag(other) => (!self.has(other)).then(|| other.to_string()),
                Needs::Profile => (!self.has("--profile-seconds")
                    && !self.has("--profile-diagnostic"))
                .then(|| "--profile-seconds or --profile-diagnostic".into()),
                Needs::Diagnostics(least) => {
                    (diagnostics < least).then(|| format!("--diagnostics {}", least.name()))
                }
            };
            if let Some(missing) = missing {
                return Err(format!("{} requires {missing}", flag.name));
            }
        }
        Ok(())
    }

    pub(crate) fn has(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub(crate) fn path(&self, key: &str) -> Option<PathBuf> {
        self.0.get(key).map(PathBuf::from)
    }

    pub(crate) fn value(&self, key: &str) -> Result<Option<&str>, String> {
        self.0
            .get(key)
            .map(|s| {
                s.to_str()
                    .ok_or_else(|| format!("{key} requires a UTF-8 value"))
            })
            .transpose()
    }

    /// One of the named values, or `default` when the flag is absent.
    pub(crate) fn choice<T: Copy>(
        &self,
        key: &str,
        default: &str,
        options: &[(&str, T)],
    ) -> Result<T, String> {
        let value = self.value(key)?.unwrap_or(default);
        options
            .iter()
            .find(|(name, _)| *name == value)
            .map(|(_, v)| *v)
            .ok_or_else(|| {
                let names: Vec<_> = options.iter().map(|(name, _)| *name).collect();
                format!("{key} requires {}", names.join(", "))
            })
    }

    /// `on | off`, on when absent.
    pub(crate) fn switch(&self, key: &str) -> Result<bool, String> {
        self.choice(key, "on", &[("on", true), ("off", false)])
    }

    pub(crate) fn number<T: std::str::FromStr + PartialOrd + Display + Copy>(
        &self,
        key: &str,
        default: T,
        range: RangeInclusive<T>,
    ) -> Result<T, String> {
        self.value(key)?.map_or(Ok(default), |v| {
            v.parse::<T>()
                .ok()
                .filter(|n| range.contains(n))
                .ok_or_else(|| format!("{key} requires {}..{}", range.start(), range.end()))
        })
    }

    pub(crate) fn list(&self, key: &str, default: &[f32]) -> Result<Vec<f32>, String> {
        self.value(key)?.map_or(Ok(default.to_vec()), |v| {
            v.split(',')
                .map(|n| n.trim().parse::<f32>().ok().filter(|n| n.is_finite()))
                .collect::<Option<Vec<_>>>()
                .filter(|l| !l.is_empty())
                .ok_or_else(|| format!("{key} requires comma-separated numbers"))
        })
    }
}

pub(crate) fn help() -> String {
    let mut help = String::from("Yarra game\nUsage: yarra-app-game [OPTIONS]\n");
    for group in Group::ALL {
        help.push_str(&format!("\n{}:\n", group.title()));
        for flag in FLAGS.iter().filter(|f| f.group == group) {
            help.push_str(&format!("  {}\n      {}\n", flag.usage(), flag.help));
        }
    }
    help.push_str("\nPrecedence: normal defaults, launch options, repro preset, profile presentation.\nProfile FPS owns pacing unless --profile-native-pacing is set.\nF1 Reset restores the effective launch configuration.\n");
    help
}

/// The launch table in docs/PERFORMANCE.md, one row per group (a test keeps them equal).
#[cfg(test)]
pub(crate) fn markdown_table() -> String {
    let mut table = String::from("| Purpose | Controls |\n| --- | --- |\n");
    for group in Group::ALL.into_iter().filter(|g| *g != Help) {
        let controls: Vec<_> = FLAGS
            .iter()
            .filter(|f| f.group == group)
            .map(|f| format!("`{}`", f.usage().replace('|', "\\|")))
            .collect();
        table.push_str(&format!(
            "| {} | {} |\n",
            group.title(),
            controls.join(", ")
        ));
    }
    table
}
