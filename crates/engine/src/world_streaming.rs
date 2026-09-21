mod database;
use database::{DatabaseRequest, DatabaseResult, WorldDatabaseWorker};
mod generation;
mod residency;
use residency::SourceResidency;
pub use residency::StreamingStats;
#[cfg(test)]
use residency::{MAX_PENDING_SOURCE_PAGES, PageState, attachment::PageAttachment};
mod rebase;
mod smoke;
mod source_demand;
pub use crate::object_lod::{
    GeneratedEnvironmentObject, StreamedVisualObject, VisualLodScale, spawn_collection_visual,
};
pub use rebase::WorldRenderRoot;
pub use smoke::StreamingSmokePlugin;
pub(crate) mod terrain_lod;
pub use terrain_lod::{
    LiveTerrainPreview, TerrainContactReadiness, TerrainLodPreview, TerrainLodStats,
    TerrainPreviewRequest,
};

use std::path::PathBuf;

use bevy::{
    camera::primitives::{Aabb, Frustum},
    prelude::*,
    transform::TransformSystems,
};
use crossbeam_channel::{TryRecvError, TrySendError};
use terrain_render::TerrainMaterial;
use vegetation::{VegetationCatalog, VegetationFieldPageData};
use world::{
    CellCoord, ObjectDefinitionId, PageDomain, PageKey, StableObjectId, TerrainHeightfield,
    WorldPosition, WorldSpaceId,
};
use world_db::{CellDescriptor, RuntimeManifest};

use crate::actor::{CharacterMotion, CharacterMotor, MoveIntent, WorldStreamFocus};

const INDEX_RADIUS_CELLS: i32 = 3;
// Local objects/tools and the legacy diagnostic retain their existing cell window.
// The terrain hierarchy's camera source radius is separate and measured in metres.
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
            .init_resource::<ActiveWorldSpace>()
            .init_resource::<WorldCatalog>()
            .init_resource::<WorldGenerationReload>()
            .init_resource::<WorldViewpoint>()
            .init_resource::<crate::WorldStartView>()
            .init_resource::<WorldOrigin>()
            .init_resource::<WorldDetailDemand>()
            .init_resource::<source_demand::SourceView>()
            .init_resource::<StreamingStats>()
            .init_resource::<WorldDebugControls>()
            .add_plugins(crate::object_lod::ObjectLodPlugin)
            .add_systems(Startup, residency::attachment::create_world_render_assets)
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
                    residency::receive_decode_results,
                    residency::attach_prepared_pages,
                    residency::cool_and_remove_pages,
                    residency::update_streaming_stats,
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
    pub atmosphere: world::atmosphere::AtmosphereProfile,
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

#[derive(Resource, Default)]
struct WorldStream {
    phase: StreamPhase,
    manifest: Option<RuntimeManifest>,
    index_windows: Option<Vec<source_demand::Window>>,
    height_only: bool,
    demand_error: Option<String>,
    index_revision: u64,
    requested_index: Option<(u64, WorldSpaceId)>,
    descriptors: Vec<CellDescriptor>,
}

#[derive(Default)]
enum StreamPhase {
    #[default]
    Opening,
    Ready,
    Failed(String),
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
    mut residency: ResMut<SourceResidency>,
    mut reload: ResMut<WorldGenerationReload>,
) {
    let Some(worker) = worker else {
        return;
    };
    loop {
        match worker.try_recv() {
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
                            atmosphere: space.atmosphere.clone(),
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
                residency.receive_page(request_id, key, result);
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
            atmosphere: space.atmosphere.clone(),
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
    debug: Res<WorldDebugControls>,
    keys: Res<ButtonInput<KeyCode>>,
    config: Res<WorldStreamingConfig>,
    stream: Res<WorldStream>,
    mut active_space: ResMut<ActiveWorldSpace>,
) {
    if !config.keyboard_world_space_cycle || !debug.world_switch || !keys.just_pressed(KeyCode::Tab)
    {
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
    mut residency: ResMut<SourceResidency>,
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
            &mut residency,
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
    mut residency: ResMut<SourceResidency>,
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
            &mut residency,
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
    residency: &mut SourceResidency,
) {
    residency.clear(commands, terrain_meshes, terrain_materials, terrain_images);
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
    match worker.try_send(request) {
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
    mut residency: ResMut<SourceResidency>,
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
    residency.set_demand(priorities);

    let Some(worker) = worker else {
        return;
    };
    if let Err(error) = residency.request_missing(&worker, &generation, stream.height_only) {
        stream.phase = StreamPhase::Failed(error);
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
            }],
            vegetation,
        },
        WorldOrigin {
            space: Some(space),
            cell,
        },
    )
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
}

fn sync_world_atmosphere(
    catalog: Res<WorldCatalog>,
    active: Res<ActiveWorldSpace>,
    mut atmosphere: Option<ResMut<crate::AtmosphereState>>,
    mut initialized: Local<bool>,
) {
    let Some(state) = atmosphere.as_mut() else {
        return;
    };
    if state.owner != crate::AtmosphereOwner::Game {
        return;
    }
    let Some(space) = active.current.and_then(|id| catalog.world_space(id)) else {
        return;
    };
    if !*initialized {
        state.phase = space.atmosphere.initial_phase;
        *initialized = true;
    }
    if state.profile != space.atmosphere {
        state.profile = space.atmosphere.clone();
    }
}

#[cfg(test)]
mod atmosphere_tests {
    use super::*;
    #[test]
    fn world_changes_and_publication_keep_time_and_editor_ownership() {
        let mut app = App::new();
        let first = world::atmosphere::AtmosphereProfile::default();
        let second = world::atmosphere::AtmosphereProfile {
            initial_phase: 0.1,
            outdoor: false,
            ..first.clone()
        };
        app.insert_resource(WorldCatalog {
            world_spaces: vec![
                WorldSpaceInfo {
                    id: WorldSpaceId(1),
                    name: "outdoor".into(),
                    cell_size: 32.0,
                    minimum_y: 0.0,
                    maximum_y: 1.0,
                    atmosphere: first.clone(),
                },
                WorldSpaceInfo {
                    id: WorldSpaceId(2),
                    name: "inside".into(),
                    cell_size: 32.0,
                    minimum_y: 0.0,
                    maximum_y: 1.0,
                    atmosphere: second,
                },
            ],
            ..default()
        })
        .init_resource::<ActiveWorldSpace>()
        .init_resource::<crate::AtmosphereState>()
        .add_systems(Update, sync_world_atmosphere);
        app.world_mut().resource_mut::<ActiveWorldSpace>().current = Some(WorldSpaceId(1));
        app.update();
        assert_eq!(
            app.world().resource::<crate::AtmosphereState>().phase,
            first.initial_phase
        );
        app.world_mut()
            .resource_mut::<crate::AtmosphereState>()
            .phase = 0.7;
        app.world_mut().resource_mut::<ActiveWorldSpace>().current = Some(WorldSpaceId(2));
        app.update();
        let state = app.world().resource::<crate::AtmosphereState>();
        assert_eq!(state.phase, 0.7);
        assert!(!state.profile.outdoor);
        app.world_mut()
            .resource_mut::<crate::AtmosphereState>()
            .owner = crate::AtmosphereOwner::Editor;
        app.world_mut().resource_mut::<ActiveWorldSpace>().current = Some(WorldSpaceId(1));
        app.update();
        assert!(
            !app.world()
                .resource::<crate::AtmosphereState>()
                .profile
                .outdoor
        );
    }
}
