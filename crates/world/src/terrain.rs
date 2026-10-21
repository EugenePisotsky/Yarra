//! Terrain catalogs and ground: surfaces, texture sets and profiles, and each cell's heights and
//! normals.
use serde::{Deserialize, Serialize};

use crate::{TerrainSurfaceId, TerrainTextureSetId, TerrainWear, WorldSpaceId};

/// Surfaces a terrain page holds, four per RGBA weight page: natural ground and what roads wear
/// it to, so varied ground along a road keeps both.
pub const MAX_TERRAIN_SURFACES_PER_CELL: usize = 12;

pub const MAX_TERRAIN_WEIGHT_PAGES: usize = MAX_TERRAIN_SURFACES_PER_CELL.div_ceil(4);
pub const MAX_TERRAIN_WEIGHT_RESOLUTION: u16 = 257;
pub const MAX_TERRAIN_HEIGHTFIELD_RESOLUTION: u16 = 257;

/// Terrain heights are whole multiples of this step (1/1024 m) on one grid shared by every
/// page. Scaling by a power of two is exact, so rounding never depends on the page.
pub const TERRAIN_HEIGHT_STEP: f32 = 1.0 / 1024.0;

/// Precision of each octahedral terrain normal axis (about 0.05°). The low bits of the
/// stored `i16` are zero.
pub const TERRAIN_NORMAL_BITS: u32 = 12;

/// Rounds a height to the shared terrain grid. Equal inputs give equal outputs on every page.
pub fn quantize_terrain_height(height: f32) -> f32 {
    (height / TERRAIN_HEIGHT_STEP).round() * TERRAIN_HEIGHT_STEP
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainSurface {
    pub id: TerrainSurfaceId,
    pub key: String,
    pub display_name: String,
    /// The world-space width and height, in metres, of one texture repetition.
    pub tile_size: f32,
    /// Breaks up visible repetition by blending stable, randomly transformed
    /// albedo samples. This is an authored surface property because noisy
    /// surfaces benefit from it while strongly directional ones may not.
    pub anti_tiling: bool,
    pub normal_y_sign: f32,
    pub normal_strength: f32,
    pub roughness_min: f32,
    pub roughness_max: f32,
    /// How deep the surface's soft ground (snow) lies over the firm ground the heights hold, in
    /// metres: the ground is drawn that much higher and feet sink through it; 0 for firm ground.
    pub soft_depth: f32,
    /// How much of the world's large-scale albedo variation (`TerrainProfile`'s macro variation)
    /// the surface takes, 0..1: a snowfield's whiteness hardly varies, its light does.
    pub macro_strength: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainTextureSet {
    pub id: TerrainTextureSetId,
    pub key: String,
    pub base_color_universal_uri: String,
    pub normal_material_universal_uri: String,
    pub macro_variation_universal_uri: String,
    pub base_color_astc_uri: String,
    pub normal_material_astc_uri: String,
    pub macro_variation_astc_uri: String,
    pub universal_gpu_bytes: u64,
    pub astc_gpu_bytes: u64,
}

impl TerrainTextureSet {
    pub fn runtime_uris(&self) -> (&str, &str, &str) {
        if cfg!(target_os = "ios") {
            (
                &self.base_color_astc_uri,
                &self.normal_material_astc_uri,
                &self.macro_variation_astc_uri,
            )
        } else {
            (
                &self.base_color_universal_uri,
                &self.normal_material_universal_uri,
                &self.macro_variation_universal_uri,
            )
        }
    }

    pub fn runtime_gpu_bytes(&self) -> u64 {
        if cfg!(target_os = "ios") {
            self.astc_gpu_bytes
        } else {
            self.universal_gpu_bytes
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerrainTextureLayer {
    pub texture_set: TerrainTextureSetId,
    pub surface: TerrainSurfaceId,
    pub layer: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainProfile {
    pub space: WorldSpaceId,
    pub texture_set: TerrainTextureSetId,
    pub weight_resolution: u16,
    pub macro_scales: [f32; 3],
    pub macro_contrast: f32,
    pub macro_albedo_strength: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainWeightPage {
    pub resolution: u16,
    pub rgba: Vec<u8>,
}

/// Endpoint-inclusive terrain samples for one streamed cell.
///
/// Constructors round heights to [`TERRAIN_HEIGHT_STEP`] and normals to
/// [`TERRAIN_NORMAL_BITS`]. The height grid is world-wide, never per page, so cookers that
/// evaluate shared endpoints identically still produce bit-identical edges at any altitude.
/// Serialization stores the rounded values compactly and round-trips them exactly; see
/// `terrain_heightfield_codec`.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainHeightfield {
    pub resolution: u16,
    pub heights: Vec<f32>,
    pub normals_oct: Vec<[i16; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainSurfaceSample {
    pub height: f32,
    pub normal: [f32; 3],
}

impl TerrainHeightfield {
    pub fn from_heights(
        resolution: u16,
        heights: &[f32],
        minimum_height: f32,
        maximum_height: f32,
        cell_size: f32,
    ) -> Result<Self, TerrainHeightfieldError> {
        if !cell_size.is_finite() || cell_size <= 0.0 {
            return Err(TerrainHeightfieldError::InvalidDimensionsOrRange);
        }

        let resolution_usize = usize::from(resolution);
        let spacing = cell_size / (resolution_usize.saturating_sub(1)) as f32;
        let mut normals = Vec::with_capacity(heights.len());

        if resolution_usize >= 2 && heights.len() == resolution_usize * resolution_usize {
            for z in 0..resolution_usize {
                for x in 0..resolution_usize {
                    let left = x.saturating_sub(1);
                    let right = (x + 1).min(resolution_usize - 1);
                    let down = z.saturating_sub(1);
                    let up = (z + 1).min(resolution_usize - 1);
                    let height_dx = (heights[z * resolution_usize + right]
                        - heights[z * resolution_usize + left])
                        / ((right - left) as f32 * spacing);
                    let height_dz = (heights[up * resolution_usize + x]
                        - heights[down * resolution_usize + x])
                        / ((up - down) as f32 * spacing);
                    normals.push(normalize3([-height_dx, 1.0, -height_dz]));
                }
            }
        }

        Self::from_heights_and_normals(
            resolution,
            heights,
            &normals,
            minimum_height,
            maximum_height,
        )
    }

    pub fn from_heights_and_normals(
        resolution: u16,
        heights: &[f32],
        normals: &[[f32; 3]],
        minimum_height: f32,
        maximum_height: f32,
    ) -> Result<Self, TerrainHeightfieldError> {
        let resolution_usize = usize::from(resolution);

        if !(2..=usize::from(MAX_TERRAIN_HEIGHTFIELD_RESOLUTION)).contains(&resolution_usize)
            || heights.len() != resolution_usize * resolution_usize
            || normals.len() != heights.len()
            || !minimum_height.is_finite()
            || !maximum_height.is_finite()
            || maximum_height < minimum_height
            || !heights.iter().all(|height| height.is_finite())
            || !normals
                .iter()
                .flatten()
                .all(|component| component.is_finite())
        {
            return Err(TerrainHeightfieldError::InvalidDimensionsOrRange);
        }

        if heights
            .iter()
            .any(|height| *height < minimum_height || *height > maximum_height)
        {
            return Err(TerrainHeightfieldError::HeightOutsideRange);
        }

        // The range is checked before rounding; bounds off the grid may be exceeded by at most
        // half a step.
        let heightfield = Self {
            resolution,
            heights: heights
                .iter()
                .copied()
                .map(quantize_terrain_height)
                .collect(),
            normals_oct: normals
                .iter()
                .copied()
                .map(encode_octahedral_normal)
                .collect(),
        };
        heightfield.validate()?;
        Ok(heightfield)
    }

    pub fn validate(&self) -> Result<(), TerrainHeightfieldError> {
        let resolution = usize::from(self.resolution);

        if !(2..=usize::from(MAX_TERRAIN_HEIGHTFIELD_RESOLUTION)).contains(&resolution)
            || self.heights.len() != resolution * resolution
            || self.normals_oct.len() != resolution * resolution
            || !self.heights.iter().all(|height| height.is_finite())
        {
            return Err(TerrainHeightfieldError::InvalidDimensionsOrRange);
        }

        Ok(())
    }

    pub fn height_at(&self, x: usize, z: usize) -> f32 {
        let resolution = usize::from(self.resolution);
        debug_assert!(x < resolution && z < resolution);
        self.heights[z * resolution + x]
    }

    pub fn height_bounds(&self) -> [f32; 2] {
        self.heights
            .iter()
            .fold([f32::INFINITY, f32::NEG_INFINITY], |bounds, &height| {
                [bounds[0].min(height), bounds[1].max(height)]
            })
    }

    /// Samples the rendered grid triangles in local metres. Inputs clamp to the page edges.
    /// Validate the field when loading/constructing it, not for every sample.
    pub fn sample(&self, local_xz: [f32; 2], cell_size: f32) -> TerrainSurfaceSample {
        let (corners, weights) = self.triangle(local_xz, cell_size);
        let mut height = 0.0;
        let mut normal = [0.0_f32; 3];

        for (index, weight) in corners.into_iter().zip(weights) {
            height += self.heights[index] * weight;
            let vertex_normal = decode_octahedral_normal(self.normals_oct[index]);
            normal[0] += vertex_normal[0] * weight;
            normal[1] += vertex_normal[1] * weight;
            normal[2] += vertex_normal[2] * weight;
        }

        TerrainSurfaceSample {
            height,
            normal: normalize3(normal),
        }
    }

    /// The height of [`Self::sample`], bit for bit, without decoding normals.
    pub fn sample_height(&self, local_xz: [f32; 2], cell_size: f32) -> f32 {
        let (corners, weights) = self.triangle(local_xz, cell_size);
        let mut height = 0.0;

        for (index, weight) in corners.into_iter().zip(weights) {
            height += self.heights[index] * weight;
        }

        height
    }

    /// Corner samples and rendered-triangle weights at a clamped local position.
    fn triangle(&self, local_xz: [f32; 2], cell_size: f32) -> ([usize; 4], [f32; 4]) {
        debug_assert!(cell_size.is_finite() && cell_size > 0.0);
        let resolution = usize::from(self.resolution);
        debug_assert!((2..=usize::from(MAX_TERRAIN_HEIGHTFIELD_RESOLUTION)).contains(&resolution));
        debug_assert_eq!(self.heights.len(), resolution * resolution);
        debug_assert_eq!(self.normals_oct.len(), resolution * resolution);
        let local = local_xz.map(|v| v / cell_size);
        vegetation::surface_triangle(local, resolution)
    }

    pub fn normal_at(&self, x: usize, z: usize) -> [f32; 3] {
        let resolution = usize::from(self.resolution);
        debug_assert!(x < resolution && z < resolution);
        decode_octahedral_normal(self.normals_oct[z * resolution + x])
    }
}

pub(crate) fn encode_octahedral_normal(normal: [f32; 3]) -> [i16; 2] {
    vegetation::encode_octahedral_normal_bits(normal, TERRAIN_NORMAL_BITS)
}

pub(crate) use vegetation::decode_octahedral_normal;

fn normalize3(value: [f32; 3]) -> [f32; 3] {
    vegetation::normalize3_or(value, [0.0, 1.0, 0.0])
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TerrainHeightfieldError {
    #[error("terrain heightfield dimensions, samples or height range are invalid")]
    InvalidDimensionsOrRange,
    #[error("terrain height sample lies outside the allowed height range")]
    HeightOutsideRange,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainHeightfieldPage {
    pub heightfield: TerrainHeightfield,
    /// The cell's ground surfaces, whose slots `wear` names. Which of them shows where is in
    /// the cell's control tile (`terrain_control`).
    pub surfaces: Vec<TerrainSurfaceId>,
    /// How worn the ground is, where roads or paths wear it; none where nothing does.
    pub wear: Option<TerrainWear>,
}
