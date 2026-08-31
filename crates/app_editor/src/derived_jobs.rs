//! Bounded, revision-aware coordination for editor-derived work.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};
use world::{CellCoord, StableObjectId, WorldSpaceId};
use world_db::{
    DenseSourceRecord, ProjectReader, SourceCellRecord, SourceObjectRecord,
    SourceTerrainCellWeightPageRecord,
};

use crate::{
    domain_editing::DenseDomainWorkingSets,
    editing::EditorObjectWorkingSet,
    project_store::ProjectEditorStore,
    tools::{DerivedProduct, EditorToolId, OBJECT_TOOL, TERRAIN_TOOL},
};

const MAX_PENDING_DERIVED_JOBS: usize = 512;
const MAX_RUNNING_DERIVED_JOBS: usize = 4;
const DERIVED_EXECUTOR_CAPACITY: usize = MAX_RUNNING_DERIVED_JOBS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DerivedJobScope {
    Cell {
        space: WorldSpaceId,
        cell: CellCoord,
    },
    Region {
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DerivedJobKey {
    pub(crate) product: DerivedProduct,
    pub(crate) scope: DerivedJobScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DerivedJobRequest {
    key: DerivedJobKey,
    input_revision: u64,
}

#[derive(Debug)]
struct RunningDerivedJob {
    request: DerivedJobRequest,
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub(crate) struct DerivedJobLease {
    pub(crate) id: u64,
    pub(crate) key: DerivedJobKey,
    pub(crate) input_revision: u64,
    cancelled: Arc<AtomicBool>,
}

impl DerivedJobLease {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DerivedJobCompletion {
    Accepted,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DerivedArtifact {
    CookedObjectPage {
        object_count: usize,
        fingerprint: u64,
    },
    TerrainPage {
        page_count: usize,
        texel_count: usize,
        average_weight: f32,
        fingerprint: u64,
    },
    Collision {
        cell_count: usize,
        object_count: usize,
        fingerprint: u64,
    },
    Navigation {
        cell_count: usize,
        obstacle_count: usize,
        fingerprint: u64,
    },
    Overview {
        coarse_heights: Vec<(CellCoord, f32)>,
        object_icons: Vec<(StableObjectId, CellCoord)>,
        cell_status: Vec<(CellCoord, i64)>,
        fingerprint: u64,
    },
}

#[derive(Resource, Default)]
pub(crate) struct DerivedArtifactStore {
    artifacts: HashMap<DerivedJobKey, (u64, DerivedArtifact)>,
    failures: HashMap<DerivedJobKey, (u64, String)>,
    failed_jobs: u64,
}

impl DerivedArtifactStore {
    pub(crate) fn get(&self, key: DerivedJobKey) -> Option<&DerivedArtifact> {
        self.artifacts.get(&key).map(|(_, artifact)| artifact)
    }

    pub(crate) fn accepted_count(&self) -> usize {
        self.artifacts.len()
    }

    pub(crate) fn failure(&self, key: DerivedJobKey) -> Option<&str> {
        self.failures.get(&key).map(|(_, error)| error.as_str())
    }

    pub(crate) fn failed_jobs(&self) -> u64 {
        self.failed_jobs
    }

    /// A successful full cook proves the saved source is publishable, so old failure diagnostics
    /// can be retired. Accepted artifacts remain useful: they are either valid for that checkpoint
    /// or represent newer unsaved commands made while the cook was running.
    pub(crate) fn clear_failures_for_generation_adoption(&mut self) {
        self.failures.clear();
    }

    fn publish(
        &mut self,
        key: DerivedJobKey,
        input_revision: u64,
        result: Result<DerivedArtifact, String>,
    ) {
        match result {
            Ok(artifact) => {
                self.failures.remove(&key);
                self.artifacts.insert(key, (input_revision, artifact));
            }
            Err(error) => {
                self.failed_jobs = self.failed_jobs.saturating_add(1);
                self.artifacts.remove(&key);
                self.failures.insert(key, (input_revision, error));
            }
        }
    }
}

#[derive(Resource)]
pub(crate) struct DerivedJobScheduler {
    pending: VecDeque<DerivedJobRequest>,
    running: HashMap<u64, RunningDerivedJob>,
    latest_requested: HashMap<DerivedJobKey, u64>,
    accepted: HashMap<DerivedJobKey, u64>,
    tool_keys: HashMap<EditorToolId, HashSet<DerivedJobKey>>,
    next_job_id: u64,
    maximum_pending: usize,
    maximum_running: usize,
    coalesced: u64,
    cancelled: u64,
    stale_results: u64,
    capacity_rejections: u64,
}

impl Default for DerivedJobScheduler {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            running: HashMap::new(),
            latest_requested: HashMap::new(),
            accepted: HashMap::new(),
            tool_keys: HashMap::new(),
            next_job_id: 0,
            maximum_pending: MAX_PENDING_DERIVED_JOBS,
            maximum_running: MAX_RUNNING_DERIVED_JOBS,
            coalesced: 0,
            cancelled: 0,
            stale_results: 0,
            capacity_rejections: 0,
        }
    }
}

impl DerivedJobScheduler {
    pub(crate) fn replace_tool_requests(
        &mut self,
        tool: EditorToolId,
        requests: impl IntoIterator<Item = (DerivedJobKey, u64)>,
    ) {
        let requests = requests.into_iter().collect::<HashMap<_, _>>();
        let previous = self.tool_keys.remove(&tool).unwrap_or_default();
        let desired = requests.keys().copied().collect::<HashSet<_>>();
        for obsolete in previous.difference(&desired).copied().collect::<Vec<_>>() {
            self.cancel_key(obsolete);
        }
        for (key, revision) in requests {
            if self.request(key, revision) {
                self.tool_keys.entry(tool).or_default().insert(key);
            }
        }
    }

    fn request(&mut self, key: DerivedJobKey, input_revision: u64) -> bool {
        if self
            .accepted
            .get(&key)
            .is_some_and(|accepted| *accepted >= input_revision)
        {
            return true;
        }
        if let Some(pending) = self.pending.iter_mut().find(|request| request.key == key) {
            if input_revision > pending.input_revision {
                pending.input_revision = input_revision;
                self.latest_requested.insert(key, input_revision);
                self.coalesced = self.coalesced.saturating_add(1);
            }
            return true;
        }
        if self.pending.len() >= self.maximum_pending {
            self.capacity_rejections = self.capacity_rejections.saturating_add(1);
            return false;
        }
        if let Some(running) = self
            .running
            .values()
            .find(|running| running.request.key == key)
            && input_revision > running.request.input_revision
            && !running.cancelled.swap(true, Ordering::AcqRel)
        {
            self.cancelled = self.cancelled.saturating_add(1);
        }
        let latest = self.latest_requested.entry(key).or_insert(0);
        *latest = (*latest).max(input_revision);
        self.pending.push_back(DerivedJobRequest {
            key,
            input_revision,
        });
        true
    }

    fn cancel_key(&mut self, key: DerivedJobKey) {
        let before = self.pending.len();
        self.pending.retain(|request| request.key != key);
        self.cancelled = self
            .cancelled
            .saturating_add((before - self.pending.len()) as u64);
        for running in self
            .running
            .values()
            .filter(|running| running.request.key == key)
        {
            if !running.cancelled.swap(true, Ordering::AcqRel) {
                self.cancelled = self.cancelled.saturating_add(1);
            }
        }
        self.latest_requested.remove(&key);
    }

    pub(crate) fn lease_next(&mut self) -> Option<DerivedJobLease> {
        if self.running.len() >= self.maximum_running {
            return None;
        }
        let request = self.pending.pop_front()?;
        self.next_job_id = self.next_job_id.wrapping_add(1).max(1);
        let id = self.next_job_id;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.running.insert(
            id,
            RunningDerivedJob {
                request,
                cancelled: cancelled.clone(),
            },
        );
        Some(DerivedJobLease {
            id,
            key: request.key,
            input_revision: request.input_revision,
            cancelled,
        })
    }

    pub(crate) fn complete(&mut self, lease: &DerivedJobLease) -> DerivedJobCompletion {
        let Some(running) = self.running.remove(&lease.id) else {
            return DerivedJobCompletion::Unknown;
        };
        let current = self.latest_requested.get(&running.request.key).copied();
        if running.cancelled.load(Ordering::Acquire)
            || current != Some(running.request.input_revision)
        {
            self.stale_results = self.stale_results.saturating_add(1);
            return DerivedJobCompletion::Stale;
        }
        self.accepted
            .insert(running.request.key, running.request.input_revision);
        DerivedJobCompletion::Accepted
    }

    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub(crate) fn running_count(&self) -> usize {
        self.running.len()
    }

    pub(crate) fn stale_results(&self) -> u64 {
        self.stale_results
    }

    pub(crate) fn cancelled_count(&self) -> u64 {
        self.cancelled
    }

    pub(crate) fn capacity_rejections(&self) -> u64 {
        self.capacity_rejections
    }
}

#[derive(Debug)]
enum DerivedJobInput {
    Objects(Vec<SourceObjectRecord>),
    Terrain(Vec<SourceTerrainCellWeightPageRecord>),
    Spatial {
        cells: Vec<SourceCellRecord>,
        objects: Vec<SourceObjectRecord>,
    },
    OverviewQuery {
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
    },
}

#[derive(Debug)]
struct DerivedExecutorRequest {
    lease: DerivedJobLease,
    input: DerivedJobInput,
}

#[derive(Debug)]
struct DerivedExecutorResult {
    lease: DerivedJobLease,
    result: Result<DerivedArtifact, String>,
}

#[derive(Resource)]
struct DerivedExecutor {
    requests: Sender<DerivedExecutorRequest>,
    results: Receiver<DerivedExecutorResult>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for DerivedExecutor {
    fn drop(&mut self) {
        let (replacement, _) = bounded(1);
        let requests = std::mem::replace(&mut self.requests, replacement);
        drop(requests);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn start_derived_executor(mut commands: Commands, database: Res<DerivedProjectDatabase>) {
    let (request_sender, request_receiver) = bounded(DERIVED_EXECUTOR_CAPACITY);
    let (result_sender, result_receiver) = bounded(DERIVED_EXECUTOR_CAPACITY);
    let executor_thread = thread::Builder::new()
        .name("yarra-editor-derived".into())
        .spawn({
            let database_path = database.0.clone();
            move || derived_executor(database_path, request_receiver, result_sender)
        })
        .expect("failed to spawn derived-product executor");
    commands.insert_resource(DerivedExecutor {
        requests: request_sender,
        results: result_receiver,
        thread: Some(executor_thread),
    });
}

fn derived_executor(
    database_path: PathBuf,
    requests: Receiver<DerivedExecutorRequest>,
    results: Sender<DerivedExecutorResult>,
) {
    let project_reader = ProjectReader::open_read_only(&database_path)
        .map_err(|error| format!("could not open {}: {error}", database_path.display()));
    while let Ok(request) = requests.recv() {
        if request.lease.is_cancelled() {
            let _ = results.try_send(DerivedExecutorResult {
                lease: request.lease,
                result: Err("cancelled".into()),
            });
            continue;
        }
        let result = match request.input {
            DerivedJobInput::OverviewQuery {
                space,
                minimum,
                maximum,
            } => project_reader
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|reader| build_overview_artifact(reader, space, minimum, maximum)),
            input => build_artifact(request.lease.key.product, input),
        };
        let _ = results.try_send(DerivedExecutorResult {
            lease: request.lease,
            result,
        });
    }
}

fn build_artifact(
    product: DerivedProduct,
    input: DerivedJobInput,
) -> Result<DerivedArtifact, String> {
    match (product, input) {
        (DerivedProduct::CookedObjectPage, DerivedJobInput::Objects(objects)) => {
            Ok(DerivedArtifact::CookedObjectPage {
                object_count: objects.len(),
                fingerprint: fingerprint_objects(&objects),
            })
        }
        (DerivedProduct::TerrainPage, DerivedJobInput::Terrain(pages)) => {
            let texel_count = pages.iter().map(|page| page.rgba.len() / 4).sum::<usize>();
            let total = pages
                .iter()
                .flat_map(|page| page.rgba.iter())
                .map(|value| u64::from(*value))
                .sum::<u64>();
            let channel_count = pages.iter().map(|page| page.rgba.len()).sum::<usize>();
            Ok(DerivedArtifact::TerrainPage {
                page_count: pages.len(),
                texel_count,
                average_weight: normalized_average(total, channel_count),
                fingerprint: fingerprint_terrain(&pages),
            })
        }
        (DerivedProduct::Collision, DerivedJobInput::Spatial { cells, objects }) => {
            Ok(DerivedArtifact::Collision {
                cell_count: cells.len(),
                object_count: objects.len(),
                fingerprint: fingerprint_spatial(&cells, &objects),
            })
        }
        (DerivedProduct::Navigation, DerivedJobInput::Spatial { cells, objects }) => {
            Ok(DerivedArtifact::Navigation {
                cell_count: cells.len(),
                obstacle_count: objects.len(),
                fingerprint: fingerprint_spatial(&cells, &objects),
            })
        }
        (product, _) => Err(format!("derived input does not match {product:?}")),
    }
}

fn build_overview_artifact(
    reader: &ProjectReader,
    space: WorldSpaceId,
    minimum: CellCoord,
    maximum: CellCoord,
) -> Result<DerivedArtifact, String> {
    const MAX_CELLS: usize = 256;
    const MAX_OBJECT_ICONS: usize = 2_048;
    const MAX_TERRAIN_PAGES: usize = MAX_CELLS * 2;

    let cells = reader
        .read_cells(space, minimum, maximum, MAX_CELLS)
        .map_err(|error| error.to_string())?;
    let objects = reader
        .read_objects_in_cells(space, minimum, maximum, MAX_OBJECT_ICONS)
        .map_err(|error| error.to_string())?;
    let terrain = reader
        .read_terrain_weight_pages_in_cells(space, minimum, maximum, MAX_TERRAIN_PAGES)
        .map_err(|error| error.to_string())?;
    if cells.truncated || objects.truncated || terrain.truncated {
        return Err("overview tile exceeded its bounded source artifact capacity".into());
    }

    let coarse_heights = cells
        .records
        .iter()
        .map(|cell| (cell.cell, cell.height))
        .collect::<Vec<_>>();
    let cell_status = cells
        .records
        .iter()
        .map(|cell| (cell.cell, cell.source_revision))
        .collect::<Vec<_>>();
    let object_icons = objects
        .records
        .iter()
        .map(|object| (object.id, object.owner_cell))
        .collect::<Vec<_>>();
    let fingerprint = fingerprint_spatial(&cells.records, &objects.records)
        ^ fingerprint_terrain(&terrain.records).rotate_left(13);
    Ok(DerivedArtifact::Overview {
        coarse_heights,
        object_icons,
        cell_status,
        fingerprint,
    })
}

fn normalized_average(total: u64, count: usize) -> f32 {
    if count == 0 {
        0.0
    } else {
        total as f32 / count as f32 / 255.0
    }
}

fn fingerprint_objects(objects: &[SourceObjectRecord]) -> u64 {
    objects.iter().fold(0xcbf29ce484222325, |hash, object| {
        object
            .id
            .0
            .iter()
            .chain(object.definition.0.iter())
            .fold(hash, |hash, byte| fnv_byte(hash, *byte))
            ^ (object.source_revision as u64).rotate_left(17)
    })
}

fn fingerprint_terrain(pages: &[SourceTerrainCellWeightPageRecord]) -> u64 {
    pages.iter().fold(0xcbf29ce484222325, |hash, page| {
        page.rgba.iter().fold(
            hash ^ (page.space.0 as u64)
                ^ (page.cell.x as u64).rotate_left(11)
                ^ (page.cell.z as u64).rotate_left(23)
                ^ u64::from(page.page),
            |hash, byte| fnv_byte(hash, *byte),
        )
    })
}

fn fingerprint_spatial(cells: &[SourceCellRecord], objects: &[SourceObjectRecord]) -> u64 {
    cells
        .iter()
        .fold(fingerprint_objects(objects), |hash, cell| {
            hash ^ (cell.space.0 as u64)
                ^ (cell.cell.x as u64).rotate_left(7)
                ^ (cell.cell.z as u64).rotate_left(31)
                ^ cell.height.to_bits() as u64
                ^ (cell.source_revision as u64).rotate_left(41)
        })
}

fn fnv_byte(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
}

#[derive(Resource)]
struct DerivedProjectDatabase(PathBuf);

pub(crate) struct DerivedJobsPlugin {
    database_path: PathBuf,
}

impl DerivedJobsPlugin {
    pub(crate) fn new(database_path: impl Into<PathBuf>) -> Self {
        Self {
            database_path: database_path.into(),
        }
    }
}

impl Plugin for DerivedJobsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(DerivedProjectDatabase(self.database_path.clone()))
            .init_resource::<DerivedJobScheduler>()
            .init_resource::<DerivedArtifactStore>()
            .add_systems(Startup, start_derived_executor)
            .add_systems(
                Update,
                (
                    invalidate_object_products,
                    invalidate_dense_products,
                    receive_derived_results,
                    dispatch_derived_jobs,
                )
                    .chain(),
            );
    }
}

fn invalidate_object_products(
    objects: Res<EditorObjectWorkingSet>,
    mut scheduler: ResMut<DerivedJobScheduler>,
) {
    let revision = objects.edit_revision();
    let mut requests = HashMap::<DerivedJobKey, u64>::new();
    for snapshot in objects.dirty_snapshots() {
        for object in snapshot.base.iter().chain(snapshot.current.iter()) {
            for product in OBJECT_TOOL.invalidates {
                let scope = DerivedJobScope::Cell {
                    space: object.space,
                    cell: object.owner_cell,
                };
                requests.insert(
                    DerivedJobKey {
                        product: *product,
                        scope,
                    },
                    revision,
                );
            }
        }
    }
    scheduler.replace_tool_requests(OBJECT_TOOL.id, requests);
}

fn invalidate_dense_products(
    dense: Res<DenseDomainWorkingSets>,
    project: Res<ProjectEditorStore>,
    mut scheduler: ResMut<DerivedJobScheduler>,
) {
    let revision = dense.edit_revision().saturating_add(project.source_epoch());
    let mut terrain_requests = HashMap::<DerivedJobKey, u64>::new();
    for record in dense.runtime_divergent_records() {
        let DenseSourceRecord::TerrainWeights(record) = record;
        for product in TERRAIN_TOOL.invalidates {
            let scope = DerivedJobScope::Cell {
                space: record.space,
                cell: record.cell,
            };
            terrain_requests.insert(
                DerivedJobKey {
                    product: *product,
                    scope,
                },
                revision,
            );
        }
    }
    scheduler.replace_tool_requests(TERRAIN_TOOL.id, terrain_requests);
}

fn dispatch_derived_jobs(
    executor: Option<Res<DerivedExecutor>>,
    mut scheduler: ResMut<DerivedJobScheduler>,
    project: Res<ProjectEditorStore>,
    objects: Res<EditorObjectWorkingSet>,
    dense: Res<DenseDomainWorkingSets>,
) {
    let Some(executor) = executor else {
        return;
    };
    while let Some(lease) = scheduler.lease_next() {
        let input = snapshot_job_input(lease.key, &project, &objects, &dense);
        if executor
            .requests
            .try_send(DerivedExecutorRequest { lease, input })
            .is_err()
        {
            break;
        }
    }
}

fn receive_derived_results(
    executor: Option<Res<DerivedExecutor>>,
    mut scheduler: ResMut<DerivedJobScheduler>,
    mut artifacts: ResMut<DerivedArtifactStore>,
) {
    let Some(executor) = executor else {
        return;
    };
    loop {
        match executor.results.try_recv() {
            Ok(completed) => {
                let completion = scheduler.complete(&completed.lease);
                if completion != DerivedJobCompletion::Accepted {
                    continue;
                }
                artifacts.publish(
                    completed.lease.key,
                    completed.lease.input_revision,
                    completed.result,
                );
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                artifacts.failed_jobs = artifacts.failed_jobs.saturating_add(1);
                break;
            }
        }
    }
}

fn snapshot_job_input(
    key: DerivedJobKey,
    project: &ProjectEditorStore,
    objects: &EditorObjectWorkingSet,
    dense: &DenseDomainWorkingSets,
) -> DerivedJobInput {
    let object_records = source_objects(project, objects, key.scope);
    let dense_records = dense.current_records();
    match key.product {
        DerivedProduct::CookedObjectPage => DerivedJobInput::Objects(object_records),
        DerivedProduct::TerrainPage => DerivedJobInput::Terrain(
            dense_records
                .into_iter()
                .filter_map(|record| match record {
                    DenseSourceRecord::TerrainWeights(record)
                        if scope_contains(key.scope, record.space, record.cell) =>
                    {
                        Some(record)
                    }
                    _ => None,
                })
                .collect(),
        ),
        DerivedProduct::Collision | DerivedProduct::Navigation => DerivedJobInput::Spatial {
            cells: project
                .cells()
                .iter()
                .filter(|cell| scope_contains(key.scope, cell.space, cell.cell))
                .cloned()
                .collect(),
            objects: object_records,
        },
        DerivedProduct::Overview => {
            let (space, minimum, maximum) = match key.scope {
                DerivedJobScope::Cell { space, cell } => (space, cell, cell),
                DerivedJobScope::Region {
                    space,
                    minimum,
                    maximum,
                } => (space, minimum, maximum),
            };
            DerivedJobInput::OverviewQuery {
                space,
                minimum,
                maximum,
            }
        }
    }
}

fn source_objects(
    project: &ProjectEditorStore,
    working: &EditorObjectWorkingSet,
    scope: DerivedJobScope,
) -> Vec<SourceObjectRecord> {
    let mut records = project
        .objects()
        .iter()
        .map(|view| view.object.clone())
        .filter(|record| scope_contains(scope, record.space, record.owner_cell))
        .map(|record| (record.id, record))
        .collect::<HashMap<_, _>>();
    for snapshot in working.dirty_snapshots() {
        if let Some(current) = snapshot.current {
            if scope_contains(scope, current.space, current.owner_cell) {
                records.insert(current.id, current);
            } else {
                records.remove(&current.id);
            }
        } else if let Some(base) = snapshot.base {
            records.remove(&base.id);
        }
    }
    let mut records = records.into_values().collect::<Vec<_>>();
    records.sort_by_key(|record| record.id.0);
    records
}

fn scope_contains(scope: DerivedJobScope, space: WorldSpaceId, cell: CellCoord) -> bool {
    match scope {
        DerivedJobScope::Cell {
            space: scope_space,
            cell: scope_cell,
        } => scope_space == space && scope_cell == cell,
        DerivedJobScope::Region {
            space: scope_space,
            minimum,
            maximum,
        } => {
            scope_space == space
                && cell.x >= minimum.x
                && cell.x <= maximum.x
                && cell.z >= minimum.z
                && cell.z <= maximum.z
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn key(cell: i32) -> DerivedJobKey {
        DerivedJobKey {
            product: DerivedProduct::CookedObjectPage,
            scope: DerivedJobScope::Cell {
                space: WorldSpaceId(1),
                cell: CellCoord { x: cell, z: 0 },
            },
        }
    }

    #[test]
    fn newer_revision_cancels_running_work_and_rejects_its_late_result() {
        let mut scheduler = DerivedJobScheduler::default();
        scheduler.replace_tool_requests(OBJECT_TOOL.id, [(key(1), 1)]);
        let old = scheduler.lease_next().unwrap();
        scheduler.replace_tool_requests(OBJECT_TOOL.id, [(key(1), 2)]);
        assert!(old.is_cancelled());
        assert_eq!(scheduler.complete(&old), DerivedJobCompletion::Stale);
        let current = scheduler.lease_next().unwrap();
        assert_eq!(current.input_revision, 2);
        assert_eq!(scheduler.complete(&current), DerivedJobCompletion::Accepted);
        assert_eq!(scheduler.stale_results(), 1);
    }

    #[test]
    fn a_failed_accepted_revision_cannot_leave_an_older_artifact_presented() {
        let key = key(1);
        let mut artifacts = DerivedArtifactStore::default();
        artifacts.publish(
            key,
            1,
            Ok(DerivedArtifact::CookedObjectPage {
                object_count: 1,
                fingerprint: 41,
            }),
        );
        assert!(artifacts.get(key).is_some());
        artifacts.publish(key, 2, Err("compile failed".into()));
        assert!(artifacts.get(key).is_none());
        assert_eq!(artifacts.failure(key), Some("compile failed"));
    }

    #[test]
    fn generation_adoption_retires_failures_without_dropping_valid_artifacts() {
        let valid = key(1);
        let failed = key(2);
        let mut artifacts = DerivedArtifactStore::default();
        artifacts.publish(
            valid,
            3,
            Ok(DerivedArtifact::CookedObjectPage {
                object_count: 2,
                fingerprint: 17,
            }),
        );
        artifacts.publish(failed, 3, Err("old failure".into()));

        artifacts.clear_failures_for_generation_adoption();
        assert!(artifacts.get(valid).is_some());
        assert!(artifacts.failure(failed).is_none());
    }

    #[test]
    fn replacing_tool_demand_cancels_obsolete_cells() {
        let mut scheduler = DerivedJobScheduler::default();
        scheduler.replace_tool_requests(OBJECT_TOOL.id, [(key(1), 1), (key(2), 1)]);
        scheduler.replace_tool_requests(OBJECT_TOOL.id, [(key(2), 1)]);
        assert_eq!(scheduler.pending_count(), 1);
        assert_eq!(scheduler.cancelled_count(), 1);
        assert_eq!(scheduler.lease_next().unwrap().key, key(2));
    }

    #[test]
    fn pending_and_running_queues_are_strictly_bounded() {
        let mut scheduler = DerivedJobScheduler {
            maximum_pending: 2,
            maximum_running: 1,
            ..default()
        };
        scheduler.replace_tool_requests(OBJECT_TOOL.id, [(key(1), 1), (key(2), 1), (key(3), 1)]);
        assert_eq!(scheduler.pending_count(), 2);
        assert_eq!(scheduler.capacity_rejections(), 1);
        assert!(scheduler.lease_next().is_some());
        assert!(scheduler.lease_next().is_none());
        assert_eq!(scheduler.running_count(), 1);
    }

    #[test]
    fn terrain_executor_builds_a_typed_bounded_artifact() {
        let artifact = build_artifact(
            DerivedProduct::TerrainPage,
            DerivedJobInput::Terrain(vec![SourceTerrainCellWeightPageRecord {
                space: WorldSpaceId(1),
                cell: CellCoord::ZERO,
                page: 0,
                resolution: 2,
                rgba: vec![
                    0, 64, 128, 255, 0, 64, 128, 255, 0, 64, 128, 255, 0, 64, 128, 255,
                ],
                source_revision: 1,
            }]),
        )
        .unwrap();
        let DerivedArtifact::TerrainPage {
            page_count,
            texel_count,
            average_weight,
            fingerprint,
        } = artifact
        else {
            panic!("terrain work should publish a terrain artifact");
        };
        assert_eq!(page_count, 1);
        assert_eq!(texel_count, 4);
        assert!(average_weight > 0.4 && average_weight < 0.5);
        assert_ne!(fingerprint, 0);
    }

    #[test]
    fn overview_executor_queries_one_concrete_database_region() {
        let database =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/demo.project.sqlite");
        let reader = ProjectReader::open_read_only(&database).unwrap();
        let space = reader.manifest().default_world_space;
        let artifact = build_overview_artifact(
            &reader,
            space,
            CellCoord { x: -8, z: -8 },
            CellCoord { x: 7, z: 7 },
        )
        .unwrap();
        let DerivedArtifact::Overview {
            coarse_heights,
            object_icons,
            cell_status,
            fingerprint,
        } = artifact
        else {
            panic!("overview work should publish the complete coarse product bundle");
        };
        assert!(!coarse_heights.is_empty());
        assert_eq!(coarse_heights.len(), cell_status.len());
        assert!(!object_icons.is_empty());
        assert_ne!(fingerprint, 0);
    }
}
