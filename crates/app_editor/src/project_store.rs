//! Bounded, asynchronous access to the mutable project database.

use std::{
    path::PathBuf,
    thread::{self, JoinHandle},
};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
use engine::{WorldCatalog, WorldViewpoint};
use world::{CellCoord, StableObjectId, WorldSpaceId};
use world_db::{
    DenseSourceRecord, DenseSourceRecordKey, DenseSourceWrite, DenseSourceWriteTransactionResult,
    ObjectWriteTransactionResult, ProjectManifest, ProjectReader, ProjectWriter, SourceCellRecord,
    SourceGroundCoverCellMaskRecord, SourceGroundCoverLayerRecord, SourceObjectRecord,
    SourceObjectViewRecord, SourceObjectWrite, SourceObjectWriteCommit,
    SourceTerrainCellWeightPageRecord,
};

use crate::preview::{EditorPreviewMode, PreviewModeState};
use crate::tools::{EditorSourceDomain, EditorToolRegistry};
use crate::workspaces::EditorWorkspace;

const SOURCE_RADIUS_CELLS: i32 = 2;
const MAX_SOURCE_CELLS: usize = 25;
const MAX_SOURCE_OBJECTS: usize = 2_048;
const MAX_SOURCE_TERRAIN_WEIGHT_PAGES: usize = MAX_SOURCE_CELLS * 2;
const MAX_SOURCE_GROUND_COVER_LAYERS: usize = 64;
const MAX_SOURCE_GROUND_COVER_MASKS: usize = 512;
const PROJECT_REQUEST_CAPACITY: usize = 2;

pub(crate) struct ProjectEditorStorePlugin {
    database_path: PathBuf,
}

impl ProjectEditorStorePlugin {
    pub(crate) fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
        }
    }
}

impl Plugin for ProjectEditorStorePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ProjectDatabasePath(self.database_path.clone()))
            .init_resource::<ProjectEditorStore>()
            .add_systems(Startup, start_project_worker)
            .add_systems(
                Update,
                (
                    receive_project_results,
                    validate_project_catalog,
                    update_project_query_demand,
                    dispatch_project_save,
                    dispatch_project_query,
                )
                    .chain(),
            );
    }
}

#[derive(Resource)]
struct ProjectDatabasePath(PathBuf);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProjectQueryWindow {
    pub(crate) space: WorldSpaceId,
    pub(crate) minimum: CellCoord,
    pub(crate) maximum: CellCoord,
}

impl ProjectQueryWindow {
    fn around(space: WorldSpaceId, center: CellCoord) -> Self {
        Self {
            space,
            minimum: CellCoord {
                x: center.x.saturating_sub(SOURCE_RADIUS_CELLS),
                z: center.z.saturating_sub(SOURCE_RADIUS_CELLS),
            },
            maximum: CellCoord {
                x: center.x.saturating_add(SOURCE_RADIUS_CELLS),
                z: center.z.saturating_add(SOURCE_RADIUS_CELLS),
            },
        }
    }
}

#[derive(Default)]
enum ProjectStorePhase {
    #[default]
    Opening,
    Ready,
    Failed(String),
}

#[derive(Debug, Clone, Copy)]
struct InFlightQuery {
    revision: u64,
    window: ProjectQueryWindow,
    domains: ProjectSourceDomains,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ProjectSourceDomains {
    cells: bool,
    objects: bool,
    terrain_weights: bool,
    ground_cover_masks: bool,
}

impl ProjectSourceDomains {
    fn from_tool(tool: crate::tools::EditorToolDescriptor) -> Self {
        Self::from_domains(tool.source_domains)
    }

    fn from_domains(domains: &[EditorSourceDomain]) -> Self {
        Self {
            cells: domains.contains(&EditorSourceDomain::CellDescriptors),
            objects: domains.contains(&EditorSourceDomain::ObjectPlacements),
            terrain_weights: domains.contains(&EditorSourceDomain::TerrainWeights),
            ground_cover_masks: domains.contains(&EditorSourceDomain::GroundCoverMask),
        }
    }

    fn any(self) -> bool {
        self.cells || self.objects || self.terrain_weights || self.ground_cover_masks
    }
}

#[derive(Debug, Clone)]
struct PendingObjectSave {
    request_id: u64,
    writes: Vec<SourceObjectWrite>,
}

#[derive(Debug, Clone)]
struct PendingDenseSave {
    request_id: u64,
    writes: Vec<DenseSourceWrite>,
}

#[derive(Debug, Clone)]
pub(crate) struct ObjectSaveCompletion {
    pub(crate) request_id: u64,
    pub(crate) outcome: ObjectSaveOutcome,
}

#[derive(Debug, Clone)]
pub(crate) enum ObjectSaveOutcome {
    Committed(Vec<SourceObjectWriteCommit>),
    Conflict {
        object: StableObjectId,
        actual: Option<SourceObjectRecord>,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) struct DenseSaveCompletion {
    pub(crate) request_id: u64,
    pub(crate) outcome: DenseSaveOutcome,
}

#[derive(Debug, Clone)]
pub(crate) enum DenseSaveOutcome {
    Committed(Vec<DenseSourceRecord>),
    Conflict {
        key: DenseSourceRecordKey,
        actual: Option<DenseSourceRecord>,
    },
    Failed(String),
}

#[derive(Resource, Default)]
pub(crate) struct ProjectEditorStore {
    phase: ProjectStorePhase,
    manifest: Option<ProjectManifest>,
    write_error: Option<String>,
    catalog_compatible: Option<bool>,
    desired_window: Option<ProjectQueryWindow>,
    desired_domains: ProjectSourceDomains,
    loaded_window: Option<ProjectQueryWindow>,
    loaded_domains: ProjectSourceDomains,
    failed_window: Option<ProjectQueryWindow>,
    failed_domains: ProjectSourceDomains,
    in_flight: Option<InFlightQuery>,
    cells: Vec<SourceCellRecord>,
    objects: Vec<SourceObjectViewRecord>,
    terrain_weight_pages: Vec<SourceTerrainCellWeightPageRecord>,
    ground_cover_layers: Vec<SourceGroundCoverLayerRecord>,
    ground_cover_masks: Vec<SourceGroundCoverCellMaskRecord>,
    cells_truncated: bool,
    objects_truncated: bool,
    terrain_weights_truncated: bool,
    ground_cover_masks_truncated: bool,
    query_error: Option<String>,
    next_revision: u64,
    source_epoch: u64,
    completed_queries: u64,
    stale_results: u64,
    pending_save: Option<PendingObjectSave>,
    save_in_flight: Option<u64>,
    save_completion: Option<ObjectSaveCompletion>,
    pending_dense_save: Option<PendingDenseSave>,
    dense_save_in_flight: Option<u64>,
    dense_save_completion: Option<DenseSaveCompletion>,
    next_save_request_id: u64,
}

impl ProjectEditorStore {
    pub(crate) fn status(&self) -> String {
        match &self.phase {
            ProjectStorePhase::Opening => "opening project SQLite".into(),
            ProjectStorePhase::Failed(error) => format!("failed: {error}"),
            ProjectStorePhase::Ready if self.catalog_compatible == Some(false) => {
                "source/runtime world-space catalogs do not match".into()
            }
            ProjectStorePhase::Ready => self
                .query_error
                .as_ref()
                .map_or_else(|| "ready".into(), |error| format!("query failed: {error}")),
        }
    }

    pub(crate) fn manifest(&self) -> Option<&ProjectManifest> {
        self.manifest.as_ref()
    }

    pub(crate) fn write_error(&self) -> Option<&str> {
        self.write_error.as_deref()
    }

    pub(crate) fn desired_window(&self) -> Option<ProjectQueryWindow> {
        self.desired_window
    }

    pub(crate) fn loaded_window(&self) -> Option<ProjectQueryWindow> {
        self.loaded_window
    }

    pub(crate) fn query_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    pub(crate) fn cells(&self) -> &[SourceCellRecord] {
        &self.cells
    }

    pub(crate) fn objects(&self) -> &[SourceObjectViewRecord] {
        &self.objects
    }

    pub(crate) fn terrain_weight_pages(&self) -> &[SourceTerrainCellWeightPageRecord] {
        &self.terrain_weight_pages
    }

    pub(crate) fn ground_cover_layers(&self) -> &[SourceGroundCoverLayerRecord] {
        &self.ground_cover_layers
    }

    pub(crate) fn ground_cover_masks(&self) -> &[SourceGroundCoverCellMaskRecord] {
        &self.ground_cover_masks
    }

    pub(crate) fn cells_truncated(&self) -> bool {
        self.cells_truncated
    }

    pub(crate) fn objects_truncated(&self) -> bool {
        self.objects_truncated
    }

    pub(crate) fn terrain_weights_truncated(&self) -> bool {
        self.terrain_weights_truncated
    }

    pub(crate) fn ground_cover_masks_truncated(&self) -> bool {
        self.ground_cover_masks_truncated
    }

    pub(crate) fn completed_queries(&self) -> u64 {
        self.completed_queries
    }

    pub(crate) fn stale_results(&self) -> u64 {
        self.stale_results
    }

    pub(crate) fn source_epoch(&self) -> u64 {
        self.source_epoch
    }

    pub(crate) fn highest_source_revision(&self) -> Option<i64> {
        self.cells
            .iter()
            .map(|cell| cell.source_revision)
            .chain(
                self.objects
                    .iter()
                    .map(|object| object.object.source_revision),
            )
            .chain(
                self.terrain_weight_pages
                    .iter()
                    .map(|record| record.source_revision),
            )
            .chain(
                self.ground_cover_masks
                    .iter()
                    .map(|record| record.source_revision),
            )
            .max()
    }

    pub(crate) fn save_in_flight(&self) -> bool {
        self.pending_save.is_some()
            || self.save_in_flight.is_some()
            || self.pending_dense_save.is_some()
            || self.dense_save_in_flight.is_some()
    }

    pub(crate) fn queue_object_transaction(
        &mut self,
        writes: Vec<SourceObjectWrite>,
    ) -> Option<u64> {
        if writes.is_empty() || self.save_in_flight() || self.write_error.is_some() {
            return None;
        }
        let request_id = self.next_save_request_id.wrapping_add(1).max(1);
        self.next_save_request_id = request_id;
        self.pending_save = Some(PendingObjectSave { request_id, writes });
        Some(request_id)
    }

    pub(crate) fn take_save_completion(&mut self) -> Option<ObjectSaveCompletion> {
        self.save_completion.take()
    }

    pub(crate) fn queue_dense_transaction(&mut self, writes: Vec<DenseSourceWrite>) -> Option<u64> {
        if writes.is_empty() || self.save_in_flight() || self.write_error.is_some() {
            return None;
        }
        let request_id = self.next_save_request_id.wrapping_add(1).max(1);
        self.next_save_request_id = request_id;
        self.pending_dense_save = Some(PendingDenseSave { request_id, writes });
        Some(request_id)
    }

    pub(crate) fn take_dense_save_completion(&mut self) -> Option<DenseSaveCompletion> {
        self.dense_save_completion.take()
    }
}

#[derive(Resource)]
struct ProjectWorker {
    requests: Sender<ProjectRequest>,
    results: Receiver<ProjectResult>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ProjectWorker {
    fn drop(&mut self) {
        let _ = self.requests.send(ProjectRequest::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

enum ProjectRequest {
    Query {
        revision: u64,
        window: ProjectQueryWindow,
        domains: ProjectSourceDomains,
    },
    SaveObjectTransaction(PendingObjectSave),
    SaveDenseTransaction(PendingDenseSave),
    Shutdown,
}

enum ProjectResult {
    Opened(Result<ProjectOpenSnapshot, String>),
    Query {
        revision: u64,
        window: ProjectQueryWindow,
        domains: ProjectSourceDomains,
        result: Result<ProjectWindowSnapshot, String>,
    },
    SaveObjectTransaction {
        request_id: u64,
        result: Result<ObjectWriteTransactionResult, String>,
    },
    SaveDenseTransaction {
        request_id: u64,
        result: Result<DenseSourceWriteTransactionResult, String>,
    },
}

struct ProjectOpenSnapshot {
    manifest: ProjectManifest,
    write_error: Option<String>,
}

struct ProjectWindowSnapshot {
    cells: Vec<SourceCellRecord>,
    objects: Vec<SourceObjectViewRecord>,
    terrain_weight_pages: Vec<SourceTerrainCellWeightPageRecord>,
    ground_cover_layers: Vec<SourceGroundCoverLayerRecord>,
    ground_cover_masks: Vec<SourceGroundCoverCellMaskRecord>,
    cells_truncated: bool,
    objects_truncated: bool,
    terrain_weights_truncated: bool,
    ground_cover_masks_truncated: bool,
}

fn start_project_worker(mut commands: Commands, path: Res<ProjectDatabasePath>) {
    let (request_sender, request_receiver) = bounded(PROJECT_REQUEST_CAPACITY);
    let (result_sender, result_receiver) = bounded(PROJECT_REQUEST_CAPACITY + 1);
    let database_path = path.0.clone();
    let worker_thread = thread::Builder::new()
        .name("yarra-project-db".into())
        .spawn(move || project_worker(database_path, request_receiver, result_sender))
        .expect("failed to spawn the project database worker");
    commands.insert_resource(ProjectWorker {
        requests: request_sender,
        results: result_receiver,
        thread: Some(worker_thread),
    });
}

fn project_worker(
    path: PathBuf,
    requests: Receiver<ProjectRequest>,
    results: Sender<ProjectResult>,
) {
    let reader = match ProjectReader::open_read_only(&path) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = results.send(ProjectResult::Opened(Err(format!(
                "could not open {}: {error}",
                path.display()
            ))));
            return;
        }
    };
    let mut writer = ProjectWriter::open(&path)
        .map_err(|error| format!("could not open {} for authoring: {error}", path.display()));
    if results
        .send(ProjectResult::Opened(Ok(ProjectOpenSnapshot {
            manifest: reader.manifest().clone(),
            write_error: writer.as_ref().err().cloned(),
        })))
        .is_err()
    {
        return;
    }

    while let Ok(request) = requests.recv() {
        match request {
            ProjectRequest::Query {
                revision,
                window,
                domains,
            } => {
                let result = (|| {
                    let cells = reader.read_cells(
                        window.space,
                        window.minimum,
                        window.maximum,
                        MAX_SOURCE_CELLS,
                    )?;
                    let objects = domains
                        .objects
                        .then(|| {
                            reader.read_object_views_in_cells(
                                window.space,
                                window.minimum,
                                window.maximum,
                                MAX_SOURCE_OBJECTS,
                            )
                        })
                        .transpose()?;
                    let terrain = domains
                        .terrain_weights
                        .then(|| {
                            reader.read_terrain_weight_pages_in_cells(
                                window.space,
                                window.minimum,
                                window.maximum,
                                MAX_SOURCE_TERRAIN_WEIGHT_PAGES,
                            )
                        })
                        .transpose()?;
                    let ground_cover_layers = if domains.ground_cover_masks {
                        reader.read_ground_cover_layers(
                            window.space,
                            MAX_SOURCE_GROUND_COVER_LAYERS,
                        )?
                    } else {
                        Vec::new()
                    };
                    let ground_cover = domains
                        .ground_cover_masks
                        .then(|| {
                            reader.read_ground_cover_masks_in_cells(
                                window.space,
                                window.minimum,
                                window.maximum,
                                MAX_SOURCE_GROUND_COVER_MASKS,
                            )
                        })
                        .transpose()?;
                    Ok::<_, world_db::WorldDbError>(ProjectWindowSnapshot {
                        cells: cells.records,
                        objects: objects
                            .as_ref()
                            .map_or_else(Vec::new, |query| query.records.clone()),
                        terrain_weight_pages: terrain
                            .as_ref()
                            .map_or_else(Vec::new, |query| query.records.clone()),
                        ground_cover_layers,
                        ground_cover_masks: ground_cover
                            .as_ref()
                            .map_or_else(Vec::new, |query| query.records.clone()),
                        cells_truncated: cells.truncated,
                        objects_truncated: objects.is_some_and(|query| query.truncated),
                        terrain_weights_truncated: terrain.is_some_and(|query| query.truncated),
                        ground_cover_masks_truncated: ground_cover
                            .is_some_and(|query| query.truncated),
                    })
                })()
                .map_err(|error| error.to_string());
                if results
                    .send(ProjectResult::Query {
                        revision,
                        window,
                        domains,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
            ProjectRequest::SaveObjectTransaction(request) => {
                let result = match writer.as_mut() {
                    Ok(writer) => writer
                        .apply_object_transaction(&request.writes)
                        .map_err(|error| error.to_string()),
                    Err(error) => Err(error.clone()),
                };
                if results
                    .send(ProjectResult::SaveObjectTransaction {
                        request_id: request.request_id,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
            ProjectRequest::SaveDenseTransaction(request) => {
                let result = match writer.as_mut() {
                    Ok(writer) => writer
                        .apply_dense_source_transaction(&request.writes)
                        .map_err(|error| error.to_string()),
                    Err(error) => Err(error.clone()),
                };
                if results
                    .send(ProjectResult::SaveDenseTransaction {
                        request_id: request.request_id,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
            ProjectRequest::Shutdown => return,
        }
    }
}

fn receive_project_results(
    worker: Option<Res<ProjectWorker>>,
    mut store: ResMut<ProjectEditorStore>,
) {
    let Some(worker) = worker else {
        return;
    };
    loop {
        match worker.results.try_recv() {
            Ok(ProjectResult::Opened(result)) => match result {
                Ok(opened) => {
                    store.manifest = Some(opened.manifest);
                    store.write_error = opened.write_error;
                    store.phase = ProjectStorePhase::Ready;
                    store.source_epoch = store.source_epoch.max(1);
                }
                Err(error) => store.phase = ProjectStorePhase::Failed(error),
            },
            Ok(ProjectResult::Query {
                revision,
                window,
                domains,
                result,
            }) => {
                if store.in_flight.is_some_and(|query| {
                    query.revision == revision && query.window == window && query.domains == domains
                }) {
                    store.in_flight = None;
                }
                if store.desired_window != Some(window) || store.desired_domains != domains {
                    store.stale_results = store.stale_results.saturating_add(1);
                    continue;
                }
                match result {
                    Ok(snapshot) => {
                        store.cells = snapshot.cells;
                        store.objects = snapshot.objects;
                        store.terrain_weight_pages = snapshot.terrain_weight_pages;
                        store.ground_cover_layers = snapshot.ground_cover_layers;
                        store.ground_cover_masks = snapshot.ground_cover_masks;
                        store.cells_truncated = snapshot.cells_truncated;
                        store.objects_truncated = snapshot.objects_truncated;
                        store.terrain_weights_truncated = snapshot.terrain_weights_truncated;
                        store.ground_cover_masks_truncated = snapshot.ground_cover_masks_truncated;
                        store.loaded_window = Some(window);
                        store.loaded_domains = domains;
                        store.failed_window = None;
                        store.failed_domains = ProjectSourceDomains::default();
                        store.query_error = None;
                        store.completed_queries = store.completed_queries.saturating_add(1);
                    }
                    Err(error) => {
                        store.failed_window = Some(window);
                        store.failed_domains = domains;
                        store.query_error = Some(error);
                    }
                }
            }
            Ok(ProjectResult::SaveObjectTransaction { request_id, result }) => {
                if store.save_in_flight == Some(request_id) {
                    store.save_in_flight = None;
                }
                let outcome = match result {
                    Ok(ObjectWriteTransactionResult::Committed(commits)) => {
                        store.source_epoch = store.source_epoch.wrapping_add(1).max(1);
                        store.loaded_window = None;
                        store.objects.clear();
                        store.objects_truncated = false;
                        ObjectSaveOutcome::Committed(commits)
                    }
                    Ok(ObjectWriteTransactionResult::Conflict { object, actual }) => {
                        ObjectSaveOutcome::Conflict { object, actual }
                    }
                    Err(error) => ObjectSaveOutcome::Failed(error),
                };
                store.save_completion = Some(ObjectSaveCompletion {
                    request_id,
                    outcome,
                });
            }
            Ok(ProjectResult::SaveDenseTransaction { request_id, result }) => {
                if store.dense_save_in_flight == Some(request_id) {
                    store.dense_save_in_flight = None;
                }
                let outcome = match result {
                    Ok(DenseSourceWriteTransactionResult::Committed(commits)) => {
                        store.source_epoch = store.source_epoch.wrapping_add(1).max(1);
                        store.loaded_window = None;
                        store.terrain_weight_pages.clear();
                        store.ground_cover_masks.clear();
                        DenseSaveOutcome::Committed(commits)
                    }
                    Ok(DenseSourceWriteTransactionResult::Conflict { key, actual }) => {
                        DenseSaveOutcome::Conflict { key, actual }
                    }
                    Err(error) => DenseSaveOutcome::Failed(error),
                };
                store.dense_save_completion = Some(DenseSaveCompletion {
                    request_id,
                    outcome,
                });
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                store.phase = ProjectStorePhase::Failed("project database worker stopped".into());
                break;
            }
        }
    }
}

fn validate_project_catalog(runtime: Res<WorldCatalog>, mut store: ResMut<ProjectEditorStore>) {
    let Some(source) = store.manifest.as_ref() else {
        return;
    };
    if runtime.world_spaces().is_empty() {
        return;
    }
    store.catalog_compatible = Some(
        source.world_spaces.len() == runtime.world_spaces().len()
            && source.world_spaces.iter().all(|source_space| {
                runtime
                    .world_space(source_space.id)
                    .is_some_and(|runtime_space| {
                        runtime_space.name == source_space.name
                            && runtime_space.cell_size.to_bits() == source_space.cell_size.to_bits()
                    })
            }),
    );
}

fn update_project_query_demand(
    viewpoint: Res<WorldViewpoint>,
    workspace: Res<State<EditorWorkspace>>,
    preview: Res<PreviewModeState>,
    tools: Res<EditorToolRegistry>,
    mut store: ResMut<ProjectEditorStore>,
) {
    if !matches!(store.phase, ProjectStorePhase::Ready) || store.catalog_compatible == Some(false) {
        return;
    }
    let Some(active_preview) = preview.active() else {
        return;
    };
    let desired_domains = if active_preview == EditorPreviewMode::Authoring {
        let Some(tool) = tools.active(*workspace.get()) else {
            return;
        };
        ProjectSourceDomains::from_tool(tool)
    } else {
        ProjectSourceDomains::from_domains(active_preview.descriptor().domains)
    };
    if !desired_domains.any() {
        return;
    }
    let Some(position) = viewpoint.position() else {
        return;
    };
    let desired = ProjectQueryWindow::around(position.space, position.cell);
    if store.desired_window == Some(desired) && store.desired_domains == desired_domains {
        return;
    }

    if store
        .loaded_window
        .is_some_and(|loaded| loaded.space != desired.space)
        || store.loaded_domains != desired_domains
    {
        store.loaded_window = None;
        store.cells.clear();
        store.objects.clear();
        store.terrain_weight_pages.clear();
        store.ground_cover_layers.clear();
        store.ground_cover_masks.clear();
        store.cells_truncated = false;
        store.objects_truncated = false;
        store.terrain_weights_truncated = false;
        store.ground_cover_masks_truncated = false;
    }
    store.desired_window = Some(desired);
    store.desired_domains = desired_domains;
    if store.failed_window != Some(desired) || store.failed_domains != desired_domains {
        store.failed_window = None;
        store.failed_domains = ProjectSourceDomains::default();
        store.query_error = None;
    }
}

fn dispatch_project_query(
    worker: Option<Res<ProjectWorker>>,
    mut store: ResMut<ProjectEditorStore>,
) {
    if !matches!(store.phase, ProjectStorePhase::Ready)
        || store.catalog_compatible == Some(false)
        || store.in_flight.is_some()
    {
        return;
    }
    let Some(window) = store.desired_window else {
        return;
    };
    let domains = store.desired_domains;
    if (store.loaded_window == Some(window) && store.loaded_domains == domains)
        || (store.failed_window == Some(window) && store.failed_domains == domains)
    {
        return;
    }
    let Some(worker) = worker else {
        return;
    };

    let revision = store.next_revision.wrapping_add(1).max(1);
    match worker.requests.try_send(ProjectRequest::Query {
        revision,
        window,
        domains,
    }) {
        Ok(()) => {
            store.next_revision = revision;
            store.in_flight = Some(InFlightQuery {
                revision,
                window,
                domains,
            });
        }
        Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => {
            store.phase = ProjectStorePhase::Failed("project request channel closed".into());
        }
    }
}

fn dispatch_project_save(
    worker: Option<Res<ProjectWorker>>,
    mut store: ResMut<ProjectEditorStore>,
) {
    if !matches!(store.phase, ProjectStorePhase::Ready)
        || store.save_in_flight.is_some()
        || store.dense_save_in_flight.is_some()
    {
        return;
    }
    let Some(worker) = worker else {
        return;
    };

    if let Some(request) = store.pending_save.clone() {
        match worker
            .requests
            .try_send(ProjectRequest::SaveObjectTransaction(request.clone()))
        {
            Ok(()) => {
                store.pending_save = None;
                store.save_in_flight = Some(request.request_id);
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                store.phase = ProjectStorePhase::Failed("project request channel closed".into());
            }
        }
        return;
    }

    if let Some(request) = store.pending_dense_save.clone() {
        match worker
            .requests
            .try_send(ProjectRequest::SaveDenseTransaction(request.clone()))
        {
            Ok(()) => {
                store.pending_dense_save = None;
                store.dense_save_in_flight = Some(request.request_id);
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                store.phase = ProjectStorePhase::Failed("project request channel closed".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_window_is_bounded_and_saturates_at_coordinate_edges() {
        let regular = ProjectQueryWindow::around(WorldSpaceId(1), CellCoord { x: 10, z: -10 });
        assert_eq!(regular.minimum, CellCoord { x: 8, z: -12 });
        assert_eq!(regular.maximum, CellCoord { x: 12, z: -8 });

        let edge = ProjectQueryWindow::around(
            WorldSpaceId(1),
            CellCoord {
                x: i32::MAX,
                z: i32::MIN,
            },
        );
        assert_eq!(edge.maximum.x, i32::MAX);
        assert_eq!(edge.minimum.z, i32::MIN);
    }

    #[test]
    fn one_in_flight_query_records_its_window() {
        let window = ProjectQueryWindow::around(WorldSpaceId(1), CellCoord::ZERO);
        let query = InFlightQuery {
            revision: 7,
            window,
            domains: ProjectSourceDomains {
                cells: true,
                objects: true,
                ..default()
            },
        };
        assert_eq!(query.window, window);
    }

    #[test]
    fn object_save_queue_allows_only_one_bounded_request() {
        let mut store = ProjectEditorStore::default();
        let transform = world_db::SourceObjectTransform {
            space: WorldSpaceId(1),
            owner_cell: CellCoord::ZERO,
            local_translation: [1.0, 2.0, 3.0],
            yaw: 0.0,
            scale: 1.0,
        };
        assert!(
            store
                .queue_object_transaction(vec![SourceObjectWrite::UpdateTransform {
                    object: StableObjectId([1; 16]),
                    expected_source_revision: 4,
                    transform,
                }])
                .is_some()
        );
        assert!(
            store
                .queue_object_transaction(vec![SourceObjectWrite::Delete {
                    object: StableObjectId([2; 16]),
                    expected_source_revision: 7,
                }])
                .is_none()
        );
    }

    #[test]
    fn project_worker_reads_a_bounded_demo_window() {
        let database =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/demo.project.sqlite");
        let (request_sender, request_receiver) = bounded(PROJECT_REQUEST_CAPACITY);
        let (result_sender, result_receiver) = bounded(PROJECT_REQUEST_CAPACITY + 1);
        let worker =
            thread::spawn(move || project_worker(database, request_receiver, result_sender));

        let ProjectResult::Opened(Ok(opened)) = result_receiver.recv().unwrap() else {
            panic!("project worker did not open the checked-in authoring database");
        };
        let manifest = opened.manifest;
        assert_eq!(manifest.world_spaces.len(), 2);
        let window = ProjectQueryWindow::around(manifest.default_world_space, CellCoord::ZERO);
        request_sender
            .send(ProjectRequest::Query {
                revision: 1,
                window,
                domains: ProjectSourceDomains {
                    cells: true,
                    objects: true,
                    terrain_weights: true,
                    ground_cover_masks: true,
                },
            })
            .unwrap();
        let ProjectResult::Query {
            revision,
            window: returned_window,
            result: Ok(snapshot),
            ..
        } = result_receiver.recv().unwrap()
        else {
            panic!("project worker did not return the requested source window");
        };
        assert_eq!(revision, 1);
        assert_eq!(returned_window, window);
        assert!(!snapshot.cells.is_empty());
        assert!(snapshot.cells.len() <= MAX_SOURCE_CELLS);
        assert!(snapshot.objects.len() <= MAX_SOURCE_OBJECTS);
        assert!(snapshot.terrain_weight_pages.len() <= MAX_SOURCE_TERRAIN_WEIGHT_PAGES);
        assert!(snapshot.ground_cover_masks.len() <= MAX_SOURCE_GROUND_COVER_MASKS);
        assert!(!snapshot.ground_cover_layers.is_empty());
        assert!(
            snapshot
                .objects
                .iter()
                .all(|object| object.visual_uri.is_some()),
            "the bounded demo object view should include its authoring proxy scene URI"
        );

        request_sender.send(ProjectRequest::Shutdown).unwrap();
        worker.join().unwrap();
    }
}
