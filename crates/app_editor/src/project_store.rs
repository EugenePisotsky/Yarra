//! Bounded, asynchronous access to the mutable project database.

use std::{path::PathBuf, sync::Arc};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use engine::{WorldCatalog, WorldViewpoint};
use vegetation::VegetationCatalog;
use world::{CellCoord, StableObjectId, WorldSpaceId};
use world_db::{
    EnvironmentCellKey, EnvironmentCellWrite, EnvironmentDefinitionWrite, EnvironmentSourceCommit,
    EnvironmentSourceWriteResult, ObjectWriteTransactionResult, ProjectManifest, ProjectReader,
    ProjectWriter, SourceCellRecord, SourceEnvironmentCellRecord, SourceObjectRecord,
    SourceObjectViewRecord, SourceObjectWrite, SourceObjectWriteCommit,
    VegetationCatalogWriteResult,
};

use crate::tools::{EditorSourceDomain, EditorToolRegistry};
use crate::workspaces::EditorWorkspace;
use demand::{
    dispatch_project_query, dispatch_project_save, update_project_query_demand,
    validate_project_catalog,
};
use receive::receive_project_results;
use worker::start_project_worker;

mod demand;
mod receive;
mod worker;

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
    writes: Vec<EnvironmentCellWrite>,
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
        key: EnvironmentCellKey,
        actual: Option<SourceEnvironmentCellRecord>,
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
    gameplay_areas: Option<world_db::GameplayAreasRecord>,
    pending_area_save: Option<(u64, i64, Arc<[world::GameplayArea]>)>,
    area_save_in_flight: Option<u64>,
    pub(crate) area_completion: Option<(u64, Result<world_db::GameplayAreasWriteResult, String>)>,
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

    /// The project's gameplay areas as last read or saved.
    pub(crate) fn gameplay_areas(&self) -> Option<&world_db::GameplayAreasRecord> {
        self.gameplay_areas.as_ref()
    }

    pub(crate) fn queue_gameplay_areas(
        &mut self,
        expected_revision: i64,
        areas: Arc<[world::GameplayArea]>,
    ) -> Option<u64> {
        if self.save_in_flight() || self.write_error.is_some() {
            return None;
        }
        self.next_save_request_id = self.next_save_request_id.wrapping_add(1).max(1);
        self.pending_area_save = Some((self.next_save_request_id, expected_revision, areas));
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
            || self.pending_area_save.is_some()
            || self.area_save_in_flight.is_some()
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
        writes: Vec<EnvironmentCellWrite>,
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

type ProjectWorker = crate::worker::Worker<ProjectRequest, ProjectResult>;

enum ProjectRequest {
    Query {
        revision: u64,
        window: ProjectQueryWindow,
        domains: ProjectSourceDomains,
    },
    SaveAtmospheres(u64, Vec<world_db::AtmosphereWrite>),
    SaveGameplayAreas(u64, i64, Arc<[world::GameplayArea]>),
    SaveObjectTransaction(PendingObjectSave),
    SaveDenseTransaction(PendingDenseSave),
    SaveVegetationCatalog(PendingVegetationSave),
}

enum ProjectResult {
    // Boxed: a conflict carries a whole atmosphere profile, including authored weather.
    SaveAtmospheres(u64, Box<Result<world_db::AtmosphereWriteResult, String>>),
    SaveGameplayAreas(u64, Result<world_db::GameplayAreasWriteResult, String>),
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
    gameplay_areas: world_db::GameplayAreasRecord,
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

#[cfg(test)]
mod tests {
    use super::worker::project_worker;
    use super::*;
    use crossbeam_channel::bounded;
    use std::thread;

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

        drop(request_sender);
        worker.join().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }
}
