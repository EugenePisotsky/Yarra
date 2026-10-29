//! A character's face: its face slider values (`character_kit` shapes the face from them), and
//! faces kept as files, `assets/characters/faces/<name>.face.ron`.

use std::{collections::BTreeMap, fs, path::PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

const FACE_VERSION: u32 = 1;

/// Where faces are kept, under the asset root.
const FACES: &str = "characters/faces";

const FACE_EXTENSION: &str = ".face.ron";

/// A character's face: each face slider's value by its label. Sliders it leaves out stay at 0;
/// values are held to each slider's range.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct CharacterAppearance {
    pub sliders: BTreeMap<String, f32>,
}

/// A face as a file keeps it.
#[derive(Serialize, Deserialize)]
struct FaceFile {
    version: u32,
    sliders: BTreeMap<String, f32>,
}

impl CharacterAppearance {
    fn from_ron(text: &str) -> Result<Self, String> {
        let file: FaceFile = ron::from_str(text).map_err(|e| e.to_string())?;

        if file.version != FACE_VERSION {
            return Err(format!(
                "face version {} (expected {FACE_VERSION})",
                file.version
            ));
        }

        if file.sliders.values().any(|value| !value.is_finite()) {
            return Err("a slider value is not finite".into());
        }

        Ok(Self {
            sliders: file.sliders,
        })
    }

    /// The face as a file keeps it: sliders at 0 left out.
    fn to_ron(&self) -> String {
        let file = FaceFile {
            version: FACE_VERSION,
            sliders: self
                .sliders
                .iter()
                .filter(|(_, value)| **value != 0.0)
                .map(|(label, value)| (label.clone(), *value))
                .collect(),
        };
        ron::ser::to_string_pretty(&file, ron::ser::PrettyConfig::default())
            .expect("a face serialises")
    }

    /// The face kept under this name.
    pub fn load(name: &str) -> Result<Self, String> {
        let text = fs::read_to_string(face_path(name)?).map_err(|e| format!("{name}: {e}"))?;
        Self::from_ron(&text).map_err(|e| format!("{name}: {e}"))
    }

    /// Keeps the face under this name; returns its file.
    pub fn save(&self, name: &str) -> Result<PathBuf, String> {
        let path = face_path(name)?;
        fs::create_dir_all(faces_folder()).map_err(|e| e.to_string())?;
        fs::write(&path, self.to_ron()).map_err(|e| e.to_string())?;
        Ok(path)
    }
}

fn faces_folder() -> PathBuf {
    crate::resolve_asset_root().join(FACES)
}

/// The names of the faces kept, sorted.
pub fn saved_faces() -> Vec<String> {
    let mut names = fs::read_dir(faces_folder())
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.strip_suffix(FACE_EXTENSION).map(str::to_owned)
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// A face's file: its name takes lower-case letters, digits, '-' and '_' only.
fn face_path(name: &str) -> Result<PathBuf, String> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');

    if !valid {
        return Err(format!(
            "face name {name:?}: lower-case letters, digits, '-' and '_' only"
        ));
    }

    Ok(faces_folder().join(format!("{name}{FACE_EXTENSION}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_read_back_as_written_without_their_zero_sliders() {
        let face = CharacterAppearance {
            sliders: [("lift".to_string(), 0.25), ("point".to_string(), 0.0)].into(),
        };
        let read = CharacterAppearance::from_ron(&face.to_ron()).expect("a written face reads");
        assert_eq!(read.sliders, [("lift".to_string(), 0.25)].into());
        assert!(CharacterAppearance::from_ron("(version: 2, sliders: {})").is_err());
    }

    #[test]
    fn face_names_cannot_leave_the_faces_folder() {
        assert!(face_path("tall_elf-2").is_ok());
        assert!(face_path("../world").is_err());
        assert!(face_path("Face").is_err());
        assert!(face_path("").is_err());
    }
}
