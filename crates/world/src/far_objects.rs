//! Impostor-drawn objects grouped into blocks of cells, streamed further than cells.
use serde::{Deserialize, Serialize};

use crate::{CellCoord, DecodeError, MAX_DECODED_PAGE_BYTES, codec};

/// An asset's last LOD variant may be an impostor: a descriptor of baked views drawn as one
/// camera-facing quad per object, instead of a glTF scene.
pub const IMPOSTOR_SUFFIX: &str = ".impostor.json";

pub fn is_impostor_uri(uri: &str) -> bool {
    uri.ends_with(IMPOSTOR_SUFFIX)
}

/// Cells per side of a far-object block.
pub const FAR_OBJECT_BLOCK_CELLS: i32 = 16;

pub const MAX_FAR_OBJECT_FORMS: usize = 256;
pub const MAX_FAR_OBJECTS_PER_BLOCK: usize = 262_144;

/// The block of [`FAR_OBJECT_BLOCK_CELLS`]² cells containing `cell`, as its lowest cell
/// divided by the block size.
pub fn far_object_block(cell: CellCoord) -> CellCoord {
    CellCoord {
        x: cell.x.div_euclid(FAR_OBJECT_BLOCK_CELLS),
        z: cell.z.div_euclid(FAR_OBJECT_BLOCK_CELLS),
    }
}

/// Every impostor-drawn static object of one block, derived from its cells' static-object
/// pages when a cook finishes. Streamed much further than cells, it carries each object's
/// whole LOD chain: the game draws every representation from it, the editor only the
/// impostors (its cells draw the meshes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FarObjectsPage {
    pub forms: Vec<FarObjectForm>,
    pub instances: Vec<FarObjectInstance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FarObjectForm {
    /// The asset's impostor descriptor (its last LOD).
    pub uri: String,
    /// Object height at unit scale (the largest LOD bounds).
    pub height: f32,
    /// The mesh LODs before the impostor, nearest first.
    pub meshes: Vec<FarObjectMesh>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FarObjectMesh {
    pub uri: String,
    /// Projected height in logical pixels below which the next LOD draws.
    pub minimum_screen_height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FarObjectInstance {
    /// Index into the page's forms.
    pub form: u16,
    /// X and Z from the block's lowest corner; Y as in the cell page.
    pub translation: [f32; 3],
    pub yaw: f32,
    pub scale: f32,
}

impl FarObjectsPage {
    pub fn validate(&self) -> Result<(), String> {
        if self.forms.is_empty()
            || self.forms.len() > MAX_FAR_OBJECT_FORMS
            || self.instances.is_empty()
            || self.instances.len() > MAX_FAR_OBJECTS_PER_BLOCK
        {
            return Err("far-object page has no or too many forms or instances".into());
        }

        if self.forms.iter().any(|f| !is_impostor_uri(&f.uri)) {
            return Err("far-object form is not an impostor".into());
        }

        // Mesh LODs nearest first with descending thresholds, the last one handing off.
        if self.forms.iter().any(|f| {
            !(f.height.is_finite() && f.height > 0.0)
                || f.meshes.is_empty()
                || f.meshes.iter().any(|m| {
                    is_impostor_uri(&m.uri)
                        || !(m.minimum_screen_height.is_finite() && m.minimum_screen_height > 0.0)
                })
                || f.meshes
                    .windows(2)
                    .any(|w| w[1].minimum_screen_height > w[0].minimum_screen_height)
        }) {
            return Err("far-object form has no valid mesh LOD chain".into());
        }

        if self.instances.iter().any(|i| {
            usize::from(i.form) >= self.forms.len()
                || !i.translation.iter().all(|v| v.is_finite())
                || !i.yaw.is_finite()
                || !i.scale.is_finite()
                || i.scale <= 0.0
        }) {
            return Err("far-object instance is invalid".into());
        }

        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, bincode::error::EncodeError> {
        codec::encode(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        codec::decode::<_, { MAX_DECODED_PAGE_BYTES as usize }>(bytes)
    }
}
