use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

pub const DEFAULT_CELL_SIZE: f32 = 32.0;
pub const MAX_DECODED_PAGE_BYTES: u64 = 64 * 1024 * 1024;
pub const PROJECT_SCHEMA_VERSION: i64 = 7;
pub const RUNTIME_SCHEMA_VERSION: i64 = 7;
pub const PAGE_PAYLOAD_VERSION: u16 = 5;
pub const MAX_TERRAIN_SURFACES_PER_CELL: usize = 8;
pub const MAX_TERRAIN_WEIGHT_PAGES: usize = 2;
pub const MAX_TERRAIN_WEIGHT_RESOLUTION: u16 = 257;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorldSpaceId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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
            (self.cell.x - origin_cell.x) as f32 * cell_size + self.local[0],
            self.local[1],
            (self.cell.z - origin_cell.z) as f32 * cell_size + self.local[2],
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StableObjectId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectDefinitionId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroundCoverSpeciesId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroundCoverLayerId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TerrainSurfaceId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TerrainTextureSetId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(pub [u8; 32]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(i64)]
pub enum PageDomain {
    TerrainRender = 1,
    StaticObjects = 2,
    Foliage = 3,
    GroundCover = 4,
    Collision = 5,
    Navigation = 6,
    ShadowCasters = 7,
    GameplayObjects = 8,
}

impl TryFrom<i64> for PageDomain {
    type Error = UnknownPageDomain;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::TerrainRender),
            2 => Ok(Self::StaticObjects),
            3 => Ok(Self::Foliage),
            4 => Ok(Self::GroundCover),
            5 => Ok(Self::Collision),
            6 => Ok(Self::Navigation),
            7 => Ok(Self::ShadowCasters),
            8 => Ok(Self::GameplayObjects),
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

/// One globally defined decorative ground-cover species.
///
/// Ground cover has no stable identity per plant. A species describes its
/// visual response while [`GroundCoverCluster`] describes authored coverage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundCoverSpecies {
    pub id: GroundCoverSpeciesId,
    pub key: String,
    pub bottom_color: [f32; 3],
    pub top_color: [f32; 3],
    pub minimum_card_height: f32,
    pub maximum_card_height: f32,
    pub minimum_card_width: f32,
    pub maximum_card_width: f32,
    pub flattened_card_probability: f32,
    pub maximum_wind_displacement: f32,
}

/// A renderer-sized unit of authored ground-cover coverage.
///
/// Coordinates are local to the owning world cell. `coverage_half_extents`
/// describe the authored patch used for density, while `half_extents` are
/// conservative animated/card bounds used only for visibility tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundCoverCluster {
    pub species: GroundCoverSpeciesId,
    pub local_center: [f32; 3],
    pub half_extents: [f32; 3],
    pub coverage_half_extents: [f32; 2],
    pub density_per_square_meter: f32,
    pub seed: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundCoverPage {
    pub clusters: Vec<GroundCoverCluster>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticObjectInstance {
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
    GroundCover(GroundCoverPage),
    ShadowCasters(StaticObjectsPage),
    GameplayObjects(GameplayObjectsPage),
}

impl PagePayload {
    pub fn domain(&self) -> PageDomain {
        match self {
            Self::TerrainRender(_) => PageDomain::TerrainRender,
            Self::StaticObjects(_) => PageDomain::StaticObjects,
            Self::GroundCover(_) => PageDomain::GroundCover,
            Self::ShadowCasters(_) => PageDomain::ShadowCasters,
            Self::GameplayObjects(_) => PageDomain::GameplayObjects,
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
    fn ground_cover_page_round_trips_without_expanding_tufts() {
        let species = GroundCoverSpeciesId([11; 16]);
        let payload = PagePayload::GroundCover(GroundCoverPage {
            clusters: vec![GroundCoverCluster {
                species,
                local_center: [4.0, 0.0, 6.0],
                half_extents: [1.25, 0.5, 1.25],
                coverage_half_extents: [1.0, 1.0],
                density_per_square_meter: 8.0,
                seed: 42,
            }],
        });

        let bytes = encode_page_payload(&payload).unwrap();
        assert_eq!(decode_page_payload(&bytes).unwrap(), payload);
        assert_eq!(payload.domain(), PageDomain::GroundCover);
    }
}
