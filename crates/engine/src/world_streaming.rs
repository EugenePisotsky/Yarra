mod catalog;
use catalog::adopt_runtime_manifest;
pub use catalog::{WorldCatalog, WorldSpaceInfo};
mod database;
use database::{DatabaseRequest, DatabaseResult, NotSent, RequestId, WorldDatabaseWorker};
mod far_objects;
mod generation;
pub use generation::WorldGenerationReload;
mod origin;
pub use origin::{WorldOrigin, WorldViewpoint};
mod replies;
mod residency;
use residency::SourceResidency;
pub use residency::StreamingStats;
#[cfg(test)]
use residency::{MAX_PENDING_SOURCE_PAGES, PageState, attachment::PageAttachment};
mod rebase;
mod smoke;
mod source_demand;
mod surface;
pub use surface::{
    StreamedTerrainSurface, StreamedVegetationFieldPage, sample_resident_terrain_surface,
};
mod valley_mist;
mod world_space;
pub use crate::object_lod::{
    GeneratedEnvironmentObject, StreamedVisualObject, VisualLodScale, spawn_collection_visual,
};
pub use rebase::WorldRenderRoot;
pub use smoke::StreamingSmokePlugin;
pub use world_space::ActiveWorldSpace;
use world_space::WorldSpaceTransition;
pub(crate) use world_space::sync_world_atmosphere;
pub(crate) mod terrain_lod;
pub use terrain_lod::{
    LiveTerrainPreview, TerrainContactReadiness, TerrainLodStats, TerrainPreviewRequest,
};

use std::path::PathBuf;

use bevy::{
    camera::primitives::{Aabb, Frustum},
    prelude::*,
    transform::TransformSystems,
};
use crossbeam_channel::TryRecvError;
use vegetation::{VegetationCatalog, VegetationFieldPageData};
use world::{
    CellCoord, ObjectDefinitionId, PageDomain, PageKey, StableObjectId, TerrainHeightfield,
    WorldPosition, WorldSpaceId,
};
use world_db::{CellDescriptor, RuntimeManifest};

use crate::actor::{CharacterMotion, CharacterMotor, MoveIntent, WorldStreamFocus};

const INDEX_RADIUS_CELLS: i32 = 3;
// Local tools and contact keep a cell window around the focus.
// Normal camera source and object visibility radii are separate and measured in metres.
const VISUAL_SOURCE_RESIDENCY_RADIUS_CELLS: u32 = 3;
const GAMEPLAY_PRELOAD_RADIUS_CELLS: u32 = 1;

pub struct WorldStreamingPlugin {
    database_path: PathBuf,
    config: WorldStreamingConfig,
}

impl WorldStreamingPlugin {
    pub fn game(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
            config: WorldStreamingConfig::game(),
        }
    }

    pub fn editor(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
            config: WorldStreamingConfig::editor(),
        }
    }
}

impl Plugin for WorldStreamingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            sync_world_atmosphere.before(crate::ApplyAtmosphere),
        );
        terrain_lod::install(app);
        app.add_plugins(database::WorldDatabasePlugin(self.database_path.clone()))
            .insert_resource(self.config)
            .init_resource::<WorldStream>()
            .init_resource::<SourceResidency>()
            .init_resource::<far_objects::FarObjects>()
            .init_resource::<valley_mist::MistTerrain>()
            .init_resource::<ActiveWorldSpace>()
            .init_resource::<WorldCatalog>()
            .init_resource::<WorldGenerationReload>()
            .init_resource::<WorldViewpoint>()
            .init_resource::<crate::WorldStartView>()
            .add_message::<crate::WorldStartAdopted>()
            .init_resource::<WorldOrigin>()
            .init_resource::<WorldDetailDemand>()
            .init_resource::<source_demand::SourceView>()
            .init_resource::<StreamingStats>()
            .init_resource::<WorldDebugControls>()
            .add_plugins(crate::object_lod::ObjectLodPlugin)
            .add_systems(
                Update,
                (
                    replies::receive_database_results,
                    generation::request_reload,
                    world_space::request_world_space_from_keyboard,
                    terrain_lod::entry::prepare,
                    generation::advance_reload,
                    world_space::apply_world_space_transition,
                    origin::sync_stream_focus_to_viewpoint,
                    origin::update_world_origin,
                    rebase::sync_vegetation_origin,
                    source_demand::request_cell_index,
                    source_demand::calculate_page_demand,
                    residency::receive_decode_results,
                    residency::attach_prepared_pages,
                    residency::cool_and_remove_pages,
                    residency::update_streaming_stats,
                    far_objects::update,
                    valley_mist::update,
                )
                    .chain()
                    .in_set(WorldStreamingSystems),
            )
            .add_systems(
                PostUpdate,
                source_demand::collect_view.after(TransformSystems::Propagate),
            );
    }
}

/// Explicit application-owned development controls; inactive in normal launches.
#[derive(Resource, Clone, Copy, Default)]
pub struct WorldDebugControls {
    pub world_switch: bool,
}

/// Source attachment and coordinate changes finish before consumers rebuild render data.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorldStreamingSystems;

/// The render origin moves to the focus's cell once the focus is more than this many cells
/// away, keeping render coordinates small anywhere in a large world.
pub const FLOATING_ORIGIN_THRESHOLD_CELLS: u32 = 8;

#[derive(Resource, Clone, Copy, Debug)]
pub struct WorldStreamingConfig {
    gameplay_pages: bool,
    keyboard_world_space_cycle: bool,
}

impl WorldStreamingConfig {
    pub const fn game() -> Self {
        Self {
            gameplay_pages: true,
            keyboard_world_space_cycle: true,
        }
    }

    pub const fn editor() -> Self {
        Self {
            gameplay_pages: false,
            keyboard_world_space_cycle: false,
        }
    }
}

/// Marks the camera whose frustum drives visual world-page demand.
#[derive(Component, Debug, Clone, Copy)]
pub struct WorldViewCamera;

/// Allows an overview editor to keep the local preload ring without expanding detailed frustum
/// demand across an entire region. Gameplay leaves this enabled.
#[derive(Resource, Debug, Clone, Copy)]
pub struct WorldDetailDemand {
    enabled: bool,
}

impl Default for WorldDetailDemand {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl WorldDetailDemand {
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

#[derive(Resource, Default)]
struct WorldStream {
    phase: StreamPhase,
    manifest: Option<RuntimeManifest>,
    index_windows: Option<Vec<source_demand::Window>>,
    demand_error: Option<String>,
    requested_index: Option<(RequestId, WorldSpaceId)>,
    descriptors: Vec<CellDescriptor>,
}

#[derive(Default)]
enum StreamPhase {
    #[default]
    Opening,
    Ready,
    Failed(String),
}

fn clear_streamed_pages(
    commands: &mut Commands,
    stream: &mut WorldStream,
    residency: &mut SourceResidency,
) {
    residency.clear(commands);
    stream.demand_error = None;
    stream.descriptors.clear();
    stream.index_windows = None;
    stream.requested_index = None;
}

#[derive(Component, Debug, Clone, Copy)]
pub struct GameplayObject {
    pub id: StableObjectId,
    pub definition: ObjectDefinitionId,
}

#[derive(Component)]
struct StreamedPageEntity(PageKey);

#[cfg(test)]
pub(crate) fn test_world_resources(
    space: WorldSpaceId,
    cell: CellCoord,
    vegetation: Option<VegetationCatalog>,
) -> (WorldCatalog, WorldOrigin) {
    (
        WorldCatalog {
            generation_id: "test-world".into(),
            default_world_space: Some(space),
            world_spaces: vec![WorldSpaceInfo {
                atmosphere: default(),
                id: space,
                name: "test".into(),
                cell_size: 16.,
                minimum_y: -100.,
                maximum_y: 100.,
                sea_level: None,
            }],
            vegetation,
            gameplay_areas: default(),
        },
        WorldOrigin {
            space: Some(space),
            cell,
        },
    )
}
