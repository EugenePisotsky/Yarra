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
use ground_cover::{GroundCoverPage3d, GroundCoverPageAsset};
use terrain_render::{
    PrepareTerrainMaterialContext, TerrainMacroVariation, TerrainMaterial, TerrainSurfaceLayer,
    prepare_terrain_material,
};
use world::{
    AssetId, CellCoord, GroundCoverSpecies, ObjectActivationPolicy, ObjectDefinitionId, PageDomain,
    PageKey, PagePayload, StableObjectId, TerrainTextureSetId, WorldSpaceId,
};
use world_db::{
    CellDescriptor, DecodedPage, EncodedPage, PageDependency, RuntimeManifest,
    RuntimeObjectDefinition, RuntimeReader, TerrainRenderResources,
};

use crate::{MainCamera, MovableObject, MovementTarget, TargetIndicator};

const INDEX_RADIUS_CELLS: i32 = 3;
const PLAYER_PRELOAD_RADIUS_CELLS: u32 = 1;
const COOLING_SECONDS: f32 = 2.0;
const MAX_DATABASE_REQUESTS_IN_FLIGHT: usize = 16;
const MAX_ATTACHMENTS_PER_FRAME: usize = 2;
const MAX_LOD_SWITCHES_PER_FRAME: usize = 32;
const MAX_RESIDENT_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RESIDENT_GPU_BYTES_ESTIMATE: u64 = 256 * 1024 * 1024;
const LOD_HYSTERESIS_FRACTION: f32 = 0.12;

pub(crate) struct WorldStreamingPlugin {
    database_path: PathBuf,
}

impl WorldStreamingPlugin {
    pub(crate) fn new(database_path: PathBuf) -> Self {
        Self { database_path }
    }
}

impl Plugin for WorldStreamingPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(WorldDatabasePath(self.database_path.clone()))
            .init_resource::<WorldStream>()
            .init_resource::<ActiveWorldSpace>()
            .init_resource::<StreamingStats>()
            .add_systems(Startup, (start_database_worker, create_world_render_assets))
            .add_systems(
                Update,
                (
                    receive_database_results,
                    request_world_space_from_keyboard,
                    apply_world_space_transition,
                    request_cell_index,
                    calculate_page_demand,
                    receive_decode_results,
                    attach_prepared_pages,
                    cool_and_remove_pages,
                    update_streaming_stats,
                    report_streaming_smoke,
                )
                    .chain(),
            )
            .add_systems(
                PostUpdate,
                update_screen_space_lods.after(TransformSystems::Propagate),
            );
    }
}

#[derive(Resource)]
struct WorldDatabasePath(PathBuf);

#[derive(Resource, Debug, Default)]
pub struct ActiveWorldSpace {
    current: Option<WorldSpaceId>,
    requested: Option<WorldSpaceTransition>,
}

impl ActiveWorldSpace {
    pub fn current(&self) -> Option<WorldSpaceId> {
        self.current
    }

    pub fn request(&mut self, space: WorldSpaceId, local_position: [f32; 3]) {
        self.requested = Some(WorldSpaceTransition {
            space,
            local_position,
        });
    }
}

#[derive(Debug, Clone, Copy)]
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
    ReadIndex {
        revision: u64,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
    },
    ReadPage {
        request_id: u64,
        key: PageKey,
    },
    Shutdown,
}

#[derive(Debug)]
enum DatabaseResult {
    Opened(Result<RuntimeManifest, String>),
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
    ground_cover_species: Vec<GroundCoverSpecies>,
    terrain: Option<TerrainRenderResources>,
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
    let reader = match RuntimeReader::open_immutable(&path) {
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

    while let Ok(request) = requests.recv() {
        match request {
            DatabaseRequest::ReadIndex {
                revision,
                space,
                minimum,
                maximum,
            } => {
                let result = reader
                    .read_cell_descriptors(space, minimum, maximum)
                    .map_err(|error| error.to_string());
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
            DatabaseRequest::ReadPage { request_id, key } => {
                let result = reader
                    .read_page(key)
                    .and_then(|page| {
                        page.map(|page| {
                            let dependencies = reader.read_dependencies(key)?;
                            let definitions = if key.domain == PageDomain::GameplayObjects {
                                reader.read_object_definitions(key)?
                            } else {
                                Vec::new()
                            };
                            let ground_cover_species = if key.domain == PageDomain::GroundCover {
                                reader.read_ground_cover_species(key)?
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
                                ground_cover_species,
                                terrain,
                            })
                        })
                        .transpose()
                    })
                    .map_err(|error| error.to_string());
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
    index_center: Option<CellCoord>,
    index_revision: u64,
    requested_index: Option<(u64, WorldSpaceId)>,
    descriptors: Vec<CellDescriptor>,
    desired: BTreeSet<PageKey>,
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
    ground_cover_species: Vec<GroundCoverSpecies>,
    terrain: Option<TerrainRenderResources>,
}

struct DecodeTask {
    request_id: u64,
    key: PageKey,
    task: Task<Result<PreparedPage, String>>,
}

struct PageAttachment {
    entities: Vec<Entity>,
    owned_terrain_materials: Vec<Handle<TerrainMaterial>>,
    owned_terrain_images: Vec<Handle<Image>>,
    owned_ground_cover_pages: Vec<Handle<GroundCoverPageAsset>>,
    decoded_bytes: u64,
    gpu_bytes_estimate: u64,
    gameplay_objects: usize,
    ground_cover_clusters: usize,
    terrain_texture_set: Option<(TerrainTextureSetId, u64)>,
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

fn receive_database_results(
    worker: Option<Res<WorldDatabaseWorker>>,
    mut active_space: ResMut<ActiveWorldSpace>,
    mut stream: ResMut<WorldStream>,
) {
    let Some(worker) = worker else {
        return;
    };
    loop {
        match worker.results.try_recv() {
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
                    stream.manifest = Some(manifest);
                    stream.phase = StreamPhase::Ready;
                }
                Err(error) => {
                    error!("{error}");
                    stream.phase = StreamPhase::Failed(error);
                }
            },
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
                                .map(|decoded| PreparedPage {
                                    decoded,
                                    dependencies: fetched.dependencies,
                                    definitions: fetched.definitions,
                                    ground_cover_species: fetched.ground_cover_species,
                                    terrain: fetched.terrain,
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
                if !matches!(stream.phase, StreamPhase::Failed(_)) {
                    stream.phase = StreamPhase::Failed("database worker stopped".into());
                }
                break;
            }
        }
    }
}

fn request_world_space_from_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    stream: Res<WorldStream>,
    mut active_space: ResMut<ActiveWorldSpace>,
) {
    if !keys.just_pressed(KeyCode::Tab) {
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
    active_space.request(next.id, [0.0, 0.5, 0.0]);
}

fn apply_world_space_transition(
    mut commands: Commands,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut terrain_images: ResMut<Assets<Image>>,
    mut ground_cover_pages: ResMut<Assets<GroundCoverPageAsset>>,
    mut active_space: ResMut<ActiveWorldSpace>,
    mut stream: ResMut<WorldStream>,
    mut object: Single<(&mut Transform, &mut MovementTarget), With<MovableObject>>,
    mut indicator: Single<&mut Visibility, With<TargetIndicator>>,
) {
    let Some(transition) = active_space.requested.take() else {
        return;
    };
    let Some(space_name) = stream
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.world_space(transition.space))
        .map(|space| space.name.clone())
    else {
        error!(
            "ignored transition to unknown world space {:?}",
            transition.space
        );
        return;
    };

    if active_space.current != Some(transition.space) {
        for (_, state) in stream.pages.drain() {
            match state {
                PageState::Resident(attachment) | PageState::Cooling { attachment, .. } => {
                    despawn_attachment(
                        &mut commands,
                        &mut terrain_materials,
                        &mut terrain_images,
                        &mut ground_cover_pages,
                        attachment,
                    );
                }
                _ => {}
            }
        }
        stream.decode_tasks.clear();
        stream.desired.clear();
        stream.descriptors.clear();
        stream.index_center = None;
        stream.index_revision = stream.index_revision.wrapping_add(1).max(1);
        stream.requested_index = None;
    }

    active_space.current = Some(transition.space);
    object.0.translation = Vec3::from_array(transition.local_position);
    object.1.0 = None;
    **indicator = Visibility::Hidden;
    info!(
        "entered world space {} ({:?}) at {:?}",
        space_name, transition.space, transition.local_position
    );
}

fn request_cell_index(
    worker: Option<Res<WorldDatabaseWorker>>,
    active_space: Res<ActiveWorldSpace>,
    object: Single<&Transform, With<MovableObject>>,
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
    let center = CellCoord::containing(
        f64::from(object.translation.x),
        f64::from(object.translation.z),
        space.cell_size,
    );
    if stream.index_center == Some(center) {
        return;
    }
    let Some(worker) = worker else {
        return;
    };
    let revision = stream.index_revision.wrapping_add(1).max(1);
    let request = DatabaseRequest::ReadIndex {
        revision,
        space: space_id,
        minimum: CellCoord {
            x: center.x - INDEX_RADIUS_CELLS,
            z: center.z - INDEX_RADIUS_CELLS,
        },
        maximum: CellCoord {
            x: center.x + INDEX_RADIUS_CELLS,
            z: center.z + INDEX_RADIUS_CELLS,
        },
    };
    match worker.requests.try_send(request) {
        Ok(()) => {
            stream.index_center = Some(center);
            stream.requested_index = Some((revision, space_id));
        }
        Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => {
            stream.phase = StreamPhase::Failed("database request channel closed".into());
        }
    }
}

fn calculate_page_demand(
    worker: Option<Res<WorldDatabaseWorker>>,
    active_space: Res<ActiveWorldSpace>,
    camera: Single<&Frustum, With<MainCamera>>,
    object: Single<&Transform, With<MovableObject>>,
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
    let player_cell = CellCoord::containing(
        f64::from(object.translation.x),
        f64::from(object.translation.z),
        cell_size,
    );
    let mut desired = BTreeSet::new();
    for descriptor in &stream.descriptors {
        let visible = cell_intersects_frustum(&camera, descriptor, cell_size);
        let preloaded =
            descriptor.cell.chebyshev_distance(player_cell) <= PLAYER_PRELOAD_RADIUS_CELLS;
        if !visible && !preloaded {
            continue;
        }
        if descriptor.has_domain(PageDomain::TerrainRender) {
            desired.insert(PageKey {
                space: space_id,
                cell: descriptor.cell,
                domain: PageDomain::TerrainRender,
                lod: 0,
            });
        }
        if visible && descriptor.has_domain(PageDomain::StaticObjects) {
            desired.insert(PageKey {
                space: space_id,
                cell: descriptor.cell,
                domain: PageDomain::StaticObjects,
                lod: 0,
            });
        }
        if preloaded && descriptor.has_domain(PageDomain::GameplayObjects) {
            desired.insert(PageKey {
                space: space_id,
                cell: descriptor.cell,
                domain: PageDomain::GameplayObjects,
                lod: 0,
            });
        }
        if (visible || preloaded) && descriptor.has_domain(PageDomain::GroundCover) {
            desired.insert(PageKey {
                space: space_id,
                cell: descriptor.cell,
                domain: PageDomain::GroundCover,
                lod: 0,
            });
        }
    }
    stream.desired = desired;

    let Some(worker) = worker else {
        return;
    };
    let missing: Vec<_> = stream
        .desired
        .iter()
        .filter(|key| !stream.pages.contains_key(key))
        .copied()
        .collect();
    for key in missing {
        let request_id = stream.next_request_id.wrapping_add(1).max(1);
        match worker
            .requests
            .try_send(DatabaseRequest::ReadPage { request_id, key })
        {
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

fn cell_intersects_frustum(frustum: &Frustum, descriptor: &CellDescriptor, cell_size: f32) -> bool {
    let center = descriptor.cell.center(cell_size);
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
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut terrain_images: ResMut<Assets<Image>>,
    macro_variation: Res<TerrainMacroVariation>,
    mut ground_cover_pages: ResMut<Assets<GroundCoverPageAsset>>,
    mut stream: ResMut<WorldStream>,
) {
    let Some(render_assets) = render_assets else {
        return;
    };
    let mut keys: Vec<_> = stream
        .pages
        .iter()
        .filter_map(|(key, state)| matches!(state, PageState::Prepared(_)).then_some(*key))
        .collect();
    keys.sort();
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
            + prepared.terrain.as_ref().map_or(0, |terrain| {
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
            stream.pages.insert(key, PageState::Prepared(prepared));
            continue;
        }
        match attach_page(
            &mut commands,
            &asset_server,
            &render_assets,
            &mut terrain_materials,
            &mut terrain_images,
            &mut ground_cover_pages,
            *macro_variation,
            cell_size,
            prepared,
        ) {
            Ok(attachment) => {
                admitted_decoded_bytes =
                    admitted_decoded_bytes.saturating_add(attachment.decoded_bytes);
                admitted_gpu_bytes =
                    admitted_gpu_bytes.saturating_add(attachment.gpu_bytes_estimate);
                if let Some((texture_set, gpu_bytes)) = attachment.terrain_texture_set {
                    if admitted_terrain_texture_sets.insert(texture_set) {
                        admitted_gpu_bytes = admitted_gpu_bytes.saturating_add(gpu_bytes);
                    }
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
    terrain_materials: &mut Assets<TerrainMaterial>,
    terrain_images: &mut Assets<Image>,
    ground_cover_pages: &mut Assets<GroundCoverPageAsset>,
    macro_variation: TerrainMacroVariation,
    cell_size: f32,
    prepared: PreparedPage,
) -> Result<PageAttachment, String> {
    let key = prepared.decoded.key;
    let mut entities = Vec::new();
    let mut owned_terrain_materials = Vec::new();
    let mut owned_terrain_images = Vec::new();
    let mut owned_ground_cover_pages = Vec::new();
    let mut gameplay_objects = 0;
    let mut ground_cover_clusters = 0;
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
            let center = key.cell.center(cell_size);
            let prepared_material = prepare_terrain_material(PrepareTerrainMaterialContext {
                asset_server,
                images: terrain_images,
                materials: terrain_materials,
                cell: key.cell,
                cell_size,
                page: &terrain,
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
                    StreamedPageEntity(key),
                    Name::new(format!("Terrain cell {}, {}", key.cell.x, key.cell.z)),
                ))
                .id();
            entities.push(entity);
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
            let cell_origin = key.cell.origin(cell_size);
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
                        StreamedPageEntity(key),
                        Name::new(format!("Streamed object {:?}", instance.id)),
                    ))
                    .id();
                entities.push(entity);
            }
        }
        PagePayload::GroundCover(page) => {
            let species: HashMap<_, _> = prepared
                .ground_cover_species
                .iter()
                .map(|species| (species.id, species))
                .collect();
            for cluster in &page.clusters {
                if !species.contains_key(&cluster.species) {
                    return Err(format!(
                        "ground-cover cluster references unresolved species {:?}",
                        cluster.species
                    ));
                }
            }
            ground_cover_clusters = page.clusters.len();
            let asset = ground_cover_pages.add(GroundCoverPageAsset {
                key,
                cell_size,
                page,
                species: prepared.ground_cover_species,
            });
            let entity = commands
                .spawn((
                    GroundCoverPage3d(asset.clone()),
                    StreamedPageEntity(key),
                    Name::new(format!("Ground cover cell {}, {}", key.cell.x, key.cell.z)),
                ))
                .id();
            entities.push(entity);
            owned_ground_cover_pages.push(asset);
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
            let cell_origin = key.cell.origin(cell_size);
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
        owned_terrain_materials,
        owned_terrain_images,
        owned_ground_cover_pages,
        decoded_bytes: prepared.decoded.decoded_bytes,
        gpu_bytes_estimate: prepared.decoded.gpu_bytes_estimate
            + prepared
                .dependencies
                .iter()
                .map(|dependency| dependency.gpu_bytes_estimate)
                .sum::<u64>(),
        gameplay_objects,
        ground_cover_clusters,
        terrain_texture_set,
    })
}

fn update_screen_space_lods(
    camera: Single<(&Camera, &GlobalTransform), With<MainCamera>>,
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

#[derive(Component, Debug, Clone, Copy)]
pub struct GameplayObject {
    pub id: StableObjectId,
    pub definition: ObjectDefinitionId,
}

#[derive(Component)]
struct StreamedPageEntity(PageKey);

fn despawn_attachment(
    commands: &mut Commands,
    terrain_materials: &mut Assets<TerrainMaterial>,
    terrain_images: &mut Assets<Image>,
    ground_cover_pages: &mut Assets<GroundCoverPageAsset>,
    attachment: PageAttachment,
) {
    for entity in attachment.entities {
        commands.entity(entity).despawn();
    }
    for material in attachment.owned_terrain_materials {
        terrain_materials.remove(material.id());
    }
    for image in attachment.owned_terrain_images {
        terrain_images.remove(image.id());
    }
    for page in attachment.owned_ground_cover_pages {
        ground_cover_pages.remove(page.id());
    }
}

fn cool_and_remove_pages(
    mut commands: Commands,
    time: Res<Time>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut terrain_images: ResMut<Assets<Image>>,
    mut ground_cover_pages: ResMut<Assets<GroundCoverPageAsset>>,
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
                    &mut terrain_materials,
                    &mut terrain_images,
                    &mut ground_cover_pages,
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
pub(crate) struct StreamingStats {
    pub(crate) status: String,
    pub(crate) demanded: usize,
    pub(crate) loading: usize,
    pub(crate) prepared: usize,
    pub(crate) resident: usize,
    pub(crate) cooling: usize,
    pub(crate) failed: usize,
    pub(crate) owned_entities: usize,
    pub(crate) decoded_bytes: u64,
    pub(crate) gpu_bytes_estimate: u64,
    pub(crate) cached_definitions: usize,
    pub(crate) gameplay_objects: usize,
    pub(crate) ground_cover_clusters: usize,
    pub(crate) lod_counts: BTreeMap<u8, usize>,
    pub(crate) minimum_projected_height: f32,
    pub(crate) maximum_projected_height: f32,
    terrain_texture_sets: BTreeSet<TerrainTextureSetId>,
}

fn update_streaming_stats(
    stream: Res<WorldStream>,
    active_space: Res<ActiveWorldSpace>,
    lod_objects: Query<&ScreenSpaceLod>,
    mut stats: ResMut<StreamingStats>,
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
    stats.ground_cover_clusters = 0;
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
            PageState::Prepared(_) => stats.prepared += 1,
            PageState::Resident(attachment) => {
                stats.resident += 1;
                stats.owned_entities += attachment.entities.len();
                stats.decoded_bytes += attachment.decoded_bytes;
                stats.gpu_bytes_estimate += attachment.gpu_bytes_estimate;
                stats.gameplay_objects += attachment.gameplay_objects;
                stats.ground_cover_clusters += attachment.ground_cover_clusters;
                account_terrain_texture_set(&mut stats, attachment);
            }
            PageState::Cooling { attachment, .. } => {
                stats.cooling += 1;
                stats.owned_entities += attachment.entities.len();
                stats.decoded_bytes += attachment.decoded_bytes;
                stats.gpu_bytes_estimate += attachment.gpu_bytes_estimate;
                stats.gameplay_objects += attachment.gameplay_objects;
                stats.ground_cover_clusters += attachment.ground_cover_clusters;
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
    mut object: Single<&mut Transform, With<MovableObject>>,
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
        assert!(
            stats.ground_cover_clusters > 0,
            "the overworld did not stream any ground-cover clusters"
        );
        println!(
            "YARRA_STREAMING_SMOKE initial status={:?} demanded={} resident={} failed={} \
             owned_entities={} ground_cover_clusters={} decoded_bytes={} gpu_bytes_estimate={} lods={:?}",
            stats.status,
            stats.demanded,
            stats.resident,
            stats.failed,
            stats.owned_entities,
            stats.ground_cover_clusters,
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
    if smoke.stage == 1 && time.elapsed_secs() >= 7.0 {
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
        active_space.request(next_space, [0.0, 0.5, 0.0]);
        smoke.expected_space = Some(next_space);
        smoke.stage = 2;
        return;
    }
    if smoke.stage == 2 && time.elapsed_secs() >= 10.0 {
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
}
