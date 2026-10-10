//! Cooked ground appearance. Its residency and LOD are independent of terrain meshes.
use crate::{TerrainNodeKey, terrain_hierarchy::TerrainHierarchyError};
use serde::{Deserialize, Serialize};

/// Version 2 stores rain hollowness in colour alpha.
pub const TERRAIN_COMPOSITE_VERSION: u16 = 2;
pub const TERRAIN_COMPOSITE_INTERIOR: usize = 64;
pub const TERRAIN_COMPOSITE_GUTTER: usize = 4;
pub const TERRAIN_COMPOSITE_MIPS: usize = 3;
pub const MAX_TERRAIN_COMPOSITE_BYTES: usize = 64 * 1024;
/// Depth below the surrounding ground at which composite hollowness saturates.
pub const TERRAIN_HOLLOW_DEPTH_METRES: f32 = 0.15;
/// Radius of the surrounding ground that hollowness compares against.
pub const TERRAIN_HOLLOW_RADIUS_METRES: f32 = 1.5;

/// Overlapping surfaces blend by height. Each surface's base colour alpha is its blend height,
/// 0 to 1 (the import scales the height map by the surface's `blend_height`), so stones and
/// needles rise through sand and moss instead of cross-fading with them. A surface's score is
/// its share of the ground weights plus this multiple of its height.
/// `shaders/terrain_blend.wesl` mirrors these values and [`terrain_height_blend`].
pub const TERRAIN_BLEND_HEIGHT: f32 = 0.5;
/// Score range below the highest surface that still shows; narrower is crisper.
pub const TERRAIN_BLEND_DEPTH: f32 = 0.12;
/// Surfaces shaded at one point: the largest ground weights, as the GPU evaluates them.
pub const TERRAIN_BLEND_SURFACES: usize = 3;

/// Indices and weights of the [`TERRAIN_BLEND_SURFACES`] largest `weights`, largest first.
/// Ties keep the lower index; unused entries have zero weight.
pub fn terrain_blend_candidates(weights: &[f32]) -> [(usize, f32); TERRAIN_BLEND_SURFACES] {
    let mut best = [(0, 0.0_f32); TERRAIN_BLEND_SURFACES];
    for (index, &weight) in weights.iter().enumerate() {
        if let Some(rank) = best.iter().position(|&(_, w)| weight > w) {
            best.copy_within(rank..TERRAIN_BLEND_SURFACES - 1, rank + 1);
            best[rank] = (index, weight);
        }
    }
    best
}

/// Blend weights of surfaces with ground weights `weights` (any positive scale) and blend
/// heights `heights`. Zero-weight surfaces stay zero; the result sums to one.
pub fn terrain_height_blend<const N: usize>(weights: [f32; N], heights: [f32; N]) -> [f32; N] {
    let sum: f32 = weights.iter().sum();
    if sum <= 0.0 {
        return weights;
    }
    let scores: [f32; N] = std::array::from_fn(|i| {
        if weights[i] > 0.0 {
            weights[i] / sum + heights[i] * TERRAIN_BLEND_HEIGHT
        } else {
            f32::NEG_INFINITY
        }
    });
    let top = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let raw: [f32; N] = std::array::from_fn(|i| (scores[i] - top + TERRAIN_BLEND_DEPTH).max(0.0));
    let total: f32 = raw.iter().sum();
    raw.map(|v| v / total)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellCoord, TerrainNodeKey, WorldSpaceId};

    #[test]
    fn composites_round_trip_and_other_versions_are_rejected() {
        let mips = (0..TERRAIN_COMPOSITE_MIPS)
            .map(|level| {
                let texels = TerrainComposite::mip_size(level).pow(2);
                TerrainCompositeMip {
                    color: [90, 80, 70, 17].repeat(texels),
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
        let bytes = encode_terrain_composite(&tile).unwrap();
        assert_eq!(decode_terrain_composite(&bytes).unwrap(), tile);
        let v1 =
            bincode::serde::encode_to_vec((1_u16, &tile), bincode::config::standard()).unwrap();
        assert!(decode_terrain_composite(&v1).is_err());
    }

    #[test]
    fn blend_candidates_are_the_largest_weights_with_stable_ties() {
        assert_eq!(
            terrain_blend_candidates(&[0.1, 0.4, 0.0, 0.4, 0.1]),
            [(1, 0.4), (3, 0.4), (0, 0.1)]
        );
        assert_eq!(
            terrain_blend_candidates(&[1.0]),
            [(0, 1.0), (0, 0.0), (0, 0.0)]
        );
    }

    #[test]
    fn taller_surfaces_rise_through_lower_ones() {
        // Equal heights blend only near equal weights; the larger weight otherwise wins.
        assert_eq!(terrain_height_blend([0.8, 0.2], [0.5, 0.5]), [1.0, 0.0]);
        let even = terrain_height_blend([0.5, 0.5], [0.5, 0.5]);
        assert!((even[0] - 0.5).abs() < 1e-6);
        // A stone top (height 1) dominates where it covers only 30% (sand height 0.1).
        let stone = terrain_height_blend([0.7, 0.3], [0.1, 1.0]);
        assert!(stone[1] > 0.6, "{stone:?}");
        // ...but a trace of it does not.
        assert_eq!(terrain_height_blend([0.95, 0.05], [0.1, 1.0])[1], 0.0);
        // Absent surfaces stay absent whatever their height.
        assert_eq!(terrain_height_blend([1.0, 0.0], [0.0, 1.0]), [1.0, 0.0]);
        assert_eq!(terrain_height_blend([0.0, 0.0], [0.0, 1.0]), [0.0, 0.0]);
    }
}
