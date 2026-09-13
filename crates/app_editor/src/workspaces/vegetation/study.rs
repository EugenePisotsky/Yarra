//! Versioned, editor-only reproduction data. Never cooked into the game database.
use std::path::PathBuf;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use vegetation::{
    VegetationCatalog, VegetationScene, candidate_domain, fixtures::full_coverage_page,
};
use vegetation_render::{
    VegetationDebugSettings, VegetationLighting, VegetationShapeInspection, VegetationWind,
};

pub(super) const WIDTH: u32 = 1280;
pub(super) const HEIGHT: u32 = 720;
pub(super) const PATCH_SIZE: f32 = 4.0;
pub(super) const MAX_CANDIDATES: u32 = 65_536;
pub(super) const MAX_FIELD_CANDIDATES: u32 = 4_194_304;

pub(super) fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.editor/vegetation")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct StudyCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
    pub fov: f32,
}

impl Default for StudyCamera {
    fn default() -> Self {
        Self::preset("overhead").unwrap()
    }
}

impl StudyCamera {
    pub fn preset(name: &str) -> Result<Self, String> {
        let (pitch, distance) = match name {
            "low" => (17.0, 2.5),
            "overhead" => (58.0, 3.5),
            "top" => (89.0, 4.7),
            "scale" => (22.0, 4.4),
            "game-close" => (10.0, 4.0),
            _ => {
                return Err(format!(
                    "Unknown camera {name}; use low, overhead, top, scale, or game-close"
                ));
            }
        };
        Ok(Self {
            yaw: 0.0,
            pitch,
            distance,
            target: [
                0.0,
                match name {
                    "scale" => 0.6,
                    "game-close" => 0.9,
                    _ => 0.18,
                },
                0.0,
            ],
            fov: if name == "game-close" { 45.0 } else { 50.0 },
        })
    }

    pub fn transform(&self) -> Transform {
        let yaw = self.yaw.to_radians();
        let pitch = self.pitch.to_radians();
        let target = Vec3::from_array(self.target);
        let offset = Vec3::new(
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            yaw.cos() * pitch.cos(),
        );
        Transform::from_translation(target + offset * self.distance).looking_at(target, Vec3::Y)
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if ![self.yaw, self.pitch, self.distance, self.fov]
            .into_iter()
            .chain(self.target)
            .all(f32::is_finite)
            || !(3.0..=89.5).contains(&self.pitch)
            || !(0.3..=30.0).contains(&self.distance)
            || !(15.0..=100.0).contains(&self.fov)
            || self.target.iter().any(|v| v.abs() > 20.0)
        {
            return Err("Study camera is outside the bounded inspection stage".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct WindStudy {
    pub enabled: bool,
    pub time: f32,
    pub direction: [f32; 2],
    pub strength: f32,
    pub spatial_frequency: f32,
    pub speed: f32,
    pub gustiness: f32,
    pub flutter: f32,
}

impl From<VegetationWind> for WindStudy {
    fn from(w: VegetationWind) -> Self {
        Self {
            enabled: w.enabled,
            time: 0.0,
            direction: w.direction.to_array(),
            strength: w.strength,
            spatial_frequency: w.spatial_frequency,
            speed: w.speed,
            gustiness: w.gustiness,
            flutter: w.flutter,
        }
    }
}

impl WindStudy {
    pub fn apply(&self, w: &mut VegetationWind) {
        w.externally_driven = true;
        w.enabled = self.enabled;
        w.direction = Vec2::from_array(self.direction);
        w.strength = self.strength;
        w.spatial_frequency = self.spatial_frequency;
        w.speed = self.speed;
        w.gustiness = self.gustiness;
        w.flutter = self.flutter;
        w.set_phase_seconds(self.time);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct StudyDocument {
    pub version: u32,
    pub camera: StudyCamera,
    pub seed: u32,
    pub population: String,
    pub wind: WindStudy,
    pub settings: VegetationDebugSettings,
    pub lighting: VegetationLighting,
    pub catalog: VegetationCatalog,
    pub reference: Option<String>,
    #[serde(default)]
    pub comparison: super::comparison::ComparisonView,
    pub render_size: [u32; 2],
    pub patch_size: f32,
    pub msaa_samples: u32,
    pub exposure_ev100: f32,
    pub sun_rotation: [f32; 4],
    pub sun_color: [f32; 4],
    pub sun_illuminance: f32,
    pub ambient_color: [f32; 4],
    pub ambient_brightness: f32,
    pub ground_color: [f32; 3],
    pub show_character: bool,
    #[serde(default = "super::stage::legacy_ruler")]
    pub show_ruler: bool,
    #[serde(default)]
    pub character: super::stage::CharacterPlacement,
    #[serde(default)]
    pub ground: super::stage::GroundMode,
    #[serde(default)]
    pub edge: Option<super::stage::GrassEdge>,
    pub character_profile: String,
    pub build: String,
}

impl StudyDocument {
    pub fn validate(&self) -> Result<(), String> {
        if self.settings.profile_mode != vegetation_render::VegetationProfileMode::Full {
            return Err("Study replay requires the Full render workload. Select Full before saving or capturing.".into());
        }
        if ![1, 2].contains(&self.version)
            || ![[WIDTH, HEIGHT], [1280, 960]].contains(&self.render_size)
            || self.msaa_samples != 1
            || self.exposure_ev100 != 13.0
            || self.ground_color != super::GROUND_COLOR
            || self.character_profile != engine::DEFAULT_CHARACTER_PRESENTATION_ID
        {
            return Err("Unsupported study version or render configuration".into());
        }
        if self.version == 1
            && (self.patch_size != PATCH_SIZE
                || self.ground != super::stage::GroundMode::Neutral
                || self.character != super::stage::CharacterPlacement::default()
                || self.edge.is_some())
        {
            return Err("Version 1 studies require the original fixed stage".into());
        }
        if self.settings.shape_inspection != VegetationShapeInspection::Off
            && self.patch_size > 16.0
        {
            return Err("Shape comparison requires a 4 m or 16 m field; it uses high topology for every retained root.".into());
        }
        self.camera.validate()?;
        self.character.validate()?;
        super::stage::validate_field_size(self.patch_size)?;
        self.comparison.validate()?;
        let w = &self.wind;
        if ![
            w.time,
            w.strength,
            w.spatial_frequency,
            w.speed,
            w.gustiness,
            w.flutter,
        ]
        .into_iter()
        .chain(w.direction)
        .all(f32::is_finite)
            || !(0.0..4096.0).contains(&w.time)
            || !(0.0..=2.0).contains(&w.strength)
            || !(0.0..=4.0).contains(&w.spatial_frequency)
            || !(0.0..=10.0).contains(&w.speed)
            || !(0.0..=1.0).contains(&w.gustiness)
            || !(0.0..=1.0).contains(&w.flutter)
        {
            return Err("Invalid wind transport or response".into());
        }
        let scalars = [
            self.sun_illuminance,
            self.ambient_brightness,
            self.lighting.diffuse_strength,
            self.lighting.specular_strength,
            self.lighting.transmission_strength,
            self.lighting.received_shadow_strength,
        ];
        if !scalars
            .into_iter()
            .chain(self.sun_rotation)
            .chain(self.sun_color)
            .chain(self.ambient_color)
            .chain(self.ground_color)
            .all(f32::is_finite)
            || !(0.0..=150_000.0).contains(&self.sun_illuminance)
            || !(0.0..=20_000.0).contains(&self.ambient_brightness)
            || (Quat::from_array(self.sun_rotation).length() - 1.0).abs() > 0.001
        {
            return Err("Invalid study lighting".into());
        }
        let index = self
            .catalog
            .populations
            .iter()
            .position(|p| p.key == self.population)
            .ok_or("Study population is missing from catalog")?;
        bounded_field_with_edge(&self.catalog, index, self.seed, self.patch_size, self.edge)?;
        Ok(())
    }

    pub fn read(path: &std::path::Path) -> Result<Self, String> {
        if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 4 * 1024 * 1024 {
            return Err("Study metadata exceeds 4 MiB".into());
        }
        let value: Self = ron::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }

    pub fn write(&self, path: &std::path::Path) -> Result<(), String> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let data = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())?;
        let temp = path.with_extension("ron.writing");
        std::fs::write(&temp, data).map_err(|e| e.to_string())?;
        std::fs::rename(temp, path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
pub(super) fn bounded_scene(
    catalog: &VegetationCatalog,
    index: usize,
    seed: u32,
) -> Result<(VegetationScene, u32), String> {
    bounded_field(catalog, index, seed, PATCH_SIZE)
}

#[cfg(test)]
pub(super) fn bounded_field(
    catalog: &VegetationCatalog,
    index: usize,
    seed: u32,
    size: f32,
) -> Result<(VegetationScene, u32), String> {
    bounded_field_with_edge(catalog, index, seed, size, None)
}

pub(super) fn bounded_field_with_edge(
    catalog: &VegetationCatalog,
    index: usize,
    seed: u32,
    size: f32,
    edge: Option<super::stage::GrassEdge>,
) -> Result<(VegetationScene, u32), String> {
    super::stage::validate_field_size(size)?;
    if let Some(edge) = edge {
        edge.validate()?;
    }
    catalog.validate().map_err(|e| e.to_string())?;
    let mut catalog = catalog.clone();
    let population = catalog
        .populations
        .get_mut(index)
        .ok_or("No population selected")?;
    population.seed = seed;
    let page_size = size.min(16.0);
    let side = (size / page_size) as u32;
    // Large fields extend farther in the default viewing direction (-Z). Nested fields keep
    // the same 16 m pages and world-space candidate identities in their common area.
    let origin = [
        -size / 2.0,
        if size >= 64.0 {
            -size * 0.75
        } else {
            -size / 2.0
        },
    ];
    let mut pages = Vec::new();
    let mut candidates = 0u32;
    let limit = if size == PATCH_SIZE {
        MAX_CANDIDATES
    } else {
        MAX_FIELD_CANDIDATES
    };
    for z in 0..side {
        for x in 0..side {
            let mut page = full_coverage_page(
                [
                    origin[0] + x as f32 * page_size,
                    origin[1] + z as f32 * page_size,
                ],
                page_size,
                population.id,
            );
            candidates =
                candidates.saturating_add(candidate_domain(&page, population).candidate_count());
            if let Some(edge) = edge {
                edge.apply(&mut page);
            }
            pages.push(page);
        }
    }
    if candidates > limit {
        return Err(format!(
            "Field needs {candidates} candidates; study limit is {limit}. Reduce field size, density or attraction. Preview suspended; no silent thinning."
        ));
    }
    let scene = VegetationScene { catalog, pages };
    scene.validate().map_err(|e| e.to_string())?;
    Ok((scene, candidates))
}

#[derive(Default)]
pub(super) struct StudyLaunch {
    pub open: bool,
    pub references: Vec<PathBuf>,
    pub camera: Option<StudyCamera>,
    pub time: Option<f32>,
    pub capture: Option<PathBuf>,
    pub load: Option<StudyDocument>,
    pub exit: bool,
    pub character: bool,
    pub select_reference: Option<String>,
    pub zoom: Option<f32>,
    pub show_inspector: bool,
    pub show_picker: bool,
    pub field_size: Option<f32>,
    pub ground: Option<super::stage::GroundMode>,
    pub hide_character: bool,
    pub edge: Option<bool>,
    pub ruler: bool,
    pub play: bool,
    pub shape: Option<VegetationShapeInspection>,
    pub no_opening: bool,
    pub no_wind: bool,
}

impl StudyLaunch {
    pub fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| format!("Missing value after {arg}"))
            };
            match arg.as_str() {
                "--vegetation-study" => options.open = true,
                "--study-character" => options.character = true,
                "--study-no-character" => options.hide_character = true,
                "--study-ruler" => options.ruler = true,
                "--study-play" => options.play = true,
                "--study-no-wind" => options.no_wind = true,
                "--study-no-opening" => options.no_opening = true,
                "--study-shape" => {
                    options.shape = Some(match value()?.as_str() {
                        "production" => VegetationShapeInspection::Off,
                        "current" => VegetationShapeInspection::Current,
                        "full" => VegetationShapeInspection::Full,
                        "low" => VegetationShapeInspection::Low,
                        "morph" => VegetationShapeInspection::Morph,
                        "cause" => VegetationShapeInspection::Cause,
                        _ => return Err(
                            "--study-shape must be production, current, full, low, morph, or cause"
                                .into(),
                        ),
                    })
                }
                "--study-edge" => options.edge = Some(true),
                "--study-no-edge" => options.edge = Some(false),
                "--study-field" => {
                    let size = value()?.parse().map_err(|_| "Invalid --study-field")?;
                    super::stage::validate_field_size(size)?;
                    options.field_size = Some(size);
                }
                "--study-ground" => {
                    options.ground = Some(match value()?.as_str() {
                        "neutral" => super::stage::GroundMode::Neutral,
                        "meadow" => super::stage::GroundMode::Meadow,
                        "dried" => super::stage::GroundMode::Dried,
                        _ => return Err("--study-ground must be neutral, meadow, or dried".into()),
                    })
                }
                "--study-select-reference" => options.select_reference = Some(value()?),
                "--study-inspector" => options.show_inspector = true,
                "--study-picker" => options.show_picker = true,
                "--study-zoom" => {
                    let zoom: f32 = value()?.parse().map_err(|_| "Invalid --study-zoom")?;
                    if !zoom.is_finite() || !(1.0..=6.0).contains(&zoom) {
                        return Err("--study-zoom must be 1..6".into());
                    }
                    options.zoom = Some(zoom);
                }
                "--study-reference" => options.references.push(PathBuf::from(value()?)),
                "--study-camera" => {
                    let name = value()?;
                    options.character |= name == "scale";
                    options.camera = Some(StudyCamera::preset(&name)?);
                }
                "--study-time" => {
                    let time: f32 = value()?.parse().map_err(|_| "Invalid --study-time")?;
                    if !time.is_finite() || !(0.0..4096.0).contains(&time) {
                        return Err("--study-time must be 0..4096 seconds".into());
                    }
                    options.time = Some(time);
                }
                "--study-capture" => options.capture = Some(PathBuf::from(value()?)),
                "--study-load" => {
                    options.load = Some(StudyDocument::read(&PathBuf::from(value()?))?)
                }
                "--study-exit-after-capture" => options.exit = true,
                _ => {}
            }
        }
        if options.exit && options.capture.is_none() {
            return Err("--study-exit-after-capture requires --study-capture".into());
        }
        options.open |= options.capture.is_some() || options.load.is_some();
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_grass_checkpoint_loads_as_a_valid_study() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/vegetation/distance-01.ron");
        StudyDocument::read(&path).expect("tracked grass checkpoint must remain replayable");
    }

    #[test]
    fn patch_replay_is_deterministic_and_does_not_mutate_catalog() {
        let catalog = vegetation::fixtures::reference_catalog();
        let original = catalog.clone();
        let (a, count) = bounded_scene(&catalog, 3, 123).unwrap();
        let (b, _) = bounded_scene(&catalog, 3, 123).unwrap();
        assert_eq!(a, b);
        assert!(count > 0 && count < MAX_CANDIDATES);
        assert_eq!(catalog, original);
        assert_ne!(a, bounded_scene(&catalog, 3, 124).unwrap().0);
    }
    #[test]
    fn rejects_expensive_candidate_domains_without_lowering_density() {
        let mut catalog = vegetation::fixtures::reference_catalog();
        catalog.populations[3].density_per_square_meter = 512.0;
        if let vegetation::VegetationGroupingProfile::Voronoi(ref mut p) =
            catalog.populations[3].grouping
        {
            p.spacing = 16.0;
            p.root_attraction = 1.0;
        }
        assert!(
            bounded_scene(&catalog, 3, 1)
                .unwrap_err()
                .contains("study limit")
        );
    }
    #[test]
    fn launch_rejects_nonfinite_phase_and_unknown_camera() {
        for args in [
            ["--study-time", "NaN"],
            ["--study-camera", "bogus"],
            ["--study-field", "32"],
            ["--study-ground", "missing"],
        ] {
            assert!(StudyLaunch::parse(args.into_iter().map(str::to_owned)).is_err());
        }
    }

    #[test]
    fn lod_fields_keep_density_and_common_pages_stable_and_enforce_budget() {
        let catalog = vegetation::fixtures::reference_catalog();
        let (small, small_count) = bounded_field(&catalog, 3, 123, 64.0).unwrap();
        let (large, large_count) = bounded_field(&catalog, 3, 123, 128.0).unwrap();
        assert_eq!(small.pages.len(), 16);
        assert_eq!(large.pages.len(), 64);
        assert!(large_count > small_count);
        assert!(small.pages.iter().all(|p| large.pages.contains(p)));
        assert_eq!(
            large.catalog.populations[3].density_per_square_meter,
            catalog.populations[3].density_per_square_meter
        );
        let mut expensive = catalog;
        expensive.populations[3].density_per_square_meter = 512.0;
        assert!(
            bounded_field(&expensive, 3, 123, 128.0)
                .unwrap_err()
                .contains("study limit")
        );
    }

    #[test]
    fn edge_clears_foreground_without_changing_catalog_or_candidate_identity() {
        let catalog = vegetation::fixtures::reference_catalog();
        let original = catalog.clone();
        let (full, count) = bounded_field(&catalog, 3, 123, 16.0).unwrap();
        let edge = super::super::stage::GrassEdge::default();
        let (cut, cut_count) = bounded_field_with_edge(&catalog, 3, 123, 16.0, Some(edge)).unwrap();
        assert_eq!(catalog, original);
        assert_eq!(full.catalog, cut.catalog);
        assert_eq!(count, cut_count);
        let page = &cut.pages[0];
        let field = &page.fields[0];
        assert_eq!(
            field.sample_coverage(page, [0.72, 0.0]),
            0.0,
            "Character stands on the clear side"
        );
        assert_eq!(field.sample_coverage(page, [0.0, 2.0]), 0.0);
        assert_eq!(field.sample_coverage(page, [0.0, -2.0]), 1.0);
        assert!(field.coverage.iter().any(|v| *v > 0 && *v < 255));
        assert_eq!(
            candidate_domain(&full.pages[0], &full.catalog.populations[3]),
            candidate_domain(page, &cut.catalog.populations[3])
        );
        let mut bad = edge;
        bad.angle = f32::NAN;
        assert!(bounded_field_with_edge(&catalog, 3, 123, 16.0, Some(bad)).is_err());
    }
}
