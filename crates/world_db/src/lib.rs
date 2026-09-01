mod schema;

use std::{
    collections::HashSet,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use vegetation::{VegetationCatalog, VegetationFieldPageData};
use world::{
    AssetId, CellCoord, MAX_DECODED_PAGE_BYTES, ObjectActivationPolicy, ObjectDefinitionId,
    PROJECT_SCHEMA_VERSION, PageCodec, PageDomain, PageKey, PagePayload, RUNTIME_SCHEMA_VERSION,
    StableObjectId, TerrainProfile, TerrainSurface, TerrainSurfaceId, TerrainTextureLayer,
    TerrainTextureSet, TerrainTextureSetId, WorldSpaceId, decode_page_payload,
};

pub const MAX_OBJECT_WRITES_PER_TRANSACTION: usize = 256;
pub const MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION: usize = 64;
const VEGETATION_CATALOG_FORMAT_VERSION: i64 = 6;
const MIGRATABLE_PROJECT_SCHEMA_VERSIONS: [i64; 4] = [12, 13, 14, 15];

#[derive(Debug, Clone)]
pub struct ProjectDocument {
    pub default_world_space: WorldSpaceId,
    pub world_spaces: Vec<WorldSpaceRecord>,
    pub vegetation_catalog: Option<VegetationCatalog>,
    pub cells: Vec<SourceCellRecord>,
    pub terrain_surfaces: Vec<TerrainSurface>,
    pub terrain_texture_sets: Vec<TerrainTextureSet>,
    pub terrain_texture_layers: Vec<TerrainTextureLayer>,
    pub terrain_profiles: Vec<TerrainProfile>,
    pub terrain_cell_surface_slots: Vec<SourceTerrainCellSurfaceSlotRecord>,
    pub terrain_cell_weight_pages: Vec<SourceTerrainCellWeightPageRecord>,
    pub terrain_cell_heightfields: Vec<SourceTerrainCellHeightfieldRecord>,
    pub vegetation_field_pages: Vec<SourceVegetationFieldPageRecord>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct SourceVegetationFieldPageRecord {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub data: VegetationFieldPageData,
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

/// Editable endpoint-inclusive terrain relief for one source cell.
///
/// Authoring keeps f32 samples. Cooking performs the world-space quantization used by runtime
/// pages, which lets the editor evolve without tying its precision to the renderer.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceTerrainCellHeightfieldRecord {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub resolution: u16,
    pub heights: Vec<f32>,
    pub source_revision: i64,
}

#[derive(Debug, Clone)]
pub struct SourceTerrainCellWeightPageQuery {
    pub records: Vec<SourceTerrainCellWeightPageRecord>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DenseSourceRecordKey {
    TerrainWeights {
        space: WorldSpaceId,
        cell: CellCoord,
        page: u8,
    },
}

#[derive(Debug, Clone)]
pub enum DenseSourceWrite {
    TerrainWeights {
        expected_source_revision: Option<i64>,
        record: SourceTerrainCellWeightPageRecord,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenseSourceRecord {
    TerrainWeights(SourceTerrainCellWeightPageRecord),
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
    pub dependencies: Vec<PageDependencyRecord>,
    pub definition_dependencies: Vec<PageObjectDefinitionRecord>,
    pub terrain_surface_dependencies: Vec<PageTerrainSurfaceRecord>,
}

#[derive(Debug, Clone)]
pub struct RuntimeManifest {
    pub schema_version: i64,
    pub generation_id: String,
    pub content_hash: [u8; 32],
    pub default_world_space: WorldSpaceId,
    pub world_spaces: Vec<WorldSpaceRecord>,
    pub vegetation_catalog: Option<VegetationCatalog>,
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
    if MIGRATABLE_PROJECT_SCHEMA_VERSIONS.contains(&actual) && PROJECT_SCHEMA_VERSION == 16 {
        // Catalog format 6 adds an authored projected-width floor for ribbon species. Vegetation
        // catalog payloads are bincode sequences and therefore cannot safely default a newly
        // appended field. Discard only that incompatible legacy payload while preserving every
        // other authored domain. A current project or an explicit reference-fixture reset supplies
        // the new-format catalog; normal saves and publications never replace it implicitly.
        let transaction = connection.transaction()?;
        transaction.execute_batch(
            "DROP TABLE vegetation_catalog;
             CREATE TABLE vegetation_catalog (
                 singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                 format_version INTEGER NOT NULL CHECK(format_version = 6),
                 payload BLOB NOT NULL
             ) STRICT;",
        )?;
        transaction.pragma_update(None, "user_version", PROJECT_SCHEMA_VERSION)?;
        transaction.commit()?;
        return Ok(true);
    }
    Err(WorldDbError::SchemaVersion {
        database: "project",
        expected: PROJECT_SCHEMA_VERSION,
        actual,
    })
}

fn write_vegetation_catalog(
    transaction: &Transaction<'_>,
    catalog: Option<&VegetationCatalog>,
) -> Result<(), WorldDbError> {
    let Some(catalog) = catalog else {
        return Ok(());
    };
    let payload = encode_vegetation_catalog(catalog)?;
    transaction.execute(
        "INSERT INTO vegetation_catalog(singleton, format_version, payload) \
         VALUES (1, ?1, ?2)",
        params![VEGETATION_CATALOG_FORMAT_VERSION, payload],
    )?;
    Ok(())
}

fn encode_vegetation_catalog(catalog: &VegetationCatalog) -> Result<Vec<u8>, WorldDbError> {
    catalog.validate()?;
    Ok(bincode::serde::encode_to_vec(
        catalog,
        bincode::config::standard()
            .with_little_endian()
            .with_fixed_int_encoding(),
    )?)
}

fn read_vegetation_catalog(
    connection: &Connection,
) -> Result<Option<VegetationCatalog>, WorldDbError> {
    connection
        .query_row(
            "SELECT format_version, payload FROM vegetation_catalog WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?
        .map(|(format_version, payload)| {
            if format_version != VEGETATION_CATALOG_FORMAT_VERSION {
                return Err(WorldDbError::VegetationCatalogFormatVersion {
                    expected: VEGETATION_CATALOG_FORMAT_VERSION,
                    actual: format_version,
                });
            }
            let (catalog, consumed): (VegetationCatalog, usize) =
                bincode::serde::decode_from_slice(
                    &payload,
                    bincode::config::standard()
                        .with_little_endian()
                        .with_fixed_int_encoding()
                        .with_limit::<{ MAX_DECODED_PAGE_BYTES as usize }>(),
                )?;
            if consumed != payload.len() {
                return Err(WorldDbError::VegetationCatalogTrailingBytes {
                    consumed,
                    total: payload.len(),
                });
            }
            catalog.validate()?;
            Ok(catalog)
        })
        .transpose()
}

fn encode_vegetation_field_page(data: &VegetationFieldPageData) -> Result<Vec<u8>, WorldDbError> {
    Ok(bincode::serde::encode_to_vec(
        data,
        bincode::config::standard()
            .with_little_endian()
            .with_fixed_int_encoding(),
    )?)
}

fn decode_vegetation_field_page(payload: &[u8]) -> Result<VegetationFieldPageData, WorldDbError> {
    let (data, consumed): (VegetationFieldPageData, usize) = bincode::serde::decode_from_slice(
        payload,
        bincode::config::standard()
            .with_little_endian()
            .with_fixed_int_encoding()
            .with_limit::<{ MAX_DECODED_PAGE_BYTES as usize }>(),
    )?;
    if consumed != payload.len() {
        return Err(WorldDbError::VegetationFieldPageTrailingBytes {
            consumed,
            total: payload.len(),
        });
    }
    Ok(data)
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
    write_vegetation_catalog(transaction, document.vegetation_catalog.as_ref())?;
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
    for heightfield in &document.terrain_cell_heightfields {
        transaction.execute(
            "INSERT INTO terrain_cell_heightfields( \
                world_space_id, cell_x, cell_z, resolution, heights, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                heightfield.space.0,
                heightfield.cell.x,
                heightfield.cell.z,
                i64::from(heightfield.resolution),
                encode_f32_blob(&heightfield.heights),
                heightfield.source_revision,
            ],
        )?;
    }
    for page in &document.vegetation_field_pages {
        let catalog = document
            .vegetation_catalog
            .as_ref()
            .ok_or(WorldDbError::MissingVegetationCatalog)?;
        page.data.validate(catalog)?;
        transaction.execute(
            "INSERT INTO vegetation_field_pages( \
                world_space_id, cell_x, cell_z, format_version, payload, source_revision \
             ) VALUES (?1, ?2, ?3, 1, ?4, ?5)",
            params![
                page.space.0,
                page.cell.x,
                page.cell.z,
                encode_vegetation_field_page(&page.data)?,
                page.source_revision,
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

pub fn read_project_database(path: &Path) -> Result<ProjectDocument, WorldDbError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;

    let world_spaces = query_world_spaces(&connection)?;
    let default_world_space = connection.query_row(
        "SELECT default_world_space_id FROM project_settings WHERE singleton = 1",
        [],
        |row| Ok(WorldSpaceId(row.get(0)?)),
    )?;
    let vegetation_catalog = read_vegetation_catalog(&connection)?;
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
    let mut statement = connection.prepare(
        "SELECT world_space_id, cell_x, cell_z, resolution, heights, source_revision \
         FROM terrain_cell_heightfields \
         ORDER BY world_space_id, cell_x, cell_z",
    )?;
    let terrain_cell_heightfields = statement
        .query_map([], |row| {
            Ok(SourceTerrainCellHeightfieldRecord {
                space: WorldSpaceId(row.get(0)?),
                cell: CellCoord {
                    x: row.get(1)?,
                    z: row.get(2)?,
                },
                resolution: row.get::<_, i64>(3)? as u16,
                heights: decode_f32_blob(row.get_ref(4)?.as_blob()?, "terrain heights")?,
                source_revision: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut statement = connection.prepare(
        "SELECT world_space_id, cell_x, cell_z, payload, source_revision \
         FROM vegetation_field_pages WHERE format_version = 1 \
         ORDER BY world_space_id, cell_x, cell_z",
    )?;
    let vegetation_field_pages = statement
        .query_map([], |row| {
            let payload = row.get::<_, Vec<u8>>(3)?;
            Ok((
                WorldSpaceId(row.get(0)?),
                CellCoord {
                    x: row.get(1)?,
                    z: row.get(2)?,
                },
                payload,
                row.get(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(space, cell, payload, source_revision)| {
            Ok(SourceVegetationFieldPageRecord {
                space,
                cell,
                data: decode_vegetation_field_page(&payload)?,
                source_revision,
            })
        })
        .collect::<Result<Vec<_>, WorldDbError>>()?;
    if !vegetation_field_pages.is_empty() {
        let catalog = vegetation_catalog
            .as_ref()
            .ok_or(WorldDbError::MissingVegetationCatalog)?;
        for page in &vegetation_field_pages {
            page.data.validate(catalog)?;
        }
    }

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
        vegetation_catalog,
        cells,
        terrain_surfaces,
        terrain_texture_sets,
        terrain_texture_layers,
        terrain_profiles,
        terrain_cell_surface_slots,
        terrain_cell_weight_pages,
        terrain_cell_heightfields,
        vegetation_field_pages,
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

    /// Reads the generation-global vegetation source catalog without constructing the spatial
    /// project document. Editors keep this small global domain separate from bounded cell queries.
    pub fn read_vegetation_catalog(&self) -> Result<Option<VegetationCatalog>, WorldDbError> {
        read_vegetation_catalog(&self.connection)
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

#[derive(Debug, Clone, PartialEq)]
pub enum VegetationCatalogWriteResult {
    Committed,
    Conflict { actual: Option<VegetationCatalog> },
}

impl ProjectWriter {
    pub fn open(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 1000;")?;
        ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;
        Ok(Self { connection })
    }

    /// Replaces the generation-global vegetation catalog in one transaction.
    ///
    /// This is intentionally separate from spatial field-page writes: profile authoring changes a
    /// small global source domain, while painting and placement tools mutate bounded page domains.
    pub fn replace_vegetation_catalog(
        &mut self,
        catalog: &VegetationCatalog,
    ) -> Result<(), WorldDbError> {
        let payload = encode_vegetation_catalog(catalog)?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO vegetation_catalog(singleton, format_version, payload) \
             VALUES (1, ?1, ?2) \
             ON CONFLICT(singleton) DO UPDATE SET \
                 format_version = excluded.format_version, payload = excluded.payload",
            params![VEGETATION_CATALOG_FORMAT_VERSION, payload],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Replaces the global vegetation catalog only when the persisted catalog still matches the
    /// editor baseline. This gives the singleton source domain the same optimistic-concurrency
    /// behavior as spatial object and dense-page transactions without coupling it to their
    /// per-record revision columns.
    pub fn replace_vegetation_catalog_if_matches(
        &mut self,
        expected: Option<&VegetationCatalog>,
        replacement: &VegetationCatalog,
    ) -> Result<VegetationCatalogWriteResult, WorldDbError> {
        replacement.validate()?;
        let transaction = self.connection.transaction()?;
        let actual = read_vegetation_catalog(&transaction)?;
        if actual.as_ref() != expected {
            return Ok(VegetationCatalogWriteResult::Conflict { actual });
        }

        let payload = encode_vegetation_catalog(replacement)?;
        transaction.execute(
            "INSERT INTO vegetation_catalog(singleton, format_version, payload) \
             VALUES (1, ?1, ?2) \
             ON CONFLICT(singleton) DO UPDATE SET \
                 format_version = excluded.format_version, payload = excluded.payload",
            params![VEGETATION_CATALOG_FORMAT_VERSION, payload],
        )?;
        transaction.commit()?;
        Ok(VegetationCatalogWriteResult::Committed)
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
    write_vegetation_catalog(transaction, manifest.vegetation_catalog.as_ref())?;
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
    let vegetation_catalog = read_vegetation_catalog(connection)?;
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
                vegetation_catalog,
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

fn encode_f32_blob(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_f32_blob(bytes: &[u8], field: &'static str) -> rusqlite::Result<Vec<f32>> {
    if !bytes.len().is_multiple_of(size_of::<f32>()) {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Blob,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{field} byte length must be divisible by four"),
            )
            .into(),
        ));
    }
    Ok(bytes
        .chunks_exact(size_of::<f32>())
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte chunk")))
        .collect())
}

#[derive(Debug, thiserror::Error)]
pub enum WorldDbError {
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("page payload is invalid: {0}")]
    Payload(#[from] world::PagePayloadDecodeError),
    #[error("vegetation catalog is invalid: {0}")]
    VegetationCatalog(#[from] vegetation::CatalogValidationError),
    #[error("vegetation field page is invalid: {0}")]
    VegetationFieldPage(#[from] vegetation::PageValidationError),
    #[error("vegetation data encoding failed: {0}")]
    VegetationEncode(#[from] bincode::error::EncodeError),
    #[error("vegetation data decoding failed: {0}")]
    VegetationDecode(#[from] bincode::error::DecodeError),
    #[error("vegetation catalog contains trailing bytes: decoded {consumed} of {total}")]
    VegetationCatalogTrailingBytes { consumed: usize, total: usize },
    #[error("vegetation catalog format version is {actual}, expected {expected}")]
    VegetationCatalogFormatVersion { expected: i64, actual: i64 },
    #[error("vegetation field page contains trailing bytes: decoded {consumed} of {total}")]
    VegetationFieldPageTrailingBytes { consumed: usize, total: usize },
    #[error("vegetation field pages require a vegetation catalog")]
    MissingVegetationCatalog,
    #[error("database already exists: {0}")]
    AlreadyExists(PathBuf),
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
    #[error("dense source transaction must contain 1 to 64 unique terrain writes")]
    InvalidDenseSourceTransaction,
    #[error("dense source record has an invalid page, resolution, payload length, or revision")]
    InvalidDenseSourceRecord,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_schema_migration_replaces_only_the_incompatible_catalog_table() {
        for (schema_version, catalog_format) in [(12, 2), (13, 3), (14, 4), (15, 5)] {
            verify_project_catalog_migration(schema_version, catalog_format);
        }
    }

    fn verify_project_catalog_migration(schema_version: i64, catalog_format: i64) {
        let directory = unique_test_directory();
        fs::create_dir_all(&directory).unwrap();
        let project_path = directory.join("migration.sqlite");
        let connection = Connection::open(&project_path).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE vegetation_catalog (
                     singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                     format_version INTEGER NOT NULL CHECK(format_version = {catalog_format}),
                     payload BLOB NOT NULL
                 ) STRICT;
                 CREATE TABLE preserved_authored_data(value TEXT NOT NULL) STRICT;
                 INSERT INTO vegetation_catalog(singleton, format_version, payload)
                     VALUES (1, {catalog_format}, X'0102');
                 INSERT INTO preserved_authored_data(value) VALUES ('kept');
                 PRAGMA user_version = {schema_version};"
            ))
            .unwrap();
        drop(connection);

        assert!(migrate_project_database(&project_path).unwrap());
        assert!(!migrate_project_database(&project_path).unwrap());

        let connection = Connection::open(&project_path).unwrap();
        let schema_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        let catalog_rows: i64 = connection
            .query_row("SELECT COUNT(*) FROM vegetation_catalog", [], |row| {
                row.get(0)
            })
            .unwrap();
        let preserved: String = connection
            .query_row("SELECT value FROM preserved_authored_data", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(schema_version, PROJECT_SCHEMA_VERSION);
        assert_eq!(catalog_rows, 0);
        assert_eq!(preserved, "kept");
        connection
            .execute(
                "INSERT INTO vegetation_catalog(singleton, format_version, payload)
                 VALUES (1, 6, X'')",
                [],
            )
            .unwrap();
        drop(connection);
        fs::remove_dir_all(directory).unwrap();
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
            minimum_y: -8.0,
            maximum_y: 16.0,
        };
        let vegetation_catalog = vegetation::fixtures::reference_catalog();
        let project = ProjectDocument {
            default_world_space: space.id,
            world_spaces: vec![space.clone()],
            vegetation_catalog: Some(vegetation_catalog.clone()),
            cells: vec![SourceCellRecord {
                space: space.id,
                cell: CellCoord::ZERO,
                height: 1.5,
                source_revision: 1,
            }],
            terrain_surfaces: Vec::new(),
            terrain_texture_sets: Vec::new(),
            terrain_texture_layers: Vec::new(),
            terrain_profiles: Vec::new(),
            terrain_cell_surface_slots: Vec::new(),
            terrain_cell_weight_pages: Vec::new(),
            terrain_cell_heightfields: Vec::new(),
            vegetation_field_pages: Vec::new(),
            assets: Vec::new(),
            asset_variants: Vec::new(),
            definitions: Vec::new(),
            objects: Vec::new(),
        };
        write_project_database(&project_path, &project).unwrap();
        let read_project = read_project_database(&project_path).unwrap();
        assert_eq!(read_project.default_world_space, space.id);
        assert_eq!(read_project.cells.len(), 1);
        assert_eq!(
            read_project
                .vegetation_catalog
                .as_ref()
                .map(|catalog| catalog.populations.len()),
            Some(vegetation_catalog.populations.len())
        );

        let mut replacement_catalog = vegetation_catalog.clone();
        replacement_catalog.populations[0].density_per_square_meter = 5.5;
        ProjectWriter::open(&project_path)
            .unwrap()
            .replace_vegetation_catalog(&replacement_catalog)
            .unwrap();
        assert_eq!(
            ProjectReader::open_read_only(&project_path)
                .unwrap()
                .read_vegetation_catalog()
                .unwrap(),
            Some(replacement_catalog.clone())
        );

        let mut authored_catalog = replacement_catalog.clone();
        authored_catalog.populations[0].density_per_square_meter = 6.25;
        let mut writer = ProjectWriter::open(&project_path).unwrap();
        assert_eq!(
            writer
                .replace_vegetation_catalog_if_matches(
                    Some(&replacement_catalog),
                    &authored_catalog,
                )
                .unwrap(),
            VegetationCatalogWriteResult::Committed
        );

        let mut stale_catalog = replacement_catalog.clone();
        stale_catalog.populations[0].density_per_square_meter = 7.0;
        assert_eq!(
            writer
                .replace_vegetation_catalog_if_matches(Some(&replacement_catalog), &stale_catalog,)
                .unwrap(),
            VegetationCatalogWriteResult::Conflict {
                actual: Some(authored_catalog.clone()),
            }
        );
        assert_eq!(
            ProjectReader::open_read_only(&project_path)
                .unwrap()
                .read_vegetation_catalog()
                .unwrap(),
            Some(authored_catalog)
        );

        let runtime = RuntimeBuild {
            manifest: RuntimeManifest {
                schema_version: RUNTIME_SCHEMA_VERSION,
                generation_id: "test-generation".into(),
                content_hash: [7; 32],
                default_world_space: space.id,
                world_spaces: vec![space],
                vegetation_catalog: Some(vegetation_catalog.clone()),
            },
            cells: Vec::new(),
            pages: Vec::new(),
            terrain_surfaces: Vec::new(),
            terrain_texture_sets: Vec::new(),
            terrain_texture_layers: Vec::new(),
            terrain_profiles: Vec::new(),
            assets: Vec::new(),
            definitions: Vec::new(),
            dependencies: Vec::new(),
            definition_dependencies: Vec::new(),
            terrain_surface_dependencies: Vec::new(),
        };
        write_runtime_database(&runtime_path, &runtime).unwrap();
        let reader = RuntimeReader::open_immutable(&runtime_path).unwrap();
        assert_eq!(reader.manifest().generation_id, "test-generation");
        assert_eq!(
            reader
                .manifest()
                .vegetation_catalog
                .as_ref()
                .map(|catalog| catalog.populations.len()),
            Some(vegetation_catalog.populations.len())
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
