//! Cooked ground appearance. Its residency and LOD are independent of terrain meshes.
use crate::{TerrainNodeKey, terrain_hierarchy::TerrainHierarchyError};
use serde::{Deserialize, Serialize};

pub const TERRAIN_COMPOSITE_VERSION: u16 = 1;
pub const TERRAIN_COMPOSITE_INTERIOR: usize = 64;
pub const TERRAIN_COMPOSITE_GUTTER: usize = 4;
pub const TERRAIN_COMPOSITE_MIPS: usize = 3;
pub const MAX_TERRAIN_COMPOSITE_BYTES: usize = 64 * 1024;

/// The grid matches terrain addressing, but this key never selects geometry detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TerrainMaterialKey(pub TerrainNodeKey);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainCompositeMip {
    /// RGB sRGB albedo, opaque alpha. No lighting, shadows or grass canopy baked in.
    pub color: Vec<u8>,
    /// World normal octahedral X/Z UNORM8, perceptual roughness, material AO.
    pub response: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainComposite {
    pub key: TerrainMaterialKey,
    /// Baker, input textures, surface/profile settings and spatial dependencies.
    pub fingerprint: [u8; 32],
    pub mips: Vec<TerrainCompositeMip>,
}

impl TerrainComposite {
    pub fn mip_size(level: usize) -> usize {
        (TERRAIN_COMPOSITE_INTERIOR + 2 * TERRAIN_COMPOSITE_GUTTER) >> level
    }
    pub fn gpu_bytes() -> usize {
        (0..TERRAIN_COMPOSITE_MIPS)
            .map(|l| Self::mip_size(l).pow(2) * 8)
            .sum()
    }
    pub fn validate(&self) -> Result<(), TerrainHierarchyError> {
        self.key.0.cell_bounds()?;
        if self.mips.len() != TERRAIN_COMPOSITE_MIPS
            || self.mips.iter().enumerate().any(|(l, m)| {
                let n = Self::mip_size(l).pow(2) * 4;
                m.color.len() != n
                    || m.response.len() != n
                    || m.color.chunks_exact(4).any(|c| c[3] != 255)
            })
        {
            return Err(TerrainHierarchyError::Invalid(
                "terrain composite dimensions/alpha",
            ));
        }
        Ok(())
    }
}

pub fn encode_terrain_composite(value: &TerrainComposite) -> Result<Vec<u8>, String> {
    value.validate().map_err(|e| e.to_string())?;
    bincode::serde::encode_to_vec(
        (TERRAIN_COMPOSITE_VERSION, value),
        bincode::config::standard(),
    )
    .map_err(|e| e.to_string())
}
pub fn decode_terrain_composite(bytes: &[u8]) -> Result<TerrainComposite, String> {
    if bytes.len() > MAX_TERRAIN_COMPOSITE_BYTES {
        return Err("terrain composite size limit".into());
    }
    let ((version, value), read): ((u16, TerrainComposite), usize) =
        bincode::serde::decode_from_slice(
            bytes,
            bincode::config::standard().with_limit::<MAX_TERRAIN_COMPOSITE_BYTES>(),
        )
        .map_err(|e| e.to_string())?;
    if version != TERRAIN_COMPOSITE_VERSION || read != bytes.len() {
        return Err("terrain composite version/trailing bytes".into());
    }
    value.validate().map_err(|e| e.to_string())?;
    Ok(value)
}
