pub mod terrain_hierarchy;
pub use terrain_hierarchy::*;

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use vegetation::VegetationFieldPageData;

/// Default authoring source relative to the repository root.
pub const DEFAULT_PROJECT_DATABASE: &str = "content/world.project.sqlite";
/// Default cooked world relative to the asset root (also used in iOS bundles).
pub const DEFAULT_RUNTIME_DATABASE: &str = "generated/world.runtime.sqlite";

pub const DEFAULT_CELL_SIZE: f32 = 32.0;
pub const MAX_DECODED_PAGE_BYTES: u64 = 64 * 1024 * 1024;
pub const PROJECT_SCHEMA_VERSION: i64 = 22;
pub const RUNTIME_SCHEMA_VERSION: i64 = 17;
pub const PAGE_PAYLOAD_VERSION: u16 = 8;
pub const MAX_TERRAIN_SURFACES_PER_CELL: usize = 8;
pub const MAX_TERRAIN_WEIGHT_PAGES: usize = 2;
pub const MAX_TERRAIN_WEIGHT_RESOLUTION: u16 = 257;
pub const MAX_TERRAIN_HEIGHTFIELD_RESOLUTION: u16 = 257;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorldSpaceId(pub i64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct CellCoord {
    pub x: i32,
    pub z: i32,
}

impl CellCoord {
    pub const ZERO: Self = Self { x: 0, z: 0 };

    pub fn containing(world_x: f64, world_z: f64, cell_size: f32) -> Self {
        assert!(cell_size.is_finite() && cell_size > 0.0);
        let cell_size = f64::from(cell_size);
        Self {
            x: floor_to_i32(world_x / cell_size),
            z: floor_to_i32(world_z / cell_size),
        }
    }

    pub fn origin(self, cell_size: f32) -> [f64; 2] {
        [
            f64::from(self.x) * f64::from(cell_size),
            f64::from(self.z) * f64::from(cell_size),
        ]
    }

    pub fn center(self, cell_size: f32) -> [f32; 2] {
        [
            (self.x as f32 + 0.5) * cell_size,
            (self.z as f32 + 0.5) * cell_size,
        ]
    }

    pub fn chebyshev_distance(self, other: Self) -> u32 {
        self.x.abs_diff(other.x).max(self.z.abs_diff(other.z))
    }
}

fn floor_to_i32(value: f64) -> i32 {
    let value = value.floor();
    assert!(
        value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX),
        "world coordinate is outside the supported cell range"
    );
    value as i32
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WorldPosition {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub local: [f32; 3],
}

impl WorldPosition {
    pub fn from_world(space: WorldSpaceId, world: [f64; 3], cell_size: f32) -> Self {
        let cell = CellCoord::containing(world[0], world[2], cell_size);
        let origin = cell.origin(cell_size);
        Self {
            space,
            cell,
            local: [
                (world[0] - origin[0]) as f32,
                world[1] as f32,
                (world[2] - origin[1]) as f32,
            ],
        }
    }

    pub fn relative_to(self, origin_cell: CellCoord, cell_size: f32) -> [f32; 3] {
        [
            (i64::from(self.cell.x) - i64::from(origin_cell.x)) as f32 * cell_size + self.local[0],
            self.local[1],
            (i64::from(self.cell.z) - i64::from(origin_cell.z)) as f32 * cell_size + self.local[2],
        ]
    }

    pub fn world(self, cell_size: f32) -> [f64; 3] {
        let origin = self.cell.origin(cell_size);
        [
            origin[0] + f64::from(self.local[0]),
            f64::from(self.local[1]),
            origin[1] + f64::from(self.local[2]),
        ]
    }

    pub fn translated(self, delta: [f32; 3], cell_size: f32) -> Self {
        let world = self.world(cell_size);
        Self::from_world(
            self.space,
            [
                world[0] + f64::from(delta[0]),
                world[1] + f64::from(delta[1]),
                world[2] + f64::from(delta[2]),
            ],
            cell_size,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StableObjectId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectDefinitionId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TerrainSurfaceId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TerrainTextureSetId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AssetId(pub [u8; 32]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(i64)]
pub enum PageDomain {
    TerrainRender = 1,
    StaticObjects = 2,
    Foliage = 3,
    Collision = 5,
    Navigation = 6,
    ShadowCasters = 7,
    GameplayObjects = 8,
    Vegetation = 9,
}

impl TryFrom<i64> for PageDomain {
    type Error = UnknownPageDomain;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::TerrainRender),
            2 => Ok(Self::StaticObjects),
            3 => Ok(Self::Foliage),
            5 => Ok(Self::Collision),
            6 => Ok(Self::Navigation),
            7 => Ok(Self::ShadowCasters),
            8 => Ok(Self::GameplayObjects),
            9 => Ok(Self::Vegetation),
            _ => Err(UnknownPageDomain(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownPageDomain(pub i64);

impl fmt::Display for UnknownPageDomain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown page domain {}", self.0)
    }
}

impl Error for UnknownPageDomain {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageKey {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub domain: PageDomain,
    pub lod: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i64)]
pub enum PageCodec {
    Raw = 0,
    Zstd = 1,
}

impl TryFrom<i64> for PageCodec {
    type Error = UnknownPageCodec;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Raw),
            1 => Ok(Self::Zstd),
            _ => Err(UnknownPageCodec(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownPageCodec(pub i64);

impl fmt::Display for UnknownPageCodec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown page codec {}", self.0)
    }
}

impl Error for UnknownPageCodec {}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainRenderPage {
    pub height: f32,
    /// Local material slots. Their order maps directly to RGBA channels in
    /// `weight_pages`, four surfaces per page.
    pub surfaces: Vec<TerrainSurfaceId>,
    pub weight_pages: Vec<TerrainWeightPage>,
}

/// Endpoint-inclusive terrain samples for one streamed cell.
///
/// Heights retain source f32 precision even in worlds with kilometre-scale relief. Cookers
/// must evaluate shared endpoints identically; there is no per-page height quantization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

        let heightfield = Self {
            resolution,
            heights: heights.to_vec(),
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
    pub fn sample(&self, local_xz: [f32; 2], cell_size: f32) -> TerrainSurfaceSample {
        debug_assert!(self.validate().is_ok());
        debug_assert!(cell_size.is_finite() && cell_size > 0.0);
        let resolution = usize::from(self.resolution);
        let maximum_index = (resolution - 1) as f32;
        let grid_x = (local_xz[0] / cell_size).clamp(0.0, 1.0) * maximum_index;
        let grid_z = (local_xz[1] / cell_size).clamp(0.0, 1.0) * maximum_index;
        let x0 = grid_x.floor() as usize;
        let z0 = grid_z.floor() as usize;
        let x1 = (x0 + 1).min(resolution - 1);
        let z1 = (z0 + 1).min(resolution - 1);
        let tx = grid_x - x0 as f32;
        let tz = grid_z - z0 as f32;
        let corners = [(x0, z0), (x1, z0), (x0, z1), (x1, z1)];
        let weights = vegetation::surface_triangle_weights(tx, tz);
        let mut height = 0.0;
        let mut normal = [0.0_f32; 3];
        for ((x, z), weight) in corners.into_iter().zip(weights) {
            height += self.height_at(x, z) * weight;
            let vertex_normal = self.normal_at(x, z);
            normal[0] += vertex_normal[0] * weight;
            normal[1] += vertex_normal[1] * weight;
            normal[2] += vertex_normal[2] * weight;
        }
        TerrainSurfaceSample {
            height,
            normal: normalize3(normal),
        }
    }

    pub fn normal_at(&self, x: usize, z: usize) -> [f32; 3] {
        let resolution = usize::from(self.resolution);
        debug_assert!(x < resolution && z < resolution);
        decode_octahedral_normal(self.normals_oct[z * resolution + x])
    }
}

fn encode_octahedral_normal(normal: [f32; 3]) -> [i16; 2] {
    let normal = normalize3(normal);
    let inverse_l1 = (normal[0].abs() + normal[1].abs() + normal[2].abs()).recip();
    let mut encoded = [normal[0] * inverse_l1, normal[2] * inverse_l1];
    if normal[1] < 0.0 {
        encoded = [
            (1.0 - encoded[1].abs()).copysign(encoded[0]),
            (1.0 - encoded[0].abs()).copysign(encoded[1]),
        ];
    }
    [
        (encoded[0].clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16,
        (encoded[1].clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16,
    ]
}

fn decode_octahedral_normal(encoded: [i16; 2]) -> [f32; 3] {
    let encoded = [
        f32::from(encoded[0]) / f32::from(i16::MAX),
        f32::from(encoded[1]) / f32::from(i16::MAX),
    ];
    let mut normal = [
        encoded[0],
        1.0 - encoded[0].abs() - encoded[1].abs(),
        encoded[1],
    ];
    if normal[1] < 0.0 {
        normal = [
            (1.0 - encoded[1].abs()).copysign(encoded[0]),
            normal[1],
            (1.0 - encoded[0].abs()).copysign(encoded[1]),
        ];
    }
    normalize3(normal)
}

fn normalize3(value: [f32; 3]) -> [f32; 3] {
    let length_squared = value[0] * value[0] + value[1] * value[1] + value[2] * value[2];
    if length_squared <= f32::EPSILON || !length_squared.is_finite() {
        return [0.0, 1.0, 0.0];
    }
    let inverse_length = length_squared.sqrt().recip();
    [
        value[0] * inverse_length,
        value[1] * inverse_length,
        value[2] * inverse_length,
    ]
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
    /// Local material slots. Their order maps directly to RGBA channels in
    /// `weight_pages`, four surfaces per page.
    pub surfaces: Vec<TerrainSurfaceId>,
    pub weight_pages: Vec<TerrainWeightPage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticObjectInstance {
    /// Derived environment placement, never an editable manual source object.
    pub generated: bool,
    pub id: StableObjectId,
    pub asset: AssetId,
    pub translation: [f32; 3],
    pub yaw: f32,
    pub scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticObjectsPage {
    pub instances: Vec<StaticObjectInstance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i64)]
pub enum ObjectActivationPolicy {
    RenderOnly = 0,
    Proximity = 1,
}

impl TryFrom<i64> for ObjectActivationPolicy {
    type Error = UnknownObjectActivationPolicy;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::RenderOnly),
            1 => Ok(Self::Proximity),
            _ => Err(UnknownObjectActivationPolicy(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownObjectActivationPolicy(pub i64);

impl fmt::Display for UnknownObjectActivationPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown object activation policy {}", self.0)
    }
}

impl Error for UnknownObjectActivationPolicy {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameplayObjectInstance {
    pub id: StableObjectId,
    pub definition: ObjectDefinitionId,
    pub translation: [f32; 3],
    pub yaw: f32,
    pub scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameplayObjectsPage {
    pub instances: Vec<GameplayObjectInstance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PagePayload {
    TerrainRender(TerrainRenderPage),
    StaticObjects(StaticObjectsPage),
    ShadowCasters(StaticObjectsPage),
    GameplayObjects(GameplayObjectsPage),
    /// Appended to preserve the serialized discriminants of all legacy variants.
    TerrainHeightfield(TerrainHeightfieldPage),
    /// Terrain-independent V2 coverage fields, joined to the resident terrain page at runtime.
    Vegetation(VegetationFieldPageData),
}

impl PagePayload {
    pub fn domain(&self) -> PageDomain {
        match self {
            Self::TerrainRender(_) => PageDomain::TerrainRender,
            Self::StaticObjects(_) => PageDomain::StaticObjects,
            Self::ShadowCasters(_) => PageDomain::ShadowCasters,
            Self::GameplayObjects(_) => PageDomain::GameplayObjects,
            Self::TerrainHeightfield(_) => PageDomain::TerrainRender,
            Self::Vegetation(_) => PageDomain::Vegetation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct VersionedPagePayload {
    version: u16,
    payload: PagePayload,
}

pub fn encode_page_payload(payload: &PagePayload) -> Result<Vec<u8>, bincode::error::EncodeError> {
    bincode::serde::encode_to_vec(
        VersionedPagePayload {
            version: PAGE_PAYLOAD_VERSION,
            payload: payload.clone(),
        },
        bincode::config::standard()
            .with_little_endian()
            .with_fixed_int_encoding(),
    )
}

pub fn decode_page_payload(bytes: &[u8]) -> Result<PagePayload, PagePayloadDecodeError> {
    let (versioned, consumed): (VersionedPagePayload, usize) = bincode::serde::decode_from_slice(
        bytes,
        bincode::config::standard()
            .with_little_endian()
            .with_fixed_int_encoding()
            .with_limit::<{ MAX_DECODED_PAGE_BYTES as usize }>(),
    )?;
    if consumed != bytes.len() {
        return Err(PagePayloadDecodeError::TrailingBytes {
            consumed,
            total: bytes.len(),
        });
    }
    if versioned.version != PAGE_PAYLOAD_VERSION {
        return Err(PagePayloadDecodeError::UnsupportedVersion(
            versioned.version,
        ));
    }
    Ok(versioned.payload)
}

#[derive(Debug, thiserror::Error)]
pub enum PagePayloadDecodeError {
    #[error("page payload codec failed: {0}")]
    Bincode(#[from] bincode::error::DecodeError),
    #[error("page payload contains trailing bytes: decoded {consumed} of {total}")]
    TrailingBytes { consumed: usize, total: usize },
    #[error("page payload version {0} is not supported")]
    UnsupportedVersion(u16),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_coordinates_use_floor_cells() {
        assert_eq!(
            CellCoord::containing(-0.01, -32.0, 32.0),
            CellCoord { x: -1, z: -1 }
        );
        assert_eq!(
            CellCoord::containing(-32.01, 31.99, 32.0),
            CellCoord { x: -2, z: 0 }
        );
    }

    #[test]
    fn world_position_is_normalized_and_relative() {
        let position = WorldPosition::from_world(WorldSpaceId(7), [-32.5, 4.0, 64.25], 32.0);
        assert_eq!(position.cell, CellCoord { x: -2, z: 2 });
        assert_eq!(position.local, [31.5, 4.0, 0.25]);
        assert_eq!(
            position.relative_to(CellCoord { x: -1, z: 1 }, 32.0),
            [-0.5, 4.0, 32.25]
        );
        assert_eq!(position.world(32.0), [-32.5, 4.0, 64.25]);
    }

    #[test]
    fn world_position_translation_stays_precise_far_from_render_origin() {
        let position = WorldPosition {
            space: WorldSpaceId(7),
            cell: CellCoord {
                x: 1_000_000,
                z: -1_000_000,
            },
            local: [31.5, 4.0, 0.25],
        };
        let moved = position.translated([1.0, 2.0, -1.0], 32.0);

        assert_eq!(
            moved.cell,
            CellCoord {
                x: 1_000_001,
                z: -1_000_001,
            }
        );
        assert_eq!(moved.local, [0.5, 6.0, 31.25]);
        assert_eq!(moved.relative_to(position.cell, 32.0), [32.5, 6.0, -0.75]);
    }

    #[test]
    fn page_payload_round_trips_with_version() {
        let payload = PagePayload::TerrainRender(TerrainRenderPage {
            height: 2.0,
            surfaces: vec![TerrainSurfaceId([4; 16]), TerrainSurfaceId([5; 16])],
            weight_pages: vec![TerrainWeightPage {
                resolution: 2,
                rgba: vec![255, 0, 0, 0, 128, 127, 0, 0, 64, 191, 0, 0, 0, 255, 0, 0],
            }],
        });
        let bytes = encode_page_payload(&payload).unwrap();
        assert_eq!(decode_page_payload(&bytes).unwrap(), payload);
    }

    #[test]
    fn heightfield_payload_round_trips_and_samples_relief() {
        let heightfield = TerrainHeightfield::from_heights(
            3,
            &[0.0, 1.0, 2.0, 1.0, 2.0, 3.0, 2.0, 3.0, 4.0],
            0.0,
            4.0,
            4.0,
        )
        .unwrap();
        let payload = PagePayload::TerrainHeightfield(TerrainHeightfieldPage {
            heightfield: heightfield.clone(),
            surfaces: vec![TerrainSurfaceId([4; 16])],
            weight_pages: Vec::new(),
        });

        let bytes = encode_page_payload(&payload).unwrap();
        assert_eq!(decode_page_payload(&bytes).unwrap(), payload);
        assert_eq!(payload.domain(), PageDomain::TerrainRender);

        let center = heightfield.sample([2.0, 2.0], 4.0);
        assert!((center.height - 2.0).abs() < 0.001);
        assert!(center.normal[0] < -0.4 && center.normal[2] < -0.4);
        assert!(center.normal[1] > 0.5);
    }

    #[test]
    fn source_height_precision_keeps_adjacent_edges_identical() {
        let left =
            TerrainHeightfield::from_heights(2, &[0.0, 1.25, 0.5, 1.75], -8.0, 8.0, 32.0).unwrap();
        let right =
            TerrainHeightfield::from_heights(2, &[1.25, 2.0, 1.75, 2.5], -8.0, 8.0, 32.0).unwrap();

        assert_eq!(left.heights[1], right.heights[0]);
        assert_eq!(left.heights[3], right.heights[2]);
        assert_eq!(left.height_at(1, 0), right.height_at(0, 0));
        assert_eq!(left.height_at(1, 1), right.height_at(0, 1));
    }

    #[test]
    fn mountain_range_preserves_shallow_road_relief() {
        let heights = [1500.0, 1499.995, 1499.99, 1500.0];
        let field = TerrainHeightfield::from_heights(2, &heights, -500.0, 2500.0, 8.0).unwrap();
        assert_eq!(field.heights, heights);
        assert_eq!(field.height_bounds(), [1499.99, 1500.0]);
        let mut invalid = field.clone();
        invalid.heights[1] = f32::NAN;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn terrain_and_vegetation_follow_both_triangles_of_a_saddle() {
        let terrain =
            TerrainHeightfield::from_heights(2, &[0.0, 10.0, 20.0, 0.0], 0.0, 20.0, 1.0).unwrap();
        let surface = vegetation::VegetationSurfaceField {
            resolution: 2,
            heights: terrain.heights.clone(),
            normals_oct: terrain.normals_oct.clone(),
            validity: vec![255; 4],
        };
        // The rendered 00--11 diagonal is zero; bilinear sampling would return 7.5.
        for (xz, expected) in [
            ([0.5, 0.5], 0.0),
            ([0.75, 0.25], 5.0),
            ([0.25, 0.75], 10.0),
            ([1.0, 1.0], 0.0),
            ([1.0, 0.5], 5.0),
        ] {
            let ground = terrain.sample(xz, 1.0);
            let grass = surface.sample([0.0; 2], 1.0, xz);
            assert_eq!(ground.height, expected);
            assert_eq!(ground.height, grass.height);
            for axis in 0..3 {
                assert!((ground.normal[axis] - grass.normal[axis]).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn gameplay_page_keeps_definition_identity() {
        let definition = ObjectDefinitionId([3; 16]);
        let payload = PagePayload::GameplayObjects(GameplayObjectsPage {
            instances: vec![GameplayObjectInstance {
                id: StableObjectId([7; 16]),
                definition,
                translation: [2.0, 0.0, 4.0],
                yaw: 0.5,
                scale: 1.0,
            }],
        });
        let bytes = encode_page_payload(&payload).unwrap();
        assert_eq!(decode_page_payload(&bytes).unwrap(), payload);
        assert_eq!(payload.domain(), PageDomain::GameplayObjects);
    }

    #[test]
    fn vegetation_page_round_trips_without_duplicating_terrain_relief() {
        let data = vegetation::fixtures::reference_page([0.0, 0.0]).data();
        let payload = PagePayload::Vegetation(data.clone());

        let bytes = encode_page_payload(&payload).unwrap();
        assert_eq!(decode_page_payload(&bytes).unwrap(), payload);
        assert_eq!(payload.domain(), PageDomain::Vegetation);
        assert_eq!(data.fields.len(), 4);
    }
}
