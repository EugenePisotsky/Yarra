//! Messages between residency/terrain consumers and immutable SQLite IO.
//! Encoded payloads cross this boundary; decoding, admission and GPU state stay with consumers.
use world::{CellCoord, PageKey, TerrainMaterialKey, TerrainNodeKey, WorldSpaceId};
use world_db::{
    CellDescriptor, EncodedPage, EncodedTerrainComposite, EncodedTerrainNode, PageDependency,
    RuntimeManifest, RuntimeObjectDefinition, TerrainCompositeDescriptor, TerrainNodeDescriptor,
    TerrainRenderResources,
};

/// Encoded far-object pages by block; `None` for a block without far objects.
pub(in crate::world_streaming) type FarObjectPayloads = Vec<(CellCoord, Option<Vec<u8>>)>;

/// Names a request; its reply carries the same id. The worker numbers requests from 1, so
/// id 0 is the opening reply, which nothing requested.
pub(in crate::world_streaming) type RequestId = u64;

#[derive(Debug)]
pub(in crate::world_streaming) enum DatabaseRequest {
    Terrain {
        generation: String,
        query: TerrainQuery,
    },
    /// Opens the published database beside the live one, as a candidate named by this
    /// request's id.
    Reload {
        expected_generation: String,
    },
    CommitReload {
        reload: RequestId,
        expected_generation: String,
    },
    DiscardReload {
        reload: RequestId,
    },
    ReadIndex {
        generation: String,
        space: WorldSpaceId,
        windows: Vec<[CellCoord; 2]>,
    },
    ReadPage {
        generation: String,
        key: PageKey,
    },
    /// Encoded far-object pages of these blocks (see [`world::far_object_block`]).
    ReadFarObjects {
        generation: String,
        space: WorldSpaceId,
        blocks: Vec<CellCoord>,
    },
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // 32 queued replies; boxing would allocate per page.
pub(in crate::world_streaming) enum DatabaseResult {
    Terrain(Result<TerrainReply, String>),
    Opened(Result<RuntimeManifest, String>),
    Reloaded(Result<RuntimeManifest, String>),
    ReloadCommitted(Result<(), String>),
    Index(Result<Vec<CellDescriptor>, String>),
    Page {
        key: PageKey,
        result: Result<Option<FetchedPage>, String>,
    },
    /// Every requested block.
    FarObjects(Result<FarObjectPayloads, String>),
}

#[derive(Debug)]
pub(in crate::world_streaming) struct FetchedPage {
    pub(in crate::world_streaming) encoded: EncodedPage,
    pub(in crate::world_streaming) dependencies: Vec<PageDependency>,
    pub(in crate::world_streaming) definitions: Vec<RuntimeObjectDefinition>,
    pub(in crate::world_streaming) terrain: Option<TerrainRenderResources>,
}

#[derive(Clone, Debug)]
pub(in crate::world_streaming) enum TerrainQuery {
    Roots(WorldSpaceId),
    Metadata(Vec<TerrainNodeKey>),
    Node(TerrainNodeKey),
    Material(TerrainMaterialQuery),
}
#[derive(Debug)]
pub(in crate::world_streaming) enum TerrainReply {
    Metadata(Vec<TerrainNodeDescriptor>),
    Node(EncodedTerrainNode),
    Material(TerrainMaterialReply),
}
#[derive(Clone, Debug)]
pub(in crate::world_streaming) enum TerrainMaterialQuery {
    Presence(WorldSpaceId),
    Descriptors(Vec<TerrainMaterialKey>),
    Tile(TerrainMaterialKey),
}
#[derive(Debug)]
pub(in crate::world_streaming) enum TerrainMaterialReply {
    /// The finest published composite level, or `None` without baked ground.
    Presence(Option<u8>),
    Descriptors(Vec<TerrainCompositeDescriptor>),
    Tile(EncodedTerrainComposite),
}
