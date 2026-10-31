//! A character's face: its face slider values (`character_kit` shapes the face from them), what its
//! skin paints (`character_skin`), and faces kept as files,
//! `assets/characters/faces/<name>.face.ron`.

use std::{collections::BTreeMap, fs, path::PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

const FACE_VERSION: u32 = 5;

/// Where faces are kept, under the asset root.
const FACES: &str = "characters/faces";

const FACE_EXTENSION: &str = ".face.ron";

/// A colour painted over the skin, sRGB, and how strongly.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkinPaint {
    pub color: [f32; 3],
    pub strength: f32,
}

impl SkinPaint {
    const fn unworn(color: [f32; 3]) -> Self {
        Self {
            color,
            strength: 0.0,
        }
    }
}

/// What a character's skin paints. A brow is its label in the kit; an unknown one (or a kit's
/// empty default) takes the kit's first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkinLook {
    pub brow: String,
    pub brow_paint: SkinPaint,
    /// The hair style by its label in the kit: an unknown one (or a kit's empty default) takes the
    /// kit's first, and `NO_HAIR` wears none.
    pub hair_style: String,
    /// The hair's colour, over its grey cards and the skin under and in front of them.
    pub hair: SkinPaint,
    pub eyeshadow: SkinPaint,
    pub liner: SkinPaint,
    pub blush: SkinPaint,
    pub lipstick: SkinPaint,
}

impl Default for SkinLook {
    fn default() -> Self {
        Self {
            brow: String::new(),
            brow_paint: SkinPaint {
                color: [0.11, 0.07, 0.05],
                strength: 1.0,
            },
            hair_style: String::new(),
            hair: SkinPaint {
                color: [0.42, 0.25, 0.16],
                strength: 1.0,
            },
            eyeshadow: SkinPaint::unworn([0.36, 0.25, 0.24]),
            liner: SkinPaint::unworn([0.05, 0.035, 0.03]),
            blush: SkinPaint::unworn([0.86, 0.50, 0.48]),
            lipstick: SkinPaint::unworn([0.58, 0.14, 0.18]),
        }
    }
}

/// A character's face: each face slider's value by its label, and what its skin paints. Sliders it
/// leaves out stay at 0; values are held to each slider's range.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct CharacterAppearance {
    pub sliders: BTreeMap<String, f32>,
    pub look: SkinLook,
}

/// A face as a file keeps it.
#[derive(Serialize, Deserialize)]
struct FaceFile {
    version: u32,
    sliders: BTreeMap<String, f32>,
    look: SkinLook,
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
            look: file.look,
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
            look: self.look.clone(),
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
            look: SkinLook {
                brow: "Full arch".into(),
                ..default()
            },
        };
        let read = CharacterAppearance::from_ron(&face.to_ron()).expect("a written face reads");
        assert_eq!(read.sliders, [("lift".to_string(), 0.25)].into());
        assert_eq!(read.look, face.look);
        assert!(CharacterAppearance::from_ron("(version: 4, sliders: {})").is_err());
    }

    #[test]
    fn face_names_cannot_leave_the_faces_folder() {
        assert!(face_path("tall_elf-2").is_ok());
        assert!(face_path("../world").is_err());
        assert!(face_path("Face").is_err());
        assert!(face_path("").is_err());
    }
}
