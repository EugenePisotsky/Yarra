//! Cooked ground appearance. Its residency and LOD are independent of terrain meshes.
use crate::{TerrainNodeKey, terrain_hierarchy::TerrainHierarchyError};
use serde::{Deserialize, Serialize};

/// Version 2 stores rain hollowness in colour alpha; version 1 pages decode with none.
pub const TERRAIN_COMPOSITE_VERSION: u16 = 2;
pub const TERRAIN_COMPOSITE_INTERIOR: usize = 64;
pub const TERRAIN_COMPOSITE_GUTTER: usize = 4;
pub const TERRAIN_COMPOSITE_MIPS: usize = 3;
pub const MAX_TERRAIN_COMPOSITE_BYTES: usize = 64 * 1024;
/// Depth below the surrounding ground at which composite hollowness saturates.
pub const TERRAIN_HOLLOW_DEPTH_METRES: f32 = 0.15;
/// Radius of the surrounding ground that hollowness compares against.
pub const TERRAIN_HOLLOW_RADIUS_METRES: f32 = 1.5;

/// The grid matches terrain addressing, but this key never selects geometry detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TerrainMaterialKey(pub TerrainNodeKey);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainCompositeMip {
    /// RGB sRGB albedo. No lighting, shadows or grass canopy baked in. Linear alpha is rain
    /// hollowness: depth below the surrounding ground, saturating at
    /// `TERRAIN_HOLLOW_DEPTH_METRES`, where standing water collects.
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
                m.color.len() != n || m.response.len() != n
            })
        {
            return Err(TerrainHierarchyError::Invalid(
                "terrain composite dimensions",
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
    let ((version, mut value), read): ((u16, TerrainComposite), usize) =
        bincode::serde::decode_from_slice(
            bytes,
            bincode::config::standard().with_limit::<MAX_TERRAIN_COMPOSITE_BYTES>(),
        )
        .map_err(|e| e.to_string())?;
    if !(1..=TERRAIN_COMPOSITE_VERSION).contains(&version) || read != bytes.len() {
        return Err("terrain composite version/trailing bytes".into());
    }
    value.validate().map_err(|e| e.to_string())?;
    if version == 1 {
        // Opaque alpha predates hollowness: publish no standing water until re-cooked.
        if value
            .mips
            .iter()
            .any(|m| m.color.chunks_exact(4).any(|c| c[3] != 255))
        {
            return Err("terrain composite version 1 requires opaque alpha".into());
        }
        for mip in &mut value.mips {
            for texel in mip.color.chunks_exact_mut(4) {
                texel[3] = 0;
            }
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellCoord, TerrainNodeKey, WorldSpaceId};

    #[test]
    fn version_one_pages_decode_without_standing_water() {
        let mips = (0..TERRAIN_COMPOSITE_MIPS)
            .map(|level| {
                let texels = TerrainComposite::mip_size(level).pow(2);
                TerrainCompositeMip {
                    color: [90, 80, 70, 255].repeat(texels),
                    response: [128, 128, 200, 255].repeat(texels),
                }
            })
            .collect();
        let tile = TerrainComposite {
            key: TerrainMaterialKey(TerrainNodeKey::leaf(
                WorldSpaceId(1),
                CellCoord { x: 0, z: 0 },
            )),
            fingerprint: [7; 32],
            mips,
        };
        let v1 =
            bincode::serde::encode_to_vec((1_u16, &tile), bincode::config::standard()).unwrap();
        let decoded = decode_terrain_composite(&v1).unwrap();
        assert!(decoded.mips.iter().all(|m| {
            m.color
                .chunks_exact(4)
                .all(|c| c[..3] == [90, 80, 70] && c[3] == 0)
        }));
        let v2 = encode_terrain_composite(&tile).unwrap();
        assert_eq!(decode_terrain_composite(&v2).unwrap(), tile);
        let mut hollow = tile.clone();
        hollow.mips[0].color[3] = 17;
        let v1 =
            bincode::serde::encode_to_vec((1_u16, &hollow), bincode::config::standard()).unwrap();
        assert!(
            decode_terrain_composite(&v1).is_err(),
            "v1 alpha was always opaque"
        );
    }
}
