mod schema;

use std::{
    collections::HashSet,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use world::{
    AssetId, CellCoord, GroundCoverBladeRecipe, GroundCoverLayerId, GroundCoverPresetId,
    GroundCoverRegionId, GroundCoverSpecies, GroundCoverSpeciesId, GroundCoverVisualId,
    MAX_DECODED_PAGE_BYTES, ObjectActivationPolicy, ObjectDefinitionId, PROJECT_SCHEMA_VERSION,
    PageCodec, PageDomain, PageKey, PagePayload, RUNTIME_SCHEMA_VERSION, StableObjectId,
    TerrainProfile, TerrainSurface, TerrainSurfaceId, TerrainTextureLayer, TerrainTextureSet,
    TerrainTextureSetId, WorldSpaceId, decode_page_payload, generate_ground_cover_card_artwork,
};

pub const MAX_OBJECT_WRITES_PER_TRANSACTION: usize = 256;
pub const MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION: usize = 64;
pub const MAX_GROUND_COVER_REGION_WRITES_PER_TRANSACTION: usize = 64;
pub const MAX_GROUND_COVER_CATALOG_WRITES_PER_TRANSACTION: usize = 64;

#[derive(Debug, Clone)]
pub struct ProjectDocument {
    pub default_world_space: WorldSpaceId,
    pub world_spaces: Vec<WorldSpaceRecord>,
    pub cells: Vec<SourceCellRecord>,
    pub terrain_surfaces: Vec<TerrainSurface>,
    pub terrain_texture_sets: Vec<TerrainTextureSet>,
    pub terrain_texture_layers: Vec<TerrainTextureLayer>,
    pub terrain_profiles: Vec<TerrainProfile>,
    pub terrain_cell_surface_slots: Vec<SourceTerrainCellSurfaceSlotRecord>,
    pub terrain_cell_weight_pages: Vec<SourceTerrainCellWeightPageRecord>,
    pub ground_cover_visuals: Vec<SourceGroundCoverVisualRecord>,
    pub ground_cover_presets: Vec<SourceGroundCoverPresetRecord>,
    pub ground_cover_layers: Vec<SourceGroundCoverLayerRecord>,
    pub ground_cover_regions: Vec<SourceGroundCoverRegionRecord>,
    pub ground_cover_masks: Vec<SourceGroundCoverCellMaskRecord>,
    pub assets: Vec<SourceAssetRecord>,
    pub asset_variants: Vec<SourceAssetVariantRecord>,
    pub definitions: Vec<SourceObjectDefinitionRecord>,
    pub objects: Vec<SourceObjectRecord>,
}

/// Small project header retained by targeted editor readers.
#[derive(Debug, Clone)]
pub struct ProjectManifest {
    pub schema_version: i64,
    pub default_world_space: WorldSpaceId,
    pub world_spaces: Vec<WorldSpaceRecord>,
}

impl ProjectManifest {
    pub fn world_space(&self, id: WorldSpaceId) -> Option<&WorldSpaceRecord> {
        self.world_spaces.iter().find(|space| space.id == id)
    }
}

#[derive(Debug, Clone)]
pub struct SourceCellQuery {
    pub records: Vec<SourceCellRecord>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct SourceObjectQuery {
    pub records: Vec<SourceObjectRecord>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct SourceObjectViewQuery {
    pub records: Vec<SourceObjectViewRecord>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct SourceObjectPaletteQuery {
    pub records: Vec<SourceObjectPaletteRecord>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceObjectOutlinerCursor {
    pub owner_cell: CellCoord,
    pub object: StableObjectId,
}

#[derive(Debug, Clone)]
pub struct SourceObjectOutlinerPage {
    pub records: Vec<SourceObjectViewRecord>,
    pub next_cursor: Option<SourceObjectOutlinerCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceObjectPaletteCursor {
    pub definition_key: String,
    pub definition: ObjectDefinitionId,
}

#[derive(Debug, Clone)]
pub struct SourceObjectPalettePage {
    pub records: Vec<SourceObjectPaletteRecord>,
    pub next_cursor: Option<SourceObjectPaletteCursor>,
}

#[derive(Debug, Clone)]
pub struct WorldSpaceRecord {
    pub id: WorldSpaceId,
    pub name: String,
    pub cell_size: f32,
    pub minimum_y: f32,
    pub maximum_y: f32,
}

#[derive(Debug, Clone)]
pub struct SourceCellRecord {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub height: f32,
    pub source_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTerrainCellSurfaceSlotRecord {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub slot: u8,
    pub surface: TerrainSurfaceId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTerrainCellWeightPageRecord {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub page: u8,
    pub resolution: u16,
    pub rgba: Vec<u8>,
    pub source_revision: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum SourceGroundCoverVisualFamily {
    CardCluster = 0,
}

impl TryFrom<i64> for SourceGroundCoverVisualFamily {
    type Error = UnknownGroundCoverVisualFamily;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::CardCluster),
            _ => Err(UnknownGroundCoverVisualFamily(value)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceGroundCoverCardVisualRecord {
    pub built_in_atlas_version: u32,
    /// `None` preserves the frozen built-in-v1 recipe. `Some` is an independently editable copy.
    pub procedural_recipe: Option<GroundCoverBladeRecipe>,
    pub bottom_color: [f32; 3],
    pub top_color: [f32; 3],
    pub minimum_card_height: f32,
    pub maximum_card_height: f32,
    pub minimum_card_width: f32,
    pub maximum_card_width: f32,
    pub flattened_card_probability: f32,
    pub maximum_wind_displacement: f32,
}

impl SourceGroundCoverCardVisualRecord {
    /// Complete visual values used by the original demo meadow, not just its frozen artwork mask.
    /// This is a named starter template rather than an implicit default for every new species.
    pub const fn original_meadow_v1() -> Self {
        Self {
            built_in_atlas_version: 1,
            procedural_recipe: None,
            bottom_color: [0.025, 0.055, 0.020],
            top_color: [0.105, 0.205, 0.075],
            minimum_card_height: 0.55,
            maximum_card_height: 0.78,
            minimum_card_width: 0.7,
            maximum_card_width: 1.4,
            flattened_card_probability: 0.2,
            maximum_wind_displacement: 0.22,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceGroundCoverVisualDefinition {
    CardCluster(SourceGroundCoverCardVisualRecord),
}

impl SourceGroundCoverVisualDefinition {
    pub const fn family(&self) -> SourceGroundCoverVisualFamily {
        match self {
            Self::CardCluster(_) => SourceGroundCoverVisualFamily::CardCluster,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceGroundCoverVisualRecord {
    pub id: GroundCoverVisualId,
    pub key: String,
    pub display_name: String,
    pub source_revision: i64,
    pub definition: SourceGroundCoverVisualDefinition,
}

impl SourceGroundCoverVisualRecord {
    /// Compatibility projection consumed by the existing optimized runtime renderer.
    pub fn runtime_species(&self) -> GroundCoverSpecies {
        let SourceGroundCoverVisualDefinition::CardCluster(card) = &self.definition;
        GroundCoverSpecies {
            id: GroundCoverSpeciesId(self.id.0),
            key: self.key.clone(),
            bottom_color: card.bottom_color,
            top_color: card.top_color,
            minimum_card_height: card.minimum_card_height,
            maximum_card_height: card.maximum_card_height,
            minimum_card_width: card.minimum_card_width,
            maximum_card_width: card.maximum_card_width,
            flattened_card_probability: card.flattened_card_probability,
            maximum_wind_displacement: card.maximum_wind_displacement,
            artwork: generate_ground_cover_card_artwork(
                card.procedural_recipe
                    .unwrap_or_else(GroundCoverBladeRecipe::built_in_v1),
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceGroundCoverPresetRecord {
    pub id: GroundCoverPresetId,
    pub key: String,
    pub display_name: String,
    pub enabled: bool,
    pub visual: GroundCoverVisualId,
    pub density_per_square_meter: f32,
    pub seed: u32,
    pub source_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceGroundCoverLayerRecord {
    pub id: GroundCoverLayerId,
    pub space: WorldSpaceId,
    pub key: String,
    pub display_name: String,
    pub enabled: bool,
    pub sort_order: i32,
    pub source_revision: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceGroundCoverRegionRecord {
    pub id: GroundCoverRegionId,
    pub layer: GroundCoverLayerId,
    pub space: WorldSpaceId,
    pub preset: GroundCoverPresetId,
    pub display_name: String,
    pub enabled: bool,
    pub density_multiplier: f32,
    pub source_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceGroundCoverCellMaskRecord {
    pub region: GroundCoverRegionId,
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub resolution: u8,
    pub coverage: Vec<u8>,
    pub source_revision: i64,
}

#[derive(Debug, Clone)]
pub struct SourceTerrainCellWeightPageQuery {
    pub records: Vec<SourceTerrainCellWeightPageRecord>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct SourceGroundCoverCellMaskQuery {
    pub records: Vec<SourceGroundCoverCellMaskRecord>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DenseSourceRecordKey {
    TerrainWeights {
        space: WorldSpaceId,
        cell: CellCoord,
        page: u8,
    },
    GroundCoverMask {
        region: GroundCoverRegionId,
        space: WorldSpaceId,
        cell: CellCoord,
    },
}

#[derive(Debug, Clone)]
pub enum DenseSourceWrite {
    TerrainWeights {
        expected_source_revision: Option<i64>,
        record: SourceTerrainCellWeightPageRecord,
    },
    GroundCoverMask {
        expected_source_revision: Option<i64>,
        record: SourceGroundCoverCellMaskRecord,
    },
}

impl DenseSourceWrite {
    pub fn key(&self) -> DenseSourceRecordKey {
        match self {
            Self::TerrainWeights { record, .. } => DenseSourceRecordKey::TerrainWeights {
                space: record.space,
                cell: record.cell,
                page: record.page,
            },
            Self::GroundCoverMask { record, .. } => DenseSourceRecordKey::GroundCoverMask {
                region: record.region,
                space: record.space,
                cell: record.cell,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenseSourceRecord {
    TerrainWeights(SourceTerrainCellWeightPageRecord),
    GroundCoverMask(SourceGroundCoverCellMaskRecord),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenseSourceWriteTransactionResult {
    Committed(Vec<DenseSourceRecord>),
    Conflict {
        key: DenseSourceRecordKey,
        actual: Option<DenseSourceRecord>,
    },
}

#[derive(Debug, Clone)]
pub enum GroundCoverRegionWrite {
    Create {
        record: SourceGroundCoverRegionRecord,
    },
    Update {
        expected_source_revision: i64,
        record: SourceGroundCoverRegionRecord,
    },
    DeleteEmpty {
        region: GroundCoverRegionId,
        expected_source_revision: i64,
    },
}

impl GroundCoverRegionWrite {
    pub const fn region(&self) -> GroundCoverRegionId {
        match self {
            Self::Create { record } => record.id,
            Self::Update { record, .. } => record.id,
            Self::DeleteEmpty { region, .. } => *region,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GroundCoverRegionWriteCommit {
    Created(SourceGroundCoverRegionRecord),
    Updated(SourceGroundCoverRegionRecord),
    Deleted(GroundCoverRegionId),
}

#[derive(Debug, Clone, PartialEq)]
pub enum GroundCoverRegionWriteTransactionResult {
    Committed(Vec<GroundCoverRegionWriteCommit>),
    Conflict {
        region: GroundCoverRegionId,
        actual: Option<SourceGroundCoverRegionRecord>,
    },
    BlockedByCoverage {
        region: GroundCoverRegionId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroundCoverCatalogKey {
    Visual(GroundCoverVisualId),
    Preset(GroundCoverPresetId),
}

#[derive(Debug, Clone, PartialEq)]
pub enum GroundCoverCatalogRecord {
    Visual(SourceGroundCoverVisualRecord),
    Preset(SourceGroundCoverPresetRecord),
}

impl GroundCoverCatalogRecord {
    pub const fn key(&self) -> GroundCoverCatalogKey {
        match self {
            Self::Visual(record) => GroundCoverCatalogKey::Visual(record.id),
            Self::Preset(record) => GroundCoverCatalogKey::Preset(record.id),
        }
    }

    pub const fn source_revision(&self) -> i64 {
        match self {
            Self::Visual(record) => record.source_revision,
            Self::Preset(record) => record.source_revision,
        }
    }
}

#[derive(Debug, Clone)]
pub enum GroundCoverCatalogWrite {
    CreateVisual {
        record: SourceGroundCoverVisualRecord,
    },
    UpdateVisual {
        expected_source_revision: i64,
        record: SourceGroundCoverVisualRecord,
    },
    DeleteVisual {
        visual: GroundCoverVisualId,
        expected_source_revision: i64,
    },
    CreatePreset {
        record: SourceGroundCoverPresetRecord,
    },
    UpdatePreset {
        expected_source_revision: i64,
        record: SourceGroundCoverPresetRecord,
    },
    DeletePreset {
        preset: GroundCoverPresetId,
        expected_source_revision: i64,
    },
}

impl GroundCoverCatalogWrite {
    pub const fn key(&self) -> GroundCoverCatalogKey {
        match self {
            Self::CreateVisual { record } | Self::UpdateVisual { record, .. } => {
                GroundCoverCatalogKey::Visual(record.id)
            }
            Self::DeleteVisual { visual, .. } => GroundCoverCatalogKey::Visual(*visual),
            Self::CreatePreset { record } | Self::UpdatePreset { record, .. } => {
                GroundCoverCatalogKey::Preset(record.id)
            }
            Self::DeletePreset { preset, .. } => GroundCoverCatalogKey::Preset(*preset),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GroundCoverCatalogWriteCommit {
    Created(GroundCoverCatalogRecord),
    Updated(GroundCoverCatalogRecord),
    Deleted(GroundCoverCatalogKey),
}

impl GroundCoverCatalogWriteCommit {
    pub const fn source_revision(&self) -> Option<i64> {
        match self {
            Self::Created(record) | Self::Updated(record) => Some(record.source_revision()),
            Self::Deleted(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroundCoverCatalogDependency {
    PresetRegions,
    VisualPresets,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GroundCoverCatalogWriteTransactionResult {
    Committed(Vec<GroundCoverCatalogWriteCommit>),
    Conflict {
        key: GroundCoverCatalogKey,
        actual: Option<GroundCoverCatalogRecord>,
    },
    BlockedByDependency {
        key: GroundCoverCatalogKey,
        dependency: GroundCoverCatalogDependency,
    },
}

#[derive(Debug, Clone)]
pub struct SourceAssetRecord {
    pub id: AssetId,
    pub key: String,
    pub kind: String,
    pub source_uri: String,
}

#[derive(Debug, Clone)]
pub struct SourceAssetVariantRecord {
    pub asset: AssetId,
    pub lod: u8,
    pub uri: String,
    pub bounds: [f32; 3],
    pub gpu_bytes_estimate: u64,
    pub shadow_policy: i64,
    pub minimum_screen_height: f32,
}

#[derive(Debug, Clone)]
pub struct SourceObjectDefinitionRecord {
    pub id: ObjectDefinitionId,
    pub key: String,
    pub display_name: String,
    pub visual_asset: Option<AssetId>,
    pub activation: ObjectActivationPolicy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceObjectRecord {
    pub id: StableObjectId,
    pub space: WorldSpaceId,
    pub owner_cell: CellCoord,
    pub definition: ObjectDefinitionId,
    pub local_translation: [f32; 3],
    pub yaw: f32,
    pub scale: f32,
    pub source_revision: i64,
}

/// An object placement plus the small catalog record needed to present it in editor UI.
///
/// This remains a bounded query result, not a durable editor identity. Callers pin the stable ID
/// and this snapshot when an object is selected.
#[derive(Debug, Clone)]
pub struct SourceObjectViewRecord {
    pub object: SourceObjectRecord,
    pub definition: SourceObjectDefinitionRecord,
    /// LOD0 visual scene URI used by the disposable editor proxy, when available.
    pub visual_uri: Option<String>,
    /// Full local-space visual dimensions from source asset LOD0, when available.
    pub visual_bounds: Option<[f32; 3]>,
}

/// Definition metadata needed by a bounded editor object palette.
#[derive(Debug, Clone)]
pub struct SourceObjectPaletteRecord {
    pub definition: SourceObjectDefinitionRecord,
    pub visual_uri: Option<String>,
    pub visual_bounds: Option<[f32; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceObjectTransform {
    pub space: WorldSpaceId,
    pub owner_cell: CellCoord,
    pub local_translation: [f32; 3],
    pub yaw: f32,
    pub scale: f32,
}

impl From<&SourceObjectRecord> for SourceObjectTransform {
    fn from(object: &SourceObjectRecord) -> Self {
        Self {
            space: object.space,
            owner_cell: object.owner_cell,
            local_translation: object.local_translation,
            yaw: object.yaw,
            scale: object.scale,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ObjectTransformWriteResult {
    Updated(SourceObjectRecord),
    Conflict { actual: Option<SourceObjectRecord> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceObjectWrite {
    Create {
        object: SourceObjectRecord,
    },
    UpdateTransform {
        object: StableObjectId,
        expected_source_revision: i64,
        transform: SourceObjectTransform,
    },
    Delete {
        object: StableObjectId,
        expected_source_revision: i64,
    },
}

impl SourceObjectWrite {
    pub const fn object(&self) -> StableObjectId {
        match self {
            Self::Create { object } => object.id,
            Self::UpdateTransform { object, .. } | Self::Delete { object, .. } => *object,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceObjectWriteCommit {
    Updated(SourceObjectRecord),
    Deleted(StableObjectId),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjectWriteTransactionResult {
    Committed(Vec<SourceObjectWriteCommit>),
    Conflict {
        object: StableObjectId,
        actual: Option<SourceObjectRecord>,
    },
}

#[derive(Debug, Clone)]
pub struct RuntimeBuild {
    pub manifest: RuntimeManifest,
    pub cells: Vec<RuntimeCellRecord>,
    pub pages: Vec<EncodedPage>,
    pub terrain_surfaces: Vec<TerrainSurface>,
    pub terrain_texture_sets: Vec<TerrainTextureSet>,
    pub terrain_texture_layers: Vec<TerrainTextureLayer>,
    pub terrain_profiles: Vec<TerrainProfile>,
    pub assets: Vec<AssetVariantRecord>,
    pub definitions: Vec<RuntimeObjectDefinition>,
    pub ground_cover_species: Vec<GroundCoverSpecies>,
    pub dependencies: Vec<PageDependencyRecord>,
    pub definition_dependencies: Vec<PageObjectDefinitionRecord>,
    pub ground_cover_species_dependencies: Vec<PageGroundCoverSpeciesRecord>,
    pub terrain_surface_dependencies: Vec<PageTerrainSurfaceRecord>,
}

#[derive(Debug, Clone)]
pub struct RuntimeManifest {
    pub schema_version: i64,
    pub generation_id: String,
    pub content_hash: [u8; 32],
    pub default_world_space: WorldSpaceId,
    pub world_spaces: Vec<WorldSpaceRecord>,
}

impl RuntimeManifest {
    pub fn world_space(&self, id: WorldSpaceId) -> Option<&WorldSpaceRecord> {
        self.world_spaces.iter().find(|space| space.id == id)
    }

    pub fn default_world_space(&self) -> &WorldSpaceRecord {
        self.world_space(self.default_world_space)
            .expect("validated runtime manifest has no default world space")
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeCellRecord {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub minimum_y: f32,
    pub maximum_y: f32,
    pub domain_mask: u64,
    pub source_revision: i64,
}

#[derive(Debug, Clone)]
pub struct CellDescriptor {
    pub cell: CellCoord,
    pub minimum_y: f32,
    pub maximum_y: f32,
    pub domain_mask: u64,
}

impl CellDescriptor {
    pub fn has_domain(&self, domain: PageDomain) -> bool {
        self.domain_mask & domain_bit(domain) != 0
    }
}

#[derive(Debug, Clone)]
pub struct EncodedPage {
    pub key: PageKey,
    pub codec: PageCodec,
    pub decoded_bytes: u64,
    pub gpu_bytes_estimate: u64,
    pub checksum: [u8; 32],
    pub payload: Vec<u8>,
}

impl EncodedPage {
    pub fn decode(self) -> Result<DecodedPage, WorldDbError> {
        if self.decoded_bytes > MAX_DECODED_PAGE_BYTES {
            return Err(WorldDbError::PageTooLarge {
                actual: self.decoded_bytes,
                maximum: MAX_DECODED_PAGE_BYTES,
            });
        }

        let bytes = match self.codec {
            PageCodec::Raw => self.payload,
            PageCodec::Zstd => {
                let decoder = zstd::stream::read::Decoder::new(Cursor::new(self.payload))?;
                let mut bytes = Vec::with_capacity(self.decoded_bytes as usize);
                decoder
                    .take(MAX_DECODED_PAGE_BYTES + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_DECODED_PAGE_BYTES {
                    return Err(WorldDbError::PageTooLarge {
                        actual: bytes.len() as u64,
                        maximum: MAX_DECODED_PAGE_BYTES,
                    });
                }
                bytes
            }
        };

        if bytes.len() as u64 != self.decoded_bytes {
            return Err(WorldDbError::DecodedSizeMismatch {
                expected: self.decoded_bytes,
                actual: bytes.len() as u64,
            });
        }
        let actual_checksum = *blake3::hash(&bytes).as_bytes();
        if actual_checksum != self.checksum {
            return Err(WorldDbError::ChecksumMismatch(self.key));
        }
        let payload = decode_page_payload(&bytes)?;
        if payload.domain() != self.key.domain {
            return Err(WorldDbError::DomainMismatch {
                expected: self.key.domain,
                actual: payload.domain(),
            });
        }

        Ok(DecodedPage {
            key: self.key,
            payload,
            decoded_bytes: self.decoded_bytes,
            gpu_bytes_estimate: self.gpu_bytes_estimate,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DecodedPage {
    pub key: PageKey,
    pub payload: PagePayload,
    pub decoded_bytes: u64,
    pub gpu_bytes_estimate: u64,
}

#[derive(Debug, Clone)]
pub struct AssetVariantRecord {
    pub asset: AssetId,
    pub lod: u8,
    pub kind: String,
    pub uri: String,
    pub bounds: [f32; 3],
    pub gpu_bytes_estimate: u64,
    pub shadow_policy: i64,
    pub minimum_screen_height: f32,
}

#[derive(Debug, Clone)]
pub struct PageDependencyRecord {
    pub page: PageKey,
    pub asset: AssetId,
    pub asset_lod: u8,
}

#[derive(Debug, Clone)]
pub struct RuntimeObjectDefinition {
    pub id: ObjectDefinitionId,
    pub key: String,
    pub display_name: String,
    pub visual_asset: Option<AssetId>,
    pub activation: ObjectActivationPolicy,
}

#[derive(Debug, Clone)]
pub struct PageObjectDefinitionRecord {
    pub page: PageKey,
    pub definition: ObjectDefinitionId,
}

#[derive(Debug, Clone)]
pub struct PageGroundCoverSpeciesRecord {
    pub page: PageKey,
    pub species: GroundCoverSpeciesId,
}

#[derive(Debug, Clone)]
pub struct PageTerrainSurfaceRecord {
    pub page: PageKey,
    pub surface: TerrainSurfaceId,
}

#[derive(Debug, Clone)]
pub struct RuntimeTerrainSurface {
    pub surface: TerrainSurface,
    pub layer: u16,
}

#[derive(Debug, Clone)]
pub struct TerrainRenderResources {
    pub profile: TerrainProfile,
    pub texture_set: TerrainTextureSet,
    pub surfaces: Vec<RuntimeTerrainSurface>,
}

#[derive(Debug, Clone)]
pub struct PageDependency {
    pub asset: AssetId,
    pub asset_lod: u8,
    pub kind: String,
    pub uri: String,
    pub bounds: [f32; 3],
    pub gpu_bytes_estimate: u64,
    pub shadow_policy: i64,
    pub minimum_screen_height: f32,
}

pub fn domain_bit(domain: PageDomain) -> u64 {
    1 << (domain as u64 - 1)
}

pub fn write_project_database(path: &Path, document: &ProjectDocument) -> Result<(), WorldDbError> {
    ensure_new_database_path(path)?;
    let mut connection = Connection::open(path)?;
    connection.execute_batch(schema::PROJECT_SCHEMA)?;
    let transaction = connection.transaction()?;
    write_project_document(&transaction, document)?;
    transaction.commit()?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

/// Transactionally upgrades an existing mutable project database to the current schema.
///
/// Runtime databases are immutable publication artifacts and are never migrated in place.
pub fn migrate_project_database(path: &Path) -> Result<bool, WorldDbError> {
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 1000;")?;
    let actual: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if actual == PROJECT_SCHEMA_VERSION {
        return Ok(false);
    }
    if !(7..PROJECT_SCHEMA_VERSION).contains(&actual) {
        return Err(WorldDbError::SchemaVersion {
            database: "project",
            expected: PROJECT_SCHEMA_VERSION,
            actual,
        });
    }

    let transaction = connection.transaction()?;
    if actual == 7 {
        transaction.execute_batch(schema::PROJECT_MIGRATION_7_TO_8)?;
    }
    transaction.execute_batch(schema::PROJECT_MIGRATION_8_TO_9)?;
    let violation_count: i64 =
        transaction.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violation_count != 0 {
        return Err(WorldDbError::MigrationForeignKeyViolations(violation_count));
    }
    transaction.commit()?;
    ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;
    Ok(true)
}

fn write_project_document(
    transaction: &Transaction<'_>,
    document: &ProjectDocument,
) -> Result<(), WorldDbError> {
    transaction.execute(
        "INSERT INTO project_metadata(key, value) VALUES ('schema_name', 'yarra-project')",
        [],
    )?;
    for space in &document.world_spaces {
        transaction.execute(
            "INSERT INTO world_spaces(id, name, cell_size, minimum_y, maximum_y) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                space.id.0,
                space.name,
                space.cell_size,
                space.minimum_y,
                space.maximum_y
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO project_settings(singleton, default_world_space_id) VALUES (1, ?1)",
        [document.default_world_space.0],
    )?;
    for cell in &document.cells {
        transaction.execute(
            "INSERT INTO source_cells(world_space_id, cell_x, cell_z, height, source_revision) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                cell.space.0,
                cell.cell.x,
                cell.cell.z,
                cell.height,
                cell.source_revision
            ],
        )?;
    }
    write_terrain_catalog(
        transaction,
        &document.terrain_surfaces,
        &document.terrain_texture_sets,
        &document.terrain_texture_layers,
        &document.terrain_profiles,
    )?;
    for slot in &document.terrain_cell_surface_slots {
        transaction.execute(
            "INSERT INTO terrain_cell_surface_slots( \
                world_space_id, cell_x, cell_z, slot, surface_id \
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                slot.space.0,
                slot.cell.x,
                slot.cell.z,
                i64::from(slot.slot),
                slot.surface.0.as_slice(),
            ],
        )?;
    }
    for weights in &document.terrain_cell_weight_pages {
        transaction.execute(
            "INSERT INTO terrain_cell_weight_pages( \
                world_space_id, cell_x, cell_z, page, resolution, rgba, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                weights.space.0,
                weights.cell.x,
                weights.cell.z,
                i64::from(weights.page),
                i64::from(weights.resolution),
                weights.rgba,
                weights.source_revision,
            ],
        )?;
    }
    write_source_ground_cover_catalog(
        transaction,
        &document.ground_cover_visuals,
        &document.ground_cover_presets,
    )?;
    for layer in &document.ground_cover_layers {
        transaction.execute(
            "INSERT INTO ground_cover_layers( \
                layer_id, world_space_id, layer_key, display_name, enabled, sort_order, \
                source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                layer.id.0.as_slice(),
                layer.space.0,
                layer.key,
                layer.display_name,
                layer.enabled,
                layer.sort_order,
                layer.source_revision,
            ],
        )?;
    }
    for region in &document.ground_cover_regions {
        transaction.execute(
            "INSERT INTO ground_cover_regions( \
                region_id, layer_id, world_space_id, preset_id, display_name, enabled, \
                density_multiplier, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                region.id.0.as_slice(),
                region.layer.0.as_slice(),
                region.space.0,
                region.preset.0.as_slice(),
                region.display_name,
                region.enabled,
                region.density_multiplier,
                region.source_revision,
            ],
        )?;
    }
    for mask in &document.ground_cover_masks {
        transaction.execute(
            "INSERT INTO ground_cover_region_cell_masks( \
                region_id, world_space_id, cell_x, cell_z, resolution, coverage, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                mask.region.0.as_slice(),
                mask.space.0,
                mask.cell.x,
                mask.cell.z,
                i64::from(mask.resolution),
                mask.coverage,
                mask.source_revision,
            ],
        )?;
    }
    for asset in &document.assets {
        transaction.execute(
            "INSERT INTO source_assets(asset_id, asset_key, kind, source_uri) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                asset.id.0.as_slice(),
                asset.key,
                asset.kind,
                asset.source_uri
            ],
        )?;
    }
    for variant in &document.asset_variants {
        transaction.execute(
            "INSERT INTO source_asset_variants( \
                asset_id, lod, uri, bounds_x, bounds_y, bounds_z, gpu_bytes_estimate, \
                shadow_policy, minimum_screen_height \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                variant.asset.0.as_slice(),
                i64::from(variant.lod),
                variant.uri,
                variant.bounds[0],
                variant.bounds[1],
                variant.bounds[2],
                i64::try_from(variant.gpu_bytes_estimate)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
                variant.shadow_policy,
                variant.minimum_screen_height,
            ],
        )?;
    }
    for definition in &document.definitions {
        transaction.execute(
            "INSERT INTO object_definitions( \
                definition_id, definition_key, display_name, visual_asset_id, activation_policy \
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                definition.id.0.as_slice(),
                definition.key,
                definition.display_name,
                definition.visual_asset.map(|asset| asset.0),
                definition.activation as i64,
            ],
        )?;
    }
    for object in &document.objects {
        transaction.execute(
            "INSERT INTO object_placements( \
                object_id, world_space_id, owner_cell_x, owner_cell_z, definition_id, \
                local_x, local_y, local_z, yaw, scale, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                object.id.0.as_slice(),
                object.space.0,
                object.owner_cell.x,
                object.owner_cell.z,
                object.definition.0.as_slice(),
                object.local_translation[0],
                object.local_translation[1],
                object.local_translation[2],
                object.yaw,
                object.scale,
                object.source_revision
            ],
        )?;
    }
    Ok(())
}

fn write_source_ground_cover_catalog(
    transaction: &Transaction<'_>,
    visuals: &[SourceGroundCoverVisualRecord],
    presets: &[SourceGroundCoverPresetRecord],
) -> Result<(), WorldDbError> {
    for visual in visuals {
        transaction.execute(
            "INSERT INTO ground_cover_visuals( \
                visual_id, visual_key, display_name, visual_family, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                visual.id.0.as_slice(),
                visual.key,
                visual.display_name,
                visual.definition.family() as i64,
                visual.source_revision,
            ],
        )?;
        let SourceGroundCoverVisualDefinition::CardCluster(card) = &visual.definition;
        let recipe = RecipeSqlValues::from(card.procedural_recipe);
        transaction.execute(
            "INSERT INTO ground_cover_card_visuals( \
                visual_id, built_in_atlas_version, recipe_variant_count, recipe_blade_count, \
                recipe_seed, recipe_minimum_blade_height, recipe_maximum_blade_height, \
                recipe_base_jitter, recipe_minimum_blade_half_width, \
                recipe_maximum_blade_half_width, recipe_maximum_lean, recipe_maximum_curve, \
                recipe_maximum_s_curve, bottom_color_r, bottom_color_g, \
                bottom_color_b, top_color_r, top_color_g, top_color_b, minimum_card_height, \
                maximum_card_height, minimum_card_width, maximum_card_width, \
                flattened_card_probability, maximum_wind_displacement \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                       ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
            params![
                visual.id.0.as_slice(),
                i64::from(card.built_in_atlas_version),
                recipe.variant_count,
                recipe.blade_count,
                recipe.seed,
                recipe.minimum_blade_height,
                recipe.maximum_blade_height,
                recipe.base_jitter,
                recipe.minimum_blade_half_width,
                recipe.maximum_blade_half_width,
                recipe.maximum_lean,
                recipe.maximum_curve,
                recipe.maximum_s_curve,
                card.bottom_color[0],
                card.bottom_color[1],
                card.bottom_color[2],
                card.top_color[0],
                card.top_color[1],
                card.top_color[2],
                card.minimum_card_height,
                card.maximum_card_height,
                card.minimum_card_width,
                card.maximum_card_width,
                card.flattened_card_probability,
                card.maximum_wind_displacement,
            ],
        )?;
    }
    for preset in presets {
        transaction.execute(
            "INSERT INTO ground_cover_presets( \
                preset_id, preset_key, display_name, enabled, visual_id, \
                density_per_square_meter, seed, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                preset.id.0.as_slice(),
                preset.key,
                preset.display_name,
                preset.enabled,
                preset.visual.0.as_slice(),
                preset.density_per_square_meter,
                i64::from(preset.seed),
                preset.source_revision,
            ],
        )?;
    }
    Ok(())
}

struct RecipeSqlValues {
    variant_count: Option<i64>,
    blade_count: Option<i64>,
    seed: Option<i64>,
    minimum_blade_height: Option<f32>,
    maximum_blade_height: Option<f32>,
    base_jitter: Option<f32>,
    minimum_blade_half_width: Option<f32>,
    maximum_blade_half_width: Option<f32>,
    maximum_lean: Option<f32>,
    maximum_curve: Option<f32>,
    maximum_s_curve: Option<f32>,
}

impl From<Option<GroundCoverBladeRecipe>> for RecipeSqlValues {
    fn from(recipe: Option<GroundCoverBladeRecipe>) -> Self {
        let Some(recipe) = recipe else {
            return Self {
                variant_count: None,
                blade_count: None,
                seed: None,
                minimum_blade_height: None,
                maximum_blade_height: None,
                base_jitter: None,
                minimum_blade_half_width: None,
                maximum_blade_half_width: None,
                maximum_lean: None,
                maximum_curve: None,
                maximum_s_curve: None,
            };
        };
        Self {
            variant_count: Some(i64::from(recipe.variant_count)),
            blade_count: Some(i64::from(recipe.blade_count)),
            seed: Some(i64::from(recipe.seed)),
            minimum_blade_height: Some(recipe.minimum_blade_height),
            maximum_blade_height: Some(recipe.maximum_blade_height),
            base_jitter: Some(recipe.base_jitter),
            minimum_blade_half_width: Some(recipe.minimum_blade_half_width),
            maximum_blade_half_width: Some(recipe.maximum_blade_half_width),
            maximum_lean: Some(recipe.maximum_lean),
            maximum_curve: Some(recipe.maximum_curve),
            maximum_s_curve: Some(recipe.maximum_s_curve),
        }
    }
}

fn write_terrain_catalog(
    transaction: &Transaction<'_>,
    surfaces: &[TerrainSurface],
    texture_sets: &[TerrainTextureSet],
    texture_layers: &[TerrainTextureLayer],
    profiles: &[TerrainProfile],
) -> Result<(), WorldDbError> {
    for surface in surfaces {
        transaction.execute(
            "INSERT INTO terrain_surfaces( \
                surface_id, surface_key, display_name, tile_size, anti_tiling, normal_y_sign, \
                normal_strength, roughness_min, roughness_max \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                surface.id.0.as_slice(),
                surface.key,
                surface.display_name,
                surface.tile_size,
                surface.anti_tiling,
                surface.normal_y_sign,
                surface.normal_strength,
                surface.roughness_min,
                surface.roughness_max,
            ],
        )?;
    }
    for texture_set in texture_sets {
        transaction.execute(
            "INSERT INTO terrain_texture_sets( \
                texture_set_id, texture_set_key, base_color_universal_uri, \
                normal_material_universal_uri, macro_variation_universal_uri, \
                base_color_astc_uri, normal_material_astc_uri, macro_variation_astc_uri, \
                universal_gpu_bytes, astc_gpu_bytes \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                texture_set.id.0.as_slice(),
                texture_set.key,
                texture_set.base_color_universal_uri,
                texture_set.normal_material_universal_uri,
                texture_set.macro_variation_universal_uri,
                texture_set.base_color_astc_uri,
                texture_set.normal_material_astc_uri,
                texture_set.macro_variation_astc_uri,
                i64::try_from(texture_set.universal_gpu_bytes)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
                i64::try_from(texture_set.astc_gpu_bytes)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
            ],
        )?;
    }
    for layer in texture_layers {
        transaction.execute(
            "INSERT INTO terrain_texture_set_layers(texture_set_id, layer, surface_id) \
             VALUES (?1, ?2, ?3)",
            params![
                layer.texture_set.0.as_slice(),
                i64::from(layer.layer),
                layer.surface.0.as_slice(),
            ],
        )?;
    }
    for profile in profiles {
        transaction.execute(
            "INSERT INTO world_space_terrain_profiles( \
                world_space_id, texture_set_id, weight_resolution, macro_small_scale, \
                macro_medium_scale, macro_large_scale, macro_contrast, macro_albedo_strength \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                profile.space.0,
                profile.texture_set.0.as_slice(),
                i64::from(profile.weight_resolution),
                profile.macro_scales[0],
                profile.macro_scales[1],
                profile.macro_scales[2],
                profile.macro_contrast,
                profile.macro_albedo_strength,
            ],
        )?;
    }
    Ok(())
}

fn write_ground_cover_species(
    transaction: &Transaction<'_>,
    species: &[GroundCoverSpecies],
) -> Result<(), WorldDbError> {
    for species in species {
        transaction.execute(
            "INSERT INTO ground_cover_species( \
                species_id, species_key, bottom_color_r, bottom_color_g, bottom_color_b, \
                top_color_r, top_color_g, top_color_b, minimum_card_height, \
                maximum_card_height, minimum_card_width, maximum_card_width, \
                flattened_card_probability, maximum_wind_displacement, artwork_resolution, \
                artwork_variant_count, artwork_mip_level_count, artwork_coverage_mips \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                       ?15, ?16, ?17, ?18)",
            params![
                species.id.0.as_slice(),
                species.key,
                species.bottom_color[0],
                species.bottom_color[1],
                species.bottom_color[2],
                species.top_color[0],
                species.top_color[1],
                species.top_color[2],
                species.minimum_card_height,
                species.maximum_card_height,
                species.minimum_card_width,
                species.maximum_card_width,
                species.flattened_card_probability,
                species.maximum_wind_displacement,
                i64::from(species.artwork.resolution),
                i64::from(species.artwork.variant_count),
                i64::from(species.artwork.mip_level_count),
                species.artwork.coverage_mips,
            ],
        )?;
    }
    Ok(())
}

pub fn read_project_database(path: &Path) -> Result<ProjectDocument, WorldDbError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;

    let world_spaces = query_world_spaces(&connection)?;
    let default_world_space = connection.query_row(
        "SELECT default_world_space_id FROM project_settings WHERE singleton = 1",
        [],
        |row| Ok(WorldSpaceId(row.get(0)?)),
    )?;
    let mut statement = connection.prepare(
        "SELECT world_space_id, cell_x, cell_z, height, source_revision \
         FROM source_cells ORDER BY world_space_id, cell_x, cell_z",
    )?;
    let cells = statement
        .query_map([], |row| {
            Ok(SourceCellRecord {
                space: WorldSpaceId(row.get(0)?),
                cell: CellCoord {
                    x: row.get(1)?,
                    z: row.get(2)?,
                },
                height: row.get(3)?,
                source_revision: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let terrain_surfaces = query_all_terrain_surfaces(&connection)?;
    let terrain_texture_sets = query_all_terrain_texture_sets(&connection)?;
    let terrain_texture_layers = query_all_terrain_texture_layers(&connection)?;
    let terrain_profiles = query_all_terrain_profiles(&connection)?;
    let mut statement = connection.prepare(
        "SELECT world_space_id, cell_x, cell_z, slot, surface_id \
         FROM terrain_cell_surface_slots \
         ORDER BY world_space_id, cell_x, cell_z, slot",
    )?;
    let terrain_cell_surface_slots = statement
        .query_map([], |row| {
            Ok(SourceTerrainCellSurfaceSlotRecord {
                space: WorldSpaceId(row.get(0)?),
                cell: CellCoord {
                    x: row.get(1)?,
                    z: row.get(2)?,
                },
                slot: row.get::<_, i64>(3)? as u8,
                surface: TerrainSurfaceId(blob_array(row.get_ref(4)?.as_blob()?, "surface_id")?),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut statement = connection.prepare(
        "SELECT world_space_id, cell_x, cell_z, page, resolution, rgba, source_revision \
         FROM terrain_cell_weight_pages \
         ORDER BY world_space_id, cell_x, cell_z, page",
    )?;
    let terrain_cell_weight_pages = statement
        .query_map([], |row| {
            Ok(SourceTerrainCellWeightPageRecord {
                space: WorldSpaceId(row.get(0)?),
                cell: CellCoord {
                    x: row.get(1)?,
                    z: row.get(2)?,
                },
                page: row.get::<_, i64>(3)? as u8,
                resolution: row.get::<_, i64>(4)? as u16,
                rgba: row.get(5)?,
                source_revision: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let ground_cover_visuals = query_all_source_ground_cover_visuals(&connection)?;
    let ground_cover_presets = query_all_source_ground_cover_presets(&connection)?;
    let mut statement = connection.prepare(
        "SELECT layer_id, world_space_id, layer_key, display_name, enabled, sort_order, \
                source_revision \
         FROM ground_cover_layers ORDER BY layer_id",
    )?;
    let ground_cover_layers = statement
        .query_map([], |row| {
            Ok(SourceGroundCoverLayerRecord {
                id: GroundCoverLayerId(blob_array(row.get_ref(0)?.as_blob()?, "layer_id")?),
                space: WorldSpaceId(row.get(1)?),
                key: row.get(2)?,
                display_name: row.get(3)?,
                enabled: row.get(4)?,
                sort_order: row.get(5)?,
                source_revision: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut statement = connection.prepare(
        "SELECT region_id, layer_id, world_space_id, preset_id, display_name, enabled, \
                density_multiplier, source_revision \
         FROM ground_cover_regions ORDER BY region_id",
    )?;
    let ground_cover_regions = statement
        .query_map([], source_ground_cover_region_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    let mut statement = connection.prepare(
        "SELECT region_id, world_space_id, cell_x, cell_z, resolution, coverage, source_revision \
         FROM ground_cover_region_cell_masks \
         ORDER BY region_id, world_space_id, cell_x, cell_z",
    )?;
    let ground_cover_masks = statement
        .query_map([], |row| {
            Ok(SourceGroundCoverCellMaskRecord {
                region: GroundCoverRegionId(blob_array(row.get_ref(0)?.as_blob()?, "region_id")?),
                space: WorldSpaceId(row.get(1)?),
                cell: CellCoord {
                    x: row.get(2)?,
                    z: row.get(3)?,
                },
                resolution: row.get::<_, i64>(4)? as u8,
                coverage: row.get(5)?,
                source_revision: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut statement = connection.prepare(
        "SELECT asset_id, asset_key, kind, source_uri FROM source_assets ORDER BY asset_id",
    )?;
    let assets = statement
        .query_map([], |row| {
            Ok(SourceAssetRecord {
                id: AssetId(blob_array(row.get_ref(0)?.as_blob()?, "asset_id")?),
                key: row.get(1)?,
                kind: row.get(2)?,
                source_uri: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut statement = connection.prepare(
        "SELECT asset_id, lod, uri, bounds_x, bounds_y, bounds_z, gpu_bytes_estimate, \
                shadow_policy, minimum_screen_height \
         FROM source_asset_variants ORDER BY asset_id, lod",
    )?;
    let asset_variants = statement
        .query_map([], |row| {
            let gpu_bytes_estimate: i64 = row.get(6)?;
            Ok(SourceAssetVariantRecord {
                asset: AssetId(blob_array(row.get_ref(0)?.as_blob()?, "asset_id")?),
                lod: row.get::<_, i64>(1)? as u8,
                uri: row.get(2)?,
                bounds: [row.get(3)?, row.get(4)?, row.get(5)?],
                gpu_bytes_estimate: gpu_bytes_estimate as u64,
                shadow_policy: row.get(7)?,
                minimum_screen_height: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut statement = connection.prepare(
        "SELECT definition_id, definition_key, display_name, visual_asset_id, activation_policy \
         FROM object_definitions ORDER BY definition_id",
    )?;
    let definitions = statement
        .query_map([], |row| {
            Ok(SourceObjectDefinitionRecord {
                id: ObjectDefinitionId(blob_array(row.get_ref(0)?.as_blob()?, "definition_id")?),
                key: row.get(1)?,
                display_name: row.get(2)?,
                visual_asset: row
                    .get::<_, Option<Vec<u8>>>(3)?
                    .map(|bytes| blob_array(&bytes, "visual_asset_id").map(AssetId))
                    .transpose()?,
                activation: ObjectActivationPolicy::try_from(row.get::<_, i64>(4)?).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    },
                )?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut statement = connection.prepare(
        "SELECT object_id, world_space_id, owner_cell_x, owner_cell_z, definition_id, \
                local_x, local_y, local_z, yaw, scale, source_revision \
         FROM object_placements ORDER BY object_id",
    )?;
    let objects = statement
        .query_map([], |row| {
            Ok(SourceObjectRecord {
                id: StableObjectId(blob_array(row.get_ref(0)?.as_blob()?, "object_id")?),
                space: WorldSpaceId(row.get(1)?),
                owner_cell: CellCoord {
                    x: row.get(2)?,
                    z: row.get(3)?,
                },
                definition: ObjectDefinitionId(blob_array(
                    row.get_ref(4)?.as_blob()?,
                    "definition_id",
                )?),
                local_translation: [row.get(5)?, row.get(6)?, row.get(7)?],
                yaw: row.get(8)?,
                scale: row.get(9)?,
                source_revision: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ProjectDocument {
        default_world_space,
        world_spaces,
        cells,
        terrain_surfaces,
        terrain_texture_sets,
        terrain_texture_layers,
        terrain_profiles,
        terrain_cell_surface_slots,
        terrain_cell_weight_pages,
        ground_cover_visuals,
        ground_cover_presets,
        ground_cover_layers,
        ground_cover_regions,
        ground_cover_masks,
        assets,
        asset_variants,
        definitions,
        objects,
    })
}

/// Read-only, query-shaped access to a mutable authoring database.
///
/// Unlike [`read_project_database`], this reader never constructs a complete project document.
/// It is intended to live on an editor worker thread and return explicitly bounded spatial results.
pub struct ProjectReader {
    connection: Connection,
    manifest: ProjectManifest,
    has_spatial_object_overlap_index: bool,
}

impl ProjectReader {
    pub fn open_read_only(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.execute_batch(
            "PRAGMA query_only = ON; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 250;",
        )?;
        ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;
        let world_spaces = query_world_spaces(&connection)?;
        let default_world_space = connection.query_row(
            "SELECT default_world_space_id FROM project_settings WHERE singleton = 1",
            [],
            |row| Ok(WorldSpaceId(row.get(0)?)),
        )?;
        if !world_spaces
            .iter()
            .any(|space| space.id == default_world_space)
        {
            return Err(WorldDbError::UnknownDefaultWorldSpace(default_world_space));
        }
        let has_spatial_object_overlap_index = connection.query_row(
            "SELECT EXISTS( \
                SELECT 1 FROM pragma_index_list('object_cell_overlaps') \
                WHERE name = 'object_cell_overlaps_cells' \
             )",
            [],
            |row| row.get(0),
        )?;
        Ok(Self {
            connection,
            manifest: ProjectManifest {
                schema_version: PROJECT_SCHEMA_VERSION,
                default_world_space,
                world_spaces,
            },
            has_spatial_object_overlap_index,
        })
    }

    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    pub fn read_cells(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
        maximum_records: usize,
    ) -> Result<SourceCellQuery, WorldDbError> {
        validate_spatial_query(minimum, maximum, maximum_records)?;
        let sql_limit = query_sql_limit(maximum_records)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT world_space_id, cell_x, cell_z, height, source_revision \
             FROM source_cells \
             WHERE world_space_id = ?1 \
               AND cell_x BETWEEN ?2 AND ?3 \
               AND cell_z BETWEEN ?4 AND ?5 \
             ORDER BY cell_x, cell_z \
             LIMIT ?6",
        )?;
        let mut records = statement
            .query_map(
                params![
                    space.0, minimum.x, maximum.x, minimum.z, maximum.z, sql_limit
                ],
                source_cell_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = records.len() > maximum_records;
        records.truncate(maximum_records);
        Ok(SourceCellQuery { records, truncated })
    }

    pub fn read_terrain_weight_pages_in_cells(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
        maximum_records: usize,
    ) -> Result<SourceTerrainCellWeightPageQuery, WorldDbError> {
        validate_spatial_query(minimum, maximum, maximum_records)?;
        let sql_limit = query_sql_limit(maximum_records)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT world_space_id, cell_x, cell_z, page, resolution, rgba, source_revision \
             FROM terrain_cell_weight_pages \
             WHERE world_space_id = ?1 \
               AND cell_x BETWEEN ?2 AND ?3 \
               AND cell_z BETWEEN ?4 AND ?5 \
             ORDER BY cell_x, cell_z, page \
             LIMIT ?6",
        )?;
        let mut records = statement
            .query_map(
                params![
                    space.0, minimum.x, maximum.x, minimum.z, maximum.z, sql_limit
                ],
                source_terrain_weights_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = records.len() > maximum_records;
        records.truncate(maximum_records);
        Ok(SourceTerrainCellWeightPageQuery { records, truncated })
    }

    pub fn read_ground_cover_masks_in_cells(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
        maximum_records: usize,
    ) -> Result<SourceGroundCoverCellMaskQuery, WorldDbError> {
        validate_spatial_query(minimum, maximum, maximum_records)?;
        let sql_limit = query_sql_limit(maximum_records)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT region_id, world_space_id, cell_x, cell_z, resolution, coverage, \
                    source_revision \
             FROM ground_cover_region_cell_masks \
             WHERE world_space_id = ?1 \
               AND cell_x BETWEEN ?2 AND ?3 \
               AND cell_z BETWEEN ?4 AND ?5 \
             ORDER BY cell_x, cell_z, region_id \
             LIMIT ?6",
        )?;
        let mut records = statement
            .query_map(
                params![
                    space.0, minimum.x, maximum.x, minimum.z, maximum.z, sql_limit
                ],
                source_ground_cover_mask_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = records.len() > maximum_records;
        records.truncate(maximum_records);
        Ok(SourceGroundCoverCellMaskQuery { records, truncated })
    }

    pub fn read_ground_cover_layers(
        &self,
        space: WorldSpaceId,
        maximum_records: usize,
    ) -> Result<Vec<SourceGroundCoverLayerRecord>, WorldDbError> {
        if maximum_records == 0 {
            return Err(WorldDbError::InvalidQueryLimit);
        }
        let sql_limit = query_sql_limit(maximum_records)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT layer_id, world_space_id, layer_key, display_name, enabled, sort_order, \
                    source_revision \
             FROM ground_cover_layers \
             WHERE world_space_id = ?1 \
             ORDER BY sort_order, layer_key, layer_id \
             LIMIT ?2",
        )?;
        statement
            .query_map(
                params![space.0, sql_limit],
                source_ground_cover_layer_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_ground_cover_regions(
        &self,
        space: WorldSpaceId,
        maximum_records: usize,
    ) -> Result<Vec<SourceGroundCoverRegionRecord>, WorldDbError> {
        if maximum_records == 0 {
            return Err(WorldDbError::InvalidQueryLimit);
        }
        let sql_limit = query_sql_limit(maximum_records)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT region_id, layer_id, world_space_id, preset_id, display_name, enabled, \
                    density_multiplier, source_revision \
             FROM ground_cover_regions \
             WHERE world_space_id = ?1 \
             ORDER BY layer_id, display_name, region_id \
             LIMIT ?2",
        )?;
        statement
            .query_map(
                params![space.0, sql_limit],
                source_ground_cover_region_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_ground_cover_presets(
        &self,
        maximum_records: usize,
    ) -> Result<Vec<SourceGroundCoverPresetRecord>, WorldDbError> {
        if maximum_records == 0 {
            return Err(WorldDbError::InvalidQueryLimit);
        }
        let sql_limit = query_sql_limit(maximum_records)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT preset_id, preset_key, display_name, enabled, visual_id, \
                    density_per_square_meter, seed, source_revision \
             FROM ground_cover_presets \
             ORDER BY preset_key, preset_id \
             LIMIT ?1",
        )?;
        statement
            .query_map([sql_limit], source_ground_cover_preset_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_ground_cover_visuals(
        &self,
        maximum_records: usize,
    ) -> Result<Vec<SourceGroundCoverVisualRecord>, WorldDbError> {
        if maximum_records == 0 {
            return Err(WorldDbError::InvalidQueryLimit);
        }
        let sql_limit = query_sql_limit(maximum_records)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT v.visual_id, v.visual_key, v.display_name, v.visual_family, \
                    v.source_revision, c.built_in_atlas_version, c.recipe_variant_count, \
                    c.recipe_blade_count, c.recipe_seed, c.recipe_minimum_blade_height, \
                    c.recipe_maximum_blade_height, c.recipe_base_jitter, \
                    c.recipe_minimum_blade_half_width, c.recipe_maximum_blade_half_width, \
                    c.recipe_maximum_lean, c.recipe_maximum_curve, c.recipe_maximum_s_curve, \
                    c.bottom_color_r, \
                    c.bottom_color_g, c.bottom_color_b, c.top_color_r, c.top_color_g, \
                    c.top_color_b, c.minimum_card_height, c.maximum_card_height, \
                    c.minimum_card_width, c.maximum_card_width, \
                    c.flattened_card_probability, c.maximum_wind_displacement \
             FROM ground_cover_visuals v \
             JOIN ground_cover_card_visuals c ON c.visual_id = v.visual_id \
             ORDER BY v.visual_key, v.visual_id \
             LIMIT ?1",
        )?;
        statement
            .query_map([sql_limit], source_ground_cover_visual_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_objects_in_cells(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
        maximum_records: usize,
    ) -> Result<SourceObjectQuery, WorldDbError> {
        validate_spatial_query(minimum, maximum, maximum_records)?;
        let sql_limit = query_sql_limit(maximum_records)?;
        let sql = if self.has_spatial_object_overlap_index {
            "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision \
             FROM object_placements o \
             WHERE o.world_space_id = ?1 \
               AND o.owner_cell_x BETWEEN ?2 AND ?3 \
               AND o.owner_cell_z BETWEEN ?4 AND ?5 \
             UNION \
             SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision \
             FROM object_cell_overlaps overlap \
             JOIN object_placements o ON o.object_id = overlap.object_id \
             WHERE overlap.world_space_id = ?1 \
               AND overlap.cell_x BETWEEN ?2 AND ?3 \
               AND overlap.cell_z BETWEEN ?4 AND ?5 \
             ORDER BY object_id \
             LIMIT ?6"
        } else {
            // Early version-7 databases do not have the optional overlap spatial index.
            // Owner-cell queries remain indexed; scanning every object in a space does not.
            "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision \
             FROM object_placements o \
             WHERE o.world_space_id = ?1 \
               AND o.owner_cell_x BETWEEN ?2 AND ?3 \
               AND o.owner_cell_z BETWEEN ?4 AND ?5 \
             ORDER BY o.object_id \
             LIMIT ?6"
        };
        let mut statement = self.connection.prepare_cached(sql)?;
        let mut records = statement
            .query_map(
                params![
                    space.0, minimum.x, maximum.x, minimum.z, maximum.z, sql_limit
                ],
                source_object_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = records.len() > maximum_records;
        records.truncate(maximum_records);
        Ok(SourceObjectQuery { records, truncated })
    }

    pub fn read_object(
        &self,
        object: StableObjectId,
    ) -> Result<Option<SourceObjectRecord>, WorldDbError> {
        self.connection
            .query_row(
                "SELECT object_id, world_space_id, owner_cell_x, owner_cell_z, definition_id, \
                        local_x, local_y, local_z, yaw, scale, source_revision \
                 FROM object_placements WHERE object_id = ?1",
                params![object.0.as_slice()],
                source_object_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn read_object_views_in_cells(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
        maximum_records: usize,
    ) -> Result<SourceObjectViewQuery, WorldDbError> {
        validate_spatial_query(minimum, maximum, maximum_records)?;
        let sql_limit = query_sql_limit(maximum_records)?;
        let sql = if self.has_spatial_object_overlap_index {
            "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_placements o \
             JOIN object_definitions d ON d.definition_id = o.definition_id \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             WHERE o.world_space_id = ?1 \
               AND o.owner_cell_x BETWEEN ?2 AND ?3 \
               AND o.owner_cell_z BETWEEN ?4 AND ?5 \
             UNION \
             SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_cell_overlaps overlap \
             JOIN object_placements o ON o.object_id = overlap.object_id \
             JOIN object_definitions d ON d.definition_id = o.definition_id \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             WHERE overlap.world_space_id = ?1 \
               AND overlap.cell_x BETWEEN ?2 AND ?3 \
               AND overlap.cell_z BETWEEN ?4 AND ?5 \
             ORDER BY object_id \
             LIMIT ?6"
        } else {
            // Early version-7 databases do not have the optional overlap spatial index.
            // Owner-cell queries remain indexed; scanning every object in a space does not.
            "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_placements o \
             JOIN object_definitions d ON d.definition_id = o.definition_id \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             WHERE o.world_space_id = ?1 \
               AND o.owner_cell_x BETWEEN ?2 AND ?3 \
               AND o.owner_cell_z BETWEEN ?4 AND ?5 \
             ORDER BY o.object_id \
             LIMIT ?6"
        };
        let mut statement = self.connection.prepare_cached(sql)?;
        let mut records = statement
            .query_map(
                params![
                    space.0, minimum.x, maximum.x, minimum.z, maximum.z, sql_limit
                ],
                source_object_view_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = records.len() > maximum_records;
        records.truncate(maximum_records);
        Ok(SourceObjectViewQuery { records, truncated })
    }

    pub fn read_object_view(
        &self,
        object: StableObjectId,
    ) -> Result<Option<SourceObjectViewRecord>, WorldDbError> {
        self.connection
            .query_row(
                "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                        o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                        o.source_revision, d.definition_key, d.display_name, d.visual_asset_id, \
                        d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
                 FROM object_placements o \
                 JOIN object_definitions d ON d.definition_id = o.definition_id \
                 LEFT JOIN source_asset_variants v \
                        ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
                 WHERE o.object_id = ?1",
                params![object.0.as_slice()],
                source_object_view_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Reads one stable owner-cell-ordered outliner page without materializing the project tree.
    pub fn read_object_outliner_page(
        &self,
        space: WorldSpaceId,
        search: &str,
        cursor: Option<&SourceObjectOutlinerCursor>,
        maximum_records: usize,
    ) -> Result<SourceObjectOutlinerPage, WorldDbError> {
        if maximum_records == 0 {
            return Err(WorldDbError::InvalidQueryLimit);
        }
        let sql_limit = query_sql_limit(maximum_records)?;
        let cursor_x = cursor.map(|cursor| cursor.owner_cell.x);
        let cursor_z = cursor.map(|cursor| cursor.owner_cell.z);
        let cursor_object = cursor.map(|cursor| cursor.object.0.to_vec());
        let mut statement = self.connection.prepare_cached(
            "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_placements o \
             JOIN object_definitions d ON d.definition_id = o.definition_id \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             WHERE o.world_space_id = ?1 \
               AND (?2 = '' OR instr(lower(d.definition_key), lower(?2)) > 0 \
                            OR instr(lower(d.display_name), lower(?2)) > 0) \
               AND (?3 IS NULL OR o.owner_cell_x > ?3 \
                    OR (o.owner_cell_x = ?3 AND o.owner_cell_z > ?4) \
                    OR (o.owner_cell_x = ?3 AND o.owner_cell_z = ?4 AND o.object_id > ?5)) \
             ORDER BY o.owner_cell_x, o.owner_cell_z, o.object_id \
             LIMIT ?6",
        )?;
        let mut records = statement
            .query_map(
                params![
                    space.0,
                    search.trim(),
                    cursor_x,
                    cursor_z,
                    cursor_object,
                    sql_limit
                ],
                source_object_view_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let has_next = records.len() > maximum_records;
        records.truncate(maximum_records);
        let next_cursor = has_next.then(|| {
            let last = records
                .last()
                .expect("a truncated query page has at least one retained record");
            SourceObjectOutlinerCursor {
                owner_cell: last.object.owner_cell,
                object: last.object.id,
            }
        });
        Ok(SourceObjectOutlinerPage {
            records,
            next_cursor,
        })
    }

    pub fn read_object_palette(
        &self,
        maximum_records: usize,
    ) -> Result<SourceObjectPaletteQuery, WorldDbError> {
        if maximum_records == 0 {
            return Err(WorldDbError::InvalidQueryLimit);
        }
        let sql_limit = query_sql_limit(maximum_records)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT d.definition_id, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_definitions d \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             ORDER BY d.definition_key \
             LIMIT ?1",
        )?;
        let mut records = statement
            .query_map(params![sql_limit], source_object_palette_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = records.len() > maximum_records;
        records.truncate(maximum_records);
        Ok(SourceObjectPaletteQuery { records, truncated })
    }

    /// Reads one searchable definition page using a stable key/ID cursor.
    pub fn read_object_palette_page(
        &self,
        search: &str,
        cursor: Option<&SourceObjectPaletteCursor>,
        maximum_records: usize,
    ) -> Result<SourceObjectPalettePage, WorldDbError> {
        if maximum_records == 0 {
            return Err(WorldDbError::InvalidQueryLimit);
        }
        let sql_limit = query_sql_limit(maximum_records)?;
        let cursor_key = cursor.map(|cursor| cursor.definition_key.as_str());
        let cursor_definition = cursor.map(|cursor| cursor.definition.0.to_vec());
        let mut statement = self.connection.prepare_cached(
            "SELECT d.definition_id, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_definitions d \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             WHERE (?1 = '' OR instr(lower(d.definition_key), lower(?1)) > 0 \
                             OR instr(lower(d.display_name), lower(?1)) > 0) \
               AND (?2 IS NULL OR d.definition_key > ?2 \
                    OR (d.definition_key = ?2 AND d.definition_id > ?3)) \
             ORDER BY d.definition_key, d.definition_id \
             LIMIT ?4",
        )?;
        let mut records = statement
            .query_map(
                params![search.trim(), cursor_key, cursor_definition, sql_limit],
                source_object_palette_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let has_next = records.len() > maximum_records;
        records.truncate(maximum_records);
        let next_cursor = has_next.then(|| {
            let last = records
                .last()
                .expect("a truncated query page has at least one retained record");
            SourceObjectPaletteCursor {
                definition_key: last.definition.key.clone(),
                definition: last.definition.id,
            }
        });
        Ok(SourceObjectPalettePage {
            records,
            next_cursor,
        })
    }
}

/// Narrow transactional writer used by the editor's authoring worker.
///
/// Updates and deletes compare source revisions in SQLite. A conflict rolls back the whole batch;
/// placement creation is used by the inverse of a previously committed deletion.
pub struct ProjectWriter {
    connection: Connection,
}

impl ProjectWriter {
    pub fn open(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 1000;")?;
        ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;
        Ok(Self { connection })
    }

    pub fn update_object_transform(
        &mut self,
        object: StableObjectId,
        expected_source_revision: i64,
        transform: SourceObjectTransform,
    ) -> Result<ObjectTransformWriteResult, WorldDbError> {
        let result = self.apply_object_transaction(&[SourceObjectWrite::UpdateTransform {
            object,
            expected_source_revision,
            transform,
        }])?;
        match result {
            ObjectWriteTransactionResult::Committed(mut commits) => match commits.pop() {
                Some(SourceObjectWriteCommit::Updated(object)) => {
                    Ok(ObjectTransformWriteResult::Updated(object))
                }
                _ => Err(WorldDbError::InvalidObjectTransaction),
            },
            ObjectWriteTransactionResult::Conflict { actual, .. } => {
                Ok(ObjectTransformWriteResult::Conflict { actual })
            }
        }
    }

    pub fn apply_object_transaction(
        &mut self,
        writes: &[SourceObjectWrite],
    ) -> Result<ObjectWriteTransactionResult, WorldDbError> {
        if writes.is_empty() || writes.len() > MAX_OBJECT_WRITES_PER_TRANSACTION {
            return Err(WorldDbError::InvalidObjectTransaction);
        }
        let mut objects = HashSet::with_capacity(writes.len());
        if writes.iter().any(|write| !objects.insert(write.object())) {
            return Err(WorldDbError::InvalidObjectTransaction);
        }

        let transaction = self.connection.transaction()?;
        let mut commits = Vec::with_capacity(writes.len());
        for write in writes {
            let (object, updated) = match write {
                SourceObjectWrite::Create { object } => {
                    validate_object_transform(SourceObjectTransform::from(object))?;
                    let actual = transaction
                        .query_row(
                            "SELECT object_id, world_space_id, owner_cell_x, owner_cell_z, \
                                    definition_id, local_x, local_y, local_z, yaw, scale, \
                                    source_revision \
                             FROM object_placements WHERE object_id = ?1",
                            params![object.id.0.as_slice()],
                            source_object_from_row,
                        )
                        .optional()?;
                    if actual.is_some() {
                        transaction.rollback()?;
                        return Ok(ObjectWriteTransactionResult::Conflict {
                            object: object.id,
                            actual,
                        });
                    }
                    let source_revision = object.source_revision.saturating_add(1);
                    let updated = transaction.execute(
                        "INSERT INTO object_placements( \
                            object_id, world_space_id, owner_cell_x, owner_cell_z, definition_id, \
                            local_x, local_y, local_z, yaw, scale, source_revision \
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        params![
                            object.id.0.as_slice(),
                            object.space.0,
                            object.owner_cell.x,
                            object.owner_cell.z,
                            object.definition.0.as_slice(),
                            object.local_translation[0],
                            object.local_translation[1],
                            object.local_translation[2],
                            object.yaw,
                            object.scale,
                            source_revision,
                        ],
                    )?;
                    (object.id, updated)
                }
                SourceObjectWrite::UpdateTransform {
                    object,
                    expected_source_revision,
                    transform,
                } => {
                    validate_object_transform(*transform)?;
                    let updated = transaction.execute(
                        "UPDATE object_placements \
                         SET world_space_id = ?1, owner_cell_x = ?2, owner_cell_z = ?3, \
                             local_x = ?4, local_y = ?5, local_z = ?6, yaw = ?7, scale = ?8, \
                             source_revision = source_revision + 1 \
                         WHERE object_id = ?9 AND source_revision = ?10",
                        params![
                            transform.space.0,
                            transform.owner_cell.x,
                            transform.owner_cell.z,
                            transform.local_translation[0],
                            transform.local_translation[1],
                            transform.local_translation[2],
                            transform.yaw,
                            transform.scale,
                            object.0.as_slice(),
                            expected_source_revision,
                        ],
                    )?;
                    (*object, updated)
                }
                SourceObjectWrite::Delete {
                    object,
                    expected_source_revision,
                } => {
                    let updated = transaction.execute(
                        "DELETE FROM object_placements \
                         WHERE object_id = ?1 AND source_revision = ?2",
                        params![object.0.as_slice(), expected_source_revision],
                    )?;
                    (*object, updated)
                }
            };
            if updated == 0 {
                let actual = transaction
                    .query_row(
                        "SELECT object_id, world_space_id, owner_cell_x, owner_cell_z, \
                                definition_id, local_x, local_y, local_z, yaw, scale, \
                                source_revision \
                         FROM object_placements WHERE object_id = ?1",
                        params![object.0.as_slice()],
                        source_object_from_row,
                    )
                    .optional()?;
                transaction.rollback()?;
                return Ok(ObjectWriteTransactionResult::Conflict { object, actual });
            }

            match write {
                SourceObjectWrite::Create { .. } | SourceObjectWrite::UpdateTransform { .. } => {
                    transaction.execute(
                        "DELETE FROM object_cell_overlaps WHERE object_id = ?1",
                        params![object.0.as_slice()],
                    )?;
                    let updated_object = transaction.query_row(
                        "SELECT object_id, world_space_id, owner_cell_x, owner_cell_z, \
                                definition_id, local_x, local_y, local_z, yaw, scale, \
                                source_revision \
                         FROM object_placements WHERE object_id = ?1",
                        params![object.0.as_slice()],
                        source_object_from_row,
                    )?;
                    commits.push(SourceObjectWriteCommit::Updated(updated_object));
                }
                SourceObjectWrite::Delete { .. } => {
                    commits.push(SourceObjectWriteCommit::Deleted(object));
                }
            }
        }
        transaction.commit()?;
        Ok(ObjectWriteTransactionResult::Committed(commits))
    }

    /// Creates regions and deletes only regions whose coverage is empty. The emptiness check and
    /// delete share the same SQLite transaction, so a bounded editor query can never accidentally
    /// cascade masks that were outside its loaded window.
    pub fn apply_ground_cover_region_transaction(
        &mut self,
        writes: &[GroundCoverRegionWrite],
    ) -> Result<GroundCoverRegionWriteTransactionResult, WorldDbError> {
        if writes.is_empty() || writes.len() > MAX_GROUND_COVER_REGION_WRITES_PER_TRANSACTION {
            return Err(WorldDbError::InvalidGroundCoverRegionTransaction);
        }
        let mut regions = HashSet::with_capacity(writes.len());
        if writes.iter().any(|write| !regions.insert(write.region())) {
            return Err(WorldDbError::InvalidGroundCoverRegionTransaction);
        }
        for write in writes {
            validate_ground_cover_region_write(write)?;
        }

        let transaction = self.connection.transaction()?;
        let mut commits = Vec::with_capacity(writes.len());
        for write in writes {
            match write {
                GroundCoverRegionWrite::Create { record } => {
                    let actual = read_ground_cover_region(&transaction, record.id)?;
                    if actual.is_some() {
                        transaction.rollback()?;
                        return Ok(GroundCoverRegionWriteTransactionResult::Conflict {
                            region: record.id,
                            actual,
                        });
                    }
                    let valid_references: bool = transaction.query_row(
                        "SELECT EXISTS( \
                            SELECT 1 FROM ground_cover_layers \
                            WHERE layer_id = ?1 AND world_space_id = ?2 \
                         ) AND EXISTS( \
                            SELECT 1 FROM ground_cover_presets WHERE preset_id = ?3 \
                         )",
                        params![
                            record.layer.0.as_slice(),
                            record.space.0,
                            record.preset.0.as_slice(),
                        ],
                        |row| row.get(0),
                    )?;
                    if !valid_references {
                        transaction.rollback()?;
                        return Err(WorldDbError::InvalidGroundCoverRegionRecord);
                    }
                    let source_revision = record.source_revision.saturating_add(1);
                    transaction.execute(
                        "INSERT INTO ground_cover_regions( \
                            region_id, layer_id, world_space_id, preset_id, display_name, enabled, \
                            density_multiplier, source_revision \
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            record.id.0.as_slice(),
                            record.layer.0.as_slice(),
                            record.space.0,
                            record.preset.0.as_slice(),
                            record.display_name,
                            record.enabled,
                            record.density_multiplier,
                            source_revision,
                        ],
                    )?;
                    let mut committed = record.clone();
                    committed.source_revision = source_revision;
                    commits.push(GroundCoverRegionWriteCommit::Created(committed));
                }
                GroundCoverRegionWrite::Update {
                    expected_source_revision,
                    record,
                } => {
                    let actual = read_ground_cover_region(&transaction, record.id)?;
                    if actual.as_ref().map(|record| record.source_revision)
                        != Some(*expected_source_revision)
                    {
                        transaction.rollback()?;
                        return Ok(GroundCoverRegionWriteTransactionResult::Conflict {
                            region: record.id,
                            actual,
                        });
                    }
                    let actual = actual.expect("a matching revision has an existing region");
                    if actual.layer != record.layer || actual.space != record.space {
                        transaction.rollback()?;
                        return Err(WorldDbError::InvalidGroundCoverRegionRecord);
                    }
                    let preset_exists: bool = transaction.query_row(
                        "SELECT EXISTS( \
                            SELECT 1 FROM ground_cover_presets WHERE preset_id = ?1 \
                         )",
                        params![record.preset.0.as_slice()],
                        |row| row.get(0),
                    )?;
                    if !preset_exists {
                        transaction.rollback()?;
                        return Err(WorldDbError::InvalidGroundCoverRegionRecord);
                    }
                    let updated = transaction.execute(
                        "UPDATE ground_cover_regions \
                         SET preset_id = ?1, display_name = ?2, enabled = ?3, \
                             density_multiplier = ?4, source_revision = source_revision + 1 \
                         WHERE region_id = ?5 AND source_revision = ?6",
                        params![
                            record.preset.0.as_slice(),
                            record.display_name,
                            record.enabled,
                            record.density_multiplier,
                            record.id.0.as_slice(),
                            expected_source_revision,
                        ],
                    )?;
                    debug_assert_eq!(updated, 1);
                    let committed = read_ground_cover_region(&transaction, record.id)?
                        .expect("an updated region remains present");
                    commits.push(GroundCoverRegionWriteCommit::Updated(committed));
                }
                GroundCoverRegionWrite::DeleteEmpty {
                    region,
                    expected_source_revision,
                } => {
                    let actual = read_ground_cover_region(&transaction, *region)?;
                    if actual.as_ref().map(|record| record.source_revision)
                        != Some(*expected_source_revision)
                    {
                        transaction.rollback()?;
                        return Ok(GroundCoverRegionWriteTransactionResult::Conflict {
                            region: *region,
                            actual,
                        });
                    }
                    let has_coverage: bool = transaction.query_row(
                        "SELECT EXISTS( \
                            SELECT 1 FROM ground_cover_region_cell_masks \
                            WHERE region_id = ?1 LIMIT 1 \
                         )",
                        params![region.0.as_slice()],
                        |row| row.get(0),
                    )?;
                    if has_coverage {
                        transaction.rollback()?;
                        return Ok(GroundCoverRegionWriteTransactionResult::BlockedByCoverage {
                            region: *region,
                        });
                    }
                    let deleted = transaction.execute(
                        "DELETE FROM ground_cover_regions \
                         WHERE region_id = ?1 AND source_revision = ?2",
                        params![region.0.as_slice(), expected_source_revision],
                    )?;
                    if deleted != 1 {
                        let actual = read_ground_cover_region(&transaction, *region)?;
                        transaction.rollback()?;
                        return Ok(GroundCoverRegionWriteTransactionResult::Conflict {
                            region: *region,
                            actual,
                        });
                    }
                    commits.push(GroundCoverRegionWriteCommit::Deleted(*region));
                }
            }
        }
        transaction.commit()?;
        Ok(GroundCoverRegionWriteTransactionResult::Committed(commits))
    }

    /// Atomically creates, updates, or deletes bounded reusable ground-cover definitions.
    ///
    /// Callers order upserts visual-before-preset and deletions preset-before-visual. Every write
    /// checks identity/revision before mutation; database-wide dependencies block deletion without
    /// partially committing the batch.
    pub fn apply_ground_cover_catalog_transaction(
        &mut self,
        writes: &[GroundCoverCatalogWrite],
    ) -> Result<GroundCoverCatalogWriteTransactionResult, WorldDbError> {
        if writes.is_empty() || writes.len() > MAX_GROUND_COVER_CATALOG_WRITES_PER_TRANSACTION {
            return Err(WorldDbError::InvalidGroundCoverCatalogTransaction);
        }
        let mut keys = HashSet::with_capacity(writes.len());
        if writes.iter().any(|write| !keys.insert(write.key())) {
            return Err(WorldDbError::InvalidGroundCoverCatalogTransaction);
        }
        for write in writes {
            validate_ground_cover_catalog_write(write)?;
        }

        let transaction = self.connection.transaction()?;
        let mut commits = Vec::with_capacity(writes.len());
        for write in writes {
            let key = write.key();
            let (expected_source_revision, actual) = match write {
                GroundCoverCatalogWrite::CreateVisual { record } => (
                    None,
                    read_ground_cover_visual(&transaction, record.id)?
                        .map(GroundCoverCatalogRecord::Visual),
                ),
                GroundCoverCatalogWrite::UpdateVisual {
                    expected_source_revision,
                    record,
                } => (
                    Some(*expected_source_revision),
                    read_ground_cover_visual(&transaction, record.id)?
                        .map(GroundCoverCatalogRecord::Visual),
                ),
                GroundCoverCatalogWrite::DeleteVisual {
                    visual,
                    expected_source_revision,
                } => (
                    Some(*expected_source_revision),
                    read_ground_cover_visual(&transaction, *visual)?
                        .map(GroundCoverCatalogRecord::Visual),
                ),
                GroundCoverCatalogWrite::CreatePreset { record } => (
                    None,
                    read_ground_cover_preset(&transaction, record.id)?
                        .map(GroundCoverCatalogRecord::Preset),
                ),
                GroundCoverCatalogWrite::UpdatePreset {
                    expected_source_revision,
                    record,
                } => (
                    Some(*expected_source_revision),
                    read_ground_cover_preset(&transaction, record.id)?
                        .map(GroundCoverCatalogRecord::Preset),
                ),
                GroundCoverCatalogWrite::DeletePreset {
                    preset,
                    expected_source_revision,
                } => (
                    Some(*expected_source_revision),
                    read_ground_cover_preset(&transaction, *preset)?
                        .map(GroundCoverCatalogRecord::Preset),
                ),
            };
            if actual
                .as_ref()
                .map(GroundCoverCatalogRecord::source_revision)
                != expected_source_revision
            {
                transaction.rollback()?;
                return Ok(GroundCoverCatalogWriteTransactionResult::Conflict { key, actual });
            }

            let commit = match write {
                GroundCoverCatalogWrite::CreateVisual { record } => {
                    let source_revision = record.source_revision.saturating_add(1);
                    transaction.execute(
                        "INSERT INTO ground_cover_visuals( \
                            visual_id, visual_key, display_name, visual_family, source_revision \
                         ) VALUES (?1, ?2, ?3, 0, ?4)",
                        params![
                            record.id.0.as_slice(),
                            record.key,
                            record.display_name,
                            source_revision,
                        ],
                    )?;
                    insert_ground_cover_card_visual(&transaction, record)?;
                    let mut committed = record.clone();
                    committed.source_revision = source_revision;
                    GroundCoverCatalogWriteCommit::Created(GroundCoverCatalogRecord::Visual(
                        committed,
                    ))
                }
                GroundCoverCatalogWrite::UpdateVisual {
                    expected_source_revision,
                    record,
                } => {
                    let source_revision = expected_source_revision.saturating_add(1);
                    let updated = transaction.execute(
                        "UPDATE ground_cover_visuals \
                         SET visual_key = ?1, display_name = ?2, \
                             source_revision = ?3 \
                         WHERE visual_id = ?4 AND source_revision = ?5",
                        params![
                            record.key,
                            record.display_name,
                            source_revision,
                            record.id.0.as_slice(),
                            expected_source_revision,
                        ],
                    )?;
                    debug_assert_eq!(updated, 1);
                    let SourceGroundCoverVisualDefinition::CardCluster(card) = &record.definition;
                    let recipe = RecipeSqlValues::from(card.procedural_recipe);
                    let updated = transaction.execute(
                        "UPDATE ground_cover_card_visuals \
                         SET built_in_atlas_version = ?1, \
                             recipe_variant_count = ?2, recipe_blade_count = ?3, \
                             recipe_seed = ?4, recipe_minimum_blade_height = ?5, \
                             recipe_maximum_blade_height = ?6, recipe_base_jitter = ?7, \
                             recipe_minimum_blade_half_width = ?8, \
                             recipe_maximum_blade_half_width = ?9, recipe_maximum_lean = ?10, \
                             recipe_maximum_curve = ?11, recipe_maximum_s_curve = ?12, \
                             bottom_color_r = ?13, bottom_color_g = ?14, bottom_color_b = ?15, \
                             top_color_r = ?16, top_color_g = ?17, top_color_b = ?18, \
                             minimum_card_height = ?19, maximum_card_height = ?20, \
                             minimum_card_width = ?21, maximum_card_width = ?22, \
                             flattened_card_probability = ?23, \
                             maximum_wind_displacement = ?24 \
                         WHERE visual_id = ?25",
                        params![
                            i64::from(card.built_in_atlas_version),
                            recipe.variant_count,
                            recipe.blade_count,
                            recipe.seed,
                            recipe.minimum_blade_height,
                            recipe.maximum_blade_height,
                            recipe.base_jitter,
                            recipe.minimum_blade_half_width,
                            recipe.maximum_blade_half_width,
                            recipe.maximum_lean,
                            recipe.maximum_curve,
                            recipe.maximum_s_curve,
                            card.bottom_color[0],
                            card.bottom_color[1],
                            card.bottom_color[2],
                            card.top_color[0],
                            card.top_color[1],
                            card.top_color[2],
                            card.minimum_card_height,
                            card.maximum_card_height,
                            card.minimum_card_width,
                            card.maximum_card_width,
                            card.flattened_card_probability,
                            card.maximum_wind_displacement,
                            record.id.0.as_slice(),
                        ],
                    )?;
                    debug_assert_eq!(updated, 1);
                    let mut committed = record.clone();
                    committed.source_revision = source_revision;
                    GroundCoverCatalogWriteCommit::Updated(GroundCoverCatalogRecord::Visual(
                        committed,
                    ))
                }
                GroundCoverCatalogWrite::DeleteVisual { visual, .. } => {
                    let has_presets: bool = transaction.query_row(
                        "SELECT EXISTS( \
                            SELECT 1 FROM ground_cover_presets WHERE visual_id = ?1 LIMIT 1 \
                         )",
                        params![visual.0.as_slice()],
                        |row| row.get(0),
                    )?;
                    if has_presets {
                        transaction.rollback()?;
                        return Ok(
                            GroundCoverCatalogWriteTransactionResult::BlockedByDependency {
                                key,
                                dependency: GroundCoverCatalogDependency::VisualPresets,
                            },
                        );
                    }
                    transaction.execute(
                        "DELETE FROM ground_cover_visuals WHERE visual_id = ?1",
                        params![visual.0.as_slice()],
                    )?;
                    GroundCoverCatalogWriteCommit::Deleted(key)
                }
                GroundCoverCatalogWrite::CreatePreset { record } => {
                    if read_ground_cover_visual(&transaction, record.visual)?.is_none() {
                        transaction.rollback()?;
                        return Err(WorldDbError::InvalidGroundCoverCatalogRecord);
                    }
                    let source_revision = record.source_revision.saturating_add(1);
                    transaction.execute(
                        "INSERT INTO ground_cover_presets( \
                            preset_id, preset_key, display_name, enabled, visual_id, \
                            density_per_square_meter, seed, source_revision \
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            record.id.0.as_slice(),
                            record.key,
                            record.display_name,
                            record.enabled,
                            record.visual.0.as_slice(),
                            record.density_per_square_meter,
                            i64::from(record.seed),
                            source_revision,
                        ],
                    )?;
                    let mut committed = record.clone();
                    committed.source_revision = source_revision;
                    GroundCoverCatalogWriteCommit::Created(GroundCoverCatalogRecord::Preset(
                        committed,
                    ))
                }
                GroundCoverCatalogWrite::UpdatePreset {
                    expected_source_revision,
                    record,
                } => {
                    if read_ground_cover_visual(&transaction, record.visual)?.is_none() {
                        transaction.rollback()?;
                        return Err(WorldDbError::InvalidGroundCoverCatalogRecord);
                    }
                    let source_revision = expected_source_revision.saturating_add(1);
                    let updated = transaction.execute(
                        "UPDATE ground_cover_presets \
                         SET preset_key = ?1, display_name = ?2, enabled = ?3, visual_id = ?4, \
                             density_per_square_meter = ?5, seed = ?6, source_revision = ?7 \
                         WHERE preset_id = ?8 AND source_revision = ?9",
                        params![
                            record.key,
                            record.display_name,
                            record.enabled,
                            record.visual.0.as_slice(),
                            record.density_per_square_meter,
                            i64::from(record.seed),
                            source_revision,
                            record.id.0.as_slice(),
                            expected_source_revision,
                        ],
                    )?;
                    debug_assert_eq!(updated, 1);
                    let mut committed = record.clone();
                    committed.source_revision = source_revision;
                    GroundCoverCatalogWriteCommit::Updated(GroundCoverCatalogRecord::Preset(
                        committed,
                    ))
                }
                GroundCoverCatalogWrite::DeletePreset { preset, .. } => {
                    let has_regions: bool = transaction.query_row(
                        "SELECT EXISTS( \
                            SELECT 1 FROM ground_cover_regions WHERE preset_id = ?1 LIMIT 1 \
                         )",
                        params![preset.0.as_slice()],
                        |row| row.get(0),
                    )?;
                    if has_regions {
                        transaction.rollback()?;
                        return Ok(
                            GroundCoverCatalogWriteTransactionResult::BlockedByDependency {
                                key,
                                dependency: GroundCoverCatalogDependency::PresetRegions,
                            },
                        );
                    }
                    transaction.execute(
                        "DELETE FROM ground_cover_presets WHERE preset_id = ?1",
                        params![preset.0.as_slice()],
                    )?;
                    GroundCoverCatalogWriteCommit::Deleted(key)
                }
            };
            commits.push(commit);
        }
        transaction.commit()?;
        Ok(GroundCoverCatalogWriteTransactionResult::Committed(commits))
    }

    /// Atomically writes bounded terrain-weight and ground-cover-mask records.
    ///
    /// `expected_source_revision = None` means the caller expects the key not to exist. Every
    /// mismatch returns the current record and rolls back all earlier writes in the batch.
    pub fn apply_dense_source_transaction(
        &mut self,
        writes: &[DenseSourceWrite],
    ) -> Result<DenseSourceWriteTransactionResult, WorldDbError> {
        if writes.is_empty() || writes.len() > MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION {
            return Err(WorldDbError::InvalidDenseSourceTransaction);
        }
        let mut keys = HashSet::with_capacity(writes.len());
        if writes.iter().any(|write| !keys.insert(write.key())) {
            return Err(WorldDbError::InvalidDenseSourceTransaction);
        }
        for write in writes {
            validate_dense_source_write(write)?;
        }

        let transaction = self.connection.transaction()?;
        let mut commits = Vec::with_capacity(writes.len());
        for write in writes {
            let key = write.key();
            let (expected_source_revision, actual) = match write {
                DenseSourceWrite::TerrainWeights {
                    expected_source_revision,
                    record,
                } => (
                    *expected_source_revision,
                    read_terrain_weights(&transaction, record.space, record.cell, record.page)?
                        .map(DenseSourceRecord::TerrainWeights),
                ),
                DenseSourceWrite::GroundCoverMask {
                    expected_source_revision,
                    record,
                } => (
                    *expected_source_revision,
                    read_ground_cover_mask(&transaction, record.region, record.space, record.cell)?
                        .map(DenseSourceRecord::GroundCoverMask),
                ),
            };
            let actual_revision = actual.as_ref().map(dense_source_revision);
            if expected_source_revision != actual_revision {
                transaction.rollback()?;
                return Ok(DenseSourceWriteTransactionResult::Conflict { key, actual });
            }

            let source_revision = actual_revision.unwrap_or(0).saturating_add(1);
            let commit = match write {
                DenseSourceWrite::TerrainWeights { record, .. } => {
                    transaction.execute(
                        "INSERT INTO terrain_cell_weight_pages( \
                            world_space_id, cell_x, cell_z, page, resolution, rgba, \
                            source_revision \
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                         ON CONFLICT(world_space_id, cell_x, cell_z, page) DO UPDATE SET \
                            resolution = excluded.resolution, rgba = excluded.rgba, \
                            source_revision = excluded.source_revision",
                        params![
                            record.space.0,
                            record.cell.x,
                            record.cell.z,
                            i64::from(record.page),
                            i64::from(record.resolution),
                            record.rgba.as_slice(),
                            source_revision,
                        ],
                    )?;
                    let mut committed = record.clone();
                    committed.source_revision = source_revision;
                    DenseSourceRecord::TerrainWeights(committed)
                }
                DenseSourceWrite::GroundCoverMask { record, .. } => {
                    transaction.execute(
                        "INSERT INTO ground_cover_region_cell_masks( \
                            region_id, world_space_id, cell_x, cell_z, resolution, coverage, \
                            source_revision \
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                         ON CONFLICT(region_id, world_space_id, cell_x, cell_z) DO UPDATE SET \
                            resolution = excluded.resolution, coverage = excluded.coverage, \
                            source_revision = excluded.source_revision",
                        params![
                            record.region.0.as_slice(),
                            record.space.0,
                            record.cell.x,
                            record.cell.z,
                            i64::from(record.resolution),
                            record.coverage.as_slice(),
                            source_revision,
                        ],
                    )?;
                    let mut committed = record.clone();
                    committed.source_revision = source_revision;
                    DenseSourceRecord::GroundCoverMask(committed)
                }
            };
            commits.push(commit);
        }
        transaction.commit()?;
        Ok(DenseSourceWriteTransactionResult::Committed(commits))
    }
}

fn dense_source_revision(record: &DenseSourceRecord) -> i64 {
    match record {
        DenseSourceRecord::TerrainWeights(record) => record.source_revision,
        DenseSourceRecord::GroundCoverMask(record) => record.source_revision,
    }
}

fn read_terrain_weights(
    transaction: &Transaction<'_>,
    space: WorldSpaceId,
    cell: CellCoord,
    page: u8,
) -> Result<Option<SourceTerrainCellWeightPageRecord>, WorldDbError> {
    transaction
        .query_row(
            "SELECT world_space_id, cell_x, cell_z, page, resolution, rgba, source_revision \
             FROM terrain_cell_weight_pages \
             WHERE world_space_id = ?1 AND cell_x = ?2 AND cell_z = ?3 AND page = ?4",
            params![space.0, cell.x, cell.z, i64::from(page)],
            source_terrain_weights_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn read_ground_cover_mask(
    transaction: &Transaction<'_>,
    region: GroundCoverRegionId,
    space: WorldSpaceId,
    cell: CellCoord,
) -> Result<Option<SourceGroundCoverCellMaskRecord>, WorldDbError> {
    transaction
        .query_row(
            "SELECT region_id, world_space_id, cell_x, cell_z, resolution, coverage, \
                    source_revision \
             FROM ground_cover_region_cell_masks \
             WHERE region_id = ?1 AND world_space_id = ?2 AND cell_x = ?3 AND cell_z = ?4",
            params![region.0.as_slice(), space.0, cell.x, cell.z],
            source_ground_cover_mask_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn read_ground_cover_region(
    transaction: &Transaction<'_>,
    region: GroundCoverRegionId,
) -> Result<Option<SourceGroundCoverRegionRecord>, WorldDbError> {
    transaction
        .query_row(
            "SELECT region_id, layer_id, world_space_id, preset_id, display_name, enabled, \
                    density_multiplier, source_revision \
             FROM ground_cover_regions WHERE region_id = ?1",
            params![region.0.as_slice()],
            source_ground_cover_region_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn read_ground_cover_preset(
    transaction: &Transaction<'_>,
    preset: GroundCoverPresetId,
) -> Result<Option<SourceGroundCoverPresetRecord>, WorldDbError> {
    transaction
        .query_row(
            "SELECT preset_id, preset_key, display_name, enabled, visual_id, \
                    density_per_square_meter, seed, source_revision \
             FROM ground_cover_presets WHERE preset_id = ?1",
            params![preset.0.as_slice()],
            source_ground_cover_preset_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn read_ground_cover_visual(
    transaction: &Transaction<'_>,
    visual: GroundCoverVisualId,
) -> Result<Option<SourceGroundCoverVisualRecord>, WorldDbError> {
    transaction
        .query_row(
            "SELECT v.visual_id, v.visual_key, v.display_name, v.visual_family, \
                    v.source_revision, c.built_in_atlas_version, c.recipe_variant_count, \
                    c.recipe_blade_count, c.recipe_seed, c.recipe_minimum_blade_height, \
                    c.recipe_maximum_blade_height, c.recipe_base_jitter, \
                    c.recipe_minimum_blade_half_width, c.recipe_maximum_blade_half_width, \
                    c.recipe_maximum_lean, c.recipe_maximum_curve, c.recipe_maximum_s_curve, \
                    c.bottom_color_r, \
                    c.bottom_color_g, c.bottom_color_b, c.top_color_r, c.top_color_g, \
                    c.top_color_b, c.minimum_card_height, c.maximum_card_height, \
                    c.minimum_card_width, c.maximum_card_width, \
                    c.flattened_card_probability, c.maximum_wind_displacement \
             FROM ground_cover_visuals v \
             JOIN ground_cover_card_visuals c ON c.visual_id = v.visual_id \
             WHERE v.visual_id = ?1",
            params![visual.0.as_slice()],
            source_ground_cover_visual_from_row,
        )
        .optional()
        .map_err(Into::into)
}

fn insert_ground_cover_card_visual(
    transaction: &Transaction<'_>,
    record: &SourceGroundCoverVisualRecord,
) -> Result<(), WorldDbError> {
    let SourceGroundCoverVisualDefinition::CardCluster(card) = &record.definition;
    let recipe = RecipeSqlValues::from(card.procedural_recipe);
    transaction.execute(
        "INSERT INTO ground_cover_card_visuals( \
            visual_id, built_in_atlas_version, recipe_variant_count, recipe_blade_count, \
            recipe_seed, recipe_minimum_blade_height, recipe_maximum_blade_height, \
            recipe_base_jitter, recipe_minimum_blade_half_width, \
            recipe_maximum_blade_half_width, recipe_maximum_lean, recipe_maximum_curve, \
            recipe_maximum_s_curve, bottom_color_r, bottom_color_g, bottom_color_b, \
            top_color_r, top_color_g, top_color_b, minimum_card_height, maximum_card_height, \
            minimum_card_width, maximum_card_width, flattened_card_probability, \
            maximum_wind_displacement \
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                   ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
        params![
            record.id.0.as_slice(),
            i64::from(card.built_in_atlas_version),
            recipe.variant_count,
            recipe.blade_count,
            recipe.seed,
            recipe.minimum_blade_height,
            recipe.maximum_blade_height,
            recipe.base_jitter,
            recipe.minimum_blade_half_width,
            recipe.maximum_blade_half_width,
            recipe.maximum_lean,
            recipe.maximum_curve,
            recipe.maximum_s_curve,
            card.bottom_color[0],
            card.bottom_color[1],
            card.bottom_color[2],
            card.top_color[0],
            card.top_color[1],
            card.top_color[2],
            card.minimum_card_height,
            card.maximum_card_height,
            card.minimum_card_width,
            card.maximum_card_width,
            card.flattened_card_probability,
            card.maximum_wind_displacement,
        ],
    )?;
    Ok(())
}

fn validate_ground_cover_region_write(write: &GroundCoverRegionWrite) -> Result<(), WorldDbError> {
    match write {
        GroundCoverRegionWrite::Create { record } => {
            if record.display_name.trim().is_empty()
                || !record.density_multiplier.is_finite()
                || record.density_multiplier <= 0.0
                || record.source_revision < 0
            {
                return Err(WorldDbError::InvalidGroundCoverRegionRecord);
            }
        }
        GroundCoverRegionWrite::Update {
            expected_source_revision,
            record,
        } => {
            if *expected_source_revision != record.source_revision
                || record.display_name.trim().is_empty()
                || !record.density_multiplier.is_finite()
                || record.density_multiplier <= 0.0
                || record.source_revision < 0
            {
                return Err(WorldDbError::InvalidGroundCoverRegionRecord);
            }
        }
        GroundCoverRegionWrite::DeleteEmpty {
            expected_source_revision,
            ..
        } if *expected_source_revision < 0 => {
            return Err(WorldDbError::InvalidGroundCoverRegionRecord);
        }
        GroundCoverRegionWrite::DeleteEmpty { .. } => {}
    }
    Ok(())
}

fn validate_ground_cover_catalog_write(
    write: &GroundCoverCatalogWrite,
) -> Result<(), WorldDbError> {
    match write {
        GroundCoverCatalogWrite::CreatePreset { record } => validate_ground_cover_preset(record)?,
        GroundCoverCatalogWrite::UpdatePreset {
            expected_source_revision,
            record,
        } => {
            if *expected_source_revision != record.source_revision {
                return Err(WorldDbError::InvalidGroundCoverCatalogRecord);
            }
            validate_ground_cover_preset(record)?;
        }
        GroundCoverCatalogWrite::DeletePreset {
            expected_source_revision,
            ..
        }
        | GroundCoverCatalogWrite::DeleteVisual {
            expected_source_revision,
            ..
        } if *expected_source_revision < 0 => {
            return Err(WorldDbError::InvalidGroundCoverCatalogRecord);
        }
        GroundCoverCatalogWrite::DeletePreset { .. }
        | GroundCoverCatalogWrite::DeleteVisual { .. } => {}
        GroundCoverCatalogWrite::CreateVisual { record } => validate_ground_cover_visual(record)?,
        GroundCoverCatalogWrite::UpdateVisual {
            expected_source_revision,
            record,
        } => {
            if *expected_source_revision != record.source_revision {
                return Err(WorldDbError::InvalidGroundCoverCatalogRecord);
            }
            validate_ground_cover_visual(record)?;
        }
    }
    Ok(())
}

fn validate_ground_cover_preset(
    record: &SourceGroundCoverPresetRecord,
) -> Result<(), WorldDbError> {
    if record.source_revision < 0
        || record.key.trim().is_empty()
        || record.display_name.trim().is_empty()
        || !record.density_per_square_meter.is_finite()
        || record.density_per_square_meter <= 0.0
    {
        return Err(WorldDbError::InvalidGroundCoverCatalogRecord);
    }
    Ok(())
}

fn validate_ground_cover_visual(
    record: &SourceGroundCoverVisualRecord,
) -> Result<(), WorldDbError> {
    let SourceGroundCoverVisualDefinition::CardCluster(card) = &record.definition;
    if record.source_revision < 0
        || record.key.trim().is_empty()
        || record.display_name.trim().is_empty()
        || card.built_in_atlas_version != 1
        || card
            .procedural_recipe
            .is_some_and(|recipe| !recipe.is_valid())
        || !card
            .bottom_color
            .into_iter()
            .chain(card.top_color)
            .all(|component| component.is_finite() && (0.0..=1.0).contains(&component))
        || !card.minimum_card_height.is_finite()
        || card.minimum_card_height <= 0.0
        || !card.maximum_card_height.is_finite()
        || card.maximum_card_height < card.minimum_card_height
        || !card.minimum_card_width.is_finite()
        || card.minimum_card_width <= 0.0
        || !card.maximum_card_width.is_finite()
        || card.maximum_card_width < card.minimum_card_width
        || !card.flattened_card_probability.is_finite()
        || !(0.0..=1.0).contains(&card.flattened_card_probability)
        || !card.maximum_wind_displacement.is_finite()
        || card.maximum_wind_displacement < 0.0
    {
        return Err(WorldDbError::InvalidGroundCoverCatalogRecord);
    }
    Ok(())
}

fn validate_dense_source_write(write: &DenseSourceWrite) -> Result<(), WorldDbError> {
    let expected_revision = match write {
        DenseSourceWrite::TerrainWeights {
            expected_source_revision,
            record,
        } => {
            let expected_length = usize::from(record.resolution)
                .saturating_mul(usize::from(record.resolution))
                .saturating_mul(4);
            if record.page > 1
                || !(2..=257).contains(&record.resolution)
                || record.rgba.len() != expected_length
                || record.source_revision < 0
            {
                return Err(WorldDbError::InvalidDenseSourceRecord);
            }
            *expected_source_revision
        }
        DenseSourceWrite::GroundCoverMask {
            expected_source_revision,
            record,
        } => {
            let expected_length =
                usize::from(record.resolution).saturating_mul(usize::from(record.resolution));
            if !(1..=64).contains(&record.resolution)
                || record.coverage.len() != expected_length
                || record.source_revision < 0
            {
                return Err(WorldDbError::InvalidDenseSourceRecord);
            }
            *expected_source_revision
        }
    };
    if expected_revision.is_some_and(|revision| revision < 0) {
        return Err(WorldDbError::InvalidDenseSourceRecord);
    }
    Ok(())
}

fn source_cell_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceCellRecord> {
    Ok(SourceCellRecord {
        space: WorldSpaceId(row.get(0)?),
        cell: CellCoord {
            x: row.get(1)?,
            z: row.get(2)?,
        },
        height: row.get(3)?,
        source_revision: row.get(4)?,
    })
}

fn source_terrain_weights_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SourceTerrainCellWeightPageRecord> {
    Ok(SourceTerrainCellWeightPageRecord {
        space: WorldSpaceId(row.get(0)?),
        cell: CellCoord {
            x: row.get(1)?,
            z: row.get(2)?,
        },
        page: row.get::<_, i64>(3)? as u8,
        resolution: row.get::<_, i64>(4)? as u16,
        rgba: row.get(5)?,
        source_revision: row.get(6)?,
    })
}

fn source_ground_cover_layer_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SourceGroundCoverLayerRecord> {
    Ok(SourceGroundCoverLayerRecord {
        id: GroundCoverLayerId(blob_array(row.get_ref(0)?.as_blob()?, "layer_id")?),
        space: WorldSpaceId(row.get(1)?),
        key: row.get(2)?,
        display_name: row.get(3)?,
        enabled: row.get(4)?,
        sort_order: row.get(5)?,
        source_revision: row.get(6)?,
    })
}

fn source_ground_cover_region_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SourceGroundCoverRegionRecord> {
    Ok(SourceGroundCoverRegionRecord {
        id: GroundCoverRegionId(blob_array(row.get_ref(0)?.as_blob()?, "region_id")?),
        layer: GroundCoverLayerId(blob_array(row.get_ref(1)?.as_blob()?, "layer_id")?),
        space: WorldSpaceId(row.get(2)?),
        preset: GroundCoverPresetId(blob_array(row.get_ref(3)?.as_blob()?, "preset_id")?),
        display_name: row.get(4)?,
        enabled: row.get(5)?,
        density_multiplier: row.get(6)?,
        source_revision: row.get(7)?,
    })
}

fn source_ground_cover_mask_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SourceGroundCoverCellMaskRecord> {
    Ok(SourceGroundCoverCellMaskRecord {
        region: GroundCoverRegionId(blob_array(row.get_ref(0)?.as_blob()?, "region_id")?),
        space: WorldSpaceId(row.get(1)?),
        cell: CellCoord {
            x: row.get(2)?,
            z: row.get(3)?,
        },
        resolution: row.get::<_, i64>(4)? as u8,
        coverage: row.get(5)?,
        source_revision: row.get(6)?,
    })
}

fn source_object_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceObjectRecord> {
    Ok(SourceObjectRecord {
        id: StableObjectId(blob_array(row.get_ref(0)?.as_blob()?, "object_id")?),
        space: WorldSpaceId(row.get(1)?),
        owner_cell: CellCoord {
            x: row.get(2)?,
            z: row.get(3)?,
        },
        definition: ObjectDefinitionId(blob_array(row.get_ref(4)?.as_blob()?, "definition_id")?),
        local_translation: [row.get(5)?, row.get(6)?, row.get(7)?],
        yaw: row.get(8)?,
        scale: row.get(9)?,
        source_revision: row.get(10)?,
    })
}

fn source_object_view_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SourceObjectViewRecord> {
    let object = source_object_from_row(row)?;
    let visual_asset = row
        .get::<_, Option<Vec<u8>>>(13)?
        .map(|bytes| blob_array(&bytes, "visual_asset_id").map(AssetId))
        .transpose()?;
    let activation = ObjectActivationPolicy::try_from(row.get::<_, i64>(14)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            14,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    let definition = SourceObjectDefinitionRecord {
        id: object.definition,
        key: row.get(11)?,
        display_name: row.get(12)?,
        visual_asset,
        activation,
    };
    let visual_uri = row.get(15)?;
    let visual_bounds = row
        .get::<_, Option<f32>>(16)?
        .map(|bounds_x| -> rusqlite::Result<[f32; 3]> {
            Ok([bounds_x, row.get(17)?, row.get(18)?])
        })
        .transpose()?;
    Ok(SourceObjectViewRecord {
        object,
        definition,
        visual_uri,
        visual_bounds,
    })
}

fn source_object_palette_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SourceObjectPaletteRecord> {
    let id = ObjectDefinitionId(blob_array(row.get_ref(0)?.as_blob()?, "definition_id")?);
    let visual_asset = row
        .get::<_, Option<Vec<u8>>>(3)?
        .map(|bytes| blob_array(&bytes, "visual_asset_id").map(AssetId))
        .transpose()?;
    let activation = ObjectActivationPolicy::try_from(row.get::<_, i64>(4)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    let visual_bounds = row
        .get::<_, Option<f32>>(6)?
        .map(|bounds_x| -> rusqlite::Result<[f32; 3]> { Ok([bounds_x, row.get(7)?, row.get(8)?]) })
        .transpose()?;
    Ok(SourceObjectPaletteRecord {
        definition: SourceObjectDefinitionRecord {
            id,
            key: row.get(1)?,
            display_name: row.get(2)?,
            visual_asset,
            activation,
        },
        visual_uri: row.get(5)?,
        visual_bounds,
    })
}

fn validate_spatial_query(
    minimum: CellCoord,
    maximum: CellCoord,
    maximum_records: usize,
) -> Result<(), WorldDbError> {
    if minimum.x > maximum.x || minimum.z > maximum.z {
        return Err(WorldDbError::InvalidSpatialQueryBounds { minimum, maximum });
    }
    if maximum_records == 0 {
        return Err(WorldDbError::InvalidQueryLimit);
    }
    Ok(())
}

fn query_sql_limit(maximum_records: usize) -> Result<i64, WorldDbError> {
    i64::try_from(maximum_records.saturating_add(1)).map_err(|_| WorldDbError::IntegerOverflow)
}

fn validate_object_transform(transform: SourceObjectTransform) -> Result<(), WorldDbError> {
    if !transform.local_translation.into_iter().all(f32::is_finite)
        || !transform.yaw.is_finite()
        || !transform.scale.is_finite()
        || transform.scale <= 0.0
    {
        return Err(WorldDbError::InvalidObjectTransform);
    }
    Ok(())
}

pub fn write_runtime_database(path: &Path, build: &RuntimeBuild) -> Result<(), WorldDbError> {
    ensure_new_database_path(path)?;
    let mut connection = Connection::open(path)?;
    connection.execute_batch(schema::RUNTIME_SCHEMA)?;
    let transaction = connection.transaction()?;
    write_runtime_build(&transaction, build)?;
    transaction.commit()?;
    connection.execute_batch("PRAGMA optimize;")?;
    Ok(())
}

fn write_runtime_build(
    transaction: &Transaction<'_>,
    build: &RuntimeBuild,
) -> Result<(), WorldDbError> {
    let manifest = &build.manifest;
    for space in &manifest.world_spaces {
        transaction.execute(
            "INSERT INTO world_spaces(id, name, cell_size, minimum_y, maximum_y) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                space.id.0,
                space.name,
                space.cell_size,
                space.minimum_y,
                space.maximum_y
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO runtime_metadata( \
            singleton, schema_version, generation_id, content_hash, default_world_space_id \
         ) VALUES (1, ?1, ?2, ?3, ?4)",
        params![
            manifest.schema_version,
            manifest.generation_id,
            manifest.content_hash.as_slice(),
            manifest.default_world_space.0
        ],
    )?;
    write_terrain_catalog(
        transaction,
        &build.terrain_surfaces,
        &build.terrain_texture_sets,
        &build.terrain_texture_layers,
        &build.terrain_profiles,
    )?;
    for cell in &build.cells {
        transaction.execute(
            "INSERT INTO cells( \
                world_space_id, cell_x, cell_z, minimum_y, maximum_y, domain_mask, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                cell.space.0,
                cell.cell.x,
                cell.cell.z,
                cell.minimum_y,
                cell.maximum_y,
                i64::try_from(cell.domain_mask).map_err(|_| WorldDbError::IntegerOverflow)?,
                cell.source_revision
            ],
        )?;
    }
    for asset in &build.assets {
        transaction.execute(
            "INSERT INTO asset_variants( \
                asset_id, lod, kind, uri, bounds_x, bounds_y, bounds_z, \
                gpu_bytes_estimate, shadow_policy, minimum_screen_height \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                asset.asset.0.as_slice(),
                i64::from(asset.lod),
                asset.kind,
                asset.uri,
                asset.bounds[0],
                asset.bounds[1],
                asset.bounds[2],
                i64::try_from(asset.gpu_bytes_estimate)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
                asset.shadow_policy,
                asset.minimum_screen_height,
            ],
        )?;
    }
    for definition in &build.definitions {
        transaction.execute(
            "INSERT INTO object_definitions( \
                definition_id, definition_key, display_name, visual_asset_id, activation_policy \
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                definition.id.0.as_slice(),
                definition.key,
                definition.display_name,
                definition.visual_asset.map(|asset| asset.0),
                definition.activation as i64,
            ],
        )?;
    }
    write_ground_cover_species(transaction, &build.ground_cover_species)?;
    for page in &build.pages {
        transaction.execute(
            "INSERT INTO cell_pages( \
                world_space_id, cell_x, cell_z, domain, lod, codec, encoded_bytes, \
                decoded_bytes, gpu_bytes_estimate, checksum, payload \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                page.key.space.0,
                page.key.cell.x,
                page.key.cell.z,
                page.key.domain as i64,
                i64::from(page.key.lod),
                page.codec as i64,
                i64::try_from(page.payload.len()).map_err(|_| WorldDbError::IntegerOverflow)?,
                i64::try_from(page.decoded_bytes).map_err(|_| WorldDbError::IntegerOverflow)?,
                i64::try_from(page.gpu_bytes_estimate)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
                page.checksum.as_slice(),
                page.payload
            ],
        )?;
    }
    for dependency in &build.dependencies {
        transaction.execute(
            "INSERT INTO page_dependencies( \
                world_space_id, cell_x, cell_z, domain, lod, asset_id, asset_lod \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                dependency.page.space.0,
                dependency.page.cell.x,
                dependency.page.cell.z,
                dependency.page.domain as i64,
                i64::from(dependency.page.lod),
                dependency.asset.0.as_slice(),
                i64::from(dependency.asset_lod)
            ],
        )?;
    }
    for dependency in &build.definition_dependencies {
        transaction.execute(
            "INSERT INTO page_object_definitions( \
                world_space_id, cell_x, cell_z, domain, lod, definition_id \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                dependency.page.space.0,
                dependency.page.cell.x,
                dependency.page.cell.z,
                dependency.page.domain as i64,
                i64::from(dependency.page.lod),
                dependency.definition.0.as_slice(),
            ],
        )?;
    }
    for dependency in &build.ground_cover_species_dependencies {
        transaction.execute(
            "INSERT INTO page_ground_cover_species( \
                world_space_id, cell_x, cell_z, domain, lod, species_id \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                dependency.page.space.0,
                dependency.page.cell.x,
                dependency.page.cell.z,
                dependency.page.domain as i64,
                i64::from(dependency.page.lod),
                dependency.species.0.as_slice(),
            ],
        )?;
    }
    for dependency in &build.terrain_surface_dependencies {
        transaction.execute(
            "INSERT INTO page_terrain_surfaces( \
                world_space_id, cell_x, cell_z, domain, lod, surface_id \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                dependency.page.space.0,
                dependency.page.cell.x,
                dependency.page.cell.z,
                dependency.page.domain as i64,
                i64::from(dependency.page.lod),
                dependency.surface.0.as_slice(),
            ],
        )?;
    }
    Ok(())
}

pub struct RuntimeReader {
    connection: Connection,
    manifest: RuntimeManifest,
}

impl RuntimeReader {
    pub fn open_immutable(path: &Path) -> Result<Self, WorldDbError> {
        let uri = immutable_uri(path);
        let connection = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;
        connection.execute_batch("PRAGMA query_only = ON; PRAGMA foreign_keys = ON;")?;
        ensure_schema_version(&connection, RUNTIME_SCHEMA_VERSION, "runtime")?;
        let manifest = read_runtime_manifest(&connection)?;
        if manifest.schema_version != RUNTIME_SCHEMA_VERSION {
            return Err(WorldDbError::SchemaVersion {
                database: "runtime",
                expected: RUNTIME_SCHEMA_VERSION,
                actual: manifest.schema_version,
            });
        }
        Ok(Self {
            connection,
            manifest,
        })
    }

    pub fn manifest(&self) -> &RuntimeManifest {
        &self.manifest
    }

    pub fn read_cell_descriptors(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
    ) -> Result<Vec<CellDescriptor>, WorldDbError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT cell_x, cell_z, minimum_y, maximum_y, domain_mask \
             FROM cells \
             WHERE world_space_id = ?1 \
               AND cell_x BETWEEN ?2 AND ?3 \
               AND cell_z BETWEEN ?4 AND ?5 \
             ORDER BY cell_x, cell_z",
        )?;
        statement
            .query_map(
                params![space.0, minimum.x, maximum.x, minimum.z, maximum.z],
                |row| {
                    let domain_mask: i64 = row.get(4)?;
                    Ok(CellDescriptor {
                        cell: CellCoord {
                            x: row.get(0)?,
                            z: row.get(1)?,
                        },
                        minimum_y: row.get(2)?,
                        maximum_y: row.get(3)?,
                        domain_mask: domain_mask as u64,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_page(&self, key: PageKey) -> Result<Option<EncodedPage>, WorldDbError> {
        self.connection
            .query_row(
                "SELECT codec, decoded_bytes, gpu_bytes_estimate, checksum, payload \
                 FROM cell_pages \
                 WHERE world_space_id = ?1 AND cell_x = ?2 AND cell_z = ?3 \
                   AND domain = ?4 AND lod = ?5",
                params![
                    key.space.0,
                    key.cell.x,
                    key.cell.z,
                    key.domain as i64,
                    i64::from(key.lod)
                ],
                |row| {
                    let decoded_bytes: i64 = row.get(1)?;
                    let gpu_bytes_estimate: i64 = row.get(2)?;
                    Ok(EncodedPage {
                        key,
                        codec: PageCodec::try_from(row.get::<_, i64>(0)?).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })?,
                        decoded_bytes: decoded_bytes as u64,
                        gpu_bytes_estimate: gpu_bytes_estimate as u64,
                        checksum: blob_array(row.get_ref(3)?.as_blob()?, "checksum")?,
                        payload: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn read_dependencies(&self, key: PageKey) -> Result<Vec<PageDependency>, WorldDbError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT d.asset_id, d.asset_lod, a.kind, a.uri, \
                    a.bounds_x, a.bounds_y, a.bounds_z, a.gpu_bytes_estimate, a.shadow_policy, \
                    a.minimum_screen_height \
             FROM page_dependencies d \
             JOIN asset_variants a ON a.asset_id = d.asset_id AND a.lod = d.asset_lod \
             WHERE d.world_space_id = ?1 AND d.cell_x = ?2 AND d.cell_z = ?3 \
               AND d.domain = ?4 AND d.lod = ?5 \
             ORDER BY d.asset_id, d.asset_lod",
        )?;
        statement
            .query_map(
                params![
                    key.space.0,
                    key.cell.x,
                    key.cell.z,
                    key.domain as i64,
                    i64::from(key.lod)
                ],
                |row| {
                    let gpu_bytes_estimate: i64 = row.get(7)?;
                    Ok(PageDependency {
                        asset: AssetId(blob_array(row.get_ref(0)?.as_blob()?, "asset_id")?),
                        asset_lod: row.get::<_, i64>(1)? as u8,
                        kind: row.get(2)?,
                        uri: row.get(3)?,
                        bounds: [row.get(4)?, row.get(5)?, row.get(6)?],
                        gpu_bytes_estimate: gpu_bytes_estimate as u64,
                        shadow_policy: row.get(8)?,
                        minimum_screen_height: row.get(9)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_object_definitions(
        &self,
        key: PageKey,
    ) -> Result<Vec<RuntimeObjectDefinition>, WorldDbError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT d.definition_id, d.definition_key, d.display_name, \
                    d.visual_asset_id, d.activation_policy \
             FROM page_object_definitions p \
             JOIN object_definitions d ON d.definition_id = p.definition_id \
             WHERE p.world_space_id = ?1 AND p.cell_x = ?2 AND p.cell_z = ?3 \
               AND p.domain = ?4 AND p.lod = ?5 \
             ORDER BY d.definition_id",
        )?;
        statement
            .query_map(
                params![
                    key.space.0,
                    key.cell.x,
                    key.cell.z,
                    key.domain as i64,
                    i64::from(key.lod),
                ],
                |row| {
                    Ok(RuntimeObjectDefinition {
                        id: ObjectDefinitionId(blob_array(
                            row.get_ref(0)?.as_blob()?,
                            "definition_id",
                        )?),
                        key: row.get(1)?,
                        display_name: row.get(2)?,
                        visual_asset: row
                            .get::<_, Option<Vec<u8>>>(3)?
                            .map(|bytes| blob_array(&bytes, "visual_asset_id").map(AssetId))
                            .transpose()?,
                        activation: ObjectActivationPolicy::try_from(row.get::<_, i64>(4)?)
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Integer,
                                    Box::new(error),
                                )
                            })?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_ground_cover_species(
        &self,
        key: PageKey,
    ) -> Result<Vec<GroundCoverSpecies>, WorldDbError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT s.species_id, s.species_key, \
                    s.bottom_color_r, s.bottom_color_g, s.bottom_color_b, \
                    s.top_color_r, s.top_color_g, s.top_color_b, \
                    s.minimum_card_height, s.maximum_card_height, \
                    s.minimum_card_width, s.maximum_card_width, \
                    s.flattened_card_probability, s.maximum_wind_displacement, \
                    s.artwork_resolution, s.artwork_variant_count, \
                    s.artwork_mip_level_count, s.artwork_coverage_mips \
             FROM page_ground_cover_species p \
             JOIN ground_cover_species s ON s.species_id = p.species_id \
             WHERE p.world_space_id = ?1 AND p.cell_x = ?2 AND p.cell_z = ?3 \
               AND p.domain = ?4 AND p.lod = ?5 \
             ORDER BY s.species_id",
        )?;
        statement
            .query_map(
                params![
                    key.space.0,
                    key.cell.x,
                    key.cell.z,
                    key.domain as i64,
                    i64::from(key.lod),
                ],
                ground_cover_species_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_terrain_resources(
        &self,
        key: PageKey,
    ) -> Result<TerrainRenderResources, WorldDbError> {
        let profile = self.connection.query_row(
            "SELECT world_space_id, texture_set_id, weight_resolution, macro_small_scale, \
                    macro_medium_scale, macro_large_scale, macro_contrast, macro_albedo_strength \
             FROM world_space_terrain_profiles WHERE world_space_id = ?1",
            [key.space.0],
            terrain_profile_from_row,
        )?;
        let texture_set = self.connection.query_row(
            "SELECT texture_set_id, texture_set_key, base_color_universal_uri, \
                    normal_material_universal_uri, macro_variation_universal_uri, \
                    base_color_astc_uri, normal_material_astc_uri, macro_variation_astc_uri, \
                    universal_gpu_bytes, astc_gpu_bytes \
             FROM terrain_texture_sets WHERE texture_set_id = ?1",
            [profile.texture_set.0.as_slice()],
            terrain_texture_set_from_row,
        )?;
        let mut statement = self.connection.prepare_cached(
            "SELECT s.surface_id, s.surface_key, s.display_name, s.tile_size, s.anti_tiling, \
                    s.normal_y_sign, s.normal_strength, s.roughness_min, s.roughness_max, l.layer \
             FROM page_terrain_surfaces p \
             JOIN terrain_surfaces s ON s.surface_id = p.surface_id \
             JOIN terrain_texture_set_layers l \
               ON l.surface_id = s.surface_id AND l.texture_set_id = ?6 \
             WHERE p.world_space_id = ?1 AND p.cell_x = ?2 AND p.cell_z = ?3 \
               AND p.domain = ?4 AND p.lod = ?5 \
             ORDER BY l.layer",
        )?;
        let surfaces = statement
            .query_map(
                params![
                    key.space.0,
                    key.cell.x,
                    key.cell.z,
                    key.domain as i64,
                    i64::from(key.lod),
                    profile.texture_set.0.as_slice(),
                ],
                |row| {
                    Ok(RuntimeTerrainSurface {
                        surface: terrain_surface_from_row(row)?,
                        layer: row.get::<_, i64>(9)? as u16,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TerrainRenderResources {
            profile,
            texture_set,
            surfaces,
        })
    }
}

fn query_all_source_ground_cover_visuals(
    connection: &Connection,
) -> Result<Vec<SourceGroundCoverVisualRecord>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT v.visual_id, v.visual_key, v.display_name, v.visual_family, v.source_revision, \
                c.built_in_atlas_version, c.recipe_variant_count, c.recipe_blade_count, \
                c.recipe_seed, c.recipe_minimum_blade_height, c.recipe_maximum_blade_height, \
                c.recipe_base_jitter, c.recipe_minimum_blade_half_width, \
                c.recipe_maximum_blade_half_width, c.recipe_maximum_lean, \
                c.recipe_maximum_curve, c.recipe_maximum_s_curve, \
                c.bottom_color_r, c.bottom_color_g, c.bottom_color_b, \
                c.top_color_r, c.top_color_g, c.top_color_b, c.minimum_card_height, \
                c.maximum_card_height, c.minimum_card_width, c.maximum_card_width, \
                c.flattened_card_probability, c.maximum_wind_displacement \
         FROM ground_cover_visuals v \
         JOIN ground_cover_card_visuals c ON c.visual_id = v.visual_id \
         ORDER BY v.visual_id",
    )?;
    Ok(statement
        .query_map([], source_ground_cover_visual_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

fn source_ground_cover_visual_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SourceGroundCoverVisualRecord> {
    let family =
        SourceGroundCoverVisualFamily::try_from(row.get::<_, i64>(3)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?;
    let definition = match family {
        SourceGroundCoverVisualFamily::CardCluster => {
            let procedural_recipe = if let Some(variant_count) = row.get::<_, Option<i64>>(6)? {
                Some(GroundCoverBladeRecipe {
                    variant_count: variant_count as u8,
                    blade_count: row.get::<_, i64>(7)? as u16,
                    seed: row.get::<_, i64>(8)? as u32,
                    minimum_blade_height: row.get(9)?,
                    maximum_blade_height: row.get(10)?,
                    base_jitter: row.get(11)?,
                    minimum_blade_half_width: row.get(12)?,
                    maximum_blade_half_width: row.get(13)?,
                    maximum_lean: row.get(14)?,
                    maximum_curve: row.get(15)?,
                    maximum_s_curve: row.get(16)?,
                })
            } else {
                None
            };
            SourceGroundCoverVisualDefinition::CardCluster(SourceGroundCoverCardVisualRecord {
                built_in_atlas_version: row.get::<_, i64>(5)? as u32,
                procedural_recipe,
                bottom_color: [row.get(17)?, row.get(18)?, row.get(19)?],
                top_color: [row.get(20)?, row.get(21)?, row.get(22)?],
                minimum_card_height: row.get(23)?,
                maximum_card_height: row.get(24)?,
                minimum_card_width: row.get(25)?,
                maximum_card_width: row.get(26)?,
                flattened_card_probability: row.get(27)?,
                maximum_wind_displacement: row.get(28)?,
            })
        }
    };
    Ok(SourceGroundCoverVisualRecord {
        id: GroundCoverVisualId(blob_array(row.get_ref(0)?.as_blob()?, "visual_id")?),
        key: row.get(1)?,
        display_name: row.get(2)?,
        source_revision: row.get(4)?,
        definition,
    })
}

fn query_all_source_ground_cover_presets(
    connection: &Connection,
) -> Result<Vec<SourceGroundCoverPresetRecord>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT preset_id, preset_key, display_name, enabled, visual_id, \
                density_per_square_meter, seed, source_revision \
         FROM ground_cover_presets ORDER BY preset_id",
    )?;
    Ok(statement
        .query_map([], source_ground_cover_preset_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

fn source_ground_cover_preset_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SourceGroundCoverPresetRecord> {
    Ok(SourceGroundCoverPresetRecord {
        id: GroundCoverPresetId(blob_array(row.get_ref(0)?.as_blob()?, "preset_id")?),
        key: row.get(1)?,
        display_name: row.get(2)?,
        enabled: row.get(3)?,
        visual: GroundCoverVisualId(blob_array(row.get_ref(4)?.as_blob()?, "visual_id")?),
        density_per_square_meter: row.get(5)?,
        seed: row.get::<_, i64>(6)? as u32,
        source_revision: row.get(7)?,
    })
}

fn ground_cover_species_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GroundCoverSpecies> {
    Ok(GroundCoverSpecies {
        id: GroundCoverSpeciesId(blob_array(row.get_ref(0)?.as_blob()?, "species_id")?),
        key: row.get(1)?,
        bottom_color: [row.get(2)?, row.get(3)?, row.get(4)?],
        top_color: [row.get(5)?, row.get(6)?, row.get(7)?],
        minimum_card_height: row.get(8)?,
        maximum_card_height: row.get(9)?,
        minimum_card_width: row.get(10)?,
        maximum_card_width: row.get(11)?,
        flattened_card_probability: row.get(12)?,
        maximum_wind_displacement: row.get(13)?,
        artwork: world::GroundCoverCardArtwork {
            resolution: row.get::<_, i64>(14)? as u16,
            variant_count: row.get::<_, i64>(15)? as u8,
            mip_level_count: row.get::<_, i64>(16)? as u8,
            coverage_mips: row.get(17)?,
        },
    })
}

fn query_all_terrain_surfaces(
    connection: &Connection,
) -> Result<Vec<TerrainSurface>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT surface_id, surface_key, display_name, tile_size, anti_tiling, normal_y_sign, \
                normal_strength, roughness_min, roughness_max \
         FROM terrain_surfaces ORDER BY surface_id",
    )?;
    Ok(statement
        .query_map([], terrain_surface_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

fn terrain_surface_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TerrainSurface> {
    Ok(TerrainSurface {
        id: TerrainSurfaceId(blob_array(row.get_ref(0)?.as_blob()?, "surface_id")?),
        key: row.get(1)?,
        display_name: row.get(2)?,
        tile_size: row.get(3)?,
        anti_tiling: row.get(4)?,
        normal_y_sign: row.get(5)?,
        normal_strength: row.get(6)?,
        roughness_min: row.get(7)?,
        roughness_max: row.get(8)?,
    })
}

fn query_all_terrain_texture_sets(
    connection: &Connection,
) -> Result<Vec<TerrainTextureSet>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT texture_set_id, texture_set_key, base_color_universal_uri, \
                normal_material_universal_uri, macro_variation_universal_uri, \
                base_color_astc_uri, normal_material_astc_uri, macro_variation_astc_uri, \
                universal_gpu_bytes, astc_gpu_bytes \
         FROM terrain_texture_sets ORDER BY texture_set_id",
    )?;
    Ok(statement
        .query_map([], terrain_texture_set_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

fn terrain_texture_set_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TerrainTextureSet> {
    Ok(TerrainTextureSet {
        id: TerrainTextureSetId(blob_array(row.get_ref(0)?.as_blob()?, "texture_set_id")?),
        key: row.get(1)?,
        base_color_universal_uri: row.get(2)?,
        normal_material_universal_uri: row.get(3)?,
        macro_variation_universal_uri: row.get(4)?,
        base_color_astc_uri: row.get(5)?,
        normal_material_astc_uri: row.get(6)?,
        macro_variation_astc_uri: row.get(7)?,
        universal_gpu_bytes: row.get::<_, i64>(8)? as u64,
        astc_gpu_bytes: row.get::<_, i64>(9)? as u64,
    })
}

fn query_all_terrain_texture_layers(
    connection: &Connection,
) -> Result<Vec<TerrainTextureLayer>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT texture_set_id, surface_id, layer \
         FROM terrain_texture_set_layers ORDER BY texture_set_id, layer",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok(TerrainTextureLayer {
                texture_set: TerrainTextureSetId(blob_array(
                    row.get_ref(0)?.as_blob()?,
                    "texture_set_id",
                )?),
                surface: TerrainSurfaceId(blob_array(row.get_ref(1)?.as_blob()?, "surface_id")?),
                layer: row.get::<_, i64>(2)? as u16,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

fn query_all_terrain_profiles(
    connection: &Connection,
) -> Result<Vec<TerrainProfile>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT world_space_id, texture_set_id, weight_resolution, macro_small_scale, \
                macro_medium_scale, macro_large_scale, macro_contrast, macro_albedo_strength \
         FROM world_space_terrain_profiles ORDER BY world_space_id",
    )?;
    Ok(statement
        .query_map([], terrain_profile_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

fn terrain_profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TerrainProfile> {
    Ok(TerrainProfile {
        space: WorldSpaceId(row.get(0)?),
        texture_set: TerrainTextureSetId(blob_array(row.get_ref(1)?.as_blob()?, "texture_set_id")?),
        weight_resolution: row.get::<_, i64>(2)? as u16,
        macro_scales: [row.get(3)?, row.get(4)?, row.get(5)?],
        macro_contrast: row.get(6)?,
        macro_albedo_strength: row.get(7)?,
    })
}

fn query_world_spaces(connection: &Connection) -> Result<Vec<WorldSpaceRecord>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT id, name, cell_size, minimum_y, maximum_y FROM world_spaces ORDER BY id",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok(WorldSpaceRecord {
                id: WorldSpaceId(row.get(0)?),
                name: row.get(1)?,
                cell_size: row.get(2)?,
                minimum_y: row.get(3)?,
                maximum_y: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

fn read_runtime_manifest(connection: &Connection) -> Result<RuntimeManifest, WorldDbError> {
    let world_spaces = query_world_spaces(connection)?;
    let manifest = connection.query_row(
        "SELECT schema_version, generation_id, content_hash, default_world_space_id \
         FROM runtime_metadata WHERE singleton = 1",
        [],
        |row| {
            Ok(RuntimeManifest {
                schema_version: row.get(0)?,
                generation_id: row.get(1)?,
                content_hash: blob_array(row.get_ref(2)?.as_blob()?, "content_hash")?,
                default_world_space: WorldSpaceId(row.get(3)?),
                world_spaces,
            })
        },
    )?;
    if manifest.world_space(manifest.default_world_space).is_none() {
        return Err(WorldDbError::UnknownDefaultWorldSpace(
            manifest.default_world_space,
        ));
    }
    Ok(manifest)
}

fn ensure_new_database_path(path: &Path) -> Result<(), WorldDbError> {
    if path.exists() {
        return Err(WorldDbError::AlreadyExists(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn ensure_schema_version(
    connection: &Connection,
    expected: i64,
    database: &'static str,
) -> Result<(), WorldDbError> {
    let actual: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if actual != expected {
        return Err(WorldDbError::SchemaVersion {
            database,
            expected,
            actual,
        });
    }
    Ok(())
}

fn immutable_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let escaped = raw
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('?', "%3F")
        .replace('#', "%23");
    format!("file:{escaped}?immutable=1")
}

fn blob_array<const N: usize>(bytes: &[u8], field: &'static str) -> rusqlite::Result<[u8; N]> {
    bytes.try_into().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Blob,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{field} must contain exactly {N} bytes"),
            )
            .into(),
        )
    })
}

#[derive(Debug, thiserror::Error)]
#[error("unknown ground-cover visual family {0}")]
pub struct UnknownGroundCoverVisualFamily(pub i64);

#[derive(Debug, thiserror::Error)]
pub enum WorldDbError {
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("page payload is invalid: {0}")]
    Payload(#[from] world::PagePayloadDecodeError),
    #[error("database already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("project migration left {0} foreign-key violations")]
    MigrationForeignKeyViolations(i64),
    #[error("{database} schema version is {actual}, expected {expected}")]
    SchemaVersion {
        database: &'static str,
        expected: i64,
        actual: i64,
    },
    #[error("page {0:?} failed its BLAKE3 checksum")]
    ChecksumMismatch(PageKey),
    #[error("decoded page size is {actual} bytes, maximum is {maximum}")]
    PageTooLarge { actual: u64, maximum: u64 },
    #[error("decoded page size is {actual} bytes, expected {expected}")]
    DecodedSizeMismatch { expected: u64, actual: u64 },
    #[error("page domain is {actual:?}, expected {expected:?}")]
    DomainMismatch {
        expected: PageDomain,
        actual: PageDomain,
    },
    #[error("integer does not fit the SQLite representation")]
    IntegerOverflow,
    #[error("default world space {0:?} is not present in the world-space catalog")]
    UnknownDefaultWorldSpace(WorldSpaceId),
    #[error("invalid spatial query bounds: minimum {minimum:?}, maximum {maximum:?}")]
    InvalidSpatialQueryBounds {
        minimum: CellCoord,
        maximum: CellCoord,
    },
    #[error("spatial query record limit must be greater than zero")]
    InvalidQueryLimit,
    #[error("object transform values must be finite and scale must be greater than zero")]
    InvalidObjectTransform,
    #[error("object transaction must contain 1 to 256 unique object writes")]
    InvalidObjectTransaction,
    #[error("dense source transaction must contain 1 to 64 unique terrain/mask writes")]
    InvalidDenseSourceTransaction,
    #[error("dense source record has an invalid page, resolution, payload length, or revision")]
    InvalidDenseSourceRecord,
    #[error("ground-cover region transaction must contain 1 to 64 unique region writes")]
    InvalidGroundCoverRegionTransaction,
    #[error("ground-cover region record has an invalid name, density, or revision")]
    InvalidGroundCoverRegionRecord,
    #[error("ground-cover catalog transaction must contain 1 to 64 unique visual/preset writes")]
    InvalidGroundCoverCatalogTransaction,
    #[error("ground-cover visual or preset record has invalid editable values or revision")]
    InvalidGroundCoverCatalogRecord,
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::{
        GameplayObjectInstance, GameplayObjectsPage, GroundCoverCluster, GroundCoverLayerId,
        GroundCoverPage, GroundCoverPresetId, GroundCoverRegionId, GroundCoverSpecies,
        GroundCoverSpeciesId, GroundCoverVisualId, TerrainRenderPage, encode_page_payload,
    };

    #[test]
    fn project_schema_seven_migrates_ground_cover_without_touching_other_data() {
        let directory = unique_test_directory();
        fs::create_dir_all(&directory).unwrap();
        let project_path = directory.join("project-v7.sqlite");
        let connection = Connection::open(&project_path).unwrap();
        connection
            .execute_batch(
                r#"
                PRAGMA foreign_keys = ON;
                CREATE TABLE world_spaces (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    cell_size REAL NOT NULL,
                    minimum_y REAL NOT NULL,
                    maximum_y REAL NOT NULL
                ) STRICT;
                CREATE TABLE source_cells (
                    world_space_id INTEGER NOT NULL REFERENCES world_spaces(id),
                    cell_x INTEGER NOT NULL,
                    cell_z INTEGER NOT NULL,
                    height REAL NOT NULL,
                    source_revision INTEGER NOT NULL,
                    PRIMARY KEY(world_space_id, cell_x, cell_z)
                ) STRICT, WITHOUT ROWID;
                CREATE TABLE ground_cover_species (
                    species_id BLOB PRIMARY KEY,
                    species_key TEXT NOT NULL UNIQUE,
                    bottom_color_r REAL NOT NULL,
                    bottom_color_g REAL NOT NULL,
                    bottom_color_b REAL NOT NULL,
                    top_color_r REAL NOT NULL,
                    top_color_g REAL NOT NULL,
                    top_color_b REAL NOT NULL,
                    minimum_card_height REAL NOT NULL,
                    maximum_card_height REAL NOT NULL,
                    minimum_card_width REAL NOT NULL,
                    maximum_card_width REAL NOT NULL,
                    flattened_card_probability REAL NOT NULL,
                    maximum_wind_displacement REAL NOT NULL
                ) STRICT;
                CREATE TABLE ground_cover_layers (
                    layer_id BLOB PRIMARY KEY,
                    world_space_id INTEGER NOT NULL REFERENCES world_spaces(id),
                    layer_key TEXT NOT NULL,
                    species_id BLOB NOT NULL REFERENCES ground_cover_species(species_id),
                    density_per_square_meter REAL NOT NULL,
                    seed INTEGER NOT NULL,
                    UNIQUE(world_space_id, layer_key),
                    UNIQUE(layer_id, world_space_id)
                ) STRICT;
                CREATE TABLE ground_cover_cell_masks (
                    layer_id BLOB NOT NULL,
                    world_space_id INTEGER NOT NULL,
                    cell_x INTEGER NOT NULL,
                    cell_z INTEGER NOT NULL,
                    resolution INTEGER NOT NULL,
                    coverage BLOB NOT NULL,
                    source_revision INTEGER NOT NULL,
                    PRIMARY KEY(layer_id, world_space_id, cell_x, cell_z),
                    FOREIGN KEY(layer_id, world_space_id)
                        REFERENCES ground_cover_layers(layer_id, world_space_id),
                    FOREIGN KEY(world_space_id, cell_x, cell_z)
                        REFERENCES source_cells(world_space_id, cell_x, cell_z)
                ) STRICT, WITHOUT ROWID;
                CREATE TABLE preserved_editor_data(value TEXT NOT NULL) STRICT;
                PRAGMA user_version = 7;
                "#,
            )
            .unwrap();
        let species = [11_u8; 16];
        let layer = [12_u8; 16];
        connection
            .execute(
                "INSERT INTO world_spaces VALUES (1, 'test', 32.0, 0.0, 0.0)",
                [],
            )
            .unwrap();
        connection
            .execute("INSERT INTO source_cells VALUES (1, 0, 0, 0.0, 4)", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO ground_cover_species VALUES ( \
                    ?1, 'meadow', 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, \
                    0.55, 0.78, 0.7, 1.4, 0.2, 0.22 \
                 )",
                [species.as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ground_cover_layers VALUES (?1, 1, 'meadow-layer', ?2, 5.0, 91)",
                params![layer.as_slice(), species.as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ground_cover_cell_masks VALUES (?1, 1, 0, 0, 2, ?2, 7)",
                params![layer.as_slice(), vec![255_u8, 128, 0, 64]],
            )
            .unwrap();
        connection
            .execute("INSERT INTO preserved_editor_data VALUES ('keep me')", [])
            .unwrap();
        drop(connection);

        assert!(migrate_project_database(&project_path).unwrap());
        assert!(!migrate_project_database(&project_path).unwrap());

        let connection = Connection::open(&project_path).unwrap();
        let schema_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(schema_version, PROJECT_SCHEMA_VERSION);
        let preserved: String = connection
            .query_row("SELECT value FROM preserved_editor_data", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(preserved, "keep me");
        let visual_id: Vec<u8> = connection
            .query_row("SELECT visual_id FROM ground_cover_visuals", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(visual_id, species);
        let (preset_id, density, seed): (Vec<u8>, f32, i64) = connection
            .query_row(
                "SELECT preset_id, density_per_square_meter, seed FROM ground_cover_presets",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(preset_id, layer);
        assert_eq!(density, 5.0);
        assert_eq!(seed, 91);
        let (region_id, coverage): (Vec<u8>, Vec<u8>) = connection
            .query_row(
                "SELECT region_id, coverage FROM ground_cover_region_cell_masks",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(region_id, layer);
        assert_eq!(coverage, vec![255, 128, 0, 64]);
        let violations: i64 = connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(violations, 0);
    }

    #[test]
    fn project_and_runtime_databases_are_distinct_and_readable() {
        let directory = unique_test_directory();
        fs::create_dir_all(&directory).unwrap();
        let project_path = directory.join("project.sqlite");
        let runtime_path = directory.join("runtime.sqlite");
        let space = WorldSpaceRecord {
            id: WorldSpaceId(1),
            name: "test".into(),
            cell_size: 32.0,
            minimum_y: 0.0,
            maximum_y: 0.0,
        };
        let second_space = WorldSpaceRecord {
            id: WorldSpaceId(2),
            name: "second-test-space".into(),
            cell_size: 16.0,
            minimum_y: -4.0,
            maximum_y: 8.0,
        };
        let definition_id = ObjectDefinitionId([3; 16]);
        let object_id = StableObjectId([5; 16]);
        let second_object_id = StableObjectId([6; 16]);
        let asset_id = AssetId([9; 32]);
        let ground_cover_species_id = GroundCoverSpeciesId([11; 16]);
        let ground_cover_visual_id = GroundCoverVisualId(ground_cover_species_id.0);
        let ground_cover_preset_id = GroundCoverPresetId([15; 16]);
        let ground_cover_layer_id = GroundCoverLayerId([12; 16]);
        let ground_cover_region_id = GroundCoverRegionId([16; 16]);
        let terrain_surface_id = TerrainSurfaceId([13; 16]);
        let terrain_texture_set_id = TerrainTextureSetId([14; 16]);
        let terrain_surface = TerrainSurface {
            id: terrain_surface_id,
            key: "test-grass".into(),
            display_name: "Test grass".into(),
            tile_size: 2.0,
            anti_tiling: true,
            normal_y_sign: 1.0,
            normal_strength: 0.5,
            roughness_min: 0.8,
            roughness_max: 1.0,
        };
        let terrain_texture_set = TerrainTextureSet {
            id: terrain_texture_set_id,
            key: "test-terrain".into(),
            base_color_universal_uri: "base-universal.ktx2".into(),
            normal_material_universal_uri: "normal-universal.ktx2".into(),
            macro_variation_universal_uri: "macro-universal.ktx2".into(),
            base_color_astc_uri: "base-astc.ktx2".into(),
            normal_material_astc_uri: "normal-astc.ktx2".into(),
            macro_variation_astc_uri: "macro-astc.ktx2".into(),
            universal_gpu_bytes: 1200,
            astc_gpu_bytes: 600,
        };
        let ground_cover_species = GroundCoverSpecies {
            id: ground_cover_species_id,
            key: "test/meadow-grass".into(),
            bottom_color: [0.04, 0.11, 0.03],
            top_color: [0.18, 0.32, 0.10],
            minimum_card_height: 0.55,
            maximum_card_height: 0.78,
            minimum_card_width: 0.7,
            maximum_card_width: 1.4,
            flattened_card_probability: 0.2,
            maximum_wind_displacement: 0.2,
            artwork: generate_ground_cover_card_artwork(GroundCoverBladeRecipe::built_in_v1()),
        };
        let ground_cover_visual = SourceGroundCoverVisualRecord {
            id: ground_cover_visual_id,
            key: ground_cover_species.key.clone(),
            display_name: "Test meadow grass".into(),
            source_revision: 1,
            definition: SourceGroundCoverVisualDefinition::CardCluster(
                SourceGroundCoverCardVisualRecord {
                    built_in_atlas_version: 1,
                    procedural_recipe: None,
                    bottom_color: ground_cover_species.bottom_color,
                    top_color: ground_cover_species.top_color,
                    minimum_card_height: ground_cover_species.minimum_card_height,
                    maximum_card_height: ground_cover_species.maximum_card_height,
                    minimum_card_width: ground_cover_species.minimum_card_width,
                    maximum_card_width: ground_cover_species.maximum_card_width,
                    flattened_card_probability: ground_cover_species.flattened_card_probability,
                    maximum_wind_displacement: ground_cover_species.maximum_wind_displacement,
                },
            ),
        };
        write_project_database(
            &project_path,
            &ProjectDocument {
                default_world_space: space.id,
                world_spaces: vec![space.clone(), second_space.clone()],
                cells: vec![
                    SourceCellRecord {
                        space: space.id,
                        cell: CellCoord::ZERO,
                        height: 0.0,
                        source_revision: 1,
                    },
                    SourceCellRecord {
                        space: second_space.id,
                        cell: CellCoord::ZERO,
                        height: 2.0,
                        source_revision: 1,
                    },
                ],
                terrain_surfaces: vec![terrain_surface.clone()],
                terrain_texture_sets: vec![terrain_texture_set.clone()],
                terrain_texture_layers: vec![TerrainTextureLayer {
                    texture_set: terrain_texture_set_id,
                    surface: terrain_surface_id,
                    layer: 0,
                }],
                terrain_profiles: vec![
                    TerrainProfile {
                        space: space.id,
                        texture_set: terrain_texture_set_id,
                        weight_resolution: 2,
                        macro_scales: [8.0, 32.0, 128.0],
                        macro_contrast: 1.25,
                        macro_albedo_strength: 0.12,
                    },
                    TerrainProfile {
                        space: second_space.id,
                        texture_set: terrain_texture_set_id,
                        weight_resolution: 2,
                        macro_scales: [8.0, 32.0, 128.0],
                        macro_contrast: 1.25,
                        macro_albedo_strength: 0.12,
                    },
                ],
                terrain_cell_surface_slots: vec![
                    SourceTerrainCellSurfaceSlotRecord {
                        space: space.id,
                        cell: CellCoord::ZERO,
                        slot: 0,
                        surface: terrain_surface_id,
                    },
                    SourceTerrainCellSurfaceSlotRecord {
                        space: second_space.id,
                        cell: CellCoord::ZERO,
                        slot: 0,
                        surface: terrain_surface_id,
                    },
                ],
                terrain_cell_weight_pages: Vec::new(),
                ground_cover_visuals: vec![ground_cover_visual.clone()],
                ground_cover_presets: vec![SourceGroundCoverPresetRecord {
                    id: ground_cover_preset_id,
                    key: "test/meadow-preset".into(),
                    display_name: "Test meadow".into(),
                    enabled: true,
                    visual: ground_cover_visual_id,
                    density_per_square_meter: 7.0,
                    seed: 91,
                    source_revision: 1,
                }],
                ground_cover_layers: vec![SourceGroundCoverLayerRecord {
                    id: ground_cover_layer_id,
                    space: space.id,
                    key: "test/meadow".into(),
                    display_name: "Test meadow layer".into(),
                    enabled: true,
                    sort_order: 0,
                    source_revision: 1,
                }],
                ground_cover_regions: vec![SourceGroundCoverRegionRecord {
                    id: ground_cover_region_id,
                    layer: ground_cover_layer_id,
                    space: space.id,
                    preset: ground_cover_preset_id,
                    display_name: "Test region".into(),
                    enabled: true,
                    density_multiplier: 1.0,
                    source_revision: 1,
                }],
                ground_cover_masks: vec![SourceGroundCoverCellMaskRecord {
                    region: ground_cover_region_id,
                    space: space.id,
                    cell: CellCoord::ZERO,
                    resolution: 2,
                    coverage: vec![255, 128, 0, 255],
                    source_revision: 2,
                }],
                assets: vec![SourceAssetRecord {
                    id: asset_id,
                    key: "test/tree".into(),
                    kind: "gltf-scene".into(),
                    source_uri: "local/source/tree.fbx".into(),
                }],
                asset_variants: vec![SourceAssetVariantRecord {
                    asset: asset_id,
                    lod: 0,
                    uri: "local/runtime/tree_lod0.gltf".into(),
                    bounds: [2.0, 8.0, 2.0],
                    gpu_bytes_estimate: 4096,
                    shadow_policy: 1,
                    minimum_screen_height: 0.0,
                }],
                definitions: vec![SourceObjectDefinitionRecord {
                    id: definition_id,
                    key: "test-door".into(),
                    display_name: "Test door".into(),
                    visual_asset: Some(asset_id),
                    activation: ObjectActivationPolicy::Proximity,
                }],
                objects: vec![
                    SourceObjectRecord {
                        id: object_id,
                        space: space.id,
                        owner_cell: CellCoord::ZERO,
                        definition: definition_id,
                        local_translation: [1.0, 0.0, 2.0],
                        yaw: 0.0,
                        scale: 1.0,
                        source_revision: 1,
                    },
                    SourceObjectRecord {
                        id: second_object_id,
                        space: second_space.id,
                        owner_cell: CellCoord::ZERO,
                        definition: definition_id,
                        local_translation: [2.0, 0.0, 1.0],
                        yaw: 0.0,
                        scale: 1.0,
                        source_revision: 1,
                    },
                ],
            },
        )
        .unwrap();
        let project = read_project_database(&project_path).unwrap();
        assert_eq!(project.default_world_space, space.id);
        assert_eq!(project.world_spaces.len(), 2);
        assert_eq!(project.cells.len(), 2);
        assert_eq!(project.terrain_surfaces, vec![terrain_surface.clone()]);
        assert_eq!(
            project.terrain_texture_sets,
            vec![terrain_texture_set.clone()]
        );
        assert_eq!(project.terrain_cell_surface_slots.len(), 2);
        assert_eq!(project.assets[0].key, "test/tree");
        assert_eq!(project.asset_variants[0].minimum_screen_height, 0.0);
        assert_eq!(project.definitions.len(), 1);
        assert_eq!(project.objects[0].definition, definition_id);
        assert_eq!(
            project.ground_cover_visuals,
            vec![ground_cover_visual.clone()]
        );
        assert_eq!(project.ground_cover_presets.len(), 1);
        assert_eq!(project.ground_cover_layers.len(), 1);
        assert_eq!(project.ground_cover_regions.len(), 1);
        assert_eq!(
            project.ground_cover_masks[0].coverage,
            vec![255, 128, 0, 255]
        );

        let connection = Connection::open(&project_path).unwrap();
        connection
            .execute(
                "INSERT INTO object_cell_overlaps( \
                    object_id, world_space_id, cell_x, cell_z \
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![object_id.0.as_slice(), space.id.0, 4, -3],
            )
            .unwrap();
        drop(connection);

        let project_reader = ProjectReader::open_read_only(&project_path).unwrap();
        assert!(project_reader.has_spatial_object_overlap_index);
        assert_eq!(project_reader.manifest().default_world_space, space.id);
        assert_eq!(project_reader.manifest().world_spaces.len(), 2);
        assert_eq!(
            project_reader.read_ground_cover_visuals(8).unwrap().len(),
            1
        );
        assert_eq!(
            project_reader.read_ground_cover_presets(8).unwrap().len(),
            1
        );
        assert_eq!(
            project_reader
                .read_ground_cover_layers(space.id, 8)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            project_reader
                .read_ground_cover_regions(space.id, 8)
                .unwrap()
                .len(),
            1
        );
        let cells = project_reader
            .read_cells(space.id, CellCoord::ZERO, CellCoord::ZERO, 4)
            .unwrap();
        assert_eq!(cells.records.len(), 1);
        assert_eq!(cells.records[0].source_revision, 1);
        assert!(!cells.truncated);
        assert!(
            project_reader
                .read_cells(
                    space.id,
                    CellCoord { x: 10, z: 10 },
                    CellCoord { x: 11, z: 11 },
                    4,
                )
                .unwrap()
                .records
                .is_empty()
        );
        let owner_objects = project_reader
            .read_objects_in_cells(space.id, CellCoord::ZERO, CellCoord::ZERO, 4)
            .unwrap();
        assert_eq!(owner_objects.records.len(), 1);
        let overlap_objects = project_reader
            .read_objects_in_cells(
                space.id,
                CellCoord { x: 4, z: -3 },
                CellCoord { x: 4, z: -3 },
                4,
            )
            .unwrap();
        assert_eq!(overlap_objects.records[0].id, object_id);
        assert_eq!(
            project_reader.read_object(object_id).unwrap().unwrap().id,
            object_id
        );
        let object_views = project_reader
            .read_object_views_in_cells(space.id, CellCoord::ZERO, CellCoord::ZERO, 4)
            .unwrap();
        assert_eq!(object_views.records.len(), 1);
        assert_eq!(object_views.records[0].object.id, object_id);
        assert_eq!(object_views.records[0].definition.display_name, "Test door");
        assert_eq!(
            object_views.records[0].visual_uri.as_deref(),
            Some("local/runtime/tree_lod0.gltf")
        );
        assert_eq!(object_views.records[0].visual_bounds, Some([2.0, 8.0, 2.0]));
        assert_eq!(
            project_reader
                .read_object_view(object_id)
                .unwrap()
                .unwrap()
                .definition
                .key,
            "test-door"
        );
        let palette = project_reader.read_object_palette(8).unwrap();
        assert_eq!(palette.records.len(), 1);
        assert!(!palette.truncated);
        assert_eq!(palette.records[0].definition.id, definition_id);
        assert_eq!(
            palette.records[0].visual_uri.as_deref(),
            Some("local/runtime/tree_lod0.gltf")
        );
        let outliner_page = project_reader
            .read_object_outliner_page(space.id, "door", None, 1)
            .unwrap();
        assert_eq!(outliner_page.records[0].object.id, object_id);
        assert!(outliner_page.next_cursor.is_none());
        assert!(
            project_reader
                .read_object_outliner_page(space.id, "missing", None, 1)
                .unwrap()
                .records
                .is_empty()
        );
        let palette_page = project_reader
            .read_object_palette_page("door", None, 1)
            .unwrap();
        assert_eq!(palette_page.records[0].definition.id, definition_id);
        assert!(palette_page.next_cursor.is_none());
        assert!(matches!(
            project_reader.read_cells(space.id, CellCoord::ZERO, CellCoord::ZERO, 0),
            Err(WorldDbError::InvalidQueryLimit)
        ));
        assert!(matches!(
            project_reader.read_object_palette(0),
            Err(WorldDbError::InvalidQueryLimit)
        ));
        assert!(matches!(
            project_reader.read_object_palette_page("", None, 0),
            Err(WorldDbError::InvalidQueryLimit)
        ));
        let masks = project_reader
            .read_ground_cover_masks_in_cells(space.id, CellCoord::ZERO, CellCoord::ZERO, 4)
            .unwrap();
        assert_eq!(masks.records.len(), 1);
        assert_eq!(masks.records[0].source_revision, 2);
        assert!(!masks.truncated);
        assert_eq!(
            project_reader
                .read_ground_cover_layers(space.id, 4)
                .unwrap()[0]
                .id,
            ground_cover_layer_id
        );
        assert!(
            project_reader
                .read_terrain_weight_pages_in_cells(space.id, CellCoord::ZERO, CellCoord::ZERO, 4,)
                .unwrap()
                .records
                .is_empty()
        );

        let mut project_writer = ProjectWriter::open(&project_path).unwrap();
        let transform = SourceObjectTransform {
            space: space.id,
            owner_cell: CellCoord::ZERO,
            local_translation: [3.0, 0.5, 4.0],
            yaw: 0.25,
            scale: 1.5,
        };
        let ObjectTransformWriteResult::Updated(updated) = project_writer
            .update_object_transform(object_id, 1, transform)
            .unwrap()
        else {
            panic!("matching source revision should update the object");
        };
        assert_eq!(updated.source_revision, 2);
        assert_eq!(SourceObjectTransform::from(&updated), transform);
        let ObjectTransformWriteResult::Conflict {
            actual: Some(actual),
        } = project_writer
            .update_object_transform(object_id, 1, transform)
            .unwrap()
        else {
            panic!("stale source revision should report a conflict");
        };
        assert_eq!(actual.source_revision, 2);
        let overlap_count: i64 = Connection::open(&project_path)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM object_cell_overlaps WHERE object_id = ?1",
                params![object_id.0.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(overlap_count, 0);

        let mut rolled_back_transform = transform;
        rolled_back_transform.local_translation[0] = 12.0;
        let ObjectWriteTransactionResult::Conflict {
            object: conflict_object,
            actual: Some(actual),
        } = project_writer
            .apply_object_transaction(&[
                SourceObjectWrite::UpdateTransform {
                    object: object_id,
                    expected_source_revision: 2,
                    transform: rolled_back_transform,
                },
                SourceObjectWrite::Delete {
                    object: second_object_id,
                    expected_source_revision: 99,
                },
            ])
            .unwrap()
        else {
            panic!("a stale write should roll back the complete object transaction");
        };
        assert_eq!(conflict_object, second_object_id);
        assert_eq!(actual.source_revision, 1);
        let unchanged = project_reader.read_object(object_id).unwrap().unwrap();
        assert_eq!(unchanged.source_revision, 2);
        assert_eq!(SourceObjectTransform::from(&unchanged), transform);

        assert_eq!(
            project_writer
                .apply_object_transaction(&[SourceObjectWrite::Delete {
                    object: second_object_id,
                    expected_source_revision: 1,
                }])
                .unwrap(),
            ObjectWriteTransactionResult::Committed(vec![SourceObjectWriteCommit::Deleted(
                second_object_id
            )])
        );
        assert!(
            project_reader
                .read_object(second_object_id)
                .unwrap()
                .is_none()
        );
        let restored_object = SourceObjectRecord {
            id: second_object_id,
            space: second_space.id,
            owner_cell: CellCoord::ZERO,
            definition: definition_id,
            local_translation: [2.0, 0.0, 1.0],
            yaw: 0.0,
            scale: 1.0,
            source_revision: 1,
        };
        let ObjectWriteTransactionResult::Committed(restored) = project_writer
            .apply_object_transaction(&[SourceObjectWrite::Create {
                object: restored_object,
            }])
            .unwrap()
        else {
            panic!("undoing a saved deletion should recreate the source placement");
        };
        let [SourceObjectWriteCommit::Updated(restored)] = restored.as_slice() else {
            panic!("placement recreation should return its new source checkpoint");
        };
        assert_eq!(restored.id, second_object_id);
        assert_eq!(restored.source_revision, 2);

        let terrain_weights = SourceTerrainCellWeightPageRecord {
            space: space.id,
            cell: CellCoord::ZERO,
            page: 0,
            resolution: 2,
            rgba: vec![255; 16],
            source_revision: 0,
        };
        let ground_cover_mask = SourceGroundCoverCellMaskRecord {
            region: ground_cover_region_id,
            space: space.id,
            cell: CellCoord::ZERO,
            resolution: 2,
            coverage: vec![64, 96, 128, 255],
            source_revision: 2,
        };
        let DenseSourceWriteTransactionResult::Committed(dense_commits) = project_writer
            .apply_dense_source_transaction(&[
                DenseSourceWrite::TerrainWeights {
                    expected_source_revision: None,
                    record: terrain_weights,
                },
                DenseSourceWrite::GroundCoverMask {
                    expected_source_revision: Some(2),
                    record: ground_cover_mask,
                },
            ])
            .unwrap()
        else {
            panic!("matching dense revisions should commit atomically");
        };
        assert_eq!(dense_commits.len(), 2);
        assert_eq!(
            project_reader
                .read_terrain_weight_pages_in_cells(space.id, CellCoord::ZERO, CellCoord::ZERO, 4,)
                .unwrap()
                .records[0]
                .source_revision,
            1
        );
        assert_eq!(
            project_reader
                .read_ground_cover_masks_in_cells(space.id, CellCoord::ZERO, CellCoord::ZERO, 4,)
                .unwrap()
                .records[0]
                .source_revision,
            3
        );

        let DenseSourceWriteTransactionResult::Conflict { key, actual } = project_writer
            .apply_dense_source_transaction(&[
                DenseSourceWrite::TerrainWeights {
                    expected_source_revision: Some(1),
                    record: SourceTerrainCellWeightPageRecord {
                        space: space.id,
                        cell: CellCoord::ZERO,
                        page: 0,
                        resolution: 2,
                        rgba: vec![0; 16],
                        source_revision: 1,
                    },
                },
                DenseSourceWrite::GroundCoverMask {
                    expected_source_revision: Some(2),
                    record: SourceGroundCoverCellMaskRecord {
                        region: ground_cover_region_id,
                        space: space.id,
                        cell: CellCoord::ZERO,
                        resolution: 2,
                        coverage: vec![0; 4],
                        source_revision: 3,
                    },
                },
            ])
            .unwrap()
        else {
            panic!("a stale dense write should roll back the complete batch");
        };
        assert_eq!(
            key,
            DenseSourceRecordKey::GroundCoverMask {
                region: ground_cover_region_id,
                space: space.id,
                cell: CellCoord::ZERO,
            }
        );
        assert!(matches!(
            actual,
            Some(DenseSourceRecord::GroundCoverMask(record)) if record.source_revision == 3
        ));
        assert_eq!(
            project_reader
                .read_terrain_weight_pages_in_cells(space.id, CellCoord::ZERO, CellCoord::ZERO, 4,)
                .unwrap()
                .records[0]
                .rgba,
            vec![255; 16],
            "the earlier terrain update must roll back with the stale mask"
        );

        let mut edited_visual = ground_cover_visual.clone();
        edited_visual.display_name = "Edited meadow cards".into();
        let SourceGroundCoverVisualDefinition::CardCluster(edited_card) =
            &mut edited_visual.definition;
        edited_card.top_color = [0.25, 0.45, 0.12];
        let edited_preset = SourceGroundCoverPresetRecord {
            id: ground_cover_preset_id,
            key: "test/meadow-preset".into(),
            display_name: "Edited meadow".into(),
            enabled: true,
            visual: ground_cover_visual_id,
            density_per_square_meter: 8.5,
            seed: 123,
            source_revision: 1,
        };
        let GroundCoverCatalogWriteTransactionResult::Committed(catalog_commits) = project_writer
            .apply_ground_cover_catalog_transaction(&[
                GroundCoverCatalogWrite::UpdateVisual {
                    expected_source_revision: 1,
                    record: edited_visual.clone(),
                },
                GroundCoverCatalogWrite::UpdatePreset {
                    expected_source_revision: 1,
                    record: edited_preset,
                },
            ])
            .unwrap()
        else {
            panic!("matching visual and preset revisions should commit together");
        };
        assert_eq!(catalog_commits.len(), 2);
        assert!(
            catalog_commits
                .iter()
                .all(|commit| commit.source_revision() == Some(2))
        );
        assert_eq!(
            project_reader.read_ground_cover_presets(8).unwrap()[0].density_per_square_meter,
            8.5
        );
        assert_eq!(
            project_reader.read_ground_cover_visuals(8).unwrap()[0].display_name,
            "Edited meadow cards"
        );

        let mut rolled_back_visual = edited_visual;
        rolled_back_visual.source_revision = 2;
        rolled_back_visual.display_name = "Must roll back".into();
        let GroundCoverCatalogWriteTransactionResult::Conflict {
            key: GroundCoverCatalogKey::Preset(conflict_preset),
            actual: Some(GroundCoverCatalogRecord::Preset(actual_preset)),
        } = project_writer
            .apply_ground_cover_catalog_transaction(&[
                GroundCoverCatalogWrite::UpdateVisual {
                    expected_source_revision: 2,
                    record: rolled_back_visual,
                },
                GroundCoverCatalogWrite::UpdatePreset {
                    expected_source_revision: 1,
                    record: SourceGroundCoverPresetRecord {
                        source_revision: 1,
                        density_per_square_meter: 9.0,
                        ..project_reader.read_ground_cover_presets(8).unwrap()[0].clone()
                    },
                },
            ])
            .unwrap()
        else {
            panic!("a stale preset should roll back the earlier visual update");
        };
        assert_eq!(conflict_preset, ground_cover_preset_id);
        assert_eq!(actual_preset.source_revision, 2);
        assert_eq!(
            project_reader.read_ground_cover_visuals(8).unwrap()[0].display_name,
            "Edited meadow cards"
        );

        let GroundCoverRegionWriteTransactionResult::Committed(region_updates) = project_writer
            .apply_ground_cover_region_transaction(&[GroundCoverRegionWrite::Update {
                expected_source_revision: 1,
                record: SourceGroundCoverRegionRecord {
                    display_name: "Edited test region".into(),
                    density_multiplier: 0.8,
                    ..project
                        .ground_cover_regions
                        .iter()
                        .find(|region| region.id == ground_cover_region_id)
                        .unwrap()
                        .clone()
                },
            }])
            .unwrap()
        else {
            panic!("a matching region update should commit");
        };
        let [GroundCoverRegionWriteCommit::Updated(updated_region)] = region_updates.as_slice()
        else {
            panic!("region update should return its new checkpoint");
        };
        assert_eq!(updated_region.source_revision, 2);
        assert_eq!(updated_region.density_multiplier, 0.8);

        let empty_region_id = GroundCoverRegionId([21; 16]);
        let empty_region = SourceGroundCoverRegionRecord {
            id: empty_region_id,
            layer: ground_cover_layer_id,
            space: space.id,
            preset: ground_cover_preset_id,
            display_name: "Empty test region".into(),
            enabled: true,
            density_multiplier: 1.0,
            source_revision: 0,
        };
        let GroundCoverRegionWriteTransactionResult::Committed(created) = project_writer
            .apply_ground_cover_region_transaction(&[GroundCoverRegionWrite::Create {
                record: empty_region,
            }])
            .unwrap()
        else {
            panic!("an empty region should be created")
        };
        let [GroundCoverRegionWriteCommit::Created(created)] = created.as_slice() else {
            panic!("region creation should return a checkpoint")
        };
        assert_eq!(created.source_revision, 1);
        assert_eq!(
            project_writer
                .apply_ground_cover_region_transaction(&[GroundCoverRegionWrite::DeleteEmpty {
                    region: empty_region_id,
                    expected_source_revision: 1,
                },])
                .unwrap(),
            GroundCoverRegionWriteTransactionResult::Committed(vec![
                GroundCoverRegionWriteCommit::Deleted(empty_region_id),
            ])
        );
        assert_eq!(
            project_writer
                .apply_ground_cover_region_transaction(&[GroundCoverRegionWrite::DeleteEmpty {
                    region: ground_cover_region_id,
                    expected_source_revision: 2,
                },])
                .unwrap(),
            GroundCoverRegionWriteTransactionResult::BlockedByCoverage {
                region: ground_cover_region_id,
            }
        );

        let duplicate_visual_id = GroundCoverVisualId([31; 16]);
        let duplicate_preset_id = GroundCoverPresetId([32; 16]);
        let mut duplicate_visual = project_reader.read_ground_cover_visuals(8).unwrap()[0].clone();
        duplicate_visual.id = duplicate_visual_id;
        duplicate_visual.key = "test/duplicate-visual".into();
        duplicate_visual.display_name = "Duplicate visual".into();
        duplicate_visual.source_revision = 0;
        let mut custom_recipe = GroundCoverBladeRecipe::built_in_v1();
        custom_recipe.blade_count = 18;
        let SourceGroundCoverVisualDefinition::CardCluster(duplicate_card) =
            &mut duplicate_visual.definition;
        duplicate_card.procedural_recipe = Some(custom_recipe);
        let duplicate_preset = SourceGroundCoverPresetRecord {
            id: duplicate_preset_id,
            key: "test/duplicate-preset".into(),
            display_name: "Duplicate preset".into(),
            enabled: true,
            visual: duplicate_visual_id,
            density_per_square_meter: 3.0,
            seed: 99,
            source_revision: 0,
        };
        let GroundCoverCatalogWriteTransactionResult::Committed(created_catalog) = project_writer
            .apply_ground_cover_catalog_transaction(&[
                GroundCoverCatalogWrite::CreateVisual {
                    record: duplicate_visual,
                },
                GroundCoverCatalogWrite::CreatePreset {
                    record: duplicate_preset,
                },
            ])
            .unwrap()
        else {
            panic!("visual-before-preset creation should commit atomically");
        };
        assert_eq!(created_catalog.len(), 2);
        assert!(created_catalog.iter().all(|commit| matches!(
            commit,
            GroundCoverCatalogWriteCommit::Created(_)
        ) && commit.source_revision() == Some(1)));
        let persisted_duplicate = project_reader
            .read_ground_cover_visuals(8)
            .unwrap()
            .into_iter()
            .find(|visual| visual.id == duplicate_visual_id)
            .unwrap();
        let SourceGroundCoverVisualDefinition::CardCluster(persisted_card) =
            persisted_duplicate.definition;
        assert_eq!(persisted_card.procedural_recipe, Some(custom_recipe));
        assert_eq!(
            project_writer
                .apply_ground_cover_catalog_transaction(&[GroundCoverCatalogWrite::DeleteVisual {
                    visual: duplicate_visual_id,
                    expected_source_revision: 1,
                },])
                .unwrap(),
            GroundCoverCatalogWriteTransactionResult::BlockedByDependency {
                key: GroundCoverCatalogKey::Visual(duplicate_visual_id),
                dependency: GroundCoverCatalogDependency::VisualPresets,
            }
        );
        assert_eq!(
            project_writer
                .apply_ground_cover_catalog_transaction(&[
                    GroundCoverCatalogWrite::DeletePreset {
                        preset: duplicate_preset_id,
                        expected_source_revision: 1,
                    },
                    GroundCoverCatalogWrite::DeleteVisual {
                        visual: ground_cover_visual_id,
                        expected_source_revision: 2,
                    },
                ])
                .unwrap(),
            GroundCoverCatalogWriteTransactionResult::BlockedByDependency {
                key: GroundCoverCatalogKey::Visual(ground_cover_visual_id),
                dependency: GroundCoverCatalogDependency::VisualPresets,
            }
        );
        assert!(
            project_reader
                .read_ground_cover_presets(8)
                .unwrap()
                .iter()
                .any(|preset| preset.id == duplicate_preset_id),
            "a blocked later deletion must roll back the earlier preset deletion"
        );
        assert_eq!(
            project_writer
                .apply_ground_cover_catalog_transaction(&[
                    GroundCoverCatalogWrite::DeletePreset {
                        preset: duplicate_preset_id,
                        expected_source_revision: 1,
                    },
                    GroundCoverCatalogWrite::DeleteVisual {
                        visual: duplicate_visual_id,
                        expected_source_revision: 1,
                    },
                ])
                .unwrap(),
            GroundCoverCatalogWriteTransactionResult::Committed(vec![
                GroundCoverCatalogWriteCommit::Deleted(GroundCoverCatalogKey::Preset(
                    duplicate_preset_id,
                )),
                GroundCoverCatalogWriteCommit::Deleted(GroundCoverCatalogKey::Visual(
                    duplicate_visual_id,
                )),
            ])
        );
        assert_eq!(
            project_writer
                .apply_ground_cover_catalog_transaction(&[GroundCoverCatalogWrite::DeletePreset {
                    preset: ground_cover_preset_id,
                    expected_source_revision: 2,
                },])
                .unwrap(),
            GroundCoverCatalogWriteTransactionResult::BlockedByDependency {
                key: GroundCoverCatalogKey::Preset(ground_cover_preset_id),
                dependency: GroundCoverCatalogDependency::PresetRegions,
            }
        );

        let rolled_back_region_id = GroundCoverRegionId([22; 16]);
        let rolled_back_region = SourceGroundCoverRegionRecord {
            id: rolled_back_region_id,
            layer: ground_cover_layer_id,
            space: space.id,
            preset: ground_cover_preset_id,
            display_name: "Rolled back test region".into(),
            enabled: true,
            density_multiplier: 1.0,
            source_revision: 0,
        };
        assert_eq!(
            project_writer
                .apply_ground_cover_region_transaction(&[
                    GroundCoverRegionWrite::Create {
                        record: rolled_back_region,
                    },
                    GroundCoverRegionWrite::DeleteEmpty {
                        region: ground_cover_region_id,
                        expected_source_revision: 2,
                    },
                ])
                .unwrap(),
            GroundCoverRegionWriteTransactionResult::BlockedByCoverage {
                region: ground_cover_region_id,
            }
        );
        assert!(
            project_reader
                .read_ground_cover_regions(space.id, 32)
                .unwrap()
                .iter()
                .all(|region| region.id != rolled_back_region_id),
            "an earlier create must roll back when a later delete is blocked"
        );

        let payload = PagePayload::TerrainRender(TerrainRenderPage {
            height: 0.0,
            surfaces: vec![terrain_surface_id],
            weight_pages: Vec::new(),
        });
        let decoded = encode_page_payload(&payload).unwrap();
        let page = EncodedPage {
            key: PageKey {
                space: space.id,
                cell: CellCoord::ZERO,
                domain: PageDomain::TerrainRender,
                lod: 0,
            },
            codec: PageCodec::Raw,
            decoded_bytes: decoded.len() as u64,
            gpu_bytes_estimate: 128,
            checksum: *blake3::hash(&decoded).as_bytes(),
            payload: decoded,
        };
        let gameplay_payload = PagePayload::GameplayObjects(GameplayObjectsPage {
            instances: vec![GameplayObjectInstance {
                id: object_id,
                definition: definition_id,
                translation: [1.0, 0.0, 2.0],
                yaw: 0.0,
                scale: 1.0,
            }],
        });
        let gameplay_decoded = encode_page_payload(&gameplay_payload).unwrap();
        let gameplay_page = EncodedPage {
            key: PageKey {
                space: space.id,
                cell: CellCoord::ZERO,
                domain: PageDomain::GameplayObjects,
                lod: 0,
            },
            codec: PageCodec::Raw,
            decoded_bytes: gameplay_decoded.len() as u64,
            gpu_bytes_estimate: 0,
            checksum: *blake3::hash(&gameplay_decoded).as_bytes(),
            payload: gameplay_decoded,
        };
        let ground_cover_payload = PagePayload::GroundCover(GroundCoverPage {
            clusters: vec![GroundCoverCluster {
                species: ground_cover_species_id,
                local_center: [8.0, 0.35, 8.0],
                half_extents: [8.2, 0.35, 8.2],
                coverage_half_extents: [8.0, 8.0],
                density_per_square_meter: 7.0,
                seed: 91,
            }],
        });
        let ground_cover_decoded = encode_page_payload(&ground_cover_payload).unwrap();
        let ground_cover_page = EncodedPage {
            key: PageKey {
                space: space.id,
                cell: CellCoord::ZERO,
                domain: PageDomain::GroundCover,
                lod: 0,
            },
            codec: PageCodec::Raw,
            decoded_bytes: ground_cover_decoded.len() as u64,
            gpu_bytes_estimate: 64,
            checksum: *blake3::hash(&ground_cover_decoded).as_bytes(),
            payload: ground_cover_decoded,
        };
        write_runtime_database(
            &runtime_path,
            &RuntimeBuild {
                manifest: RuntimeManifest {
                    schema_version: RUNTIME_SCHEMA_VERSION,
                    generation_id: "test-generation".into(),
                    content_hash: [7; 32],
                    default_world_space: space.id,
                    world_spaces: vec![space.clone(), second_space.clone()],
                },
                cells: vec![RuntimeCellRecord {
                    space: space.id,
                    cell: CellCoord::ZERO,
                    minimum_y: 0.0,
                    maximum_y: 0.0,
                    domain_mask: domain_bit(PageDomain::TerrainRender)
                        | domain_bit(PageDomain::GameplayObjects)
                        | domain_bit(PageDomain::GroundCover),
                    source_revision: 1,
                }],
                pages: vec![
                    page.clone(),
                    gameplay_page.clone(),
                    ground_cover_page.clone(),
                ],
                terrain_surfaces: vec![terrain_surface.clone()],
                terrain_texture_sets: vec![terrain_texture_set.clone()],
                terrain_texture_layers: vec![TerrainTextureLayer {
                    texture_set: terrain_texture_set_id,
                    surface: terrain_surface_id,
                    layer: 0,
                }],
                terrain_profiles: vec![TerrainProfile {
                    space: space.id,
                    texture_set: terrain_texture_set_id,
                    weight_resolution: 2,
                    macro_scales: [8.0, 32.0, 128.0],
                    macro_contrast: 1.25,
                    macro_albedo_strength: 0.12,
                }],
                assets: Vec::new(),
                definitions: vec![RuntimeObjectDefinition {
                    id: definition_id,
                    key: "test-door".into(),
                    display_name: "Test door".into(),
                    visual_asset: None,
                    activation: ObjectActivationPolicy::Proximity,
                }],
                ground_cover_species: vec![ground_cover_species.clone()],
                dependencies: Vec::new(),
                definition_dependencies: vec![PageObjectDefinitionRecord {
                    page: gameplay_page.key,
                    definition: definition_id,
                }],
                ground_cover_species_dependencies: vec![PageGroundCoverSpeciesRecord {
                    page: ground_cover_page.key,
                    species: ground_cover_species_id,
                }],
                terrain_surface_dependencies: vec![PageTerrainSurfaceRecord {
                    page: page.key,
                    surface: terrain_surface_id,
                }],
            },
        )
        .unwrap();

        let reader = RuntimeReader::open_immutable(&runtime_path).unwrap();
        assert_eq!(reader.manifest().generation_id, "test-generation");
        assert_eq!(reader.manifest().world_spaces.len(), 2);
        assert_eq!(reader.manifest().default_world_space, space.id);
        assert_eq!(
            reader
                .read_page(page.key)
                .unwrap()
                .unwrap()
                .decode()
                .unwrap()
                .payload,
            payload
        );
        let terrain = reader.read_terrain_resources(page.key).unwrap();
        assert_eq!(terrain.profile.space, space.id);
        assert_eq!(terrain.texture_set, terrain_texture_set);
        assert_eq!(terrain.surfaces[0].surface, terrain_surface);
        assert_eq!(terrain.surfaces[0].layer, 0);
        let definitions = reader.read_object_definitions(gameplay_page.key).unwrap();
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].id, definition_id);
        assert_eq!(definitions[0].key, "test-door");
        assert_eq!(
            reader
                .read_page(ground_cover_page.key)
                .unwrap()
                .unwrap()
                .decode()
                .unwrap()
                .payload,
            ground_cover_payload
        );
        assert_eq!(
            reader
                .read_ground_cover_species(ground_cover_page.key)
                .unwrap(),
            vec![ground_cover_species]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    fn unique_test_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "yarra-world-db-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }
}
