use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

pub const DEFAULT_CELL_SIZE: f32 = 32.0;
pub const MAX_DECODED_PAGE_BYTES: u64 = 64 * 1024 * 1024;
pub const PROJECT_SCHEMA_VERSION: i64 = 9;
pub const RUNTIME_SCHEMA_VERSION: i64 = 8;
pub const PAGE_PAYLOAD_VERSION: u16 = 5;
pub const MAX_TERRAIN_SURFACES_PER_CELL: usize = 8;
pub const MAX_TERRAIN_WEIGHT_PAGES: usize = 2;
pub const MAX_TERRAIN_WEIGHT_RESOLUTION: u16 = 257;
pub const GROUND_COVER_ARTWORK_RESOLUTION: u16 = 256;
pub const MAX_GROUND_COVER_ARTWORK_VARIANTS: u8 = 8;
pub const MAX_GROUND_COVER_ARTWORK_ATLAS_LAYERS: u32 = 256;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StableObjectId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectDefinitionId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroundCoverSpeciesId(pub [u8; 16]);

/// Stable authoring identity for one reusable ground-cover visual definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroundCoverVisualId(pub [u8; 16]);

/// Stable authoring identity for one reusable ground-cover population preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroundCoverPresetId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroundCoverLayerId(pub [u8; 16]);

/// Stable authoring identity for one named painted ground-cover region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroundCoverRegionId(pub [u8; 16]);

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

/// Deterministic source recipe for the silhouettes painted into one set of card variants.
///
/// Every dimension is normalized to the artwork canvas. Physical card dimensions remain separate
/// species properties, so changing a silhouette does not silently change world-space bounds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GroundCoverBladeRecipe {
    pub variant_count: u8,
    pub blade_count: u16,
    pub seed: u32,
    pub minimum_blade_height: f32,
    pub maximum_blade_height: f32,
    pub base_jitter: f32,
    pub minimum_blade_half_width: f32,
    pub maximum_blade_half_width: f32,
    pub maximum_lean: f32,
    pub maximum_curve: f32,
    pub maximum_s_curve: f32,
}

impl GroundCoverBladeRecipe {
    /// Frozen recipe used by the original optimized meadow renderer.
    pub const fn built_in_v1() -> Self {
        Self {
            variant_count: 4,
            blade_count: 34,
            seed: 0x6f53_91d2,
            minimum_blade_height: 0.48,
            maximum_blade_height: 1.0,
            base_jitter: 0.7,
            minimum_blade_half_width: 0.004,
            maximum_blade_half_width: 0.008,
            maximum_lean: 0.18,
            maximum_curve: 0.10,
            maximum_s_curve: 0.045,
        }
    }

    pub fn is_valid(self) -> bool {
        (1..=MAX_GROUND_COVER_ARTWORK_VARIANTS).contains(&self.variant_count)
            && (1..=128).contains(&self.blade_count)
            && self.minimum_blade_height.is_finite()
            && (0.05..=1.0).contains(&self.minimum_blade_height)
            && self.maximum_blade_height.is_finite()
            && (self.minimum_blade_height..=1.0).contains(&self.maximum_blade_height)
            && self.base_jitter.is_finite()
            && (0.0..=1.0).contains(&self.base_jitter)
            && self.minimum_blade_half_width.is_finite()
            && (0.001..=0.1).contains(&self.minimum_blade_half_width)
            && self.maximum_blade_half_width.is_finite()
            && (self.minimum_blade_half_width..=0.1).contains(&self.maximum_blade_half_width)
            && self.maximum_lean.is_finite()
            && (0.0..=0.5).contains(&self.maximum_lean)
            && self.maximum_curve.is_finite()
            && (0.0..=0.5).contains(&self.maximum_curve)
            && self.maximum_s_curve.is_finite()
            && (0.0..=0.25).contains(&self.maximum_s_curve)
    }
}

/// Cooked R8 card variants, including the complete coverage-preserving mip chain per layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundCoverCardArtwork {
    pub resolution: u16,
    pub variant_count: u8,
    pub mip_level_count: u8,
    pub coverage_mips: Vec<u8>,
}

impl GroundCoverCardArtwork {
    pub fn expected_byte_len(&self) -> Option<usize> {
        let mut side = usize::from(self.resolution);
        let mut per_variant = 0_usize;
        for _ in 0..self.mip_level_count {
            per_variant = per_variant.checked_add(side.checked_mul(side)?)?;
            side = (side / 2).max(1);
        }
        per_variant.checked_mul(usize::from(self.variant_count))
    }

    pub fn is_valid(&self) -> bool {
        self.resolution == GROUND_COVER_ARTWORK_RESOLUTION
            && (1..=MAX_GROUND_COVER_ARTWORK_VARIANTS).contains(&self.variant_count)
            && self.mip_level_count == self.resolution.ilog2() as u8 + 1
            && self.expected_byte_len() == Some(self.coverage_mips.len())
    }

    pub fn base_variant(&self, variant: u8) -> Option<&[u8]> {
        if variant >= self.variant_count || !self.is_valid() {
            return None;
        }
        let per_variant = self.coverage_mips.len() / usize::from(self.variant_count);
        let start = usize::from(variant) * per_variant;
        let base_len = usize::from(self.resolution).pow(2);
        Some(&self.coverage_mips[start..start + base_len])
    }
}

/// Expands a bounded recipe into deterministic R8 variants and coverage-preserving mips.
pub fn generate_ground_cover_card_artwork(
    recipe: GroundCoverBladeRecipe,
) -> GroundCoverCardArtwork {
    assert!(recipe.is_valid(), "invalid ground-cover blade recipe");

    #[derive(Clone, Copy)]
    struct Blade {
        base: f32,
        height: f32,
        lean: f32,
        curve: f32,
        s_curve: f32,
        half_width: f32,
    }

    let size = usize::from(GROUND_COVER_ARTWORK_RESOLUTION);
    let texel = 1.0 / f32::from(GROUND_COVER_ARTWORK_RESOLUTION);
    let mut pixels = Vec::new();
    let mut mip_level_count = 0;
    for layer in 0..u32::from(recipe.variant_count) {
        let layer_seed = hash_ground_cover_u32(recipe.seed ^ layer.wrapping_mul(0x85eb_ca6b));
        let blades: Vec<_> = (0..u32::from(recipe.blade_count))
            .map(|index| {
                let seed = hash_ground_cover_u32(
                    index.wrapping_mul(0x9e37_79b9) ^ layer_seed ^ layer.wrapping_mul(0xc2b2_ae35),
                );
                let centered = (index as f32 + 0.5) / f32::from(recipe.blade_count);
                Blade {
                    base: (centered
                        + (ground_cover_unit_float(seed) - 0.5)
                            * (recipe.base_jitter / f32::from(recipe.blade_count)))
                    .clamp(0.015, 0.985),
                    height: recipe.minimum_blade_height
                        + ground_cover_unit_float(seed ^ 0xa511_e9b3)
                            * (recipe.maximum_blade_height - recipe.minimum_blade_height),
                    lean: (ground_cover_unit_float(seed ^ 0x63d8_3595) * 2.0 - 1.0)
                        * recipe.maximum_lean,
                    curve: (ground_cover_unit_float(seed ^ 0xc2b2_ae35) * 2.0 - 1.0)
                        * recipe.maximum_curve,
                    s_curve: (ground_cover_unit_float(seed ^ 0x1656_67b1) * 2.0 - 1.0)
                        * recipe.maximum_s_curve,
                    half_width: recipe.minimum_blade_half_width
                        + ground_cover_unit_float(seed ^ 0x27d4_eb2f)
                            * (recipe.maximum_blade_half_width - recipe.minimum_blade_half_width),
                }
            })
            .collect();

        let mut level = vec![0_u8; size * size];
        for y in 0..size {
            let vertical = 1.0 - y as f32 / (size - 1) as f32;
            for x in 0..size {
                let horizontal = x as f32 / (size - 1) as f32;
                let mut alpha = 0.0_f32;
                for blade in &blades {
                    if vertical > blade.height {
                        continue;
                    }
                    let along = vertical / blade.height;
                    let center = blade.base
                        + blade.lean * along.powf(1.35)
                        + blade.curve * (std::f32::consts::PI * along).sin()
                        + blade.s_curve * (std::f32::consts::TAU * along).sin();
                    let half_width = blade.half_width * (1.0 - along).powf(0.72) + texel * 0.45;
                    let edge_distance = (horizontal - center).abs() - half_width;
                    let coverage = (0.5 - edge_distance / (texel * 1.5)).clamp(0.0, 1.0);
                    alpha = alpha.max(coverage);
                }
                level[y * size + x] = (alpha * 255.0).round() as u8;
            }
        }

        pixels.extend_from_slice(&level);
        let mut current_size = size;
        let mut layer_mip_count = 1;
        while current_size > 1 {
            let next_size = (current_size / 2).max(1);
            let mut next = vec![0_u8; next_size * next_size];
            for y in 0..next_size {
                for x in 0..next_size {
                    let samples = [
                        level[(y * 2) * current_size + x * 2],
                        level[(y * 2) * current_size + (x * 2 + 1).min(current_size - 1)],
                        level[((y * 2 + 1).min(current_size - 1)) * current_size + x * 2],
                        level[((y * 2 + 1).min(current_size - 1)) * current_size
                            + (x * 2 + 1).min(current_size - 1)],
                    ];
                    let maximum = f32::from(*samples.iter().max().unwrap());
                    let average =
                        samples.iter().map(|sample| f32::from(*sample)).sum::<f32>() * 0.25;
                    next[y * next_size + x] = maximum.max(average * 1.35).min(255.0) as u8;
                }
            }
            pixels.extend_from_slice(&next);
            level = next;
            current_size = next_size;
            layer_mip_count += 1;
        }
        mip_level_count = layer_mip_count;
    }

    GroundCoverCardArtwork {
        resolution: GROUND_COVER_ARTWORK_RESOLUTION,
        variant_count: recipe.variant_count,
        mip_level_count,
        coverage_mips: pixels,
    }
}

fn hash_ground_cover_u32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

fn ground_cover_unit_float(value: u32) -> f32 {
    hash_ground_cover_u32(value) as f32 / u32::MAX as f32
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
    pub artwork: GroundCoverCardArtwork,
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
