mod generation;
mod rebase;
mod source_demand;
pub use rebase::WorldRenderRoot;
pub(crate) mod terrain_lod;
pub use terrain_lod::{
    LiveTerrainPreview, TerrainContactReadiness, TerrainLodPreview, TerrainLodStats,
    TerrainPreviewRequest,
};

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    thread::{self, JoinHandle},
    time::Duration,
};

use bevy::{
    camera::primitives::{Aabb, Frustum},
    gltf::GltfAssetLabel,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
    transform::TransformSystems,
};
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
#[cfg(not(target_os = "ios"))]
use terrain_render::build_heightfield_mesh;
use terrain_render::{
    PrepareTerrainMaterialContext, TerrainMacroVariation, TerrainMaterial, TerrainSurfaceLayer,
    prepare_terrain_material,
};
use vegetation::{VegetationCatalog, VegetationFieldPageData};
use world::{
    AssetId, CellCoord, ObjectActivationPolicy, ObjectDefinitionId, PageDomain, PageKey,
    PagePayload, StableObjectId, TerrainHeightfield, TerrainTextureSetId, WorldPosition,
    WorldSpaceId,
};
use world_db::{
    CellDescriptor, DecodedPage, EncodedPage, PageDependency, RuntimeManifest,
    RuntimeObjectDefinition, RuntimeReader, TerrainRenderResources,
};

use crate::{
    actor::{CharacterMotion, CharacterMotor, MoveIntent, WorldStreamFocus},
    character::CharacterPresentationReady,
};

const INDEX_RADIUS_CELLS: i32 = 3;
// Local objects/tools and the normal renderer retain their existing cell window.
// The hierarchy preview's camera source radius is separate and measured in metres.
const VISUAL_SOURCE_RESIDENCY_RADIUS_CELLS: u32 = 3;
const GAMEPLAY_PRELOAD_RADIUS_CELLS: u32 = 1;
const COOLING_SECONDS: f32 = 2.0;
const MAX_DATABASE_REQUESTS_IN_FLIGHT: usize = 16;
const MAX_ATTACHMENTS_PER_FRAME: usize = 2;
const MAX_LOD_SWITCHES_PER_FRAME: usize = 32;
const MAX_RESIDENT_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RESIDENT_GPU_BYTES_ESTIMATE: u64 = 256 * 1024 * 1024;
const LOD_HYSTERESIS_FRACTION: f32 = 0.12;

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
        terrain_lod::install(app);
        app.insert_resource(WorldDatabasePath(self.database_path.clone()))
            .insert_resource(self.config)
            .init_resource::<WorldStream>()
            .init_resource::<ActiveWorldSpace>()
            .init_resource::<WorldCatalog>()
            .init_resource::<WorldGenerationReload>()
            .init_resource::<WorldViewpoint>()
            .init_resource::<crate::WorldStartView>()
            .init_resource::<WorldOrigin>()
            .init_resource::<WorldDetailDemand>()
            .init_resource::<source_demand::SourceView>()
            .init_resource::<StreamingStats>()
            .add_systems(Startup, (start_database_worker, create_world_render_assets))
            .add_systems(
                Update,
                (
                    receive_database_results,
                    generation::request_reload,
                    request_world_space_from_keyboard,
                    terrain_lod::entry::prepare,
                    generation::advance_reload,
                    apply_world_space_transition,
                    sync_stream_focus_to_viewpoint,
                    update_world_origin,
                    rebase::sync_vegetation_origin,
                    request_cell_index,
                    calculate_page_demand,
                    receive_decode_results,
                    attach_prepared_pages,
                    cool_and_remove_pages,
                    update_streaming_stats,
                )
                    .chain()
                    .in_set(WorldStreamingSystems)
                    .before(crate::GameInputSystems),
            )
            .add_systems(
                PostUpdate,
                (update_screen_space_lods, source_demand::collect_view)
                    .after(TransformSystems::Propagate),
            );
        if self.config.gameplay_pages {
            app.add_systems(Update, report_streaming_smoke.after(update_streaming_stats));
        }
    }
}

/// Source attachment and coordinate changes finish before consumers rebuild render data.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorldStreamingSystems;

#[derive(Resource, Clone, Copy, Debug)]
pub struct WorldStreamingConfig {
    gameplay_pages: bool,
    keyboard_world_space_cycle: bool,
    floating_origin_threshold_cells: Option<u32>,
}

impl WorldStreamingConfig {
    pub const fn game() -> Self {
        Self {
            gameplay_pages: true,
            keyboard_world_space_cycle: true,
            floating_origin_threshold_cells: None,
        }
    }

    pub const fn editor() -> Self {
        Self {
            gameplay_pages: false,
            keyboard_world_space_cycle: false,
            floating_origin_threshold_cells: Some(8),
        }
    }

    pub const fn loads_gameplay_pages(self) -> bool {
        self.gameplay_pages
    }

    pub const fn floating_origin_threshold_cells(self) -> Option<u32> {
        self.floating_origin_threshold_cells
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

/// Logical position around which the world index and preload set are requested.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct WorldViewpoint {
    position: Option<WorldPosition>,
}

impl WorldViewpoint {
    pub fn position(&self) -> Option<WorldPosition> {
        self.position
    }

    pub fn set(&mut self, position: WorldPosition) {
        self.position = Some(position);
    }
}

/// The logical cell represented by render-space origin.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct WorldOrigin {
    space: Option<WorldSpaceId>,
    cell: CellCoord,
}

impl WorldOrigin {
    pub fn space(&self) -> Option<WorldSpaceId> {
        self.space
    }

    pub fn cell(&self) -> CellCoord {
        self.cell
    }
}

#[derive(Debug, Clone)]
pub struct WorldSpaceInfo {
    pub id: WorldSpaceId,
    pub name: String,
    pub cell_size: f32,
    pub minimum_y: f32,
    pub maximum_y: f32,
}

#[derive(Resource, Debug, Default, Clone)]
pub struct WorldCatalog {
    generation_id: String,
    default_world_space: Option<WorldSpaceId>,
    world_spaces: Vec<WorldSpaceInfo>,
    vegetation: Option<VegetationCatalog>,
}

/// Explicit handshake for replacing the streamer's immutable SQLite snapshot.
///
/// Publishing code first atomically replaces the database file, then requests the exact expected
/// generation here. The worker prepares a second reader while the current one stays live.
/// With the hierarchy preview enabled, a complete uploaded cover must also be ready.
/// Only a matching worker commit acknowledgement replaces the catalog and source pages;
/// preparation failures discard the candidate and retain the current generation.
#[derive(Resource, Debug, Default)]
pub struct WorldGenerationReload {
    next_request_id: u64,
    queued: Option<(u64, String)>,
    in_flight: Option<(u64, String)>,
    completion: Option<Result<String, String>>,
    candidate: Option<RuntimeManifest>,
    commit_requested: bool,
    committed: bool,
    failure: Option<String>,
    last_error: Option<String>,
    hierarchy: bool,
}

impl WorldGenerationReload {
    pub fn request(&mut self, expected_generation: impl Into<String>) -> bool {
        let expected_generation = expected_generation.into();
        if expected_generation.is_empty() || self.queued.is_some() || self.in_flight.is_some() {
            return false;
        }
        let request_id = self.next_request_id.wrapping_add(1).max(1);
        self.next_request_id = request_id;
        self.completion = None;
        self.last_error = None;
        self.queued = Some((request_id, expected_generation));
        true
    }

    pub fn active(&self) -> bool {
        self.queued.is_some() || self.in_flight.is_some()
    }

    pub fn take_completion(&mut self) -> Option<Result<String, String>> {
        self.completion.take()
    }
}

impl WorldCatalog {
    pub fn generation_id(&self) -> &str {
        &self.generation_id
    }

    pub fn default_world_space(&self) -> Option<WorldSpaceId> {
        self.default_world_space
    }

    pub fn world_spaces(&self) -> &[WorldSpaceInfo] {
        &self.world_spaces
    }

    pub fn world_space(&self, id: WorldSpaceId) -> Option<&WorldSpaceInfo> {
        self.world_spaces.iter().find(|space| space.id == id)
    }

    pub fn vegetation(&self) -> Option<&VegetationCatalog> {
        self.vegetation.as_ref()
    }
}

#[derive(Resource)]
struct WorldDatabasePath(PathBuf);

#[derive(Resource, Debug, Default)]
pub struct ActiveWorldSpace {
    current: Option<WorldSpaceId>,
    requested: Option<WorldSpaceTransition>,
    transition_error: Option<String>,
}

impl ActiveWorldSpace {
    pub fn current(&self) -> Option<WorldSpaceId> {
        self.current
    }

    pub fn request(&mut self, space: WorldSpaceId, local_position: [f32; 3]) {
        self.transition_error = None;
        self.requested = Some(WorldSpaceTransition {
            space,
            local_position,
        });
    }

    pub fn transition_error(&self) -> Option<&str> {
        self.transition_error.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WorldSpaceTransition {
    space: WorldSpaceId,
    local_position: [f32; 3],
}

#[derive(Resource)]
struct WorldDatabaseWorker {
    requests: Sender<DatabaseRequest>,
    results: Receiver<DatabaseResult>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for WorldDatabaseWorker {
    fn drop(&mut self) {
        let _ = self.requests.send(DatabaseRequest::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug)]
enum DatabaseRequest {
    Terrain {
        request_id: u64,
        generation: String,
        query: terrain_lod::TerrainQuery,
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
        windows: Vec<source_demand::Window>,
    },
    ReadPage {
        generation: String,
        request_id: u64,
        key: PageKey,
        height_only: bool,
    },
    Shutdown,
}

#[derive(Debug)]
enum DatabaseResult {
    Terrain {
        request_id: u64,
        result: Result<terrain_lod::TerrainReply, String>,
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
struct FetchedPage {
    encoded: EncodedPage,
    dependencies: Vec<PageDependency>,
    definitions: Vec<RuntimeObjectDefinition>,
    terrain: Option<TerrainRenderResources>,
    height_only: bool,
}

fn start_database_worker(mut commands: Commands, path: Res<WorldDatabasePath>) {
    let (request_sender, request_receiver) = bounded(MAX_DATABASE_REQUESTS_IN_FLIGHT);
    let (result_sender, result_receiver) = bounded(MAX_DATABASE_REQUESTS_IN_FLIGHT * 2);
    let database_path = path.0.clone();
    let worker_thread = thread::Builder::new()
        .name("yarra-world-db".into())
        .spawn(move || database_worker(database_path, request_receiver, result_sender))
        .expect("failed to spawn the world database worker");
    commands.insert_resource(WorldDatabaseWorker {
        requests: request_sender,
        results: result_receiver,
        thread: Some(worker_thread),
    });
}

fn database_worker(
    path: PathBuf,
    requests: Receiver<DatabaseRequest>,
    results: Sender<DatabaseResult>,
) {
    let mut reader = match RuntimeReader::open_immutable(&path) {
        Ok(reader) => {
            if results
                .send(DatabaseResult::Opened(Ok(reader.manifest().clone())))
                .is_err()
            {
                return;
            }
            reader
        }
        Err(error) => {
            let _ = results.send(DatabaseResult::Opened(Err(format!(
                "could not open {}: {error}",
                path.display()
            ))));
            return;
        }
    };

    let mut candidate: Option<(u64, RuntimeReader)> = None;
    while let Ok(request) = requests.recv() {
        match request {
            DatabaseRequest::Terrain {
                request_id,
                generation,
                query,
            } => {
                let result = if reader.manifest().generation_id == generation {
                    terrain_lod::read(&reader, query)
                } else if let Some((_, staged)) = &candidate
                    && staged.manifest().generation_id == generation
                {
                    terrain_lod::read(staged, query)
                } else {
                    Err("terrain request belongs to a stale generation".into())
                };
                if results
                    .send(DatabaseResult::Terrain { request_id, result })
                    .is_err()
                {
                    return;
                }
            }
            DatabaseRequest::Reload {
                request_id,
                expected_generation,
            } => {
                let result = RuntimeReader::open_immutable(&path)
                    .map_err(|error| format!("could not reopen {}: {error}", path.display()))
                    .and_then(|opened| {
                        let manifest = opened.manifest().clone();
                        if manifest.generation_id != expected_generation {
                            return Err(format!(
                                "published generation mismatch: expected {expected_generation}, opened {}",
                                manifest.generation_id
                            ));
                        }
                        candidate = Some((request_id, opened));
                        Ok(manifest)
                    });
                if results
                    .send(DatabaseResult::Reloaded { request_id, result })
                    .is_err()
                {
                    return;
                }
            }
            DatabaseRequest::CommitReload {
                request_id,
                expected_generation,
            } => {
                let result = if candidate.as_ref().is_some_and(|(id, r)| {
                    *id == request_id && r.manifest().generation_id == expected_generation
                }) {
                    reader = candidate.take().unwrap().1;
                    Ok(())
                } else {
                    Err("no matching prepared database generation to commit".into())
                };
                if results
                    .send(DatabaseResult::ReloadCommitted { request_id, result })
                    .is_err()
                {
                    return;
                }
            }
            DatabaseRequest::DiscardReload { request_id } => {
                if candidate.as_ref().is_some_and(|(id, _)| *id == request_id) {
                    candidate = None;
                }
            }
            DatabaseRequest::ReadIndex {
                generation,
                revision,
                space,
                windows,
            } => {
                let result = (if reader.manifest().generation_id == generation {
                    Ok(&reader)
                } else {
                    Err("source index belongs to a stale generation".to_string())
                })
                .and_then(|reader| {
                    windows
                        .into_iter()
                        .try_fold(BTreeMap::new(), |mut cells, [min, max]| {
                            for d in reader.read_cell_descriptors(space, min, max)? {
                                cells.insert(d.cell, d);
                            }
                            Ok::<_, world_db::WorldDbError>(cells)
                        })
                        .map(|cells| cells.into_values().collect())
                        .map_err(|error| error.to_string())
                });
                if results
                    .send(DatabaseResult::Index {
                        revision,
                        space,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
            DatabaseRequest::ReadPage {
                generation,
                request_id,
                key,
                height_only,
            } => {
                let result = (if reader.manifest().generation_id == generation {
                    Ok(&reader)
                } else {
                    Err("source page belongs to a stale generation".to_string())
                })
                .and_then(|reader| {
                    reader
                        .read_page(key)
                        .and_then(|page| {
                            page.map(|page| {
                                let dependencies = if height_only {
                                    Vec::new()
                                } else {
                                    reader.read_dependencies(key)?
                                };
                                let definitions = if key.domain == PageDomain::GameplayObjects {
                                    reader.read_object_definitions(key)?
                                } else {
                                    Vec::new()
                                };
                                let terrain = if key.domain == PageDomain::TerrainRender {
                                    Some(reader.read_terrain_resources(key)?)
                                } else {
                                    None
                                };
                                Ok(FetchedPage {
                                    encoded: page,
                                    dependencies,
                                    definitions,
                                    terrain,
                                    height_only,
                                })
                            })
                            .transpose()
                        })
                        .map_err(|error| error.to_string())
                });
                if results
                    .send(DatabaseResult::Page {
                        request_id,
                        key,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
            DatabaseRequest::Shutdown => return,
        }
    }
}

#[derive(Resource, Default)]
struct WorldStream {
    phase: StreamPhase,
    manifest: Option<RuntimeManifest>,
    index_windows: Option<Vec<source_demand::Window>>,
    height_only: bool,
    demand_error: Option<String>,
    admission_blocked: usize,
    index_revision: u64,
    requested_index: Option<(u64, WorldSpaceId)>,
    descriptors: Vec<CellDescriptor>,
    desired: BTreeSet<PageKey>,
    priorities: BTreeMap<PageKey, source_demand::Priority>,
    pages: HashMap<PageKey, PageState>,
    definition_cache: HashMap<ObjectDefinitionId, RuntimeObjectDefinition>,
    decode_tasks: Vec<DecodeTask>,
    next_request_id: u64,
}

#[derive(Default)]
enum StreamPhase {
    #[default]
    Opening,
    Ready,
    Failed(String),
}

enum PageState {
    Loading {
        request_id: u64,
    },
    Decoding {
        request_id: u64,
    },
    Prepared(PreparedPage),
    Resident(PageAttachment),
    Cooling {
        attachment: PageAttachment,
        remove_at: Duration,
    },
    Failed(String),
}

struct PreparedPage {
    decoded: DecodedPage,
    dependencies: Vec<PageDependency>,
    definitions: Vec<RuntimeObjectDefinition>,
    terrain: Option<TerrainRenderResources>,
    height_only: bool,
}

struct DecodeTask {
    request_id: u64,
    key: PageKey,
    task: Task<Result<PreparedPage, String>>,
}

#[derive(Default)]
struct PageAttachment {
    entities: Vec<Entity>,
    owned_terrain_meshes: Vec<Handle<Mesh>>,
    owned_terrain_materials: Vec<Handle<TerrainMaterial>>,
    owned_terrain_images: Vec<Handle<Image>>,
    decoded_bytes: u64,
    gpu_bytes_estimate: u64,
    gameplay_objects: usize,
    vegetation_pages: usize,
    height_only_pages: usize,
    terrain_texture_set: Option<(TerrainTextureSetId, u64)>,
}

/// CPU-readable relief carried by a resident streamed terrain entity.
///
/// This is the bridge for vegetation, character grounding, interactions, and later shadow proxies:
/// every consumer samples the canonical cooked surface. In the hierarchy preview
/// this entity owns only CPU data; it does not also create a ground mesh.
#[derive(Component, Debug, Clone)]
pub struct StreamedTerrainSurface {
    pub key: PageKey,
    pub cell_size: f32,
    pub heightfield: TerrainHeightfield,
}

/// Terrain-independent V2 fields attached for one resident streamed cell.
///
/// Consumers join this component with the matching [`StreamedTerrainSurface`] by page key. No
/// height or normal samples are duplicated in the vegetation payload.
#[derive(Component, Debug, Clone)]
pub struct StreamedVegetationFieldPage {
    pub key: PageKey,
    pub cell_size: f32,
    pub data: VegetationFieldPageData,
}

impl StreamedTerrainSurface {
    pub fn sample_world(&self, world_xz: [f32; 2]) -> world::TerrainSurfaceSample {
        let origin = self.key.cell.origin(self.cell_size);
        self.heightfield.sample(
            [
                world_xz[0] - origin[0] as f32,
                world_xz[1] - origin[1] as f32,
            ],
            self.cell_size,
        )
    }

    pub fn contains_world(&self, world_xz: [f32; 2]) -> bool {
        let origin = self.key.cell.origin(self.cell_size);
        world_xz[0] >= origin[0] as f32
            && world_xz[1] >= origin[1] as f32
            && world_xz[0] <= origin[0] as f32 + self.cell_size
            && world_xz[1] <= origin[1] as f32 + self.cell_size
    }
}

/// Samples resident terrain from render-space X/Z coordinates.
///
/// The conversion through [`WorldOrigin`] keeps gameplay consumers correct after a floating-origin
/// rebase. At a shared page edge the lowest stable page key wins; cooked edge samples are exact, so
/// either page produces the same height and normal.
pub fn sample_resident_terrain_surface<'a>(
    origin: &WorldOrigin,
    surfaces: impl IntoIterator<Item = &'a StreamedTerrainSurface>,
    render_xz: [f32; 2],
) -> Option<world::TerrainSurfaceSample> {
    let active_space = origin.space()?;
    let mut selected: Option<(&StreamedTerrainSurface, [f32; 2])> = None;
    for surface in surfaces {
        if surface.key.space != active_space {
            continue;
        }
        let render_origin = origin.cell.origin(surface.cell_size);
        let world_xz = [
            render_xz[0] + render_origin[0] as f32,
            render_xz[1] + render_origin[1] as f32,
        ];
        if !surface.contains_world(world_xz) {
            continue;
        }
        if selected
            .as_ref()
            .is_none_or(|(current, _)| surface.key < current.key)
        {
            selected = Some((surface, world_xz));
        }
    }
    selected.map(|(surface, world_xz)| surface.sample_world(world_xz))
}

#[derive(Component)]
struct ScreenSpaceLod {
    variants: Vec<ScreenSpaceLodVariant>,
    current: usize,
    bounds_height: f32,
    projected_height: f32,
}

struct ScreenSpaceLodVariant {
    lod: u8,
    scene: Handle<WorldAsset>,
    minimum_screen_height: f32,
}

#[derive(Resource)]
struct WorldRenderAssets {
    unit_plane: Handle<Mesh>,
}

fn create_world_render_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let mut unit_plane = Plane3d::default().mesh().size(1.0, 1.0).build();
    unit_plane
        .generate_tangents()
        .expect("the built-in terrain plane must support tangent generation");
    commands.insert_resource(WorldRenderAssets {
        unit_plane: meshes.add(unit_plane),
    });
}

#[allow(clippy::too_many_arguments)]
fn receive_database_results(
    mut terrain: ResMut<terrain_lod::TerrainLodStream>,
    mut entry: ResMut<terrain_lod::entry::TerrainEntry>,
    worker: Option<Res<WorldDatabaseWorker>>,
    mut active_space: ResMut<ActiveWorldSpace>,
    mut catalog: ResMut<WorldCatalog>,
    mut viewpoint: ResMut<WorldViewpoint>,
    start_view: Res<crate::WorldStartView>,
    mut origin: ResMut<WorldOrigin>,
    mut stream: ResMut<WorldStream>,
    mut reload: ResMut<WorldGenerationReload>,
) {
    let Some(worker) = worker else {
        return;
    };
    loop {
        match worker.results.try_recv() {
            Ok(DatabaseResult::Terrain { request_id, result }) => {
                if entry.owns_request(request_id) {
                    entry.receive(request_id, result);
                } else {
                    terrain.receive(request_id, result);
                }
            }
            Ok(DatabaseResult::Opened(result)) => match result {
                Ok(manifest) => {
                    info!(
                        "opened runtime world generation {} with {} spaces from SQLite",
                        manifest.generation_id,
                        manifest.world_spaces.len()
                    );
                    if active_space.current.is_none() {
                        active_space.current = Some(manifest.default_world_space);
                    }
                    catalog.generation_id = manifest.generation_id.clone();
                    catalog.default_world_space = Some(manifest.default_world_space);
                    catalog.world_spaces = manifest
                        .world_spaces
                        .iter()
                        .map(|space| WorldSpaceInfo {
                            id: space.id,
                            name: space.name.clone(),
                            cell_size: space.cell_size,
                            minimum_y: space.minimum_y,
                            maximum_y: space.maximum_y,
                        })
                        .collect();
                    catalog.vegetation = manifest.vegetation_catalog.clone();
                    if viewpoint.position.is_none() {
                        viewpoint.position = Some(WorldPosition::from_world(
                            manifest.default_world_space,
                            start_view
                                .0
                                .as_ref()
                                .map_or([0.; 3], |v| v.position.map(f64::from)),
                            manifest.default_world_space().cell_size,
                        ));
                    }
                    if origin.space.is_none() {
                        origin.space = Some(manifest.default_world_space);
                        origin.cell = CellCoord::ZERO;
                    }
                    stream.manifest = Some(manifest);
                    stream.phase = StreamPhase::Ready;
                }
                Err(error) => {
                    error!("{error}");
                    stream.phase = StreamPhase::Failed(error);
                }
            },
            Ok(DatabaseResult::Reloaded { request_id, result }) => {
                if reload
                    .in_flight
                    .as_ref()
                    .is_none_or(|(id, _)| *id != request_id)
                {
                    continue;
                }
                match result {
                    Ok(manifest) => reload.candidate = Some(manifest),
                    Err(error) => reload.failure = Some(error),
                }
            }
            Ok(DatabaseResult::ReloadCommitted { request_id, result }) => {
                if reload
                    .in_flight
                    .as_ref()
                    .is_none_or(|(id, _)| *id != request_id)
                {
                    continue;
                }
                match result {
                    Ok(()) => reload.committed = true,
                    Err(error) => reload.failure = Some(error),
                }
            }
            Ok(DatabaseResult::Index {
                revision,
                space,
                result,
            }) => {
                if stream.requested_index != Some((revision, space))
                    || active_space.current != Some(space)
                {
                    continue;
                }
                stream.requested_index = None;
                match result {
                    Ok(descriptors) => {
                        stream.descriptors = descriptors;
                        stream.index_revision = revision;
                    }
                    Err(error) => {
                        error!("world index request failed: {error}");
                        stream.phase = StreamPhase::Failed(error);
                    }
                }
            }
            Ok(DatabaseResult::Page {
                request_id,
                key,
                result,
            }) => {
                let request_is_current = matches!(
                    stream.pages.get(&key),
                    Some(PageState::Loading { request_id: current }) if *current == request_id
                );
                if !request_is_current {
                    continue;
                }
                if !stream.desired.contains(&key) {
                    stream.pages.remove(&key);
                    continue;
                }
                match result {
                    Ok(Some(fetched)) => {
                        let task = AsyncComputeTaskPool::get().spawn(async move {
                            fetched
                                .encoded
                                .decode()
                                .map(|mut decoded| {
                                    if fetched.height_only {
                                        decoded.gpu_bytes_estimate = 0;
                                    }
                                    PreparedPage {
                                        decoded,
                                        dependencies: fetched.dependencies,
                                        definitions: fetched.definitions,
                                        terrain: fetched.terrain,
                                        height_only: fetched.height_only,
                                    }
                                })
                                .map_err(|error| error.to_string())
                        });
                        stream.pages.insert(key, PageState::Decoding { request_id });
                        stream.decode_tasks.push(DecodeTask {
                            request_id,
                            key,
                            task,
                        });
                    }
                    Ok(None) => {
                        stream
                            .pages
                            .insert(key, PageState::Failed("page is missing".into()));
                    }
                    Err(error) => {
                        stream.pages.insert(key, PageState::Failed(error));
                    }
                }
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                if reload.active() {
                    reload.failure =
                        Some("database worker stopped during generation adoption".into());
                }
                if !matches!(stream.phase, StreamPhase::Failed(_)) {
                    stream.phase = StreamPhase::Failed("database worker stopped".into());
                }
                break;
            }
        }
    }
}

fn adopt_runtime_manifest(
    manifest: RuntimeManifest,
    active_space: &mut ActiveWorldSpace,
    catalog: &mut WorldCatalog,
    viewpoint: &mut WorldViewpoint,
    origin: &mut WorldOrigin,
    stream: &mut WorldStream,
) {
    let active = active_space
        .current
        .filter(|space| manifest.world_space(*space).is_some())
        .unwrap_or(manifest.default_world_space);
    active_space.current = Some(active);
    catalog.generation_id = manifest.generation_id.clone();
    catalog.default_world_space = Some(manifest.default_world_space);
    catalog.world_spaces = manifest
        .world_spaces
        .iter()
        .map(|space| WorldSpaceInfo {
            id: space.id,
            name: space.name.clone(),
            cell_size: space.cell_size,
            minimum_y: space.minimum_y,
            maximum_y: space.maximum_y,
        })
        .collect();
    catalog.vegetation = manifest.vegetation_catalog.clone();
    if viewpoint
        .position
        .is_none_or(|position| position.space != active)
    {
        viewpoint.position = Some(WorldPosition {
            space: active,
            cell: CellCoord::ZERO,
            local: [0.0, 0.0, 0.0],
        });
    }
    if origin.space != Some(active) {
        origin.space = Some(active);
        origin.cell = viewpoint
            .position
            .map_or(CellCoord::ZERO, |position| position.cell);
    }
    stream.manifest = Some(manifest);
    stream.phase = StreamPhase::Ready;
}

fn request_world_space_from_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    config: Res<WorldStreamingConfig>,
    stream: Res<WorldStream>,
    mut active_space: ResMut<ActiveWorldSpace>,
) {
    if !config.keyboard_world_space_cycle || !keys.just_pressed(KeyCode::Tab) {
        return;
    }
    let Some(manifest) = stream.manifest.as_ref() else {
        return;
    };
    let Some(current) = active_space.current else {
        return;
    };
    let Some(current_index) = manifest
        .world_spaces
        .iter()
        .position(|space| space.id == current)
    else {
        return;
    };
    if manifest.world_spaces.len() < 2 {
        return;
    }
    let next = &manifest.world_spaces[(current_index + 1) % manifest.world_spaces.len()];
    active_space.request(next.id, [0.0, 0.0, 0.0]);
}

fn apply_world_space_transition(
    mut commands: Commands,
    mut terrain_meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut terrain_images: ResMut<Assets<Image>>,
    mut active_space: ResMut<ActiveWorldSpace>,
    config: Res<WorldStreamingConfig>,
    mut viewpoint: ResMut<WorldViewpoint>,
    mut origin: ResMut<WorldOrigin>,
    mut stream: ResMut<WorldStream>,
    mut focuses: Query<
        (
            &mut Transform,
            &mut MoveIntent,
            &mut CharacterMotor,
            &mut CharacterMotion,
        ),
        With<WorldStreamFocus>,
    >,
    mut entry: ResMut<terrain_lod::entry::TerrainEntry>,
    mut terrain: ResMut<terrain_lod::TerrainLodStream>,
    tracker: Res<terrain_lod::UploadTracker>,
    lod_config: Res<TerrainLodPreview>,
    reload: Res<WorldGenerationReload>,
) {
    if reload.active() {
        return;
    }
    if lod_config.enabled
        && active_space
            .requested
            .is_some_and(|t| Some(t.space) != active_space.current)
        && !active_space.requested.is_some_and(|t| {
            stream
                .manifest
                .as_ref()
                .is_some_and(|m| entry.ready_for(&m.generation_id, t))
        })
    {
        return;
    }
    let Some(transition) = active_space.requested.take() else {
        return;
    };
    if !transition.local_position.iter().all(|v| v.is_finite()) {
        active_space.transition_error = Some("destination coordinates must be finite".into());
        return;
    }
    let Some(space) = stream
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.world_space(transition.space))
        .cloned()
    else {
        error!(
            "ignored transition to unknown world space {:?}",
            transition.space
        );
        return;
    };

    let changed_space = active_space.current != Some(transition.space);
    if changed_space {
        clear_streamed_pages(
            &mut commands,
            &mut terrain_meshes,
            &mut terrain_materials,
            &mut terrain_images,
            &mut stream,
        );
    }

    active_space.current = Some(transition.space);
    let position = WorldPosition::from_world(
        transition.space,
        [
            f64::from(transition.local_position[0]),
            f64::from(transition.local_position[1]),
            f64::from(transition.local_position[2]),
        ],
        space.cell_size,
    );
    viewpoint.set(position);
    if changed_space {
        origin.space = Some(transition.space);
        origin.cell = if rebase::effective_config(*config, lod_config.enabled)
            .floating_origin_threshold_cells
            .is_some()
        {
            position.cell
        } else {
            CellCoord::ZERO
        };
    }
    if changed_space && lod_config.enabled {
        entry.commit(
            &mut terrain,
            &mut commands,
            &mut terrain_meshes,
            &tracker,
            origin.cell,
            space.cell_size,
        );
    }
    for (mut transform, mut intent, mut motor, mut motion) in &mut focuses {
        transform.translation =
            Vec3::from_array(position.relative_to(origin.cell, space.cell_size));
        intent.clear();
        motor.reset();
        *motion = CharacterMotion::default();
    }
    info!(
        "entered world space {} ({:?}) at {:?}",
        space.name, transition.space, transition.local_position
    );
}

fn sync_stream_focus_to_viewpoint(
    active_space: Res<ActiveWorldSpace>,
    origin: Res<WorldOrigin>,
    stream: Res<WorldStream>,
    focuses: Query<&Transform, With<WorldStreamFocus>>,
    mut viewpoint: ResMut<WorldViewpoint>,
) {
    let Some(space_id) = active_space.current else {
        return;
    };
    let Some(space) = stream
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.world_space(space_id))
    else {
        return;
    };
    let Some(transform) = focuses.iter().next() else {
        return;
    };
    let origin_world = origin.cell.origin(space.cell_size);
    viewpoint.set(WorldPosition::from_world(
        space_id,
        [
            origin_world[0] + f64::from(transform.translation.x),
            f64::from(transform.translation.y),
            origin_world[1] + f64::from(transform.translation.z),
        ],
        space.cell_size,
    ));
}

fn update_world_origin(
    mut commands: Commands,
    config: Res<WorldStreamingConfig>,
    terrain_lod: Res<TerrainLodPreview>,
    viewpoint: Res<WorldViewpoint>,
    mut origin: ResMut<WorldOrigin>,
    mut terrain_meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut terrain_images: ResMut<Assets<Image>>,
    mut stream: ResMut<WorldStream>,
    mut roots: rebase::Roots,
) {
    let Some(position) = viewpoint.position else {
        return;
    };
    let desired_cell = desired_origin_cell(
        rebase::effective_config(*config, terrain_lod.enabled),
        *origin,
        position,
    );
    if origin.space == Some(position.space)
        && origin.cell == desired_cell
        && stream.height_only == terrain_lod.enabled
    {
        return;
    }

    let previous = *origin;
    origin.space = Some(position.space);
    origin.cell = desired_cell;
    // CPU source pages use canonical keys. Keep them and their pending work on a
    // same-space rebase; only render roots need translating. The editable renderer
    // still rebuilds its materials using the editor's existing origin contract.
    if previous.space == origin.space {
        if let Some(size) = stream
            .manifest
            .as_ref()
            .and_then(|m| m.world_space(position.space))
            .map(|s| s.cell_size)
        {
            rebase::shift_roots(
                &mut roots,
                previous.cell,
                origin.cell,
                size,
                terrain_lod.enabled && stream.height_only,
            );
        }
    }
    if previous.space != origin.space || !terrain_lod.enabled || !stream.height_only {
        clear_streamed_pages(
            &mut commands,
            &mut terrain_meshes,
            &mut terrain_materials,
            &mut terrain_images,
            &mut stream,
        );
    }
    stream.height_only = terrain_lod.enabled;
    info!(
        "rebased render origin from {:?}:{:?} to {:?}:{:?}",
        previous.space, previous.cell, origin.space, origin.cell
    );
}

fn desired_origin_cell(
    config: WorldStreamingConfig,
    origin: WorldOrigin,
    position: WorldPosition,
) -> CellCoord {
    match config.floating_origin_threshold_cells {
        Some(threshold)
            if origin.space != Some(position.space)
                || origin.cell.chebyshev_distance(position.cell) > threshold =>
        {
            position.cell
        }
        Some(_) => origin.cell,
        None => CellCoord::ZERO,
    }
}

fn clear_streamed_pages(
    commands: &mut Commands,
    terrain_meshes: &mut Assets<Mesh>,
    terrain_materials: &mut Assets<TerrainMaterial>,
    terrain_images: &mut Assets<Image>,
    stream: &mut WorldStream,
) {
    for (_, state) in stream.pages.drain() {
        match state {
            PageState::Resident(attachment) | PageState::Cooling { attachment, .. } => {
                despawn_attachment(
                    commands,
                    terrain_meshes,
                    terrain_materials,
                    terrain_images,
                    attachment,
                );
            }
            _ => {}
        }
    }
    stream.decode_tasks.clear();
    stream.desired.clear();
    stream.priorities.clear();
    stream.demand_error = None;
    stream.descriptors.clear();
    stream.index_windows = None;
    stream.index_revision = stream.index_revision.wrapping_add(1).max(1);
    stream.requested_index = None;
}

fn request_cell_index(
    source_view: Res<source_demand::SourceView>,
    worker: Option<Res<WorldDatabaseWorker>>,
    active_space: Res<ActiveWorldSpace>,
    viewpoint: Res<WorldViewpoint>,
    mut stream: ResMut<WorldStream>,
) {
    if !matches!(stream.phase, StreamPhase::Ready) || stream.requested_index.is_some() {
        return;
    }
    let Some(manifest) = stream.manifest.as_ref() else {
        return;
    };
    let Some(space_id) = active_space.current else {
        return;
    };
    let Some(space) = manifest.world_space(space_id) else {
        stream.phase = StreamPhase::Failed(format!(
            "active world space {:?} is missing from the runtime manifest",
            space_id
        ));
        return;
    };
    let Some(position) = viewpoint
        .position
        .filter(|position| position.space == space_id)
    else {
        return;
    };
    let generation = manifest.generation_id.clone();
    let windows = match source_demand::windows(position, space, &source_view, stream.height_only) {
        Ok(windows) => {
            stream.demand_error = None;
            windows
        }
        Err(error) => {
            stream.demand_error = Some(error);
            return;
        }
    };
    if stream.index_windows.as_ref() == Some(&windows) {
        return;
    }
    let Some(worker) = worker else {
        return;
    };
    let revision = stream.index_revision.wrapping_add(1).max(1);
    let request = DatabaseRequest::ReadIndex {
        generation,
        revision,
        space: space_id,
        windows: windows.clone(),
    };
    match worker.requests.try_send(request) {
        Ok(()) => {
            stream.index_windows = Some(windows);
            stream.requested_index = Some((revision, space_id));
        }
        Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => {
            stream.phase = StreamPhase::Failed("database request channel closed".into());
        }
    }
}

fn calculate_page_demand(
    source_view: Res<source_demand::SourceView>,
    worker: Option<Res<WorldDatabaseWorker>>,
    config: Res<WorldStreamingConfig>,
    detail_demand: Res<WorldDetailDemand>,
    active_space: Res<ActiveWorldSpace>,
    origin: Res<WorldOrigin>,
    camera: Query<(&Camera, &Frustum), With<WorldViewCamera>>,
    viewpoint: Res<WorldViewpoint>,
    mut stream: ResMut<WorldStream>,
) {
    if !matches!(stream.phase, StreamPhase::Ready) {
        return;
    }
    let Some(manifest) = stream.manifest.as_ref() else {
        return;
    };
    let Some(space_id) = active_space.current else {
        return;
    };
    let Some(space) = manifest.world_space(space_id) else {
        return;
    };
    let cell_size = space.cell_size;
    let generation = manifest.generation_id.clone();
    let Some(position) = viewpoint
        .position
        .filter(|position| position.space == space_id)
    else {
        return;
    };
    if stream.demand_error.is_some() {
        return;
    }
    let priorities = source_demand::demand(
        &stream.descriptors,
        position,
        cell_size,
        origin.cell,
        camera.iter().find(|(c, _)| c.is_active).map(|(_, f)| f),
        detail_demand.enabled(),
        config.gameplay_pages,
        stream.height_only,
        &source_view,
    );
    stream.desired = priorities.keys().copied().collect();
    stream.priorities = priorities;

    let Some(worker) = worker else {
        return;
    };
    // Bound all fetched, decoding and waiting-to-attach work, not just the worker's
    // channel. Otherwise a full resident budget accumulates an unbounded backlog.
    let in_flight = stream
        .pages
        .values()
        .filter(|p| {
            matches!(
                p,
                PageState::Loading { .. } | PageState::Decoding { .. } | PageState::Prepared(_)
            )
        })
        .count();
    let mut missing: Vec<_> = stream
        .priorities
        .iter()
        .filter(|(key, _)| !stream.pages.contains_key(key))
        .map(|(&k, &p)| (k, p))
        .collect();
    missing.sort_by(source_demand::compare);
    for (key, _) in missing
        .into_iter()
        .take(MAX_DATABASE_REQUESTS_IN_FLIGHT.saturating_sub(in_flight))
    {
        let request_id = stream.next_request_id.wrapping_add(1).max(1);
        match worker.requests.try_send(DatabaseRequest::ReadPage {
            generation: generation.clone(),
            request_id,
            key,
            height_only: stream.height_only && key.domain == PageDomain::TerrainRender,
        }) {
            Ok(()) => {
                stream.next_request_id = request_id;
                stream.pages.insert(key, PageState::Loading { request_id });
            }
            Err(TrySendError::Full(_)) => break,
            Err(TrySendError::Disconnected(_)) => {
                stream.phase = StreamPhase::Failed("database request channel closed".into());
                break;
            }
        }
    }
}

fn cell_intersects_frustum(
    frustum: &Frustum,
    descriptor: &CellDescriptor,
    origin_cell: CellCoord,
    cell_size: f32,
) -> bool {
    let center = [
        (i64::from(descriptor.cell.x) - i64::from(origin_cell.x)) as f32 * cell_size
            + cell_size * 0.5,
        (i64::from(descriptor.cell.z) - i64::from(origin_cell.z)) as f32 * cell_size
            + cell_size * 0.5,
    ];
    let vertical_extent = ((descriptor.maximum_y - descriptor.minimum_y) * 0.5).max(0.1);
    let aabb = Aabb {
        center: Vec3A::new(
            center[0],
            (descriptor.minimum_y + descriptor.maximum_y) * 0.5,
            center[1],
        ),
        half_extents: Vec3A::new(cell_size * 0.5, vertical_extent, cell_size * 0.5),
    };
    frustum.intersects_obb_identity(&aabb)
}

fn receive_decode_results(mut stream: ResMut<WorldStream>) {
    let mut completed = Vec::new();
    for (index, decode) in stream.decode_tasks.iter_mut().enumerate() {
        if let Some(result) = check_ready(&mut decode.task) {
            completed.push((index, decode.request_id, decode.key, result));
        }
    }
    for (index, request_id, key, result) in completed.into_iter().rev() {
        stream.decode_tasks.swap_remove(index);
        let request_is_current = matches!(
            stream.pages.get(&key),
            Some(PageState::Decoding { request_id: current }) if *current == request_id
        );
        if !request_is_current {
            continue;
        }
        if !stream.desired.contains(&key) {
            stream.pages.remove(&key);
            continue;
        }
        match result {
            Ok(prepared) => {
                for definition in &prepared.definitions {
                    stream
                        .definition_cache
                        .insert(definition.id, definition.clone());
                }
                stream.pages.insert(key, PageState::Prepared(prepared));
            }
            Err(error) => {
                stream.pages.insert(key, PageState::Failed(error));
            }
        }
    }
}

fn attach_prepared_pages(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    render_assets: Option<Res<WorldRenderAssets>>,
    stats: Res<StreamingStats>,
    mut terrain_meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut terrain_images: ResMut<Assets<Image>>,
    macro_variation: Res<TerrainMacroVariation>,
    origin: Res<WorldOrigin>,
    mut stream: ResMut<WorldStream>,
) {
    stream.admission_blocked = 0;
    let Some(render_assets) = render_assets else {
        return;
    };
    let vegetation_catalog = stream
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.vegetation_catalog.clone());
    let mut keys: Vec<_> = stream
        .pages
        .iter()
        .filter_map(|(key, state)| matches!(state, PageState::Prepared(_)).then_some(*key))
        .collect();
    keys.sort_by(|a, b| {
        source_demand::compare(
            &(*a, stream.priorities.get(a).copied().unwrap_or((3, 0.))),
            &(*b, stream.priorities.get(b).copied().unwrap_or((3, 0.))),
        )
    });
    keys.truncate(MAX_ATTACHMENTS_PER_FRAME);
    let mut admitted_decoded_bytes = stats.decoded_bytes;
    let mut admitted_gpu_bytes = stats.gpu_bytes_estimate;
    let mut admitted_terrain_texture_sets = stats.terrain_texture_sets.clone();

    for key in keys {
        let Some(cell_size) = stream
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.world_space(key.space))
            .map(|space| space.cell_size)
        else {
            stream.pages.insert(
                key,
                PageState::Failed("page references an unknown world space".into()),
            );
            continue;
        };
        let Some(PageState::Prepared(prepared)) = stream.pages.remove(&key) else {
            continue;
        };
        if !stream.desired.contains(&key) {
            continue;
        }
        let page_decoded_bytes = prepared.decoded.decoded_bytes;
        let page_gpu_bytes = prepared.decoded.gpu_bytes_estimate
            + prepared
                .dependencies
                .iter()
                .map(|dependency| dependency.gpu_bytes_estimate)
                .sum::<u64>()
            + prepared
                .terrain
                .as_ref()
                .filter(|_| !prepared.height_only)
                .map_or(0, |terrain| {
                    if admitted_terrain_texture_sets.contains(&terrain.texture_set.id) {
                        0
                    } else {
                        terrain.texture_set.runtime_gpu_bytes()
                    }
                });
        if page_decoded_bytes > MAX_RESIDENT_DECODED_BYTES
            || page_gpu_bytes > MAX_RESIDENT_GPU_BYTES_ESTIMATE
        {
            stream.pages.insert(
                key,
                PageState::Failed(format!(
                    "page exceeds the development residency profile: {} decoded bytes, {} estimated GPU bytes",
                    page_decoded_bytes, page_gpu_bytes
                )),
            );
            continue;
        }
        if admitted_decoded_bytes.saturating_add(page_decoded_bytes) > MAX_RESIDENT_DECODED_BYTES
            || admitted_gpu_bytes.saturating_add(page_gpu_bytes) > MAX_RESIDENT_GPU_BYTES_ESTIMATE
        {
            stream.admission_blocked += 1;
            stream.pages.insert(key, PageState::Prepared(prepared));
            continue;
        }
        match attach_page(
            &mut commands,
            &asset_server,
            &render_assets,
            vegetation_catalog.as_ref(),
            &mut terrain_meshes,
            &mut terrain_materials,
            &mut terrain_images,
            *macro_variation,
            origin.cell,
            cell_size,
            prepared,
        ) {
            Ok(attachment) => {
                admitted_decoded_bytes =
                    admitted_decoded_bytes.saturating_add(attachment.decoded_bytes);
                admitted_gpu_bytes =
                    admitted_gpu_bytes.saturating_add(attachment.gpu_bytes_estimate);
                if let Some((texture_set, gpu_bytes)) = attachment.terrain_texture_set
                    && admitted_terrain_texture_sets.insert(texture_set)
                {
                    admitted_gpu_bytes = admitted_gpu_bytes.saturating_add(gpu_bytes);
                }
                stream.pages.insert(key, PageState::Resident(attachment));
            }
            Err(error) => {
                stream.pages.insert(key, PageState::Failed(error));
            }
        }
    }
}

fn attach_page(
    commands: &mut Commands,
    asset_server: &AssetServer,
    render_assets: &WorldRenderAssets,
    vegetation_catalog: Option<&VegetationCatalog>,
    _terrain_meshes: &mut Assets<Mesh>,
    terrain_materials: &mut Assets<TerrainMaterial>,
    terrain_images: &mut Assets<Image>,
    macro_variation: TerrainMacroVariation,
    origin_cell: CellCoord,
    cell_size: f32,
    prepared: PreparedPage,
) -> Result<PageAttachment, String> {
    if prepared.height_only {
        return source_demand::attach_height_source(commands, prepared, cell_size);
    }
    let key = prepared.decoded.key;
    let mut entities = Vec::new();
    #[cfg(target_os = "ios")]
    let owned_terrain_meshes: Vec<Handle<Mesh>> = Vec::new();
    #[cfg(not(target_os = "ios"))]
    let mut owned_terrain_meshes = Vec::new();
    let mut owned_terrain_materials = Vec::new();
    let mut owned_terrain_images = Vec::new();
    let mut gameplay_objects = 0;
    let mut vegetation_pages = 0;
    let mut terrain_texture_set = None;
    match prepared.decoded.payload {
        PagePayload::TerrainRender(terrain) => {
            let resources = prepared
                .terrain
                .as_ref()
                .ok_or_else(|| "terrain page has no fetched render resources".to_owned())?;
            if resources.profile.space != key.space
                || resources.profile.texture_set != resources.texture_set.id
            {
                return Err("terrain page render resources are inconsistent".into());
            }
            terrain_texture_set = Some((
                resources.texture_set.id,
                resources.texture_set.runtime_gpu_bytes(),
            ));
            let surface_lookup: HashMap<_, _> = resources
                .surfaces
                .iter()
                .map(|runtime| (runtime.surface.id, runtime))
                .collect();
            let surface_layers = terrain
                .surfaces
                .iter()
                .map(|surface| {
                    let runtime = surface_lookup.get(surface).ok_or_else(|| {
                        format!("terrain page has unresolved surface {:?}", surface)
                    })?;
                    Ok(TerrainSurfaceLayer {
                        surface: runtime.surface.clone(),
                        layer: runtime.layer,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let center = [
                (i64::from(key.cell.x) - i64::from(origin_cell.x)) as f32 * cell_size
                    + cell_size * 0.5,
                (i64::from(key.cell.z) - i64::from(origin_cell.z)) as f32 * cell_size
                    + cell_size * 0.5,
            ];
            let prepared_material = prepare_terrain_material(PrepareTerrainMaterialContext {
                asset_server,
                images: terrain_images,
                materials: terrain_materials,
                cell: key.cell,
                origin_cell,
                cell_size,
                page_surfaces: &terrain.surfaces,
                weight_pages: &terrain.weight_pages,
                profile: &resources.profile,
                texture_set: &resources.texture_set,
                surfaces: &surface_layers,
                macro_variation,
            })?;
            let entity = commands
                .spawn((
                    Mesh3d(render_assets.unit_plane.clone()),
                    MeshMaterial3d(prepared_material.material.clone()),
                    Transform::from_xyz(center[0], terrain.height, center[1])
                        .with_scale(Vec3::new(cell_size, 1.0, cell_size)),
                    StreamedTerrainSurface {
                        key,
                        cell_size,
                        heightfield: TerrainHeightfield::from_heights(
                            2,
                            &[terrain.height; 4],
                            terrain.height,
                            terrain.height,
                            cell_size,
                        )
                        .map_err(|e| e.to_string())?,
                    },
                    StreamedPageEntity(key),
                    Name::new(format!("Terrain cell {}, {}", key.cell.x, key.cell.z)),
                ))
                .id();
            entities.push(entity);
            owned_terrain_materials.push(prepared_material.material);
            owned_terrain_images.push(prepared_material.weight_image);
        }
        PagePayload::TerrainHeightfield(terrain) => {
            terrain
                .heightfield
                .validate()
                .map_err(|error| error.to_string())?;
            let resources = prepared
                .terrain
                .as_ref()
                .ok_or_else(|| "terrain page has no fetched render resources".to_owned())?;
            if resources.profile.space != key.space
                || resources.profile.texture_set != resources.texture_set.id
            {
                return Err("terrain page render resources are inconsistent".into());
            }
            terrain_texture_set = Some((
                resources.texture_set.id,
                resources.texture_set.runtime_gpu_bytes(),
            ));
            let surface_lookup: HashMap<_, _> = resources
                .surfaces
                .iter()
                .map(|runtime| (runtime.surface.id, runtime))
                .collect();
            let surface_layers = terrain
                .surfaces
                .iter()
                .map(|surface| {
                    let runtime = surface_lookup.get(surface).ok_or_else(|| {
                        format!("terrain page has unresolved surface {:?}", surface)
                    })?;
                    Ok(TerrainSurfaceLayer {
                        surface: runtime.surface.clone(),
                        layer: runtime.layer,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let center = [
                (i64::from(key.cell.x) - i64::from(origin_cell.x)) as f32 * cell_size
                    + cell_size * 0.5,
                (i64::from(key.cell.z) - i64::from(origin_cell.z)) as f32 * cell_size
                    + cell_size * 0.5,
            ];
            let prepared_material = prepare_terrain_material(PrepareTerrainMaterialContext {
                asset_server,
                images: terrain_images,
                materials: terrain_materials,
                cell: key.cell,
                origin_cell,
                cell_size,
                page_surfaces: &terrain.surfaces,
                weight_pages: &terrain.weight_pages,
                profile: &resources.profile,
                texture_set: &resources.texture_set,
                surfaces: &surface_layers,
                macro_variation,
            })?;
            #[cfg(target_os = "ios")]
            let (mesh, transform, terrain_name) = (
                render_assets.unit_plane.clone(),
                Transform::from_xyz(center[0], 0.0, center[1])
                    .with_scale(Vec3::new(cell_size, 1.0, cell_size)),
                format!("Flat terrain cell {}, {}", key.cell.x, key.cell.z),
            );
            #[cfg(not(target_os = "ios"))]
            let (mesh, transform, terrain_name) = {
                let mesh =
                    _terrain_meshes.add(build_heightfield_mesh(&terrain.heightfield, cell_size)?);
                (
                    mesh,
                    Transform::from_xyz(center[0], 0.0, center[1]),
                    format!("Relief terrain cell {}, {}", key.cell.x, key.cell.z),
                )
            };
            #[cfg(target_os = "ios")]
            let streamed_heightfield =
                TerrainHeightfield::from_heights(2, &[0.0; 4], 0.0, 0.0, cell_size)
                    .map_err(|error| error.to_string())?;
            #[cfg(not(target_os = "ios"))]
            let streamed_heightfield = terrain.heightfield;
            let entity = commands
                .spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(prepared_material.material.clone()),
                    transform,
                    StreamedTerrainSurface {
                        key,
                        cell_size,
                        heightfield: streamed_heightfield,
                    },
                    StreamedPageEntity(key),
                    Name::new(terrain_name),
                ))
                .id();
            entities.push(entity);
            #[cfg(not(target_os = "ios"))]
            owned_terrain_meshes.push(mesh);
            owned_terrain_materials.push(prepared_material.material);
            owned_terrain_images.push(prepared_material.weight_image);
        }
        PagePayload::StaticObjects(objects) => {
            let mut dependencies: HashMap<AssetId, Vec<&PageDependency>> = HashMap::new();
            for dependency in &prepared.dependencies {
                dependencies
                    .entry(dependency.asset)
                    .or_default()
                    .push(dependency);
            }
            for variants in dependencies.values_mut() {
                variants.sort_by_key(|variant| variant.asset_lod);
            }
            let cell_origin = [
                (f64::from(key.cell.x) - f64::from(origin_cell.x)) * f64::from(cell_size),
                (f64::from(key.cell.z) - f64::from(origin_cell.z)) * f64::from(cell_size),
            ];
            for instance in objects.instances {
                let dependencies = dependencies.get(&instance.asset).ok_or_else(|| {
                    format!("object {:?} has no cooked asset dependency", instance.id)
                })?;
                if dependencies.is_empty() {
                    return Err(format!("asset {:?} has no LOD variants", instance.asset));
                }
                let mut previous_minimum = f32::INFINITY;
                for dependency in dependencies {
                    if dependency.kind != "gltf-scene" {
                        return Err(format!(
                            "asset {:?} has unsupported kind {}",
                            dependency.asset, dependency.kind
                        ));
                    }
                    if dependency.minimum_screen_height > previous_minimum {
                        return Err(format!(
                            "asset {:?} LOD thresholds are not descending",
                            dependency.asset
                        ));
                    }
                    previous_minimum = dependency.minimum_screen_height;
                }
                let translation = Vec3::new(
                    cell_origin[0] as f32 + instance.translation[0],
                    instance.translation[1],
                    cell_origin[1] as f32 + instance.translation[2],
                );
                let variants: Vec<_> = dependencies
                    .iter()
                    .map(|dependency| ScreenSpaceLodVariant {
                        lod: dependency.asset_lod,
                        scene: asset_server
                            .load(GltfAssetLabel::Scene(0).from_asset(dependency.uri.clone())),
                        minimum_screen_height: dependency.minimum_screen_height,
                    })
                    .collect();
                let bounds_height = dependencies
                    .iter()
                    .map(|dependency| dependency.bounds[1])
                    .fold(0.0_f32, f32::max);
                let initial_lod = variants.len() - 1;
                let entity = commands
                    .spawn((
                        WorldAssetRoot(variants[initial_lod].scene.clone()),
                        Transform::from_translation(translation)
                            .with_rotation(Quat::from_rotation_y(instance.yaw))
                            .with_scale(Vec3::splat(instance.scale)),
                        ScreenSpaceLod {
                            variants,
                            current: initial_lod,
                            bounds_height,
                            projected_height: 0.0,
                        },
                        StreamedVisualObject { id: instance.id },
                        StreamedPageEntity(key),
                        Name::new(format!("Streamed object {:?}", instance.id)),
                    ))
                    .id();
                if instance.generated {
                    commands.entity(entity).insert(GeneratedEnvironmentObject {
                        space: key.space,
                        cell: key.cell,
                    });
                }
                entities.push(entity);
            }
        }
        PagePayload::Vegetation(data) => {
            let catalog = vegetation_catalog
                .ok_or_else(|| "vegetation page has no generation catalog".to_owned())?;
            data.validate(catalog).map_err(|error| error.to_string())?;
            let entity = commands
                .spawn((
                    StreamedVegetationFieldPage {
                        key,
                        cell_size,
                        data,
                    },
                    StreamedPageEntity(key),
                    Name::new(format!("Vegetation fields {}, {}", key.cell.x, key.cell.z)),
                ))
                .id();
            entities.push(entity);
            vegetation_pages = 1;
        }
        PagePayload::ShadowCasters(_) => {
            return Err("shadow-caster page attachment is not enabled in the first slice".into());
        }
        PagePayload::GameplayObjects(objects) => {
            let definitions: HashMap<_, _> = prepared
                .definitions
                .iter()
                .map(|definition| (definition.id, definition))
                .collect();
            let cell_origin = [
                (f64::from(key.cell.x) - f64::from(origin_cell.x)) * f64::from(cell_size),
                (f64::from(key.cell.z) - f64::from(origin_cell.z)) * f64::from(cell_size),
            ];
            for instance in objects.instances {
                let definition = definitions.get(&instance.definition).ok_or_else(|| {
                    format!(
                        "gameplay object {:?} has no fetched definition {:?}",
                        instance.id, instance.definition
                    )
                })?;
                if definition.activation != ObjectActivationPolicy::Proximity {
                    return Err(format!(
                        "gameplay page contains render-only definition {}",
                        definition.key
                    ));
                }
                let translation = Vec3::new(
                    cell_origin[0] as f32 + instance.translation[0],
                    instance.translation[1],
                    cell_origin[1] as f32 + instance.translation[2],
                );
                let entity = commands
                    .spawn((
                        Transform::from_translation(translation)
                            .with_rotation(Quat::from_rotation_y(instance.yaw))
                            .with_scale(Vec3::splat(instance.scale)),
                        GameplayObject {
                            id: instance.id,
                            definition: instance.definition,
                        },
                        StreamedPageEntity(key),
                        Name::new(definition.display_name.clone()),
                    ))
                    .id();
                entities.push(entity);
                gameplay_objects += 1;
            }
        }
    }

    Ok(PageAttachment {
        entities,
        owned_terrain_meshes,
        owned_terrain_materials,
        owned_terrain_images,
        decoded_bytes: prepared.decoded.decoded_bytes,
        gpu_bytes_estimate: prepared.decoded.gpu_bytes_estimate
            + prepared
                .dependencies
                .iter()
                .map(|dependency| dependency.gpu_bytes_estimate)
                .sum::<u64>(),
        gameplay_objects,
        vegetation_pages,
        height_only_pages: 0,
        terrain_texture_set,
    })
}

fn update_screen_space_lods(
    camera: Single<(&Camera, &GlobalTransform), With<WorldViewCamera>>,
    mut objects: Query<(&GlobalTransform, &mut WorldAssetRoot, &mut ScreenSpaceLod)>,
) {
    let (camera, camera_transform) = *camera;
    let mut switches = 0;
    for (transform, mut scene_root, mut screen_lod) in &mut objects {
        let (scale, _, translation) = transform.to_scale_rotation_translation();
        let bottom = translation;
        let top = translation + Vec3::Y * screen_lod.bounds_height * scale.y.abs();
        let (Ok(bottom), Ok(top)) = (
            camera.world_to_viewport(camera_transform, bottom),
            camera.world_to_viewport(camera_transform, top),
        ) else {
            continue;
        };
        let projected_height = bottom.distance(top);
        if !projected_height.is_finite() {
            continue;
        }
        screen_lod.projected_height = projected_height;

        let current = screen_lod.current;
        let target = select_lod_index(
            screen_lod.variants.len(),
            current,
            projected_height,
            |index| screen_lod.variants[index].minimum_screen_height,
        );
        if target == current {
            continue;
        }
        if switches >= MAX_LOD_SWITCHES_PER_FRAME {
            continue;
        }

        scene_root.0 = screen_lod.variants[target].scene.clone();
        screen_lod.current = target;
        switches += 1;
    }
}

fn select_lod_index(
    variant_count: usize,
    current: usize,
    projected_height: f32,
    minimum_screen_height: impl Fn(usize) -> f32,
) -> usize {
    debug_assert!(variant_count > 0 && current < variant_count);
    let raw_target = (0..variant_count)
        .find(|index| projected_height >= minimum_screen_height(*index))
        .unwrap_or(variant_count - 1);
    if raw_target > current {
        let downgrade_below = minimum_screen_height(current) * (1.0 - LOD_HYSTERESIS_FRACTION);
        if projected_height >= downgrade_below {
            current
        } else {
            raw_target
        }
    } else if raw_target < current {
        let upgrade_above = minimum_screen_height(raw_target) * (1.0 + LOD_HYSTERESIS_FRACTION);
        if projected_height <= upgrade_above {
            current
        } else {
            raw_target
        }
    } else {
        current
    }
}

/// Marks cooked generated objects so authoring can replace only the derived cell output.
#[derive(Component)]
pub struct GeneratedEnvironmentObject {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
}

/// Editor world previews share the runtime object's screen-space LOD selection.
pub fn spawn_collection_visual(
    commands: &mut Commands,
    server: &AssetServer,
    transform: Transform,
    asset: &world_db::CollectionAssetView,
) -> Entity {
    let variants: Vec<_> = asset
        .variants
        .iter()
        .map(|v| ScreenSpaceLodVariant {
            lod: v.lod,
            scene: server.load(GltfAssetLabel::Scene(0).from_asset(v.uri.clone())),
            minimum_screen_height: v.minimum_screen_height,
        })
        .collect();
    let current = variants.len() - 1;
    commands
        .spawn((
            WorldAssetRoot(variants[current].scene.clone()),
            transform,
            ScreenSpaceLod {
                variants,
                current,
                bounds_height: asset
                    .variants
                    .iter()
                    .map(|v| v.bounds[1])
                    .fold(0.0_f32, f32::max),
                projected_height: 0.0,
            },
            Name::new(format!("Generated {}", asset.name)),
        ))
        .id()
}

#[derive(Component, Debug, Clone, Copy)]
pub struct StreamedVisualObject {
    pub id: StableObjectId,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct GameplayObject {
    pub id: StableObjectId,
    pub definition: ObjectDefinitionId,
}

#[derive(Component)]
struct StreamedPageEntity(PageKey);

fn despawn_attachment(
    commands: &mut Commands,
    terrain_meshes: &mut Assets<Mesh>,
    terrain_materials: &mut Assets<TerrainMaterial>,
    terrain_images: &mut Assets<Image>,
    attachment: PageAttachment,
) {
    for entity in attachment.entities {
        commands.entity(entity).despawn();
    }
    for mesh in attachment.owned_terrain_meshes {
        terrain_meshes.remove(mesh.id());
    }
    for material in attachment.owned_terrain_materials {
        terrain_materials.remove(material.id());
    }
    for image in attachment.owned_terrain_images {
        terrain_images.remove(image.id());
    }
}

fn cool_and_remove_pages(
    mut commands: Commands,
    time: Res<Time>,
    mut terrain_meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut terrain_images: ResMut<Assets<Image>>,
    mut stream: ResMut<WorldStream>,
) {
    let now = time.elapsed();
    let keys: Vec<_> = stream.pages.keys().copied().collect();
    for key in keys {
        let demanded = stream.desired.contains(&key);
        let Some(state) = stream.pages.remove(&key) else {
            continue;
        };
        match state {
            PageState::Resident(attachment) if !demanded => {
                stream.pages.insert(
                    key,
                    PageState::Cooling {
                        attachment,
                        remove_at: now + Duration::from_secs_f32(COOLING_SECONDS),
                    },
                );
            }
            PageState::Cooling { attachment, .. } if demanded => {
                stream.pages.insert(key, PageState::Resident(attachment));
            }
            PageState::Cooling {
                attachment,
                remove_at,
            } if now >= remove_at => {
                despawn_attachment(
                    &mut commands,
                    &mut terrain_meshes,
                    &mut terrain_materials,
                    &mut terrain_images,
                    attachment,
                );
            }
            PageState::Prepared(_) if !demanded => {}
            other => {
                stream.pages.insert(key, other);
            }
        }
    }
}

#[derive(Resource, Default)]
pub struct StreamingStats {
    pub status: String,
    pub demanded: usize,
    pub loading: usize,
    pub prepared: usize,
    pub resident: usize,
    pub cooling: usize,
    pub failed: usize,
    pub owned_entities: usize,
    pub decoded_bytes: u64,
    pub gpu_bytes_estimate: u64,
    pub cached_definitions: usize,
    pub gameplay_objects: usize,
    pub vegetation_pages: usize,
    pub height_only_pages: usize,
    pub indexed_cells: usize,
    pub source_demand_error: Option<String>,
    pub pending_decoded_bytes: u64,
    pub budget_waiting: usize,
    pub lod_counts: BTreeMap<u8, usize>,
    pub minimum_projected_height: f32,
    pub maximum_projected_height: f32,
    terrain_texture_sets: BTreeSet<TerrainTextureSetId>,
}

fn update_streaming_stats(
    stream: Res<WorldStream>,
    active_space: Res<ActiveWorldSpace>,
    lod_objects: Query<&ScreenSpaceLod>,
    mut stats: ResMut<StreamingStats>,
    reload: Res<WorldGenerationReload>,
) {
    stats.status = match &stream.phase {
        StreamPhase::Opening => "opening SQLite".into(),
        StreamPhase::Ready => stream.manifest.as_ref().map_or_else(
            || "ready".into(),
            |manifest| {
                let space = active_space
                    .current
                    .and_then(|id| manifest.world_space(id))
                    .map(|space| format!("{} ({})", space.name, space.id.0))
                    .unwrap_or_else(|| "no active space".into());
                format!("{} | generation {}", space, manifest.generation_id)
            },
        ),
        StreamPhase::Failed(error) => format!("failed: {error}"),
    };
    stats.indexed_cells = stream.descriptors.len();
    if reload.active() {
        stats.status.push_str(if reload.commit_requested {
            " | committing published generation"
        } else {
            " | preparing published generation"
        });
    }
    if let Some(error) = &reload.last_error {
        stats
            .status
            .push_str(&format!(" | publication adoption failed: {error}"));
    }
    if active_space.requested.is_some() {
        stats.status.push_str(" | preparing world entry");
    }
    if let Some(error) = &active_space.transition_error {
        stats
            .status
            .push_str(&format!(" | entry rejected: {error}"));
    }
    stats.source_demand_error = stream.demand_error.clone();
    stats.budget_waiting = stream.admission_blocked;
    if stream.admission_blocked > 0 {
        stats.status.push_str(" | source residency budget full");
    }
    if let Some(error) = &stream.demand_error {
        stats.status = format!("source demand limited: {error}");
    }
    stats.pending_decoded_bytes = 0;
    stats.height_only_pages = 0;
    stats.demanded = stream.desired.len();
    stats.loading = 0;
    stats.prepared = 0;
    stats.resident = 0;
    stats.cooling = 0;
    stats.failed = 0;
    stats.owned_entities = 0;
    stats.decoded_bytes = 0;
    stats.gpu_bytes_estimate = 0;
    stats.cached_definitions = stream.definition_cache.len();
    stats.gameplay_objects = 0;
    stats.vegetation_pages = 0;
    stats.terrain_texture_sets.clear();
    stats.lod_counts.clear();
    stats.minimum_projected_height = f32::INFINITY;
    stats.maximum_projected_height = 0.0;
    for lod in &lod_objects {
        *stats
            .lod_counts
            .entry(lod.variants[lod.current].lod)
            .or_default() += 1;
        stats.minimum_projected_height = stats.minimum_projected_height.min(lod.projected_height);
        stats.maximum_projected_height = stats.maximum_projected_height.max(lod.projected_height);
    }
    if stats.lod_counts.is_empty() {
        stats.minimum_projected_height = 0.0;
    }
    for state in stream.pages.values() {
        match state {
            PageState::Loading { .. } | PageState::Decoding { .. } => stats.loading += 1,
            PageState::Prepared(p) => {
                stats.prepared += 1;
                stats.pending_decoded_bytes += p.decoded.decoded_bytes;
            }
            PageState::Resident(attachment) => {
                stats.resident += 1;
                stats.owned_entities += attachment.entities.len();
                stats.decoded_bytes += attachment.decoded_bytes;
                stats.gpu_bytes_estimate += attachment.gpu_bytes_estimate;
                stats.gameplay_objects += attachment.gameplay_objects;
                stats.vegetation_pages += attachment.vegetation_pages;
                stats.height_only_pages += attachment.height_only_pages;
                account_terrain_texture_set(&mut stats, attachment);
            }
            PageState::Cooling { attachment, .. } => {
                stats.cooling += 1;
                stats.owned_entities += attachment.entities.len();
                stats.decoded_bytes += attachment.decoded_bytes;
                stats.gpu_bytes_estimate += attachment.gpu_bytes_estimate;
                stats.gameplay_objects += attachment.gameplay_objects;
                stats.vegetation_pages += attachment.vegetation_pages;
                stats.height_only_pages += attachment.height_only_pages;
                account_terrain_texture_set(&mut stats, attachment);
            }
            PageState::Failed(error) => {
                let _ = error;
                stats.failed += 1;
            }
        }
    }
}

fn account_terrain_texture_set(stats: &mut StreamingStats, attachment: &PageAttachment) {
    let Some((texture_set, gpu_bytes)) = attachment.terrain_texture_set else {
        return;
    };
    if stats.terrain_texture_sets.insert(texture_set) {
        stats.gpu_bytes_estimate = stats.gpu_bytes_estimate.saturating_add(gpu_bytes);
    }
}

fn report_streaming_smoke(
    stats: Res<StreamingStats>,
    time: Res<Time>,
    stream: Res<WorldStream>,
    asset_server: Res<AssetServer>,
    mut active_space: ResMut<ActiveWorldSpace>,
    mut object: Single<&mut Transform, With<WorldStreamFocus>>,
    character: Query<(), (With<WorldStreamFocus>, With<CharacterPresentationReady>)>,
    streamed_entities: Query<&StreamedPageEntity>,
    lod_objects: Query<&ScreenSpaceLod>,
    mut smoke: Local<StreamingSmokeState>,
    mut app_exit: MessageWriter<AppExit>,
) {
    if !smoke.initialized {
        smoke.initialized = true;
        smoke.enabled = std::env::args().any(|argument| argument == "--streaming-smoke");
    }
    if !smoke.enabled {
        return;
    }
    if smoke.stage == 0 && time.elapsed_secs() >= 3.0 {
        assert_streaming_is_healthy(&stats);
        assert!(
            !character.is_empty(),
            "the controlled character scene or Idle animation did not become ready"
        );
        assert!(
            !lod_objects.is_empty(),
            "the smoke-test camera did not stream any LOD object"
        );
        for lod_object in &lod_objects {
            for variant in &lod_object.variants {
                assert!(
                    asset_server.is_loaded_with_dependencies(&variant.scene),
                    "LOD{} and its dependencies did not finish loading",
                    variant.lod
                );
            }
        }
        assert_eq!(
            stats.gameplay_objects, 0,
            "distant gameplay objects were activated in the overworld"
        );
        assert_eq!(
            stats.cached_definitions, 0,
            "the distant interior definition was fetched before entering its proximity set"
        );
        println!(
            "YARRA_STREAMING_SMOKE initial status={:?} demanded={} resident={} failed={} \
             owned_entities={} decoded_bytes={} gpu_bytes_estimate={} lods={:?}",
            stats.status,
            stats.demanded,
            stats.resident,
            stats.failed,
            stats.owned_entities,
            stats.decoded_bytes,
            stats.gpu_bytes_estimate,
            stats.lod_counts,
        );
        let cell_size = active_space
            .current
            .and_then(|id| stream.manifest.as_ref()?.world_space(id))
            .map(|space| space.cell_size)
            .expect("smoke test has no active world-space record");
        object.translation.x += 5.0 * cell_size;
        smoke.stage = 1;
        return;
    }
    // Leave a small integration-frame margin beyond the two-second cooling
    // deadline. Stats are sampled after bounded page attachment/removal and
    // can otherwise report the just-expired cooling set for one final frame.
    if smoke.stage == 1 && time.elapsed_secs() >= 7.5 {
        assert_streaming_is_healthy(&stats);
        assert_eq!(
            stats.cooling, 0,
            "old pages remained in cooling after the removal deadline"
        );
        assert_eq!(
            streamed_entities.iter().count(),
            stats.owned_entities,
            "tracked page ownership does not match live streamed root entities"
        );
        let current_space = active_space
            .current
            .expect("smoke test has no active world space");
        let manifest = stream
            .manifest
            .as_ref()
            .expect("smoke test has no runtime manifest");
        let cell_size = manifest
            .world_space(current_space)
            .expect("active smoke-test space is absent from the manifest")
            .cell_size;
        let current_cell = CellCoord::containing(
            f64::from(object.translation.x),
            f64::from(object.translation.z),
            cell_size,
        );
        for entity in &streamed_entities {
            assert_eq!(
                entity.0.space, current_space,
                "an entity from another world space survived traversal"
            );
            assert!(
                entity.0.cell.chebyshev_distance(current_cell) <= INDEX_RADIUS_CELLS as u32,
                "an entity from old cell {:?} survived traversal to {:?}",
                entity.0.cell,
                current_cell
            );
        }
        println!(
            "YARRA_STREAMING_SMOKE traversal passed demanded={} resident={} cooling={} \
             owned_entities={} failed={}",
            stats.demanded, stats.resident, stats.cooling, stats.owned_entities, stats.failed,
        );
        let next_space = manifest
            .world_spaces
            .iter()
            .find(|space| space.id != current_space)
            .expect("multi-world smoke test requires a second world space")
            .id;
        active_space.request(next_space, [0.0, 0.0, 0.0]);
        smoke.expected_space = Some(next_space);
        smoke.stage = 2;
        return;
    }
    if smoke.stage == 2 && time.elapsed_secs() >= 11.0 {
        assert_streaming_is_healthy(&stats);
        let expected_space = smoke
            .expected_space
            .expect("multi-world smoke test has no expected destination");
        assert_eq!(
            active_space.current,
            Some(expected_space),
            "world-space transition did not activate its destination"
        );
        assert_eq!(
            stats.cooling, 0,
            "old world-space pages remained in the cooling set"
        );
        assert_eq!(
            stats.gameplay_objects, 1,
            "the nearby interior gameplay object was not activated"
        );
        assert_eq!(
            stats.cached_definitions, 1,
            "the nearby gameplay definition was not fetched exactly once"
        );
        assert_eq!(
            streamed_entities.iter().count(),
            stats.owned_entities,
            "tracked page ownership does not match live streamed root entities"
        );
        for entity in &streamed_entities {
            assert_eq!(
                entity.0.space, expected_space,
                "an entity from the previous world space survived the transition"
            );
        }
        println!(
            "YARRA_STREAMING_SMOKE multi-world passed active_space={} demanded={} resident={} \
             cooling={} owned_entities={} gameplay_objects={} definitions_cached={} failed={}",
            expected_space.0,
            stats.demanded,
            stats.resident,
            stats.cooling,
            stats.owned_entities,
            stats.gameplay_objects,
            stats.cached_definitions,
            stats.failed,
        );
        smoke.stage = 3;
        app_exit.write(AppExit::Success);
    }
}

#[derive(Default)]
struct StreamingSmokeState {
    initialized: bool,
    enabled: bool,
    stage: u8,
    expected_space: Option<WorldSpaceId>,
}

fn assert_streaming_is_healthy(stats: &StreamingStats) {
    assert!(
        !stats.status.starts_with("failed:"),
        "runtime database worker failed: {}",
        stats.status
    );
    assert!(stats.demanded > 0, "streaming produced no demanded pages");
    assert!(stats.resident > 0, "streaming produced no resident pages");
    assert_eq!(stats.failed, 0, "one or more streamed pages failed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_terrain_sampling_converts_from_rebased_render_space() {
        let space = WorldSpaceId(7);
        let cell = CellCoord { x: 10, z: -4 };
        let origin = WorldOrigin {
            space: Some(space),
            cell,
        };
        let heightfield =
            TerrainHeightfield::from_heights(2, &[2.5, 2.5, 2.5, 2.5], 2.5, 2.5, 32.0).unwrap();
        let surface = StreamedTerrainSurface {
            key: PageKey {
                space,
                cell,
                domain: PageDomain::TerrainRender,
                lod: 0,
            },
            cell_size: 32.0,
            heightfield,
        };

        let sample = sample_resident_terrain_surface(&origin, [&surface], [16.0, 16.0]).unwrap();
        assert_eq!(sample.height, 2.5);
        assert_eq!(sample.normal, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn editor_origin_rebases_only_after_its_threshold() {
        let origin = WorldOrigin {
            space: Some(WorldSpaceId(1)),
            cell: CellCoord { x: 10, z: 20 },
        };
        let nearby = WorldPosition {
            space: WorldSpaceId(1),
            cell: CellCoord { x: 18, z: 12 },
            local: [0.0; 3],
        };
        let remote = WorldPosition {
            cell: CellCoord { x: 19, z: 12 },
            ..nearby
        };

        assert_eq!(
            desired_origin_cell(WorldStreamingConfig::editor(), origin, nearby),
            origin.cell
        );
        assert_eq!(
            desired_origin_cell(WorldStreamingConfig::editor(), origin, remote),
            remote.cell
        );
        assert_eq!(
            desired_origin_cell(WorldStreamingConfig::game(), origin, remote),
            CellCoord::ZERO
        );
    }

    #[test]
    fn screen_space_lod_selection_has_hysteresis_in_both_directions() {
        let minimums = [320.0, 160.0, 80.0, 0.0];
        let select = |current, height| {
            select_lod_index(minimums.len(), current, height, |index| minimums[index])
        };

        assert_eq!(select(0, 300.0), 0);
        assert_eq!(select(0, 280.0), 1);
        assert_eq!(select(1, 350.0), 1);
        assert_eq!(select(1, 360.0), 0);
        assert_eq!(select(3, 85.0), 3);
        assert_eq!(select(3, 90.0), 2);
        assert_eq!(select(2, 60.0), 3);
    }

    #[test]
    fn generation_reload_is_an_exact_single_flight_handshake() {
        let mut reload = WorldGenerationReload::default();
        assert!(!reload.request(""));
        assert!(reload.request("generation-a"));
        assert!(reload.active());
        assert!(!reload.request("generation-b"));

        let queued = reload.queued.take().unwrap();
        reload.in_flight = Some(queued);
        reload.in_flight = None;
        reload.completion = Some(Ok("generation-a".into()));
        assert_eq!(reload.take_completion(), Some(Ok("generation-a".into())));
        assert!(!reload.active());
        assert!(reload.request("generation-b"));
    }

    #[test]
    fn database_worker_reopens_the_exact_published_generation() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets")
            .join(world::DEFAULT_RUNTIME_DATABASE);
        let (requests, request_receiver) = bounded(MAX_DATABASE_REQUESTS_IN_FLIGHT);
        let (results, result_receiver) = bounded(MAX_DATABASE_REQUESTS_IN_FLIGHT * 2);
        let worker = thread::spawn(move || database_worker(path, request_receiver, results));

        let DatabaseResult::Opened(Ok(manifest)) = result_receiver.recv().unwrap() else {
            panic!("runtime worker did not open the current cooked world");
        };
        let expected = manifest.generation_id;
        requests
            .send(DatabaseRequest::Reload {
                request_id: 4,
                expected_generation: "not-the-published-generation".into(),
            })
            .unwrap();
        let DatabaseResult::Reloaded {
            request_id: 4,
            result: Err(error),
        } = result_receiver.recv().unwrap()
        else {
            panic!("runtime worker accepted the wrong generation identity");
        };
        assert!(error.contains("generation mismatch"));

        requests
            .send(DatabaseRequest::Reload {
                request_id: 5,
                expected_generation: expected.clone(),
            })
            .unwrap();
        let DatabaseResult::Reloaded {
            request_id,
            result: Ok(reloaded),
        } = result_receiver.recv().unwrap()
        else {
            panic!("runtime worker did not reopen the published generation");
        };
        assert_eq!(request_id, 5);
        assert_eq!(reloaded.generation_id, expected);

        requests.send(DatabaseRequest::Shutdown).unwrap();
        worker.join().unwrap();
    }
}
