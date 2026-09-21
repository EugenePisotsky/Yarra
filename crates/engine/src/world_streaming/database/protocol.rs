//! Messages between residency/terrain consumers and immutable SQLite IO.
//! Encoded payloads cross this boundary; decoding, admission and GPU state stay with consumers.
use world::{CellCoord, PageKey, TerrainMaterialKey, TerrainNodeKey, WorldSpaceId};
use world_db::{
    CellDescriptor, EncodedPage, EncodedTerrainComposite, EncodedTerrainNode, PageDependency,
    RuntimeManifest, RuntimeObjectDefinition, TerrainCompositeDescriptor, TerrainNodeDescriptor,
    TerrainRenderResources,
};

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
    Presence(bool),
    Descriptors(Vec<TerrainCompositeDescriptor>),
    Tile(EncodedTerrainComposite),
}
