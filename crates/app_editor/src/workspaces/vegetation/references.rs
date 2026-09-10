use bevy_egui::egui;
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use super::stage::{CharacterPlacement, GrassEdge, GroundMode, ReferenceSetup};
use super::study::{StudyCamera, directory};

pub(super) fn configure_texture_limit(
    device: Option<bevy::prelude::Res<bevy::render::renderer::RenderDevice>>,
    mut inputs: bevy::prelude::Query<
        &mut bevy_egui::EguiInput,
        bevy::prelude::With<bevy_egui::PrimaryEguiContext>,
    >,
) {
    if let Some(device) = device {
        // bevy_egui leaves egui's conservative 2048px default in place. Tell it the actual
        // device limit before beginning the pass, bounded by our reference import limit.
        for mut input in &mut inputs {
            input.max_texture_side =
                Some(device.limits().max_texture_dimension_2d.min(8192) as usize);
        }
    }
}

#[derive(Default)]
pub(super) struct References {
    pub items: Vec<(PathBuf, Option<egui::TextureHandle>)>,
    pub selected: usize,
    pub import_path: String,
    pub error: Option<String>,
    selected_texture: Option<(PathBuf, egui::TextureHandle)>,
    cameras: BTreeMap<String, StudyCamera>,
    setups: BTreeMap<String, ReferenceSetup>,
}

impl References {
    pub fn load() -> Self {
        let mut paths: Vec<_> = std::fs::read_dir(directory().join("references"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "png"))
            .collect();
        paths.sort();
        let mut refs = Self {
            items: paths.into_iter().map(|p| (p, None)).collect(),
            ..Default::default()
        };
        let supplied = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/references");
        for name in [
            "top_down_close.JPG",
            "edge_1.JPG",
            "edge_2.JPG",
            "top_down.JPG",
            "bottom_straight.JPG",
            "lod.JPG",
        ] {
            let path = supplied.join(name);
            if path.is_file() {
                if let Err(error) = refs.import(&path) {
                    refs.error = Some(error);
                }
            }
        }
        refs.items.sort_by(|(a, _), (b, _)| a.cmp(b));
        refs.select_named("top_down_close-");
        let path = directory().join("reference-cameras.ron");
        if path.is_file() {
            let load = || -> Result<BTreeMap<String, StudyCamera>, String> {
                if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
                    return Err("Reference camera file exceeds 1 MiB".into());
                }
                let cameras: BTreeMap<String, StudyCamera> =
                    ron::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                for camera in cameras.values() {
                    camera.validate()?;
                }
                Ok(cameras)
            };
            match load() {
                Ok(cameras) => refs.cameras = cameras,
                Err(error) => refs.error = Some(error),
            }
        }
        let path = directory().join("reference-setups.ron");
        if path.is_file() {
            let load = || -> Result<BTreeMap<String, ReferenceSetup>, String> {
                if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
                    return Err("Reference setup file exceeds 1 MiB".into());
                }
                let setups: BTreeMap<String, ReferenceSetup> =
                    ron::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                for setup in setups.values() {
                    setup.validate()?;
                }
                Ok(setups)
            };
            match load() {
                Ok(setups) => refs.setups = setups,
                Err(error) => refs.error = Some(error),
            }
        }
        refs
    }

    pub fn select_named(&mut self, prefix: &str) {
        if let Some(index) = self.items.iter().position(|(p, _)| {
            p.file_name()
                .is_some_and(|p| p.to_string_lossy().starts_with(prefix))
        }) {
            self.selected = index;
        }
    }

    pub fn import(&mut self, path: &Path) -> Result<(), String> {
        let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if meta.len() > 32 * 1024 * 1024 {
            return Err("Reference exceeds 32 MiB".into());
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let extension = match reader.format() {
            Some(image::ImageFormat::Png) => "png",
            Some(image::ImageFormat::Jpeg) => "jpg",
            _ => return Err("References must be PNG or JPEG images".into()),
        };
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let decoded = reader.decode().map_err(|e| e.to_string())?;
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hash);
        let stem: String = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(48)
            .collect();
        let target = directory()
            .join("references")
            .join(format!("{stem}-{:016x}.png", hash.finish()));
        let original = target
            .parent()
            .unwrap()
            .join("originals")
            .join(target.file_name().unwrap())
            .with_extension(extension);
        std::fs::create_dir_all(original.parent().unwrap()).map_err(|e| e.to_string())?;
        if !target.is_file() {
            decoded
                .thumbnail(1600, 1600)
                .save(&target)
                .map_err(|e| e.to_string())?;
        }
        // Preserve the exact source, including when it came from a temporary screenshot path.
        // Existing thumbnail IDs remain stable, so reimport also upgrades old bookmarks.
        if !original.is_file() {
            let temporary = original.with_extension("writing");
            std::fs::write(&temporary, &bytes).map_err(|e| e.to_string())?;
            std::fs::rename(temporary, original).map_err(|e| e.to_string())?;
        }
        self.selected_texture = None;
        self.selected = self
            .items
            .iter()
            .position(|(p, _)| *p == target)
            .unwrap_or_else(|| {
                self.items.push((target, None));
                self.items.len() - 1
            });
        Ok(())
    }

    pub fn name(&self) -> Option<String> {
        self.items.get(self.selected).map(|(p, _)| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
    }

    pub fn title(&self) -> String {
        self.items
            .get(self.selected)
            .map(|(p, _)| reference_key(p).replace('_', " "))
            .unwrap_or_else(|| "No reference".into())
    }

    pub fn find(&self, name: &str) -> Result<usize, String> {
        if let Some(index) = self
            .items
            .iter()
            .position(|(p, _)| p.file_name().is_some_and(|p| p == name))
        {
            return Ok(index);
        }
        let matches: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, (p, _))| reference_key(p) == name)
            .map(|(i, _)| i)
            .collect();
        match matches.as_slice() {
            [index] => Ok(*index),
            [] => Err(format!("Reference {name} was not found")),
            _ => Err(format!(
                "Multiple versions of {name}; select an exact imported filename"
            )),
        }
    }

    #[cfg(test)]
    pub fn matching_camera(&self) -> StudyCamera {
        self.matching_setup().camera
    }

    pub fn matching_setup(&self) -> ReferenceSetup {
        if let Some(saved) = self.name().and_then(|name| self.setups.get(&name)) {
            return saved.clone();
        }
        let key = self
            .items
            .get(self.selected)
            .map(|(path, _)| reference_key(path))
            .unwrap_or_default();
        let mut setup = approximate_setup(&key);
        if let Some(camera) = self.name().and_then(|name| self.cameras.get(&name)) {
            setup.camera = camera.clone();
        }
        setup
    }

    /* Camera-only bookmarks remain readable; new bookmarks capture the complete stage. */
    pub fn has_saved_camera(&self) -> bool {
        self.name()
            .is_some_and(|name| self.cameras.contains_key(&name) || self.setups.contains_key(&name))
    }

    pub fn save_setup(&mut self, setup: &ReferenceSetup) -> Result<(), String> {
        setup.validate()?;
        let name = self.name().ok_or("Select a reference first")?;
        let mut setups = self.setups.clone();
        setups.insert(name, setup.clone());
        std::fs::create_dir_all(directory()).map_err(|e| e.to_string())?;
        let path = directory().join("reference-setups.ron");
        let temp = path.with_extension("ron.writing");
        let contents = ron::ser::to_string_pretty(&setups, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())?;
        std::fs::write(&temp, contents).map_err(|e| e.to_string())?;
        std::fs::rename(temp, path).map_err(|e| e.to_string())?;
        self.setups = setups;
        Ok(())
    }

    pub fn prepare_textures(&mut self, context: &egui::Context) {
        for (index, (path, texture)) in self.items.iter_mut().enumerate() {
            if texture.is_none() {
                match image::open(path) {
                    Ok(decoded) => {
                        let rgba = decoded.to_rgba8();
                        *texture = Some(context.load_texture(
                            format!("study-reference-{index}"),
                            egui::ColorImage::from_rgba_unmultiplied(
                                [rgba.width() as usize, rgba.height() as usize],
                                &rgba,
                            ),
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
        }
        let selected_path = self.items.get(self.selected).map(|(path, _)| path);
        if self.selected_texture.as_ref().map(|(path, _)| path) != selected_path {
            // At most one full-resolution reference lives on the GPU; the picker uses thumbnails.
            self.selected_texture = None;
            if let Some(path) = selected_path {
                let originals = path.parent().unwrap().join("originals");
                let original = ["png", "jpg"]
                    .into_iter()
                    .map(|ext| {
                        originals
                            .join(path.file_name().unwrap())
                            .with_extension(ext)
                    })
                    .find(|p| p.is_file());
                if let Some(original) = original {
                    match image::open(original) {
                        Ok(decoded) => {
                            let limit = context.input(|i| i.max_texture_side) as u32;
                            let decoded = if decoded.width().max(decoded.height()) > limit {
                                decoded.thumbnail(limit, limit)
                            } else {
                                decoded
                            };
                            let rgba = decoded.to_rgba8();
                            self.selected_texture = Some((
                                path.clone(),
                                context.load_texture(
                                    "study-selected-reference",
                                    egui::ColorImage::from_rgba_unmultiplied(
                                        [rgba.width() as usize, rgba.height() as usize],
                                        &rgba,
                                    ),
                                    egui::TextureOptions::LINEAR,
                                ),
                            ));
                        }
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
            }
        }
    }

    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        let (path, thumbnail) = self.items.get(self.selected)?;
        self.selected_texture
            .as_ref()
            .filter(|(selected_path, _)| selected_path == path)
            .map(|(_, texture)| texture)
            .or(thumbnail.as_ref())
    }

    /// Returns a selection event even for the current item, so clicking it can restore its camera.
    pub fn picker_ui(&mut self, ui: &mut egui::Ui) -> Option<usize> {
        let mut selection = None;
        ui.small("Selecting an image restores its camera and resets linked zoom.");
        let gallery_height = (ui.available_height() - 180.0).clamp(100.0, 430.0);
        egui::ScrollArea::vertical()
            .max_height(gallery_height)
            .show(ui, |ui| {
                egui::Grid::new("study-reference-grid")
                    .num_columns(2)
                    .spacing(egui::vec2(12.0, 12.0))
                    .show(ui, |ui| {
                        for (index, (path, texture)) in self.items.iter().enumerate() {
                            ui.vertical(|ui| {
                                if let Some(texture) = texture {
                                    if ui
                                        .add(
                                            egui::Button::image(
                                                egui::Image::new(texture)
                                                    .fit_to_exact_size(egui::vec2(164.0, 92.25)),
                                            )
                                            .selected(index == self.selected),
                                        )
                                        .on_hover_text("Select and match camera")
                                        .clicked()
                                    {
                                        selection = Some(index);
                                    }
                                }
                                ui.label(reference_key(path).replace('_', " "));
                            });
                            if index % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
            });
        ui.separator();
        ui.add(
            egui::TextEdit::singleline(&mut self.import_path)
                .hint_text("Local image path")
                .desired_width(f32::INFINITY),
        );
        if ui.button("Import reference").clicked() {
            self.error = self.import(&PathBuf::from(self.import_path.trim())).err();
            if self.error.is_none() {
                selection = Some(self.selected);
            }
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        selection
    }
}

fn reference_key(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    match stem.rsplit_once('-') {
        Some((name, hash)) if hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()) => {
            name.to_owned()
        }
        _ => stem.into_owned(),
    }
}

fn approximate_setup(key: &str) -> ReferenceSetup {
    // Manual matches use the character's body/feet and ground plane, not the grass blade size.
    // The reference's clothing/hat and unknown intrinsics preclude an exact calibration.
    let (yaw, pitch, distance, target, fov) = match key {
        "top_down_close" => (0.0, 78.0, 2.2, [0.0, 0.18, 0.0], 50.0),
        "top_down" => (0.0, 89.0, 4.7, [0.0, 0.18, 0.0], 50.0),
        "edge_1" => (0.0, 28.0, 1.7, [0.0, 0.10, -0.60], 50.0),
        "edge_2" => (0.0, 25.0, 1.8, [0.0, 0.02, -0.50], 50.0),
        "bottom_straight" => (0.0, 15.0, 2.0, [0.0, 0.02, -0.30], 50.0),
        "lod" => (0.0, 15.0, 4.77, [0.0, 0.88, -1.73], 50.0),
        _ => (0.0, 58.0, 3.5, [0.0, 0.18, 0.0], 50.0),
    };
    let (xz, facing, field_size) = match key {
        "bottom_straight" => ([-0.76, 0.0], 180.0, 64.0),
        "edge_1" => ([0.72, 0.0], -90.0, 16.0),
        "edge_2" => ([-0.55, 0.15], 180.0, 16.0),
        "top_down_close" => ([-0.55, 0.45], 180.0, 16.0),
        "top_down" => ([-0.60, 0.40], 180.0, 16.0),
        "lod" => ([-0.89, 0.0], 180.0, 128.0),
        _ => ([0.85, -0.2], 0.0, 16.0),
    };
    ReferenceSetup {
        camera: StudyCamera {
            yaw,
            pitch,
            distance,
            target,
            fov,
        },
        character: CharacterPlacement { xz, yaw: facing },
        field_size,
        ground: GroundMode::Meadow,
        edge: match key {
            "edge_1" => Some(GrassEdge::default()),
            "edge_2" => Some(GrassEdge {
                offset: -0.45,
                angle: -24.0,
                softness: 0.2,
            }),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn references_have_distinct_valid_camera_matches_and_saved_overrides_win() {
        let names = [
            "top_down_close",
            "top_down",
            "edge_1",
            "edge_2",
            "bottom_straight",
            "lod",
        ];
        let mut refs = References::default();
        for name in names {
            refs.items
                .push((PathBuf::from(format!("{name}-0123456789abcdef.png")), None));
        }
        let mut cameras = Vec::new();
        for name in names {
            refs.selected = refs.find(name).unwrap();
            let camera = refs.matching_camera();
            camera.validate().unwrap();
            assert!(!cameras.contains(&camera));
            cameras.push(camera);
        }
        let saved = StudyCamera::preset("scale").unwrap();
        refs.cameras.insert(refs.name().unwrap(), saved.clone());
        assert_eq!(refs.matching_camera(), saved);
        let mut setup = refs.matching_setup();
        setup.field_size = 64.0;
        setup.character.xz = [1.0, 2.0];
        setup.camera.distance = 3.0;
        refs.setups.insert(refs.name().unwrap(), setup.clone());
        assert_eq!(refs.matching_setup(), setup);
    }

    #[test]
    fn body_landmarks_keep_low_and_lod_cameras_at_reference_scale() {
        use bevy::prelude::*;
        let project = |setup: &ReferenceSetup, height: f32| {
            let position = setup.character.transform().translation + Vec3::Y * height;
            let view = setup
                .camera
                .transform()
                .to_matrix()
                .inverse()
                .transform_point3(position);
            let half_height = -view.z * (setup.camera.fov.to_radians() * 0.5).tan();
            Vec2::new(
                0.5 + view.x / (half_height * 2.0 * 16.0 / 9.0),
                0.5 - view.y / (2.0 * half_height),
            )
        };
        let low = approximate_setup("bottom_straight");
        let feet = project(&low, 0.0);
        assert!((0.18..0.30).contains(&feet.x));
        assert!((0.50..0.65).contains(&feet.y));
        assert!(project(&low, 1.0).y < 0.0, "Reference crops the upper body");
        let lod = approximate_setup("lod");
        let feet = project(&lod, 0.0);
        assert!((0.30..0.40).contains(&feet.x));
        assert!((0.86..0.98).contains(&feet.y));
        assert!((0.32..0.48).contains(&project(&lod, 1.7).y));
    }
}
