//! Applied source edits on the distant renderer. One cancellable CPU worker and
//! one GPU-staged revision; local source demand remains independent of visibility.
use super::*;
use crate::publication::RuntimePublicationPaths;
use engine::{LiveTerrainPreview, TerrainPreviewRequest};
use environment::roads::RoadCellBounds;
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use world::{TerrainHeightfieldPage, TerrainPreviewProducts};
use world_db::{RuntimeReader, TerrainRenderResources};
type SourceContext = (WorldSpaceId, u64, u64, u64, String);
type SourceSignature = (SourceContext, u64, u64, Vec<CellCoord>);
#[cfg(test)]
mod tests;

#[derive(Clone)]
struct Input {
    revision: u64,
    context: SourceContext,
    plan: Arc<CompilePlan>,
    catalog: vegetation::VegetationCatalog,
    definition_revision: u64,
    library_revision: u64,
    records: Vec<SourceEnvironmentCellRecord>,
    roads: Vec<crate::road_authoring::working::RoadChange>,
    regions: Option<Vec<RoadCellBounds>>,
    resident: Vec<CellCoord>,
    resources: TerrainRenderResources,
    global: bool,
    base: Option<Arc<Snapshot>>,
}
struct Snapshot {
    input: InputSummary,
    cells: BTreeMap<Key, (Stamp, CompiledCell)>,
    assets: BTreeMap<world::AssetId, world_db::CollectionAssetView>,
    products: Arc<TerrainPreviewProducts>,
    catalog: vegetation::VegetationCatalog,
}
struct InputSummary {
    context: SourceContext,
    plan: Stamp,
    records: Vec<SourceEnvironmentCellRecord>,
    regions: Option<Vec<RoadCellBounds>>,
}
struct ResultMessage {
    revision: u64,
    result: Result<Arc<Snapshot>, String>,
}
struct LiveWorker {
    sender: Sender<Input>,
    receiver: Receiver<ResultMessage>,
    latest: Arc<AtomicU64>,
}
impl Drop for LiveWorker {
    fn drop(&mut self) {
        self.latest.store(u64::MAX, Ordering::Relaxed);
    }
}
impl LiveWorker {
    fn start(paths: &RuntimePublicationPaths) -> Result<Self, String> {
        let project = paths.project_database.clone();
        let runtime = paths.runtime_database.clone();
        let assets = paths.asset_root.clone();
        let (sender, requests) = bounded::<Input>(1);
        let (results, receiver) = bounded(1);
        let latest = Arc::new(AtomicU64::new(0));
        let token = latest.clone();
        thread::Builder::new()
            .name("live-terrain-preview".into())
            .spawn(move || {
                let library =
                    world_cook::TerrainBakeLibrary::load(&assets).map_err(|e| format!("{e:#}"));
                while let Ok(job) = requests.recv() {
                    let result = library
                        .as_ref()
                        .map_err(Clone::clone)
                        .and_then(|library| compile(&project, &runtime, library, &job, &token));
                    if results
                        .send(ResultMessage {
                            revision: job.revision,
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
            sender,
            receiver,
            latest,
        })
    }
}

#[derive(Resource, Default)]
pub(crate) struct LivePreviewState {
    worker: Option<LiveWorker>,
    in_flight: bool,
    revision: u64,
    signature: Option<SourceSignature>,
    queued: Option<Input>,
    queued_at: Option<Instant>,
    accepted: Option<Arc<Snapshot>>,
    pending: Option<(u64, Arc<Snapshot>)>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update(
    source: PreviewSource,
    paths: Res<RuntimePublicationPaths>,
    mut state: ResMut<LivePreviewState>,
    mut preview: ResMut<EnvironmentPreview>,
    mut live: ResMut<LiveTerrainPreview>,
) {
    if !source.lod.enabled || *source.workspace.get() != EditorWorkspace::World {
        return;
    }
    let (Some(space), Some((plants, _, plants_revision)), Some(presets)) = (
        source.origin.space(),
        source.plants.study_source(),
        source.dense.presets(),
    ) else {
        return;
    };
    let (Some(definition), Some(resources)) = (
        source.dense.definition(space),
        source.project.terrain_resources(space),
    ) else {
        return;
    };
    let context = (
        space,
        source.dense.definition_edit_revision(),
        plants_revision,
        source.project.source_epoch(),
        source.runtime.generation_id().to_owned(),
    );
    let mut resident: Vec<_> = source
        .terrain
        .iter()
        .filter(|t| t.key.space == space)
        .map(|t| t.key.cell)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let center = source.origin.cell();
    resident.sort_by_key(|c| {
        (i64::from(c.x) - i64::from(center.x)).abs() + (i64::from(c.z) - i64::from(center.z)).abs()
    });
    resident.truncate(MAX_PREVIEW_CELLS);
    let signature = (
        context.clone(),
        source.dense.edit_revision(),
        source.dense.roads.revision,
        resident.clone(),
    );
    if state.signature.as_ref() != Some(&signature) {
        let changed_world = state
            .accepted
            .as_ref()
            .is_some_and(|s| s.input.context.0 != space || s.input.context.4 != context.4);
        if changed_world {
            state.accepted = None;
            preview.accepted.clear();
            preview.active_catalog = None;
            preview.bump();
            live.applied = None;
        }
        state.signature = Some(signature);
        state.revision = state.revision.wrapping_add(1).max(1);
        if let Some(worker) = &state.worker {
            worker.latest.store(state.revision, Ordering::Relaxed);
        }
        state.pending = None;
        state.queued = None;
        live.request = None;
        live.ready = None;
        live.commit = None;
        live.error = None;
        let plan = CompilePlan::new(
            definition,
            plants,
            presets,
            environment_compile::CompileProfile {
                terrain_resolution: resources.profile.weight_resolution,
                ..default()
            },
        );
        let catalog = source
            .dense
            .definitions()
            .map(|d| CompilePlan::new(d, plants, presets, default()))
            .collect::<Result<Vec<_>, _>>()
            .and_then(|p| {
                environment_compile::merge_runtime_catalogs(&p.iter().collect::<Vec<_>>())
            });
        match (plan, catalog) {
            (Ok(plan), Ok(catalog)) => {
                let records = source
                    .dense
                    .preview_records()
                    .into_iter()
                    .map(|r| {
                        let DenseSourceRecord::EnvironmentCoverage(r) = r;
                        r
                    })
                    .filter(|r| r.space == space)
                    .collect();
                let global = state
                    .accepted
                    .as_ref()
                    .map_or(source.dense.definition_dirty_count() > 0, |s| {
                        s.input.plan != plan.fingerprint()
                    });
                state.queued = Some(Input {
                    revision: state.revision,
                    context,
                    plan: Arc::new(plan),
                    catalog,
                    definition_revision: definition.revision,
                    library_revision: source.dense.preset_base_revision(),
                    records,
                    roads: source.dense.roads.overrides(),
                    regions: source
                        .dense
                        .roads
                        .preview_regions(space, definition.cell_size),
                    resident,
                    resources: resources.clone(),
                    global,
                    base: state.accepted.clone(),
                });
                state.queued_at = Some(Instant::now());
                preview.error = None;
            }
            (Err(e), _) | (_, Err(e)) => preview.error = Some(e.to_string()),
        }
    }
    if let Some(result) = state
        .worker
        .as_ref()
        .and_then(|w| w.receiver.try_recv().ok())
    {
        state.in_flight = false;
        if result.revision == state.revision {
            match result.result {
                Ok(snapshot) => {
                    live.request = Some(TerrainPreviewRequest {
                        revision: result.revision,
                        generation: snapshot.input.context.4.clone(),
                        space,
                        products: snapshot.products.clone(),
                    });
                    state.pending = Some((result.revision, snapshot));
                }
                Err(e) => preview.error = Some(e),
            }
        }
    }
    if let Some(e) = &live.error {
        preview.error = Some(e.clone());
    }
    if state.pending.as_ref().is_some_and(|(revision, _)| {
        live.ready == Some(*revision) && *revision == state.revision && live.error.is_none()
    }) {
        let (revision, snapshot) = state.pending.take().unwrap();
        preview.accepted = snapshot
            .cells
            .iter()
            .filter(|((s, c), _)| *s == space && state.signature.as_ref().unwrap().3.contains(c))
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        preview.assets = snapshot.assets.clone();
        preview.active_catalog = Some(snapshot.catalog.clone());
        preview.bump();
        preview.error = None;
        state.accepted = Some(snapshot);
        live.commit = Some(revision);
    }
    preview.desired = preview
        .accepted
        .iter()
        .map(|(k, (s, _))| (*k, *s))
        .collect();
    if state.queued.is_some() || state.pending.is_some() || state.in_flight {
        // One stable pending indicator, not a changing list of half-compiled cells.
        preview.desired.insert((space, center), [0; 32]);
    }
    if state.in_flight
        || state
            .queued_at
            .is_some_and(|t| t.elapsed() < Duration::from_millis(100))
    {
        return;
    }
    if let Some(job) = state.queued.take() {
        if state.worker.is_none() {
            match LiveWorker::start(&paths) {
                Ok(worker) => state.worker = Some(worker),
                Err(e) => {
                    preview.error = Some(e);
                    return;
                }
            }
        }
        let worker = state.worker.as_ref().unwrap();
        worker.latest.store(job.revision, Ordering::Relaxed);
        match worker.sender.try_send(job) {
            Ok(()) => state.in_flight = true,
            Err(e) => preview.error = Some(e.to_string()),
        }
    }
}

fn compile(
    project: &Path,
    runtime: &Path,
    library: &world_cook::TerrainBakeLibrary,
    job: &Input,
    token: &AtomicU64,
) -> Result<Arc<Snapshot>, String> {
    compile_inner(project, runtime, library, job, token).map_err(|e| format!("{e:#}"))
}

fn compile_inner(
    project: &Path,
    runtime: &Path,
    library: &world_cook::TerrainBakeLibrary,
    job: &Input,
    token: &AtomicU64,
) -> anyhow::Result<Arc<Snapshot>> {
    use anyhow::{Context, bail};
    let cancelled = || token.load(Ordering::Relaxed) != job.revision;
    let reader = RuntimeReader::open_immutable(runtime)?;
    if reader.manifest().generation_id != job.context.4 {
        bail!("Published terrain changed during preview; retry after adoption")
    }
    let source = ProjectReader::open_read_only(project)?;
    let space = job.context.0;
    let mut cells: BTreeSet<_> = job.resident.iter().copied().collect();
    if let Some(base) = &job.base {
        cells.extend(base.products.leaves.keys());
    }
    for r in job
        .records
        .iter()
        .chain(job.base.iter().flat_map(|s| s.input.records.iter()))
    {
        for dz in -1..=1 {
            for dx in -1..=1 {
                cells.insert(CellCoord {
                    x: r.cell.x + dx,
                    z: r.cell.z + dz,
                });
            }
        }
    }
    let global = job.global
        || job.regions.is_none()
        || job.base.as_ref().is_some_and(|b| b.input.regions.is_none());
    if global {
        let mut pending: Vec<_> = reader
            .read_terrain_roots(space)?
            .into_iter()
            .map(|d| d.key)
            .collect();
        while let Some(key) = pending.pop() {
            if cancelled() {
                bail!("superseded terrain preview")
            }
            if key.level == 0 {
                cells.insert(CellCoord { x: key.x, z: key.z });
            } else {
                pending.extend(key.children()?.context("terrain children")?);
            }
            if cells.len() > MAX_PREVIEW_CELLS {
                bail!(
                    "This shared edit affects more than 256 terrain cells. Save & Publish to update the whole landscape; the previous live preview is retained."
                )
            }
        }
    } else {
        for b in job.regions.iter().flatten().chain(
            job.base
                .iter()
                .flat_map(|s| s.input.regions.iter().flatten()),
        ) {
            let area = (i64::from(b.maximum.x) - i64::from(b.minimum.x) + 1)
                * (i64::from(b.maximum.z) - i64::from(b.minimum.z) + 1);
            if area > 4096 {
                bail!(
                    "Road edit exceeds the live terrain region budget; Save & Publish to continue"
                )
            }
            for z in b.minimum.z..=b.maximum.z {
                for x in b.minimum.x..=b.maximum.x {
                    cells.insert(CellCoord { x, z });
                }
            }
        }
    }
    let assets = source
        .read_collection_assets(&job.plan.collection_assets())?
        .into_iter()
        .map(|a| (a.id, a))
        .collect();
    let mut compiled = BTreeMap::new();
    let mut leaves = job
        .base
        .as_ref()
        .map_or_else(BTreeMap::new, |b| b.products.leaves.clone());
    let mut stamps = job
        .records
        .iter()
        .map(|r| (r.cell, override_stamp(r)))
        .collect::<Vec<_>>();
    stamps.sort_by_key(|s| s.0);
    let roads = blake3::hash(ron::to_string(&job.roads)?.as_bytes());
    let mut bytes = 0;
    let mut objects = 0;
    for cell in cells {
        if cancelled() {
            bail!("superseded terrain preview")
        }
        let Some(baseline) = world_cook::published_terrain_leaf(&reader, space, cell)? else {
            continue;
        };
        if compiled.len() >= MAX_PREVIEW_CELLS {
            bail!("Live terrain exceeds 256 affected/source cells; Save & Publish to continue")
        }
        let mut hash = blake3::Hasher::new();
        hash.update(&dependency_stamp(
            job.plan.fingerprint(),
            job.context.3,
            &job.context.4,
            cell,
            &stamps,
        ));
        if job
            .regions
            .as_ref()
            .is_none_or(|r| r.iter().any(|b| b.contains(cell)))
        {
            hash.update(roads.as_bytes());
        }
        let stamp = *hash.finalize().as_bytes();
        let result = if let Some((_, c)) = job
            .base
            .as_ref()
            .and_then(|b| b.cells.get(&(space, cell)))
            .filter(|(s, _)| *s == stamp)
        {
            c.clone()
        } else {
            compile_source_cells_with_roads(
                &source,
                &job.plan,
                space,
                job.definition_revision,
                job.library_revision,
                &[cell],
                &job.records,
                &job.roads,
            )
            .map_err(anyhow::Error::msg)?
            .pop()
            .context("missing compiled terrain cell")?
        };
        bytes += compiled_bytes(&result);
        objects += result.objects.len();
        if bytes > MAX_PREVIEW_BYTES / 2 || objects > 4096 {
            bail!(
                "Live source preview exceeds its memory/object budget; reduce the edited region or Save & Publish"
            )
        }
        let page = TerrainHeightfieldPage {
            heightfield: result
                .terrain
                .clone()
                .unwrap_or_else(|| baseline.heightfield.clone()),
            surfaces: result.ground.surfaces.clone(),
            weight_pages: result.ground.weight_pages.clone(),
        };
        if page == baseline {
            leaves.remove(&cell);
        } else {
            leaves.insert(cell, Arc::new(page));
        }
        compiled.insert((space, cell), (stamp, result));
    }
    let products = if let Some(base) = job.base.as_ref().filter(|b| b.products.leaves == leaves) {
        base.products.clone()
    } else {
        let mut p = world_cook::bake_terrain_preview(
            &reader,
            space,
            leaves,
            &job.resources,
            library,
            cancelled,
        )?;
        // Explicit baseline replacements undo earlier edits, including keys that
        // have since left the current source neighborhood.
        if let Some(base) = &job.base {
            for &key in base.products.nodes.keys() {
                if let std::collections::btree_map::Entry::Vacant(e) = p.nodes.entry(key) {
                    e.insert(Arc::new(
                        reader
                            .read_terrain_node(key)?
                            .context("missing published undo node")?
                            .decode()?,
                    ));
                }
            }
            for &key in base.products.composites.keys() {
                if let std::collections::btree_map::Entry::Vacant(e) = p.composites.entry(key) {
                    e.insert(Arc::new(
                        reader
                            .read_terrain_composite(key)?
                            .context("missing published undo material")?
                            .decode()?,
                    ));
                }
            }
        }
        if p.bytes() > 96 * 1024 * 1024 {
            bail!("Live terrain retained-product budget exceeded; Save & Publish to continue")
        }
        Arc::new(p)
    };
    Ok(Arc::new(Snapshot {
        input: InputSummary {
            context: job.context.clone(),
            plan: job.plan.fingerprint(),
            records: job.records.clone(),
            regions: job.regions.clone(),
        },
        cells: compiled,
        assets,
        products,
        catalog: job.catalog.clone(),
    }))
}

/// Height-only LOD sources have no Mesh3d. Keep CPU picking, vegetation roots and
/// near-material inputs on the exact same accepted edit as the hierarchy.
pub(crate) fn apply_sources(
    config: Res<engine::TerrainLodPreview>,
    preview: Res<EnvironmentPreview>,
    state: Res<LivePreviewState>,
    project: Res<ProjectEditorStore>,
    mut terrain: Query<(
        &mut StreamedTerrainSurface,
        Option<&mut terrain_render::near::NearSource>,
    )>,
) {
    if !config.enabled {
        return;
    }
    for (mut surface, near) in &mut terrain {
        let page = state
            .accepted
            .as_ref()
            .filter(|s| s.input.context.0 == surface.key.space)
            .and_then(|s| s.products.leaves.get(&surface.key.cell));
        let cell = preview.cell(surface.key.space, surface.key.cell);
        let height = cell
            .and_then(|c| c.terrain.as_ref())
            .or_else(|| page.map(|p| &p.heightfield));
        let height_changed = height.is_some_and(|h| surface.heightfield != *h);
        if let Some(height) = height
            && surface.heightfield != *height
        {
            surface.heightfield = height.clone();
        }
        let ground = cell
            .map(|c| (&c.ground.surfaces, &c.ground.weight_pages))
            .or_else(|| page.map(|p| (&p.surfaces, &p.weight_pages)));
        if let (Some(mut near), Some((surfaces, weights))) = (near, ground) {
            if near.surfaces != *surfaces || near.weights != *weights {
                near.surfaces = surfaces.clone();
                near.weights = weights.clone();
                if let Some(r) = project.terrain_resources(surface.key.space) {
                    near.layers = surfaces
                        .iter()
                        .filter_map(|id| {
                            r.surfaces.iter().find(|s| s.surface.id == *id).map(|s| {
                                TerrainSurfaceLayer {
                                    surface: s.surface.clone(),
                                    layer: s.layer,
                                }
                            })
                        })
                        .collect();
                }
            }
            if height_changed {
                near.height_bounds = surface.heightfield.height_bounds();
            }
        }
    }
}
