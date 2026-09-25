//! Bounded, asynchronous access to the mutable project database.

use std::{
    path::PathBuf,
    thread::{self, JoinHandle},
};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError, bounded};
use engine::{WorldCatalog, WorldViewpoint};
use vegetation::VegetationCatalog;
use world::{CellCoord, StableObjectId, WorldSpaceId};
use world_db::{
    DenseSourceRecord, DenseSourceRecordKey, DenseSourceWrite, EnvironmentDefinitionWrite,
    EnvironmentSourceCommit, EnvironmentSourceWriteResult, ObjectWriteTransactionResult,
    ProjectManifest, ProjectReader, ProjectWriter, SourceCellRecord, SourceEnvironmentCellRecord,
    SourceObjectRecord, SourceObjectViewRecord, SourceObjectWrite, SourceObjectWriteCommit,
    VegetationCatalogWriteResult,
};

use crate::preview::{EditorPreviewMode, PreviewModeState};
use crate::tools::{EditorSourceDomain, EditorToolRegistry};
use crate::workspaces::EditorWorkspace;

const SOURCE_RADIUS_CELLS: i32 = 2;
const MAX_SOURCE_CELLS: usize = 25;
const MAX_SOURCE_OBJECTS: usize = 2_048;
const PROJECT_REQUEST_CAPACITY: usize = 2;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProjectStoreUpdate;

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
                    .chain()
                    .in_set(ProjectStoreUpdate),
            );
    }
}

#[derive(Resource)]
pub(crate) struct ProjectDatabasePath(pub(crate) PathBuf);

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
    environment_coverage: bool,
}

impl ProjectSourceDomains {
    fn from_tool(tool: crate::tools::EditorToolDescriptor) -> Self {
        Self::from_domains(tool.source_domains)
    }

    fn from_domains(domains: &[EditorSourceDomain]) -> Self {
        Self {
            cells: domains.contains(&EditorSourceDomain::CellDescriptors),
            objects: domains.contains(&EditorSourceDomain::ObjectPlacements),
            environment_coverage: domains.contains(&EditorSourceDomain::EnvironmentCoverage),
        }
    }

    fn any(self) -> bool {
        self.cells || self.objects || self.environment_coverage
    }
}

#[derive(Debug, Clone)]
struct PendingObjectSave {
    request_id: u64,
    writes: Vec<SourceObjectWrite>,
}

#[derive(Debug, Clone)]
struct PendingDenseSave {
    roads: Vec<world_db::RoadSourceWrite>,
    road_dependencies: Vec<world_db::RoadDependency>,
    library_revision: u64,
    presets: Option<environment::PresetLibrary>,
    definitions: Vec<EnvironmentDefinitionWrite>,
    request_id: u64,
    writes: Vec<DenseSourceWrite>,
}

#[derive(Debug, Clone)]
struct PendingVegetationSave {
    request_id: u64,
    expected: Option<VegetationCatalog>,
    replacement: VegetationCatalog,
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
    RoadConflict {
        actual: world_db::RoadRecordState,
    },
    Committed(EnvironmentSourceCommit),
    LibraryConflict {
        actual: environment::PresetLibrary,
    },
    DefinitionConflict {
        space: WorldSpaceId,
        actual: Option<environment::EnvironmentDefinition>,
    },
    Conflict {
        key: DenseSourceRecordKey,
        actual: Option<DenseSourceRecord>,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) struct VegetationSaveCompletion {
    pub(crate) request_id: u64,
    pub(crate) outcome: VegetationSaveOutcome,
}

#[derive(Debug, Clone)]
pub(crate) enum VegetationSaveOutcome {
    Committed(VegetationCatalog),
    Conflict { actual: Option<VegetationCatalog> },
    Failed(String),
}

#[derive(Resource, Default)]
pub(crate) struct ProjectEditorStore {
    phase: ProjectStorePhase,
    manifest: Option<ProjectManifest>,
    vegetation_catalog: Option<VegetationCatalog>,
    environments: Vec<environment::EnvironmentDefinition>,
    presets: Option<environment::PresetLibrary>,
    terrain_resources: Vec<world_db::TerrainRenderResources>,
    height_steps: std::collections::BTreeMap<WorldSpaceId, f32>,
    write_error: Option<String>,
    catalog_compatible: Option<bool>,
    desired_window: Option<ProjectQueryWindow>,
    environment_focus: Option<(WorldSpaceId, CellCoord)>,
    desired_domains: ProjectSourceDomains,
    loaded_window: Option<ProjectQueryWindow>,
    loaded_domains: ProjectSourceDomains,
    failed_window: Option<ProjectQueryWindow>,
    failed_domains: ProjectSourceDomains,
    in_flight: Option<InFlightQuery>,
    cells: Vec<SourceCellRecord>,
    objects: Vec<SourceObjectViewRecord>,
    environment_cells: Vec<SourceEnvironmentCellRecord>,
    environment_snapshot: Option<world_db::EnvironmentReadSnapshot>,
    cells_truncated: bool,
    objects_truncated: bool,
    environment_coverage_truncated: bool,
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
    pending_vegetation_save: Option<PendingVegetationSave>,
    vegetation_save_in_flight: Option<u64>,
    vegetation_save_completion: Option<VegetationSaveCompletion>,
    next_save_request_id: u64,
    pending_atmosphere_save: Option<(u64, Vec<world_db::AtmosphereWrite>)>,
    atmosphere_save_in_flight: Option<u64>,
    pub(crate) atmosphere_completion:
        Option<(u64, Result<world_db::AtmosphereWriteResult, String>)>,
}

impl ProjectEditorStore {
    pub(crate) fn queue_atmospheres(
        &mut self,
        writes: Vec<world_db::AtmosphereWrite>,
    ) -> Option<u64> {
        if writes.is_empty() || self.save_in_flight() || self.write_error.is_some() {
            return None;
        }
        self.next_save_request_id = self.next_save_request_id.wrapping_add(1).max(1);
        self.pending_atmosphere_save = Some((self.next_save_request_id, writes));
        Some(self.next_save_request_id)
    }

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

    pub(crate) fn environment_snapshot(&self) -> Option<&world_db::EnvironmentReadSnapshot> {
        self.environment_snapshot.as_ref()
    }
    pub(crate) fn terrain_height_step(&self, space: WorldSpaceId) -> Option<f32> {
        self.height_steps.get(&space).copied()
    }
    pub(crate) fn terrain_resources(
        &self,
        space: WorldSpaceId,
    ) -> Option<&world_db::TerrainRenderResources> {
        self.terrain_resources
            .iter()
            .find(|r| r.profile.space == space)
    }
    pub(crate) fn focus_environment(&mut self, space: WorldSpaceId, cell: CellCoord) {
        self.environment_focus = Some((space, cell));
    }

    pub(crate) fn presets(&self) -> Option<&environment::PresetLibrary> {
        self.presets.as_ref()
    }

    pub(crate) fn environments(&self) -> &[environment::EnvironmentDefinition] {
        &self.environments
    }

    pub(crate) fn vegetation_catalog(&self) -> Option<&VegetationCatalog> {
        self.vegetation_catalog.as_ref()
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

    pub(crate) fn environment_cells(&self) -> &[SourceEnvironmentCellRecord] {
        &self.environment_cells
    }

    pub(crate) fn cells_truncated(&self) -> bool {
        self.cells_truncated
    }

    pub(crate) fn objects_truncated(&self) -> bool {
        self.objects_truncated
    }

    pub(crate) fn environment_coverage_truncated(&self) -> bool {
        self.environment_coverage_truncated
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
                self.environment_cells
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
            || self.pending_vegetation_save.is_some()
            || self.vegetation_save_in_flight.is_some()
            || self.pending_atmosphere_save.is_some()
            || self.atmosphere_save_in_flight.is_some()
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

    pub(crate) fn queue_dense_transaction(
        &mut self,
        library_revision: u64,
        presets: Option<environment::PresetLibrary>,
        definitions: Vec<EnvironmentDefinitionWrite>,
        writes: Vec<DenseSourceWrite>,
        roads: Vec<world_db::RoadSourceWrite>,
        road_dependencies: Vec<world_db::RoadDependency>,
    ) -> Option<u64> {
        if (writes.is_empty() && definitions.is_empty() && presets.is_none() && roads.is_empty())
            || self.save_in_flight()
            || self.write_error.is_some()
        {
            return None;
        }
        let request_id = self.next_save_request_id.wrapping_add(1).max(1);
        self.next_save_request_id = request_id;
        self.pending_dense_save = Some(PendingDenseSave {
            roads,
            road_dependencies,
            library_revision,
            presets,
            request_id,
            writes,
            definitions,
        });
        Some(request_id)
    }

    pub(crate) fn take_dense_save_completion(&mut self) -> Option<DenseSaveCompletion> {
        self.dense_save_completion.take()
    }

    pub(crate) fn queue_vegetation_catalog(
        &mut self,
        expected: Option<VegetationCatalog>,
        replacement: VegetationCatalog,
    ) -> Option<u64> {
        if self.save_in_flight() || self.write_error.is_some() || replacement.validate().is_err() {
            return None;
        }
        let request_id = self.next_save_request_id.wrapping_add(1).max(1);
        self.next_save_request_id = request_id;
        self.pending_vegetation_save = Some(PendingVegetationSave {
            request_id,
            expected,
            replacement,
        });
        Some(request_id)
    }

    pub(crate) fn take_vegetation_save_completion(&mut self) -> Option<VegetationSaveCompletion> {
        self.vegetation_save_completion.take()
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
    SaveAtmospheres(u64, Vec<world_db::AtmosphereWrite>),
    SaveObjectTransaction(PendingObjectSave),
    SaveDenseTransaction(PendingDenseSave),
    SaveVegetationCatalog(PendingVegetationSave),
    Shutdown,
}

enum ProjectResult {
    // Boxed: a conflict carries a whole atmosphere profile, including authored weather.
    SaveAtmospheres(u64, Box<Result<world_db::AtmosphereWriteResult, String>>),
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
        result: Result<EnvironmentSourceWriteResult, String>,
    },
    SaveVegetationCatalog {
        request_id: u64,
        replacement: VegetationCatalog,
        result: Result<VegetationCatalogWriteResult, String>,
    },
}

struct ProjectOpenSnapshot {
    manifest: ProjectManifest,
    vegetation_catalog: Option<VegetationCatalog>,
    environments: Vec<environment::EnvironmentDefinition>,
    presets: Option<environment::PresetLibrary>,
    terrain_resources: Vec<world_db::TerrainRenderResources>,
    height_steps: std::collections::BTreeMap<WorldSpaceId, f32>,
    write_error: Option<String>,
}

struct ProjectWindowSnapshot {
    cells: Vec<SourceCellRecord>,
    objects: Vec<SourceObjectViewRecord>,
    environment_cells: Vec<SourceEnvironmentCellRecord>,
    environment_snapshot: Option<world_db::EnvironmentReadSnapshot>,
    cells_truncated: bool,
    objects_truncated: bool,
    environment_coverage_truncated: bool,
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
    let (vegetation_catalog, presets, environments) = match reader.read_environment_catalog() {
        Ok(catalog) => catalog,
        Err(error) => {
            let _ = results.send(ProjectResult::Opened(Err(format!(
                "could not read the project vegetation catalog: {error}"
            ))));
            return;
        }
    };
    let terrain_resources = match environments
        .iter()
        .map(|d| reader.read_environment_terrain_resources(d.space))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(resources) => resources,
        Err(error) => {
            let _ = results.send(ProjectResult::Opened(Err(format!(
                "could not read environment materials: {error}"
            ))));
            return;
        }
    };
    let height_steps = match environments
        .iter()
        .map(|d| {
            reader
                .read_terrain_height_step(d.space)
                .map(|step| (d.space, step))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()
    {
        Ok(steps) => steps,
        Err(e) => {
            let _ = results.send(ProjectResult::Opened(Err(format!(
                "could not read terrain height grids: {e}"
            ))));
            return;
        }
    };
    if results
        .send(ProjectResult::Opened(Ok(ProjectOpenSnapshot {
            manifest: reader.manifest().clone(),
            vegetation_catalog,
            environments,
            presets: Some(presets),
            terrain_resources,
            height_steps,
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
                    let environment_snapshot = domains
                        .environment_coverage
                        .then(|| {
                            let mut core = Vec::new();
                            for x in window.minimum.x..=window.maximum.x {
                                for z in window.minimum.z..=window.maximum.z {
                                    core.push(CellCoord { x, z });
                                }
                            }
                            reader.read_environment_snapshot(
                                window.space,
                                &world_db::environment_dependency_cells(&core)?,
                            )
                        })
                        .transpose()?;
                    let environment_cells =
                        environment_snapshot
                            .as_ref()
                            .map_or_else(Vec::new, |snapshot| {
                                snapshot
                                    .coverage
                                    .cells
                                    .iter()
                                    .filter(|c| {
                                        c.cell.x >= window.minimum.x
                                            && c.cell.x <= window.maximum.x
                                            && c.cell.z >= window.minimum.z
                                            && c.cell.z <= window.maximum.z
                                    })
                                    .map(|c| SourceEnvironmentCellRecord {
                                        space: window.space,
                                        cell: c.cell,
                                        source_revision: c.revision as i64,
                                        definition_revision: snapshot.definition.revision,
                                        tiles: c.tiles.clone(),
                                    })
                                    .collect()
                            });
                    Ok::<_, world_db::WorldDbError>(ProjectWindowSnapshot {
                        cells: cells.records,
                        objects: objects
                            .as_ref()
                            .map_or_else(Vec::new, |query| query.records.clone()),
                        environment_cells,
                        environment_snapshot,
                        cells_truncated: cells.truncated,
                        objects_truncated: objects.is_some_and(|query| query.truncated),
                        environment_coverage_truncated: false,
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
                        .apply_environment_and_roads_transaction(
                            request.library_revision,
                            request.presets.as_ref(),
                            &request.definitions,
                            &request.writes,
                            &request.roads,
                            &request.road_dependencies,
                        )
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
            ProjectRequest::SaveVegetationCatalog(request) => {
                let result = match writer.as_mut() {
                    Ok(writer) => writer
                        .replace_vegetation_catalog_if_matches(
                            request.expected.as_ref(),
                            &request.replacement,
                        )
                        .map_err(|error| error.to_string()),
                    Err(error) => Err(error.clone()),
                };
                if results
                    .send(ProjectResult::SaveVegetationCatalog {
                        request_id: request.request_id,
                        replacement: request.replacement,
                        result,
                    })
                    .is_err()
                {
                    return;
                }
            }
            ProjectRequest::SaveAtmospheres(id, writes) => {
                let result = match writer.as_mut() {
                    Ok(writer) => writer.write_atmospheres(&writes).map_err(|e| e.to_string()),
                    Err(e) => Err(e.clone()),
                };
                if results
                    .send(ProjectResult::SaveAtmospheres(id, Box::new(result)))
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
            Ok(ProjectResult::SaveAtmospheres(id, result)) => {
                let result = *result;
                if store.atmosphere_save_in_flight != Some(id) {
                    continue;
                }
                store.atmosphere_save_in_flight = None;
                if let Ok(world_db::AtmosphereWriteResult::Committed(records)) = &result {
                    store.source_epoch = store.source_epoch.wrapping_add(1).max(1);
                    if let Some(manifest) = &mut store.manifest {
                        for (id, revision, profile) in records {
                            if let Some(space) =
                                manifest.world_spaces.iter_mut().find(|s| s.id == *id)
                            {
                                space.atmosphere = profile.clone();
                                space.atmosphere_revision = *revision;
                            }
                        }
                    }
                }
                store.atmosphere_completion = Some((id, result));
            }

            Ok(ProjectResult::Opened(result)) => match result {
                Ok(opened) => {
                    store.manifest = Some(opened.manifest);
                    store.vegetation_catalog = opened.vegetation_catalog;
                    store.environments = opened.environments;
                    store.presets = opened.presets;
                    store.terrain_resources = opened.terrain_resources;
                    store.height_steps = opened.height_steps;
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
                        store.environment_cells = snapshot.environment_cells;
                        if let Some(snapshot) = &snapshot.environment_snapshot
                            && let Some(definition) = store
                                .environments
                                .iter_mut()
                                .find(|d| d.space == snapshot.definition.space)
                        {
                            *definition = snapshot.definition.clone();
                        }
                        if let Some(environment) = &snapshot.environment_snapshot {
                            store.presets = Some(environment.presets.clone());
                        }
                        store.environment_snapshot = snapshot.environment_snapshot;
                        store.cells_truncated = snapshot.cells_truncated;
                        store.objects_truncated = snapshot.objects_truncated;
                        store.environment_coverage_truncated =
                            snapshot.environment_coverage_truncated;
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
                    Ok(EnvironmentSourceWriteResult::Committed(commits)) => {
                        if let Some(presets) = &commits.presets {
                            store.presets = Some(presets.clone());
                        }
                        for definition in &commits.definitions {
                            if let Some(old) = store
                                .environments
                                .iter_mut()
                                .find(|d| d.space == definition.space)
                            {
                                *old = definition.clone();
                            }
                        }
                        store.source_epoch = store.source_epoch.wrapping_add(1).max(1);
                        store.loaded_window = None;
                        store.environment_cells.clear();
                        store.environment_snapshot = None;
                        DenseSaveOutcome::Committed(commits)
                    }
                    Ok(EnvironmentSourceWriteResult::RoadConflict { actual }) => {
                        DenseSaveOutcome::RoadConflict { actual }
                    }
                    Ok(EnvironmentSourceWriteResult::LibraryConflict { actual }) => {
                        store.presets = Some(actual.clone());
                        DenseSaveOutcome::LibraryConflict { actual }
                    }
                    Ok(EnvironmentSourceWriteResult::DefinitionConflict { space, actual }) => {
                        DenseSaveOutcome::DefinitionConflict { space, actual }
                    }
                    Ok(EnvironmentSourceWriteResult::CoverageConflict { key, actual }) => {
                        DenseSaveOutcome::Conflict { key, actual }
                    }
                    Err(error) => DenseSaveOutcome::Failed(error),
                };
                store.dense_save_completion = Some(DenseSaveCompletion {
                    request_id,
                    outcome,
                });
            }
            Ok(ProjectResult::SaveVegetationCatalog {
                request_id,
                replacement,
                result,
            }) => {
                if store.vegetation_save_in_flight == Some(request_id) {
                    store.vegetation_save_in_flight = None;
                }
                let outcome = match result {
                    Ok(VegetationCatalogWriteResult::Committed) => {
                        store.source_epoch = store.source_epoch.wrapping_add(1).max(1);
                        store.vegetation_catalog = Some(replacement.clone());
                        VegetationSaveOutcome::Committed(replacement)
                    }
                    Ok(VegetationCatalogWriteResult::Conflict { actual }) => {
                        store.vegetation_catalog = actual.clone();
                        VegetationSaveOutcome::Conflict { actual }
                    }
                    Err(error) => VegetationSaveOutcome::Failed(error),
                };
                store.vegetation_save_completion = Some(VegetationSaveCompletion {
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
    let center = if desired_domains.environment_coverage {
        store
            .environment_focus
            .filter(|(space, _)| *space == position.space)
            .map_or(position.cell, |(_, cell)| cell)
    } else {
        position.cell
    };
    let desired = ProjectQueryWindow::around(position.space, center);
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
        store.environment_cells.clear();
        store.environment_snapshot = None;
        store.cells_truncated = false;
        store.objects_truncated = false;
        store.environment_coverage_truncated = false;
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
        || store.vegetation_save_in_flight.is_some()
        || store.atmosphere_save_in_flight.is_some()
    {
        return;
    }
    let Some(worker) = worker else {
        return;
    };

    if let Some((id, writes)) = store.pending_atmosphere_save.clone() {
        match worker
            .requests
            .try_send(ProjectRequest::SaveAtmospheres(id, writes))
        {
            Ok(()) => {
                store.pending_atmosphere_save = None;
                store.atmosphere_save_in_flight = Some(id);
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                store.write_error = Some("Project writer stopped".into());
            }
        }
        return;
    }

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
        return;
    }

    if let Some(request) = store.pending_vegetation_save.clone() {
        match worker
            .requests
            .try_send(ProjectRequest::SaveVegetationCatalog(request.clone()))
        {
            Ok(()) => {
                store.pending_vegetation_save = None;
                store.vegetation_save_in_flight = Some(request.request_id);
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
        let directory =
            std::env::temp_dir().join(format!("yarra-preset-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("project.sqlite");
        world_cook::create_demo_project(&database).unwrap();
        let (request_sender, request_receiver) = bounded(PROJECT_REQUEST_CAPACITY);
        let (result_sender, result_receiver) = bounded(PROJECT_REQUEST_CAPACITY + 1);
        let worker =
            thread::spawn(move || project_worker(database, request_receiver, result_sender));

        let ProjectResult::Opened(Ok(opened)) = result_receiver.recv().unwrap() else {
            panic!("project worker did not open the fresh authoring database");
        };
        let manifest = opened.manifest;
        assert!(opened.vegetation_catalog.is_some());
        assert_eq!(manifest.world_spaces.len(), 2);
        let window = ProjectQueryWindow::around(manifest.default_world_space, CellCoord::ZERO);
        request_sender
            .send(ProjectRequest::Query {
                revision: 1,
                window,
                domains: ProjectSourceDomains {
                    cells: true,
                    objects: true,
                    environment_coverage: true,
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
        assert!(snapshot.environment_cells.len() <= MAX_SOURCE_CELLS);
        assert!(
            snapshot
                .objects
                .iter()
                .all(|object| object.visual_uri.is_some()),
            "the bounded demo object view should include its authoring proxy scene URI"
        );

        request_sender.send(ProjectRequest::Shutdown).unwrap();
        worker.join().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
