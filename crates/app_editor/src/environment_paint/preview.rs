//! Bounded, asynchronous source compilation. A result is accepted only while its dependency
//! stamp still matches the draft. Ground and vegetation share the same accepted product.
pub(super) mod live;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
    thread,
};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, bounded};
use engine::{StreamedTerrainSurface, WorldCatalog, WorldOrigin};
use environment_compile::{CompilePlan, CompiledCell};
use terrain_render::{
    PrepareTerrainMaterialContext, TerrainMacroVariation, TerrainMaterial, TerrainSurfaceLayer,
    prepare_terrain_material,
};
use world::{CellCoord, WorldSpaceId};
use world_db::{
    DenseSourceRecord, ProjectReader, SourceEnvironmentCellRecord, environment_dependency_cells,
};

use crate::{
    domain_editing::DenseDomainWorkingSets,
    project_store::{ProjectDatabasePath, ProjectEditorStore},
    vegetation_authoring::VegetationAuthoringState,
    workspaces::EditorWorkspace,
};

const MAX_PREVIEW_CELLS: usize = 256;
const CELLS_PER_JOB: usize = 4;
const MAX_PREVIEW_BYTES: usize = 32 * 1024 * 1024;
type Key = (WorldSpaceId, CellCoord);
type Stamp = [u8; 32];

struct Job {
    plan: Arc<CompilePlan>,
    space: WorldSpaceId,
    definition_revision: u64,
    library_revision: u64,
    cells: Vec<(CellCoord, Stamp)>,
    overrides: Vec<SourceEnvironmentCellRecord>,
    roads: Vec<crate::road_authoring::working::RoadChange>,
}
struct Completion {
    space: WorldSpaceId,
    cells: Vec<(CellCoord, Stamp)>,
    result: Result<Batch, String>,
}
struct Worker {
    sender: Option<Sender<Job>>,
    receiver: Receiver<Completion>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.sender.take();
        // At most one job exists. The result channel has room for that job even at shutdown.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Worker {
    fn start(path: std::path::PathBuf) -> Result<Self, String> {
        let (sender, requests) = bounded::<Job>(1);
        let (results, receiver) = bounded(1);
        let thread = thread::Builder::new()
            .name("environment-preview".into())
            .spawn(move || {
                let reader = ProjectReader::open_read_only(&path).map_err(|e| e.to_string());
                while let Ok(job) = requests.recv() {
                    let result = reader
                        .as_ref()
                        .map_err(Clone::clone)
                        .and_then(|reader| compile_job(reader, &job));
                    if results
                        .send(Completion {
                            space: job.space,
                            cells: job.cells,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sender: Some(sender),
            receiver,
            thread: Some(thread),
        })
    }
}
#[cfg(test)]
impl From<Vec<CompiledCell>> for Batch {
    fn from(cells: Vec<CompiledCell>) -> Self {
        Self {
            cells,
            assets: vec![],
        }
    }
}
struct Batch {
    cells: Vec<CompiledCell>,
    assets: Vec<world_db::CollectionAssetView>,
}
fn compile_job(reader: &ProjectReader, job: &Job) -> Result<Batch, String> {
    let assets = reader
        .read_collection_assets(&job.plan.collection_assets())
        .map_err(|e| e.to_string())?;
    let cells = job.cells.iter().map(|(cell, _)| *cell).collect::<Vec<_>>();
    let cells = compile_source_cells_with_roads(
        reader,
        &job.plan,
        job.space,
        job.definition_revision,
        job.library_revision,
        &cells,
        &job.overrides,
        &job.roads,
    )?;
    Ok(Batch { cells, assets })
}

#[cfg(test)]
pub(super) fn compile_source_cells(
    reader: &ProjectReader,
    plan: &CompilePlan,
    space: WorldSpaceId,
    definition_revision: u64,
    library_revision: u64,
    cells: &[CellCoord],
    overrides: &[SourceEnvironmentCellRecord],
) -> Result<Vec<CompiledCell>, String> {
    compile_source_cells_with_roads(
        reader,
        plan,
        space,
        definition_revision,
        library_revision,
        cells,
        overrides,
        &[],
    )
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_source_cells_with_roads(
    reader: &ProjectReader,
    plan: &CompilePlan,
    space: WorldSpaceId,
    definition_revision: u64,
    library_revision: u64,
    cells: &[CellCoord],
    overrides: &[SourceEnvironmentCellRecord],
    roads: &[crate::road_authoring::working::RoadChange],
) -> Result<Vec<CompiledCell>, String> {
    let halo = environment_dependency_cells(cells).map_err(|e| e.to_string())?;
    if cells.len() > 1 {
        let mut result = vec![];
        for cell in cells {
            result.extend(compile_source_cells_with_roads(
                reader,
                plan,
                space,
                definition_revision,
                library_revision,
                &[*cell],
                overrides,
                roads,
            )?);
        }
        return Ok(result);
    }
    let Some(&cell) = cells.first() else {
        return Ok(vec![]);
    };
    let bounds = environment::roads::RoadCellBounds {
        minimum: world::CellCoord {
            x: cell.x - 1,
            z: cell.z - 1,
        },
        maximum: world::CellCoord {
            x: cell.x + 1,
            z: cell.z + 1,
        },
    };
    let input = reader
        .read_road_terrain_source(space, &halo, bounds)
        .map_err(|e| e.to_string())?;
    let source = input.source;
    if source.roads.roads.truncated {
        return Err("Road preview query is incomplete".into());
    }
    let mut snapshot = source.environment;
    let mut records = world_db::road_snapshot_records(&source.roads.roads);
    for change in roads {
        if let Some(r) = &change.record {
            records.insert(change.key, r.clone());
        } else {
            records.remove(&change.key);
        }
    }
    let roads = world_db::road_snapshot_from_records(
        space,
        snapshot.definition.cell_size,
        bounds,
        &records,
    )
    .map_err(|e| e.to_string())?;
    if snapshot.presets.revision != library_revision {
        return Err(
            "Shared presets changed in the database. Resolve or reload before previewing.".into(),
        );
    }
    if snapshot.definition.revision != definition_revision {
        return Err("The layer definition changed. Reload the project before painting.".into());
    }
    for record in overrides {
        if record.definition_revision != definition_revision {
            return Err("A painted cell belongs to a different layer definition.".into());
        }
        if let Some(cell) = snapshot
            .coverage
            .cells
            .iter_mut()
            .find(|cell| cell.cell == record.cell)
        {
            *cell = record.coverage();
        }
    }
    plan.compile_cell_with_terrain(
        cell,
        &snapshot.coverage,
        &roads,
        &input.terrain,
        Default::default(),
    )
    .map(|cell| vec![cell])
    .map_err(|e| e.to_string())
}

#[derive(Resource, Default)]
pub(crate) struct EnvironmentPreview {
    pub(super) assets: BTreeMap<world::AssetId, world_db::CollectionAssetView>,
    worker: Option<Worker>,
    in_flight: bool,
    context: Option<(WorldSpaceId, u64, u64, u64, String)>,
    plan: Option<Arc<CompilePlan>>,
    draft_revision: u64,
    road_revision: u64,
    resident: Vec<CellCoord>,
    overrides: Vec<SourceEnvironmentCellRecord>,
    roads: Vec<crate::road_authoring::working::RoadChange>,
    desired: BTreeMap<Key, Stamp>,
    accepted: BTreeMap<Key, (Stamp, CompiledCell)>,
    staging: Option<BTreeMap<Key, (Stamp, CompiledCell)>>,
    active_catalog: Option<vegetation::VegetationCatalog>,
    pending_catalog: Option<vegetation::VegetationCatalog>,
    failed: BTreeMap<Key, Stamp>,
    pub(crate) revision: u64,
    pub(crate) error: Option<String>,
}
impl EnvironmentPreview {
    pub(crate) fn cell(&self, space: WorldSpaceId, cell: CellCoord) -> Option<&CompiledCell> {
        self.accepted.get(&(space, cell)).map(|(_, cell)| cell)
    }
    pub(crate) fn catalog(&self) -> Option<&vegetation::VegetationCatalog> {
        self.active_catalog.as_ref()
    }
    pub(super) fn visible_cells(&self) -> &BTreeMap<Key, (Stamp, CompiledCell)> {
        &self.accepted
    }
    fn products(&self) -> &BTreeMap<Key, (Stamp, CompiledCell)> {
        self.staging.as_ref().unwrap_or(&self.accepted)
    }
    pub(crate) fn pending(&self) -> usize {
        self.desired
            .iter()
            .filter(|(key, stamp)| self.products().get(key).map(|(s, _)| s) != Some(stamp))
            .count()
    }
    fn install_staging(&mut self) {
        if self.staging.is_some() && !self.desired.is_empty() && self.pending() == 0 {
            self.accepted = self.staging.take().unwrap();
            if let Some(catalog) = self.pending_catalog.take() {
                self.active_catalog = Some(catalog);
            }
            self.bump();
        }
    }
    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
    }
}

/// Includes only the local cells in this output's halo, so an unrelated stroke cannot discard it.
fn dependency_stamp(
    plan: Stamp,
    epoch: u64,
    generation: &str,
    cell: CellCoord,
    overrides: &[(CellCoord, Stamp)],
) -> Stamp {
    let mut hash = blake3::Hasher::new();
    hash.update(&plan);
    hash.update(&epoch.to_le_bytes());
    hash.update(generation.as_bytes());
    hash.update(&cell.x.to_le_bytes());
    hash.update(&cell.z.to_le_bytes());
    for (neighbor, stamp) in overrides {
        if (i64::from(neighbor.x) - i64::from(cell.x)).abs() <= 1
            && (i64::from(neighbor.z) - i64::from(cell.z)).abs() <= 1
        {
            hash.update(&neighbor.x.to_le_bytes());
            hash.update(&neighbor.z.to_le_bytes());
            hash.update(stamp);
        }
    }
    *hash.finalize().as_bytes()
}
fn override_stamp(record: &SourceEnvironmentCellRecord) -> Stamp {
    let mut hash = blake3::Hasher::new();
    hash.update(&record.definition_revision.to_le_bytes());
    let mut tiles = record.tiles.iter().collect::<Vec<_>>();
    tiles.sort_by_key(|tile| tile.layer);
    for tile in tiles {
        hash.update(&tile.layer.0);
        hash.update(&(tile.samples.len() as u64).to_le_bytes());
        hash.update(&tile.samples);
    }
    *hash.finalize().as_bytes()
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PreviewSource<'w, 's> {
    lod: Res<'w, engine::TerrainLodPreview>,
    project: Res<'w, ProjectEditorStore>,
    dense: Res<'w, DenseDomainWorkingSets>,
    plants: Res<'w, VegetationAuthoringState>,
    origin: Res<'w, WorldOrigin>,
    runtime: Res<'w, WorldCatalog>,
    workspace: Res<'w, State<EditorWorkspace>>,
    terrain: Query<'w, 's, &'static StreamedTerrainSurface>,
}
impl PreviewSource<'_, '_> {
    fn refresh(&self, preview: &mut EnvironmentPreview) {
        let space = self.origin.space();
        let definition = space.and_then(|space| self.dense.definition(space));
        let plants = self.plants.study_source();
        let presets = self.dense.presets();
        let (Some(space), Some(definition), Some((plants, _, plants_revision)), Some(presets)) =
            (space, definition, plants, presets)
        else {
            if !preview.accepted.is_empty() || preview.active_catalog.is_some() {
                preview.accepted.clear();
                preview.active_catalog = None;
                preview.bump();
            }
            preview.staging = None;
            preview.pending_catalog = None;
            preview.failed.clear();
            preview.desired.clear();
            preview.resident.clear();
            preview.overrides.clear();
            preview.plan = None;
            preview.context = None;
            return;
        };
        let context = (
            space,
            self.dense.definition_edit_revision(),
            plants_revision,
            self.project.source_epoch(),
            self.runtime.generation_id().to_owned(),
        );
        let context_changed = preview.context.as_ref() != Some(&context);
        if context_changed {
            if preview
                .context
                .as_ref()
                .is_some_and(|old| old.0 != space || old.4 != context.4)
            {
                preview.accepted.clear();
                preview.active_catalog = None;
                preview.bump();
            }
            let old_plan = preview.plan.as_ref().map(|plan| plan.fingerprint());
            preview.plan = match CompilePlan::new(
                definition,
                plants,
                presets,
                environment_compile::CompileProfile {
                    terrain_resolution: self
                        .project
                        .terrain_resources(space)
                        .map_or(65, |r| r.profile.weight_resolution),
                    ..Default::default()
                },
            ) {
                Ok(plan) => {
                    preview.error = None;
                    Some(Arc::new(plan))
                }
                Err(error) => {
                    preview.error = Some(error.to_string());
                    None
                }
            };
            if preview.plan.as_ref().map(|plan| plan.fingerprint()) != old_plan
                || preview.active_catalog.is_none()
            {
                let plans = self
                    .dense
                    .definitions()
                    .map(|d| CompilePlan::new(d, plants, presets, Default::default()))
                    .collect::<Result<Vec<_>, _>>();
                match plans.and_then(|plans| {
                    environment_compile::merge_runtime_catalogs(&plans.iter().collect::<Vec<_>>())
                }) {
                    Ok(catalog) => {
                        preview.pending_catalog = Some(catalog);
                        preview.staging = Some(BTreeMap::new());
                    }
                    Err(error) => {
                        preview.error = Some(error.to_string());
                        preview.plan = None;
                    }
                }
            }
            preview.failed.clear();
            preview.context = Some(context);
        }
        let mut resident = self
            .terrain
            .iter()
            .filter(|terrain| terrain.key.space == space)
            .map(|terrain| terrain.key.cell)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        // Keep the visible neighborhood nearest the camera first if streaming exceeds the budget.
        let center = self.origin.cell();
        resident.sort_by_key(|cell| {
            (i64::from(cell.x) - i64::from(center.x)).abs()
                + (i64::from(cell.z) - i64::from(center.z)).abs()
        });
        // Bound the retained product bytes as well as cell count, even with 64 vegetation fields.
        let terrain_resolution = self
            .project
            .terrain_resources(space)
            .map_or(65, |r| usize::from(r.profile.weight_resolution));
        let fields = preview
            .plan
            .as_ref()
            .map_or(64, |plan| plan.bindings().len().min(64));
        let height_resolution = self
            .terrain
            .iter()
            .map(|p| usize::from(p.heightfield.resolution))
            .max()
            .unwrap_or(33)
            .max(33);
        let bytes_per_cell =
            terrain_resolution.pow(2) * 4 + fields * 64 * 64 + height_resolution.pow(2) * 6 + 4096;
        // Reserve half for a pending catalog change while the accepted preview remains visible.
        resident.truncate(MAX_PREVIEW_CELLS.min(MAX_PREVIEW_BYTES / 2 / bytes_per_cell));
        if *self.workspace.get() != EditorWorkspace::World {
            resident.clear();
        }
        if !context_changed
            && preview.road_revision == self.dense.roads.revision
            && preview.draft_revision == self.dense.edit_revision()
            && preview.resident == resident
        {
            return;
        }
        // Height edits must switch neighboring accepted cells together: publishing each
        // worker result separately would briefly expose cracks between old/new surfaces.
        if preview.road_revision != self.dense.roads.revision
            && preview.staging.is_none()
            && !preview.accepted.is_empty()
        {
            preview.staging = Some(preview.accepted.clone());
        }
        preview.draft_revision = self.dense.edit_revision();
        preview.road_revision = self.dense.roads.revision;
        preview.roads = self.dense.roads.overrides();
        let road_regions = self
            .dense
            .roads
            .preview_regions(space, definition.cell_size);
        let road_stamp = blake3::hash(ron::to_string(&preview.roads).unwrap().as_bytes());
        preview.resident = resident.clone();
        let mut records = self
            .dense
            .preview_records()
            .into_iter()
            .map(|record| {
                let DenseSourceRecord::EnvironmentCoverage(record) = record;
                record
            })
            .filter(|record| record.space == space)
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.cell);
        let stamps = records
            .iter()
            .map(|record| (record.cell, override_stamp(record)))
            .collect::<Vec<_>>();
        preview.desired.clear();
        if *self.workspace.get() == EditorWorkspace::World
            && let Some(plan) = &preview.plan
        {
            for cell in resident {
                preview.desired.insert((space, cell), {
                    let mut hash = blake3::Hasher::new();
                    hash.update(&dependency_stamp(
                        plan.fingerprint(),
                        self.project.source_epoch(),
                        self.runtime.generation_id(),
                        cell,
                        &stamps,
                    ));
                    if road_regions
                        .as_ref()
                        .is_none_or(|regions| regions.iter().any(|b| b.contains(cell)))
                    {
                        hash.update(road_stamp.as_bytes());
                    }
                    *hash.finalize().as_bytes()
                });
            }
        }
        let count = preview.accepted.len();
        preview
            .accepted
            .retain(|key, _| preview.desired.contains_key(key));
        preview
            .failed
            .retain(|key, stamp| preview.desired.get(key) == Some(stamp));
        if count != preview.accepted.len() {
            preview.bump();
        }
        if let Some(staging) = &mut preview.staging {
            staging.retain(|key, (stamp, _)| preview.desired.get(key) == Some(stamp));
        }
        preview.overrides = records;
        preview.install_staging();
    }
}

pub(crate) fn receive_preview(source: PreviewSource, mut preview: ResMut<EnvironmentPreview>) {
    if source.lod.enabled {
        return;
    }
    source.refresh(&mut preview);
    let completion = preview
        .worker
        .as_ref()
        .and_then(|worker| worker.receiver.try_recv().ok());
    let Some(completion) = completion else {
        return;
    };
    preview.accept(completion);
}
impl EnvironmentPreview {
    fn accept(&mut self, completion: Completion) {
        self.in_flight = false;
        match completion.result {
            Ok(batch) => {
                // Asset descriptors are immutable source catalog data. Keep only dependencies
                // of accepted/staged products after processing this completion.
                self.assets
                    .extend(batch.assets.into_iter().map(|a| (a.id, a)));
                for cell in batch.cells {
                    let key = (cell.space, cell.cell);
                    let stamp = completion
                        .cells
                        .iter()
                        .find(|(coord, _)| *coord == cell.cell)
                        .map(|(_, stamp)| *stamp)
                        .unwrap();
                    if self.desired.get(&key) == Some(&stamp) {
                        let accepted_bytes = self
                            .accepted
                            .iter()
                            .filter(|(k, _)| self.staging.is_some() || **k != key)
                            .map(|(_, (_, c))| compiled_bytes(c))
                            .sum::<usize>();
                        let staging_bytes = self.staging.as_ref().map_or(0, |s| {
                            s.iter()
                                .filter(|(k, _)| **k != key)
                                .map(|(_, (_, c))| compiled_bytes(c))
                                .sum::<usize>()
                        });
                        let object_count = self
                            .products()
                            .iter()
                            .filter(|(k, _)| **k != key)
                            .map(|(_, (_, c))| c.objects.len())
                            .sum::<usize>()
                            + cell.objects.len();
                        if object_count > 4096
                            || accepted_bytes + staging_bytes + compiled_bytes(&cell)
                                > MAX_PREVIEW_BYTES
                        {
                            self.failed.insert(key, stamp);
                            self.error=Some("Environment preview exceeds its memory or 4,096-object budget; reduce density, resident area or output resolution.".into());
                            continue;
                        }
                        if let Some(staging) = &mut self.staging {
                            staging.insert(key, (stamp, cell));
                        } else {
                            self.accepted.insert(key, (stamp, cell));
                            self.bump();
                        }
                        self.failed.remove(&key);
                    }
                }
                self.install_staging();
                let used: BTreeSet<_> = self
                    .accepted
                    .values()
                    .chain(self.staging.iter().flat_map(|s| s.values()))
                    .flat_map(|(_, c)| c.objects.iter().map(|o| o.asset))
                    .collect();
                self.assets.retain(|id, _| used.contains(id));
                if self.failed.is_empty() && self.plan.is_some() {
                    self.error = None;
                }
            }
            Err(error) => {
                for (cell, stamp) in completion.cells {
                    let key = (completion.space, cell);
                    if self.desired.get(&key) == Some(&stamp) {
                        self.failed.insert(key, stamp);
                        self.error = Some(error.clone());
                    }
                }
            }
        }
    }
}

fn compiled_bytes(cell: &CompiledCell) -> usize {
    1024 + cell.objects.len() * std::mem::size_of::<world::StaticObjectInstance>()
        + cell
            .ground
            .weight_pages
            .iter()
            .map(|p| p.rgba.len())
            .sum::<usize>()
        + cell
            .vegetation
            .fields
            .iter()
            .map(|f| f.coverage.len() + 128)
            .sum::<usize>()
        + cell
            .terrain
            .as_ref()
            .map_or(0, |h| h.heights.len() * 4 + h.normals_oct.len() * 4)
}

pub(super) fn queue_preview(
    source: PreviewSource,
    path: Res<ProjectDatabasePath>,
    paint: Res<super::EnvironmentPaintState>,
    mut preview: ResMut<EnvironmentPreview>,
) {
    if source.lod.enabled {
        return;
    }
    source.refresh(&mut preview);
    if preview.in_flight {
        return;
    }
    let Some(plan) = preview.plan.clone() else {
        return;
    };
    let mut cells = preview
        .desired
        .iter()
        .filter(|(key, stamp)| {
            preview.products().get(key).map(|(s, _)| s) != Some(stamp)
                && preview.failed.get(key) != Some(stamp)
        })
        .map(|(key, stamp)| (key.1, *stamp))
        .collect::<Vec<_>>();
    // Changed cells precede first-time baseline previews; prioritize the cursor within each group.
    let focus = paint
        .hover
        .map(|hit| hit.cell)
        .unwrap_or(source.origin.cell());
    cells.sort_by_key(|(cell, _)| {
        (
            !preview.overrides.iter().any(|record| {
                (i64::from(record.cell.x) - i64::from(cell.x)).abs() <= 1
                    && (i64::from(record.cell.z) - i64::from(cell.z)).abs() <= 1
            }),
            (i64::from(cell.x) - i64::from(focus.x)).abs()
                + (i64::from(cell.z) - i64::from(focus.z)).abs(),
        )
    });
    cells.truncate(CELLS_PER_JOB);
    if cells.is_empty() {
        return;
    }
    if preview.worker.is_none() {
        match Worker::start(path.0.clone()) {
            Ok(worker) => preview.worker = Some(worker),
            Err(error) => {
                preview.error = Some(error);
                return;
            }
        }
    }
    let (space, _, _, _, _) = preview.context.as_ref().unwrap();
    let definition_revision = source.dense.definition(*space).unwrap().revision;
    let overrides = preview
        .overrides
        .iter()
        .filter(|record| {
            cells.iter().any(|(cell, _)| {
                (i64::from(record.cell.x) - i64::from(cell.x)).abs() <= 1
                    && (i64::from(record.cell.z) - i64::from(cell.z)).abs() <= 1
            })
        })
        .cloned()
        .collect();
    let job = Job {
        plan,
        space: *space,
        definition_revision,
        library_revision: source.dense.preset_base_revision(),
        roads: preview.roads.clone(),
        cells,
        overrides,
    };
    match preview
        .worker
        .as_ref()
        .unwrap()
        .sender
        .as_ref()
        .unwrap()
        .try_send(job)
    {
        Ok(()) => preview.in_flight = true,
        Err(error) => preview.error = Some(error.to_string()),
    }
}

struct MaterialOverride {
    original: Handle<TerrainMaterial>,
    material: Handle<TerrainMaterial>,
    weights: Handle<Image>,
    fingerprint: Stamp,
    terrain: Option<TerrainOverride>,
}
struct TerrainOverride {
    original_mesh: Handle<Mesh>,
    original_heightfield: world::TerrainHeightfield,
    original_scale: Vec3,
    original_y: f32,
    mesh: Handle<Mesh>,
}
#[derive(Resource, Default)]
pub(super) struct PreviewMaterials {
    entries: HashMap<Entity, MaterialOverride>,
}
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_ground_preview(
    mut commands: Commands,
    project: Res<ProjectEditorStore>,
    preview: Res<EnvironmentPreview>,
    origin: Res<WorldOrigin>,
    server: Res<AssetServer>,
    variation: Res<TerrainMacroVariation>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut owned: ResMut<PreviewMaterials>,
    mut terrain: Query<(
        Entity,
        &mut StreamedTerrainSurface,
        &mut MeshMaterial3d<TerrainMaterial>,
        &mut Mesh3d,
        &mut Transform,
    )>,
) {
    owned.entries.retain(|entity, entry| {
        let keep = terrain.get(*entity).is_ok_and(|(_, surface, _, _, _)| {
            preview.cell(surface.key.space, surface.key.cell).is_some()
        });
        if !keep {
            if let Ok((_, mut surface, mut handle, mut mesh, mut transform)) =
                terrain.get_mut(*entity)
            {
                handle.0 = entry.original.clone();
                if let Some(original) = &entry.terrain {
                    mesh.0 = original.original_mesh.clone();
                    surface.heightfield = original.original_heightfield.clone();
                    transform.scale = original.original_scale;
                    transform.translation.y = original.original_y;
                    commands
                        .entity(*entity)
                        .remove::<bevy::camera::primitives::Aabb>();
                }
            }
            if let Some(original) = &entry.terrain {
                meshes.remove(original.mesh.id());
            }
            materials.remove(entry.material.id());
            images.remove(entry.weights.id());
        }
        keep
    });
    for (entity, mut surface, mut handle, mut mesh, mut transform) in &mut terrain {
        let Some(cell) = preview.cell(surface.key.space, surface.key.cell) else {
            continue;
        };
        if owned
            .entries
            .get(&entity)
            .is_some_and(|entry| entry.fingerprint == cell.input_fingerprint)
        {
            continue;
        }
        let Some(resources) = project.terrain_resources(surface.key.space) else {
            continue;
        };
        // Prepare everything before replacing a currently accepted material or surface.
        let changed_height = cell.terrain.as_ref().filter(|h| **h != surface.heightfield);
        let new_mesh = match changed_height
            .map(|h| terrain_render::build_heightfield_mesh(h, surface.cell_size))
            .transpose()
        {
            Ok(mesh) => mesh,
            Err(_) => continue,
        };
        let layers = cell
            .ground
            .surfaces
            .iter()
            .filter_map(|id| {
                resources
                    .surfaces
                    .iter()
                    .find(|s| s.surface.id == *id)
                    .map(|s| TerrainSurfaceLayer {
                        surface: s.surface.clone(),
                        layer: s.layer,
                    })
            })
            .collect::<Vec<_>>();
        let prepared = prepare_terrain_material(PrepareTerrainMaterialContext {
            asset_server: &server,
            images: &mut images,
            materials: &mut materials,
            cell: cell.cell,
            origin_cell: origin.cell(),
            cell_size: surface.cell_size,
            page_surfaces: &cell.ground.surfaces,
            weight_pages: &cell.ground.weight_pages,
            profile: &resources.profile,
            texture_set: &resources.texture_set,
            surfaces: &layers,
            macro_variation: *variation,
        });
        let Ok(prepared) = prepared else {
            continue;
        };
        let (original, mut terrain_override) = if let Some(old) = owned.entries.remove(&entity) {
            materials.remove(old.material.id());
            images.remove(old.weights.id());
            (old.original, old.terrain)
        } else {
            (handle.0.clone(), None)
        };
        if let (Some(new_mesh), Some(heightfield)) = (new_mesh, changed_height) {
            let next_mesh = meshes.add(new_mesh);
            if let Some(previous) = &mut terrain_override {
                meshes.remove(previous.mesh.id());
                previous.mesh = next_mesh.clone();
            } else {
                terrain_override = Some(TerrainOverride {
                    original_mesh: mesh.0.clone(),
                    original_heightfield: surface.heightfield.clone(),
                    original_scale: transform.scale,
                    original_y: transform.translation.y,
                    mesh: next_mesh.clone(),
                });
            }
            mesh.0 = next_mesh;
            surface.heightfield = heightfield.clone();
            transform.scale = Vec3::ONE;
            transform.translation.y = 0.0;
            commands
                .entity(entity)
                .remove::<bevy::camera::primitives::Aabb>();
        }
        handle.0 = prepared.material.clone();
        owned.entries.insert(
            entity,
            MaterialOverride {
                original,
                material: prepared.material,
                weights: prepared.weight_image,
                fingerprint: cell.input_fingerprint,
                terrain: terrain_override,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relief_results_switch_neighboring_surfaces_together_and_keep_the_catalog() {
        let space = WorldSpaceId(1);
        let a = CellCoord::ZERO;
        let b = CellCoord { x: 1, z: 0 };
        let product = |cell, version, height| CompiledCell {
            objects: vec![],
            space,
            cell,
            ground: environment_compile::CompiledGround {
                surfaces: vec![],
                weight_pages: vec![],
            },
            vegetation: vegetation::VegetationFieldPageData { fields: vec![] },
            input_fingerprint: [version; 32],
            terrain: Some(
                world::TerrainHeightfield::from_heights(2, &[height; 4], -1.0, 1.0, 8.0).unwrap(),
            ),
        };
        let mut preview = EnvironmentPreview {
            active_catalog: Some(vegetation::fixtures::reference_catalog()),
            ..Default::default()
        };
        for cell in [a, b] {
            preview
                .accepted
                .insert((space, cell), ([1; 32], product(cell, 1, 0.0)));
            preview.desired.insert((space, cell), [2; 32]);
        }
        preview.staging = Some(preview.accepted.clone());
        let original = preview.cell(space, a).unwrap().terrain.clone();
        preview.accept(Completion {
            space,
            cells: vec![(a, [2; 32])],
            result: Ok(vec![product(a, 2, -0.2)].into()),
        });
        assert_eq!(preview.cell(space, a).unwrap().terrain, original);
        assert_eq!(preview.cell(space, b).unwrap().terrain, original);
        preview.accept(Completion {
            space,
            cells: vec![(b, [2; 32])],
            result: Ok(vec![product(b, 2, -0.2)].into()),
        });
        assert!(preview.staging.is_none());
        assert!(preview.catalog().is_some());
        assert_ne!(preview.cell(space, a).unwrap().terrain, original);
        assert_eq!(
            preview.cell(space, a).unwrap().terrain,
            preview.cell(space, b).unwrap().terrain
        );
    }
    #[test]
    fn metadata_preview_swaps_all_resident_cells_with_their_catalog() {
        let space = WorldSpaceId(1);
        let cells = [CellCoord::ZERO, CellCoord { x: 1, z: 0 }];
        let product = |cell, version| CompiledCell {
            objects: vec![],
            terrain: None,
            space,
            cell,
            ground: environment_compile::CompiledGround {
                surfaces: vec![],
                weight_pages: vec![],
            },
            vegetation: vegetation::VegetationFieldPageData { fields: vec![] },
            input_fingerprint: [version; 32],
        };
        let old_catalog = vegetation::fixtures::reference_catalog();
        let mut new_catalog = old_catalog.clone();
        new_catalog.populations[0].seed = new_catalog.populations[0].seed.wrapping_add(1);
        let mut preview = EnvironmentPreview {
            active_catalog: Some(old_catalog.clone()),
            pending_catalog: Some(new_catalog.clone()),
            staging: Some(BTreeMap::new()),
            ..Default::default()
        };
        for cell in cells {
            preview
                .accepted
                .insert((space, cell), ([1; 32], product(cell, 1)));
            preview.desired.insert((space, cell), [2; 32]);
        }
        let completion = |cell, version| Completion {
            space,
            cells: vec![(cell, [version; 32])],
            result: Ok(vec![product(cell, version)].into()),
        };
        preview.accept(completion(cells[0], 2));
        assert_eq!(preview.pending(), 1);
        assert_eq!(preview.catalog(), Some(&old_catalog));
        assert_eq!(
            preview.cell(space, cells[0]).unwrap().input_fingerprint,
            [1; 32]
        );
        preview.accept(completion(cells[1], 1)); // Obsolete job cannot complete the swap.
        assert_eq!(preview.pending(), 1);
        preview.accept(completion(cells[1], 2));
        assert_eq!(preview.pending(), 0);
        assert_eq!(preview.catalog(), Some(&new_catalog));
        assert!(preview.staging.is_none());
        for cell in cells {
            assert_eq!(
                preview.cell(space, cell).unwrap().input_fingerprint,
                [2; 32]
            );
        }
    }
    #[test]
    fn preview_stamp_tracks_halo_but_not_unrelated_strokes() {
        let cell = CellCoord { x: 0, z: 0 };
        let stamp =
            |overrides: &[(CellCoord, Stamp)]| dependency_stamp([3; 32], 1, "a", cell, overrides);
        assert_eq!(stamp(&[]), stamp(&[(CellCoord { x: 2, z: 0 }, [1; 32])]));
        assert_ne!(stamp(&[]), stamp(&[(CellCoord { x: 1, z: 1 }, [1; 32])]));
        assert_ne!(stamp(&[]), dependency_stamp([3; 32], 2, "a", cell, &[]));
        assert_ne!(stamp(&[]), dependency_stamp([3; 32], 1, "b", cell, &[]));
    }
    #[test]
    fn obsolete_worker_results_never_replace_a_newer_preview() {
        let space = WorldSpaceId(1);
        let cell = CellCoord { x: 0, z: 0 };
        let product = CompiledCell {
            objects: vec![],
            terrain: None,
            space,
            cell,
            ground: environment_compile::CompiledGround {
                surfaces: vec![],
                weight_pages: vec![],
            },
            vegetation: vegetation::VegetationFieldPageData { fields: vec![] },
            input_fingerprint: [0; 32],
        };
        let completion = |stamp| Completion {
            space,
            cells: vec![(cell, stamp)],
            result: Ok(vec![product.clone()].into()),
        };
        let mut preview = EnvironmentPreview::default();
        preview.desired.insert((space, cell), [2; 32]);
        preview.accept(completion([1; 32]));
        assert!(preview.cell(space, cell).is_none());
        preview.accept(completion([2; 32]));
        assert!(preview.cell(space, cell).is_some());
        let accepted_revision = preview.revision;
        preview.desired.insert((space, cell), [3; 32]);
        preview.accept(completion([2; 32]));
        assert_eq!(preview.revision, accepted_revision);
        assert_eq!(preview.pending(), 1);
        preview.accept(completion([3; 32]));
        assert_eq!(preview.pending(), 0);
        // A result from a no-longer-resident world is also ignored.
        preview.desired.clear();
        preview.accept(completion([3; 32]));
        assert_eq!(preview.revision, accepted_revision + 1);
    }
}
