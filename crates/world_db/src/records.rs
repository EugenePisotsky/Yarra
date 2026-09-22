//! Project/runtime record contracts. Database access belongs to the readers and writers.
use crate::{EncodedPage, RoadDocument, SourceEnvironmentCellRecord};
use vegetation::VegetationCatalog;
use world::{
    AssetId, CellCoord, ObjectActivationPolicy, ObjectDefinitionId, PageDomain, PageKey,
    StableObjectId, TerrainProfile, TerrainSurface, TerrainSurfaceId, TerrainTextureLayer,
    TerrainTextureSet, WorldSpaceId,
};

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
    pub presets: environment::PresetLibrary,
    pub environments: Vec<environment::EnvironmentDefinition>,
    pub environment_cells: Vec<SourceEnvironmentCellRecord>,
    /// Whole-project import/export only. Interactive readers use bounded road snapshots.
    pub roads: RoadDocument,
    pub terrain_cell_heightfields: Vec<SourceTerrainCellHeightfieldRecord>,
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
    pub atmosphere: world::atmosphere::AtmosphereProfile,
    pub atmosphere_revision: i64,
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

#[derive(Debug, Clone, PartialEq)]
pub enum VegetationCatalogWriteResult {
    Committed,
    Conflict { actual: Option<VegetationCatalog> },
}
