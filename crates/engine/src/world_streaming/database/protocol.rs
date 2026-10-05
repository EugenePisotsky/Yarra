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

#[derive(Debug)]
pub(in crate::world_streaming) enum DatabaseRequest {
    Terrain {
        request_id: u64,
        generation: String,
        query: TerrainQuery,
    },
    Reload {
        request_id: u64,
        expected_generation: String,
    },
    CommitReload {
        request_id: u64,
        expected_generation: String,
    },
    DiscardReload {
        request_id: u64,
    },
    ReadIndex {
        generation: String,
        revision: u64,
        space: WorldSpaceId,
        windows: Vec<[CellCoord; 2]>,
    },
    ReadPage {
        generation: String,
        request_id: u64,
        key: PageKey,
        height_only: bool,
    },
    /// Encoded far-object pages of these blocks (see [`world::far_object_block`]).
    ReadFarObjects {
        generation: String,
        space: WorldSpaceId,
        blocks: Vec<CellCoord>,
    },
}

#[derive(Debug)]
pub(in crate::world_streaming) enum DatabaseResult {
    Terrain {
        request_id: u64,
        result: Result<TerrainReply, String>,
    },
    Opened(Result<RuntimeManifest, String>),
    Reloaded {
        request_id: u64,
        result: Result<RuntimeManifest, String>,
    },
    ReloadCommitted {
        request_id: u64,
        result: Result<(), String>,
    },
    Index {
        revision: u64,
        space: WorldSpaceId,
        result: Result<Vec<CellDescriptor>, String>,
    },
    Page {
        request_id: u64,
        key: PageKey,
        result: Result<Option<FetchedPage>, String>,
    },
    /// Every requested block.
    FarObjects {
        generation: String,
        space: WorldSpaceId,
        result: Result<FarObjectPayloads, String>,
    },
}

#[derive(Debug)]
pub(in crate::world_streaming) struct FetchedPage {
    pub(in crate::world_streaming) encoded: EncodedPage,
    pub(in crate::world_streaming) dependencies: Vec<PageDependency>,
    pub(in crate::world_streaming) definitions: Vec<RuntimeObjectDefinition>,
    pub(in crate::world_streaming) terrain: Option<TerrainRenderResources>,
    pub(in crate::world_streaming) height_only: bool,
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
