//! The terrain hierarchy, the ground the editor and game draw, with bounded material
//! streaming and regional live authoring.
use super::*;
use bevy::{
    math::{DMat4, DVec3},
    render::{
        Render, RenderApp, RenderSystems,
        mesh::{RenderMesh, allocator::MeshAllocator},
        render_asset::RenderAssets,
    },
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{Arc, Mutex},
};
mod contact;
mod material;
use terrain_render::TerrainCompositeMaterial;
pub(super) mod entry;
mod mesh_cache;
mod transition;
pub use contact::TerrainContactReadiness;
#[cfg(test)]
pub(crate) use contact::test_flat_contact;
mod authoring;
use super::database::{RequestId, TerrainQuery, TerrainReply};
pub use authoring::{LiveTerrainPreview, TerrainPreviewRequest};
use contact::{ContactInputs, ContactSystems};
use terrain_render::lod::{self, LodSettings, LodView, PatchMetadata, PlannedCover, StitchEdges};
use transition::{Transition, patch_transform};
use world::{TerrainNode, TerrainNodeKey};
use world_db::TerrainNodeDescriptor;

/// Membership sets for eviction; their iteration order never decides what is kept.
type KeySet = bevy::platform::collections::HashSet<TerrainNodeKey>;
/// Descriptors kept per patch of the budget: the drawn and staged covers, their ancestors and
/// the children already loaded for refinement. At 8 per patch (4,096 for 2,048 patches) a
/// settled desktop view filled the table, after which no new ground could load around a
/// moving actor.
const METADATA_PER_PATCH: usize = 12;
/// Descriptors kept after they leave the cover, most recently used first, so a view the
/// camera returns to plans its detail at once instead of refining it level by level again.
const METADATA_CACHE: usize = 4096;
const MAX_METADATA: usize =
    METADATA_PER_PATCH * if cfg!(target_os = "ios") { 512 } else { 2048 } + METADATA_CACHE;
/// Decoded node samples kept after they leave the cover (about 5 KB per node). A new cover
/// always evicts them before its sample budget is checked.
const NODE_CACHE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_NODE_BYTES: u64 = 32 * 1024 * 1024;
/// Room for a full drawn cover and a completely different staged one at the patch and
/// triangle budgets (about 157 MB each on desktop), plus smaller morphs. At 256 MiB, turning
/// the camera at the full budget exceeded it and stopped terrain streaming.
const MAX_MESH_BYTES: u64 = if cfg!(target_os = "ios") { 128 } else { 384 } * 1024 * 1024;
// Unused GPU meshes are evictable first; this is part of MAX_MESH_BYTES, not extra memory.
const MESH_CACHE_BYTES: u64 = if cfg!(target_os = "ios") { 16 } else { 96 } * 1024 * 1024;
/// Database queries in flight: metadata batches, node samples and ground materials.
const MAX_REQUESTS: usize = 12;
/// A moving view re-plans at most this often while the drawn ground satisfies contact
/// demand. A full-budget plan takes about 6 ms of a pool thread; planning after every
/// publication kept 20–40 plans a second running.
const PLAN_INTERVAL_SECONDS: f64 = 0.1;
/// Patch meshes built at once on the compute task pool.
const MAX_BUILDS: usize = 8;
type Patch = (TerrainNodeKey, StitchEdges);

#[derive(Resource, Clone, Debug, Default)]
pub struct TerrainLodStats {
    pub live_preview_bytes: u64,
    pub live_preview_revision: u64,
    pub status: String,
    pub entry_status: Option<String>,
    pub entry_decoded_bytes: u64,
    pub entry_mesh_bytes: u64,
    pub patches: usize,
    pub triangles: usize,
    pub levels: BTreeMap<u8, usize>,
    /// Target-cover error. See drawn_* for conservative bounds on the surface
    /// actually drawn while this target loads or morphs.
    pub maximum_visible_error: f64,
    pub quality_pending: bool,
    pub budget_limited: bool,
    pub contact_limited: bool,
    /// Required actor/grass regions which the drawn cover cannot yet certify.
    pub drawn_contact_limited: bool,
    /// Conservative, potentially loose, during a morph with horizontal collapse.
    pub drawn_maximum_visible_error: f64,
    pub blocked_actors: usize,
    pub blocked_grass_pages: usize,
    pub mismatched_grass_pages: usize,
    pub contact_handoffs: u64,
    /// Cumulative targets dropped because they would remove certified contact ground.
    pub replans: u64,
    /// Cumulative replacements swapped in without a morph, lacking room for its meshes.
    pub unmorphed_swaps: u64,
    /// Cumulative source-height certifications, excluding reused certificates.
    pub contact_source_checks: u64,
    pub contact_source_samples: u64,
    pub metadata: usize,
    pub decoded_bytes: u64,
    pub mesh_bytes: u64,
    pub cached_mesh_bytes: u64,
    pub material_status: String,
    pub material_tiles: usize,
    pub material_detail_tiles: usize,
    pub material_metadata: usize,
    pub material_detail_uploads: u64,
    pub material_detail_limited: bool,
    pub near_material_pages: usize,
    pub near_material_ready_pages: usize,
    pub near_material_bytes: u64,
    pub near_material_limited: bool,
    pub near_material_error: Option<String>,
    pub material_reserved_bytes: u64,
    pub entry_material_reserved_bytes: u64,
    pub pending: usize,
    pub staged: usize,
    /// Completed cover plans (they run on the async compute pool whenever the view or
    /// demand changes) and the duration of the latest one.
    pub plans: u64,
    pub plan_milliseconds: f64,
    /// A whole replacement group shares this weight; None means no visible morph.
    pub morph_weight: Option<f32>,
    pub transition_patches: usize,
    pub transition_triangles: usize,
}
#[derive(Default)]
struct Uploads {
    #[cfg(test)]
    pause_acknowledgements: bool,
    #[cfg(test)]
    pause_material_acknowledgements: bool,
    wanted: bevy::platform::collections::HashSet<bevy::asset::AssetId<Mesh>>,
    ready: bevy::platform::collections::HashSet<bevy::asset::AssetId<Mesh>>,
    probe: Option<Entity>,
    pipelines_ready: bool,
    entry_probe: Option<Entity>,
    entry_pipelines_ready: bool,
    material_wanted: std::collections::HashSet<bevy::asset::AssetId<TerrainCompositeMaterial>>,
    material_ready: std::collections::HashSet<bevy::asset::AssetId<TerrainCompositeMaterial>>,
}
#[derive(Resource, Clone, Default)]
pub(super) struct UploadTracker(Arc<Mutex<Uploads>>);
fn acknowledge_uploads(
    meshes: Res<RenderAssets<RenderMesh>>,
    allocator: Res<MeshAllocator>,
    tracker: Res<UploadTracker>,
) {
    let mut tracker = tracker.0.lock().unwrap();
    #[cfg(test)]
    if tracker.pause_acknowledgements {
        return;
    }
    // Terrain meshes are never modified, and removing one drops it from both sets, so an
    // uploaded mesh stays ready. Rebuilding the set from every wanted mesh took ~0.3 ms of
    // each render frame, after the drawable was acquired.
    let Uploads { wanted, ready, .. } = &mut *tracker;
    for &id in wanted.iter() {
        if !ready.contains(&id)
            && meshes.get(id).is_some_and(|m| {
                !m.has_morph_targets() || allocator.mesh_morph_target_slice(&id).is_some()
            })
            && allocator.mesh_vertex_slice(&id).is_some()
            && allocator.mesh_index_slice(&id).is_some()
        {
            ready.insert(id);
        }
    }
}
// Probe entities have zero-area triangles, but participate in specialization for
// every applicable view. Wait for actual compiled pipelines, not a frame delay.
fn acknowledge_pipelines(
    material: Res<bevy::pbr::SpecializedMaterialPipelineCache>,
    prepass: Res<bevy::pbr::SpecializedPrepassMaterialPipelineCache>,
    shadow: Res<bevy::pbr::SpecializedShadowMaterialPipelineCache>,
    pipelines: Res<bevy::render::render_resource::PipelineCache>,
    tracker: Res<UploadTracker>,
    prepared: Res<
        bevy::render::erased_render_asset::ErasedRenderAssets<bevy::pbr::PreparedMaterial>,
    >,
) {
    let mut tracker = tracker.0.lock().unwrap();
    #[cfg(test)]
    if tracker.pause_acknowledgements {
        return;
    }
    let acknowledge_materials = {
        #[cfg(test)]
        {
            !tracker.pause_material_acknowledgements
        }
        #[cfg(not(test))]
        {
            true
        }
    };
    if acknowledge_materials {
        tracker.material_ready = tracker
            .material_wanted
            .iter()
            .copied()
            .filter(|id| prepared.get(*id).is_some())
            .collect();
    }
    let ready = |entity: Option<Entity>| {
        let Some(entity) = entity.map(bevy::render::sync_world::MainEntity::from) else {
            return false;
        };
        let main: Vec<_> = material
            .values()
            .filter_map(|v| v.get(&entity))
            .copied()
            .collect();
        !main.is_empty()
            && main
                .into_iter()
                .chain(
                    prepass
                        .values()
                        .filter_map(|v| v.get(&entity).map(|(_, p, _)| *p)),
                )
                .chain(
                    shadow
                        .values()
                        .filter_map(|v| v.get(&entity).map(|(p, _, _)| *p)),
                )
                .all(|p| pipelines.get_render_pipeline(p).is_some())
    };
    tracker.pipelines_ready = ready(tracker.probe);
    tracker.entry_pipelines_ready = ready(tracker.entry_probe);
}
pub(super) fn install(app: &mut App) {
    let tracker = UploadTracker::default();
    contact::install(app);
    app.insert_resource(tracker.clone())
        .init_resource::<LodSettings>()
        .init_resource::<TerrainLodStats>()
        .init_resource::<TerrainLodStream>()
        .init_resource::<LiveTerrainPreview>()
        .init_resource::<entry::TerrainEntry>()
        .configure_sets(
            PostUpdate,
            (
                ContactSystems::Collect,
                TerrainLodUpdate,
                ContactSystems::Publish,
            )
                .chain()
                .after(TransformSystems::Propagate)
                .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        )
        .add_systems(
            PostUpdate,
            (
                authoring::update
                    .after(TransformSystems::Propagate)
                    .before(ContactSystems::Collect)
                    .before(terrain_render::near::NearPrepare)
                    .before(near_view),
                near_view
                    .after(TransformSystems::Propagate)
                    .before(terrain_render::near::NearPrepare),
                update
                    .in_set(TerrainLodUpdate)
                    .in_set(terrain_render::TerrainMaterialPreparation),
            ),
        );
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.insert_resource(tracker).add_systems(
            Render,
            (
                acknowledge_uploads.after(RenderSystems::PrepareMeshes),
                acknowledge_pipelines.after(RenderSystems::Render),
            ),
        );
    }
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TerrainLodUpdate;

#[derive(PartialEq)]
struct PlanIdentity {
    view: LodView,
    settings: LodSettings,
    metadata_revision: u64,
    contacts: Vec<lod::contact::ContactRegion>,
}

struct ResidentMesh {
    handle: Handle<Mesh>,
    bytes: u64,
    /// Computed with the mesh off the main thread. Patches spawn with it and `NoAutoAabb`;
    /// Bevy otherwise scanned every vertex of each new patch on the frame it appeared.
    bounds: Aabb,
}
/// A patch mesh and its bounds, built on a pool thread. The mesh moves to the render world
/// on upload: nothing reads its vertices on the CPU, and keeping a main-world copy cloned
/// every new patch during extraction and held a second copy of the resident cover.
fn build_bounded_patch_mesh(
    field: &world::TerrainHeightfield,
    extent: f32,
    edges: StitchEdges,
) -> Result<(Mesh, Aabb), String> {
    use bevy::camera::primitives::MeshAabb;
    let mut mesh = lod::build_patch_mesh(field, extent, edges)?;
    let bounds = mesh.get_aabb().ok_or("terrain patch without positions")?;
    mesh.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
    Ok((mesh, bounds))
}
/// A cover plan running on the async compute pool, from snapshots of its inputs. Plans
/// start only while no target is staged, so the drawn cover and the retained descriptors
/// they reference cannot be replaced or evicted before the result is applied.
struct PlanTask {
    identity: PlanIdentity,
    task: Task<(Result<PlannedCover, String>, f64)>,
}
struct MeshJob {
    patch: Patch,
    bytes: u64,
    task: Task<Result<(Mesh, Aabb), String>>,
}
#[derive(Resource, Default)]
pub(crate) struct TerrainLodStream {
    overlay: Option<Arc<world::TerrainPreviewProducts>>,
    overlay_revision: u64,
    identity: Option<(String, WorldSpaceId)>,
    pending: BTreeMap<RequestId, TerrainQuery>,
    roots: Option<Vec<TerrainNodeKey>>,
    descriptors: BTreeMap<TerrainNodeKey, TerrainNodeDescriptor>,
    /// Shared with a running plan; copied on write only if a reply lands during one.
    metadata: Arc<BTreeMap<TerrainNodeKey, PatchMetadata>>,
    nodes: BTreeMap<TerrainNodeKey, TerrainNode>,
    decodes: BTreeMap<TerrainNodeKey, Task<Result<TerrainNode, String>>>,
    builds: Vec<MeshJob>,
    meshes: BTreeMap<Patch, ResidentMesh>,
    mesh_last_used: HashMap<Patch, u64>,
    active: BTreeMap<Patch, Entity>,
    target: Option<PlannedCover>,
    transition: Option<Transition>,
    material: Option<Handle<TerrainCompositeMaterial>>,
    composites: material::Cover,
    error: Option<String>,
    last_report: f64,
    last_plan: Option<PlanIdentity>,
    /// When the last plan started, in seconds of app time.
    last_plan_at: Option<f64>,
    planning: Option<PlanTask>,
    replans: u64,
    unmorphed_swaps: u64,
    /// Eviction generation, and when each retained descriptor or node was last needed.
    epoch: u64,
    last_used: HashMap<TerrainNodeKey, u64>,
    /// When the actor last started waiting for ground, and when that was last reported.
    stalled_since: Option<f64>,
    last_stall_report: f64,
    failure_reported: bool,
    metadata_revision: u64,
    position_origin: Option<CellCoord>,
    draw_visible: bool,
}
impl TerrainLodStream {
    pub(super) fn receive(&mut self, request_id: RequestId, reply: Result<TerrainReply, String>) {
        let Some(query) = self.pending.remove(&request_id) else {
            return;
        };
        let reply = match reply {
            Ok(reply) => reply,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        match (query, reply) {
            (TerrainQuery::Material(query), TerrainReply::Material(reply)) => {
                self.receive_material(query, reply)
            }
            (
                query @ (TerrainQuery::Roots(_) | TerrainQuery::Metadata(_)),
                TerrainReply::Metadata(descriptors),
            ) => {
                if self.metadata.len()
                    + descriptors
                        .iter()
                        .filter(|d| !self.metadata.contains_key(&d.key))
                        .count()
                    > MAX_METADATA
                {
                    self.error = Some("terrain metadata limit exceeded".into());
                    return;
                }
                if matches!(query, TerrainQuery::Roots(_)) {
                    self.roots = Some(descriptors.iter().map(|d| d.key).collect());
                }
                self.metadata_revision = self.metadata_revision.wrapping_add(1);
                let metadata = Arc::make_mut(&mut self.metadata);
                for mut d in descriptors {
                    if let Some(node) = self.overlay.as_ref().and_then(|o| o.nodes.get(&d.key)) {
                        d = authoring::descriptor(node);
                    }
                    let Some(resolution) = d.resolution else {
                        self.error = Some("partial node inside drawable cover".into());
                        return;
                    };
                    if resolution < 3 {
                        self.error =
                            Some("terrain LOD requires edge midpoints; recook this runtime".into());
                        return;
                    }
                    metadata.insert(
                        d.key,
                        PatchMetadata {
                            key: d.key,
                            resolution,
                            height_bounds: d.height_bounds,
                            geometric_error: d.geometric_error,
                        },
                    );
                    self.descriptors.insert(d.key, d);
                }
            }
            (TerrainQuery::Node(key), TerrainReply::Node(encoded))
                if encoded.descriptor.key == key
                    && self.descriptors.get(&key) == Some(&encoded.descriptor) =>
            {
                self.decodes.insert(
                    key,
                    AsyncComputeTaskPool::get()
                        .spawn(async move { encoded.decode().map_err(|e| e.to_string()) }),
                );
            }
            _ => self.error = Some("terrain response identity mismatch".into()),
        }
    }
    fn request(&mut self, worker: &WorldDatabaseWorker, query: TerrainQuery) {
        if self.preview_request(&query) {
            return;
        }
        if self.pending.len()
            + self.decodes.len()
            + self.composites.decodes.len()
            + self.composites.detail.as_ref().map_or(0, |d| d.cpu.len())
            >= MAX_REQUESTS
        {
            return;
        }
        let Some((generation, _)) = &self.identity else {
            return;
        };
        match worker.send(DatabaseRequest::Terrain {
            generation: generation.clone(),
            query: query.clone(),
        }) {
            Ok(id) => {
                self.pending.insert(id, query);
            }
            Err(NotSent::Full) => (),
            Err(NotSent::Stopped) => self.error = Some("terrain database worker stopped".into()),
        }
    }
    fn decoded_bytes(&self) -> u64 {
        // Every resident node has a descriptor, so walk both ordered maps together rather
        // than collecting the keys into a set and looking each one up (every frame).
        let mut descriptors = self.descriptors.iter();
        let resident: u64 = self
            .nodes
            .keys()
            .map(|key| {
                descriptors
                    .find(|(k, _)| *k == key)
                    .expect("resident node without a descriptor")
                    .1
                    .decoded_bytes
            })
            .sum();
        // A few loading nodes, counted once each and never twice with a resident one.
        let mut loading: Vec<_> = self
            .decodes
            .keys()
            .chain(self.pending.values().filter_map(|q| match q {
                TerrainQuery::Node(key) => Some(key),
                _ => None,
            }))
            .filter(|k| !self.nodes.contains_key(k))
            .collect();
        loading.sort();
        loading.dedup();
        resident
            + loading
                .into_iter()
                .map(|k| self.descriptors[k].decoded_bytes)
                .sum::<u64>()
            + self.transition.as_ref().map_or(0, Transition::input_bytes)
    }
    fn mesh_bytes(&self) -> u64 {
        self.meshes.values().map(|m| m.bytes).sum::<u64>()
            + self.builds.iter().map(|b| b.bytes).sum::<u64>()
            + self.transition.as_ref().map_or(0, |t| t.bytes)
    }
    fn clear(
        &mut self,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        tracker: &UploadTracker,
    ) {
        if let Some(t) = self.transition.take() {
            t.clear(commands, meshes, tracker);
        }
        for (_, entity) in std::mem::take(&mut self.active) {
            commands.entity(entity).despawn();
        }
        for (_, mesh) in std::mem::take(&mut self.meshes) {
            meshes.remove(mesh.handle.id());
            let mut uploads = tracker.0.lock().unwrap();
            uploads.wanted.remove(&mesh.handle.id());
            uploads.ready.remove(&mesh.handle.id());
        }
        self.composites.clear(tracker);
        let material = self.material.take();
        *self = Self {
            material,
            ..default()
        };
    }
    fn poll_work(&mut self, meshes: &mut Assets<Mesh>, tracker: &UploadTracker) {
        let finished: Vec<_> = self
            .decodes
            .iter_mut()
            .filter_map(|(&key, task)| check_ready(task).map(|r| (key, r)))
            .collect();
        for (key, result) in finished {
            self.decodes.remove(&key);
            match result {
                Ok(node) => {
                    self.nodes.insert(key, node);
                }
                Err(e) => self.error = Some(e),
            }
        }
        let mut i = 0;
        while i < self.builds.len() {
            if let Some(result) = check_ready(&mut self.builds[i].task) {
                let job = self.builds.remove(i);
                match result {
                    Ok((mesh, bounds)) => {
                        let handle = meshes.add(mesh);
                        tracker.0.lock().unwrap().wanted.insert(handle.id());
                        self.meshes.insert(
                            job.patch,
                            ResidentMesh {
                                handle,
                                bytes: job.bytes,
                                bounds,
                            },
                        );
                    }
                    Err(e) => self.error = Some(e),
                }
            } else {
                i += 1;
            }
        }
    }
    fn prepare_target(
        &mut self,
        worker: &WorldDatabaseWorker,
        assets: &mut Assets<Mesh>,
        tracker: &UploadTracker,
        cell_size: f32,
        node_limit: u64,
        mesh_limit: u64,
    ) {
        let Some(plan) = &self.target else {
            return;
        };
        let patches: Vec<_> = plan.patches.iter().map(|(&k, &e)| (k, e)).collect();
        let reserved: u64 = patches
            .iter()
            .filter(|p| !self.meshes.contains_key(p) && !self.builds.iter().any(|b| b.patch == **p))
            .map(|(key, _)| self.descriptors[key].gpu_bytes_estimate)
            .sum();
        if self.mesh_bytes() + reserved > mesh_limit {
            self.trim_mesh_cache(
                assets,
                tracker,
                MESH_CACHE_BYTES,
                mesh_limit.saturating_sub(reserved),
            );
        }
        // Totals once per pass, and only once something is to start: summing every
        // resident node per patch was quadratic, and every frame of a staged cover paid it.
        let mut mesh_bytes = None;
        let mut decoded_bytes = None;
        let mut requests_open = true;
        for patch @ (key, edges) in &patches {
            if self.builds.len() >= MAX_BUILDS && !requests_open {
                break;
            }
            if (self.meshes.contains_key(patch) && self.nodes.contains_key(key))
                || self.builds.iter().any(|b| b.patch == *patch)
            {
                continue;
            }
            if let Some(field) = self.nodes.get(key).and_then(|n| n.heightfield.as_ref()) {
                if self.meshes.contains_key(patch) {
                    continue;
                }
                if self.builds.len() >= MAX_BUILDS {
                    continue;
                }
                let mesh_bytes = mesh_bytes.get_or_insert_with(|| self.mesh_bytes());
                let bytes = self.descriptors[key].gpu_bytes_estimate;
                if *mesh_bytes + bytes > mesh_limit {
                    self.error = Some("terrain replacement exceeds mesh budget".into());
                    break;
                }
                *mesh_bytes += bytes;
                let field = field.clone();
                let extent = cell_size * (1_u32 << key.level) as f32;
                let edges = *edges;
                self.builds.push(MeshJob {
                    patch: *patch,
                    bytes,
                    task: AsyncComputeTaskPool::get()
                        .spawn(async move { build_bounded_patch_mesh(&field, extent, edges) }),
                });
            } else if requests_open
                && !self.decodes.contains_key(key)
                && !self
                    .pending
                    .values()
                    .any(|q| matches!(q,TerrainQuery::Node(k) if k==key))
            {
                let mut total = match decoded_bytes {
                    Some(total) => total,
                    None => self.decoded_bytes(),
                };
                if total + self.descriptors[key].decoded_bytes > node_limit {
                    self.drop_node_cache();
                    total = self.decoded_bytes();
                }
                decoded_bytes = Some(total);
                if total + self.descriptors[key].decoded_bytes > node_limit {
                    self.error = Some("terrain replacement exceeds sample budget".into());
                    break;
                }
                let before = self.pending.len();
                self.request(worker, TerrainQuery::Node(*key));
                if self.pending.len() > before {
                    decoded_bytes = Some(total + self.descriptors[key].decoded_bytes);
                } else {
                    // At the in-flight limit: builds may continue, requests wait a frame.
                    requests_open = false;
                }
            }
        }
    }
    /// Whether `plan` is exactly the cover already drawn, stitched edges included.
    fn draws(&self, plan: &PlannedCover) -> bool {
        plan.patches.len() == self.active.len()
            && plan
                .patches
                .iter()
                .all(|(&key, &edges)| self.active.contains_key(&(key, edges)))
    }
    fn target_uploaded(&self, tracker: &UploadTracker) -> bool {
        let Some(plan) = &self.target else {
            return false;
        };
        let patches: Vec<_> = plan.patches.iter().map(|(&k, &e)| (k, e)).collect();
        let uploads = tracker.0.lock().unwrap();
        patches.iter().all(|p| {
            // GPU meshes may outlive their decoded samples. Reload samples before
            // publication: morph endpoints and contact checks still require them.
            self.nodes.contains_key(&p.0)
                && self
                    .meshes
                    .get(p)
                    .is_some_and(|m| uploads.ready.contains(&m.handle.id()))
        })
    }
    fn evict(&mut self, assets: &mut Assets<Mesh>, tracker: &UploadTracker) {
        let wanted: BTreeSet<_> = self
            .target
            .as_ref()
            .into_iter()
            .flat_map(|p| p.patches.iter().map(|(&k, &e)| (k, e)))
            .chain(self.active.keys().copied())
            .collect();
        self.epoch += 1;
        for &patch in &wanted {
            self.mesh_last_used.insert(patch, self.epoch);
        }
        self.trim_mesh_cache(assets, tracker, MESH_CACHE_BYTES, MAX_MESH_BYTES);
        let keys: KeySet = wanted
            .iter()
            .map(|(k, _)| *k)
            .chain(self.roots.iter().flatten().copied())
            .collect();
        let epoch = self.epoch;
        for key in &keys {
            self.last_used.insert(*key, epoch);
        }
        // Nodes outside the covers stay as a cache of the most recently needed ones.
        let cached = recent_within(
            self.nodes.keys().filter(|&k| !keys.contains(k)).map(|k| {
                (
                    *k,
                    self.last_used.get(k).copied().unwrap_or(0),
                    self.descriptors[k].decoded_bytes,
                )
            }),
            NODE_CACHE_BYTES,
        );
        self.nodes
            .retain(|key, _| keys.contains(key) || cached.contains(key));
        // Keep ancestors for hysteresis/balancing, direct children for pending refinements,
        // and all in-flight descriptors; older branches stay only as a bounded cache.
        let mut metadata = keys.clone();
        for key in keys {
            let mut ancestor = key;
            while let Some(parent) = ancestor.parent().ok().flatten() {
                // A parent already kept had its own ancestors walked, or will as a key.
                if !self.metadata.contains_key(&parent) || !metadata.insert(parent) {
                    break;
                }
                ancestor = parent;
            }
            if let Some(children) = key.children().ok().flatten() {
                metadata.extend(children);
            }
        }
        for query in self.pending.values() {
            match query {
                TerrainQuery::Metadata(keys) => metadata.extend(keys),
                TerrainQuery::Node(key) => {
                    metadata.insert(*key);
                }
                _ => (),
            }
        }
        metadata.extend(self.decodes.keys().copied());
        metadata.extend(self.nodes.keys().copied());
        metadata.extend(self.meshes.keys().map(|(key, _)| *key));
        for key in &metadata {
            self.last_used.insert(*key, epoch);
        }
        let cached = recent_within(
            self.metadata
                .keys()
                .filter(|&k| !metadata.contains(k))
                .map(|k| (*k, self.last_used.get(k).copied().unwrap_or(0), 1)),
            METADATA_CACHE as u64,
        );
        metadata.extend(cached);
        Arc::make_mut(&mut self.metadata).retain(|k, _| metadata.contains(k));
        self.descriptors.retain(|k, _| metadata.contains(k));
        self.last_used.retain(|k, _| metadata.contains(k));
    }
    /// Frees the decoded-node cache for a cover that needs its sample budget.
    fn drop_node_cache(&mut self) {
        let mut keys: BTreeSet<_> = self
            .target
            .as_ref()
            .into_iter()
            .flat_map(|p| p.patches.keys().copied())
            .chain(self.active.keys().map(|(k, _)| *k))
            .collect();
        keys.extend(self.roots.iter().flatten().copied());
        self.nodes.retain(|key, _| keys.contains(key));
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update(
    mut commands: Commands,
    settings: Res<LodSettings>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    active_space: Res<ActiveWorldSpace>,
    worker: Option<Res<WorldDatabaseWorker>>,
    time: Res<Time>,
    camera: Query<
        (
            &Camera,
            &GlobalTransform,
            Option<&bevy::camera::MainPassResolutionOverride>,
        ),
        With<WorldViewCamera>,
    >,
    mut stream: ResMut<TerrainLodStream>,
    mut stats: ResMut<TerrainLodStats>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TerrainCompositeMaterial>>,
    (mut images, mut buffers, upload_hub, near_stats): (
        ResMut<Assets<Image>>,
        ResMut<Assets<bevy::render::storage::ShaderBuffer>>,
        Res<terrain_render::composite::atlas::CompositeUploadHub>,
        Res<terrain_render::near::NearStats>,
    ),
    (tracker, contacts): (Res<UploadTracker>, Res<ContactInputs>),
    (entry, live): (Res<entry::TerrainEntry>, Res<LiveTerrainPreview>),
) {
    report_stall(&mut stream, &stats, &contacts, time.elapsed_secs_f64());
    let (Some(space), Some(worker)) = (active_space.current(), worker) else {
        return;
    };
    let Some(info) = catalog.world_space(space) else {
        return;
    };
    let identity = (catalog.generation_id().to_owned(), space);
    if stream.identity.as_ref() != Some(&identity) {
        stream.clear(&mut commands, &mut meshes, &tracker);
        stream.identity = Some(identity);
        stream.request(&worker, TerrainQuery::Roots(space));
    }
    if stream.material.is_none() {
        stream.material = Some(materials.add(TerrainCompositeMaterial::default()));
    }
    let visible = camera.iter().any(|(c, _, _)| c.is_active);
    // Position from canonical keys each frame; rebasing never requires terrain reload.
    if stream.position_origin != Some(origin.cell()) || stream.draw_visible != visible {
        stream.place(
            &mut commands,
            &mut materials,
            origin.cell(),
            info.cell_size,
            visible,
        );
    }
    // Keep the drawn old cover (including its current morph weight) intact while
    // staging a different world. Both covers share the same allocation limits.
    if entry.pending() && stream.transition.as_ref().is_none_or(|t| t.running()) {
        stats.status = "retaining terrain during world entry".into();
        return;
    }
    if live.request.as_ref().is_some_and(|r| {
        r.generation == catalog.generation_id()
            && r.space == space
            && live.applied != Some(r.revision)
    }) && live.error.is_none()
        && stream.transition.as_ref().is_none_or(|t| !t.running())
        && !stream.active.is_empty()
        && stream.materials_ready(&tracker)
    {
        stats.status = "preparing live terrain region; previous preview retained".into();
        return;
    }
    if let Some(error) = &contacts.error {
        stats.status = format!("terrain contact demand rejected: {error}");
        stats.budget_limited = true;
        return;
    }
    if let Some(error) = stream.error.clone() {
        stats.status = format!("terrain LOD failed (cover retained): {error}");
        if !stream.failure_reported {
            stream.failure_reported = true;
            warn!("TERRAIN_LOD_FAILED {error}");
        }
        return;
    }
    if stream.roots.is_none() && stream.pending.is_empty() {
        stream.request(&worker, TerrainQuery::Roots(space));
    }
    stream.poll_work(&mut meshes, &tracker);
    stream.poll_materials(
        &mut materials,
        &mut images,
        &tracker,
        origin.cell(),
        info.cell_size,
    );
    stream.prepare_materials(&worker, material::MAX_MATERIAL_BYTES);
    // A finished plan is applied before anything else can stage a target.
    if stream.target.is_none() {
        stream.apply_finished_plan(&worker, &mut stats, &mut meshes, &tracker);
    }
    let view =
        camera
            .iter()
            .find(|(c, _, _)| c.is_active)
            .and_then(|(camera, transform, resolution)| {
                let viewport = resolution
                    .map(|r| r.0)
                    .or_else(|| camera.physical_viewport_size())?;
                Some(lod_view(
                    camera,
                    transform,
                    viewport,
                    origin.cell(),
                    info.cell_size,
                ))
            });
    // Freeze a replacement while it uploads; changing camera demand cannot continually
    // cancel the last missing child and starve publication.
    if stream.target.is_none()
        && stream.planning.is_none()
        && visible
        && let Some(view) = &view
        && stream.roots.is_some()
    {
        stream.start_plan(view, &settings, &contacts, &stats, &time, info.cell_size);
    }
    if stream.target.is_some() {
        stream.publish_target(
            &worker,
            &settings,
            &mut commands,
            &mut meshes,
            &tracker,
            &contacts,
            &mut stats,
            origin.cell(),
            info.cell_size,
            visible,
            time.delta_secs(),
        );
    }
    if stream.materials_ready(&tracker)
        && let Some(view) = &view
    {
        stream.update_material_detail(
            &worker,
            view,
            info.cell_size,
            &mut images,
            &mut buffers,
            &mut materials,
            &upload_hub,
            time.delta_secs(),
        );
    }
    fill_stats(&mut stream, &mut stats, &near_stats, &tracker, &time);
}

/// The view a cover is planned for, in world coordinates around the render origin.
fn lod_view(
    camera: &Camera,
    transform: &GlobalTransform,
    viewport: UVec2,
    origin: CellCoord,
    cell_size: f32,
) -> LodView {
    let shift = DVec3::new(
        origin.x as f64 * cell_size as f64,
        0.,
        origin.z as f64 * cell_size as f64,
    );
    LodView {
        clip_from_world: camera.clip_from_view().as_dmat4()
            * transform.to_matrix().as_dmat4().inverse()
            * DMat4::from_translation(-shift),
        viewport: [viewport.x, viewport.y],
        contact_position: transform.translation().as_dvec3() + shift,
    }
}

impl TerrainLodStream {
    /// Moves the drawn patches to the render origin and shows or hides them with the view.
    fn place(
        &mut self,
        commands: &mut Commands,
        materials: &mut Assets<TerrainCompositeMaterial>,
        origin: CellCoord,
        cell_size: f32,
        visible: bool,
    ) {
        self.sync_material_origin(materials, origin, cell_size);
        for (&patch, &entity) in &self.active {
            let show = visible
                && self
                    .transition
                    .as_ref()
                    .is_none_or(|t| !t.running() || t.keeps(&patch));
            commands.entity(entity).insert((
                patch_transform(patch.0, origin, cell_size),
                GlobalTransform::from(patch_transform(patch.0, origin, cell_size)),
                if show {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                },
            ));
        }
        if let Some(t) = &self.transition {
            for (&key, &entity) in &t.entities {
                commands.entity(entity).insert((
                    patch_transform(key, origin, cell_size),
                    GlobalTransform::from(patch_transform(key, origin, cell_size)),
                    if visible {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    },
                ));
            }
        }
        self.position_origin = Some(origin);
        self.draw_visible = visible;
    }

    /// Takes a plan from the planning task once it is done: requests the metadata it is
    /// missing, and stages it as the target unless it draws what is drawn.
    fn apply_finished_plan(
        &mut self,
        worker: &WorldDatabaseWorker,
        stats: &mut TerrainLodStats,
        meshes: &mut Assets<Mesh>,
        tracker: &UploadTracker,
    ) {
        let Some((planned, milliseconds)) = self
            .planning
            .as_mut()
            .and_then(|p| check_ready(&mut p.task))
        else {
            return;
        };
        let identity = self.planning.take().unwrap().identity;
        stats.plans += 1;
        stats.replans = self.replans;
        stats.unmorphed_swaps = self.unmorphed_swaps;
        stats.plan_milliseconds = milliseconds;
        let plan = match planned {
            Ok(plan) => plan,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if plan.requests.is_empty() {
            self.last_plan = Some(identity);
        }
        stats.maximum_visible_error = plan.stats.maximum_visible_error;
        stats.budget_limited = plan.stats.budget_limited;
        stats.contact_limited = plan.stats.contact_limited;
        let queued: BTreeSet<_> = self
            .pending
            .values()
            .filter_map(|q| {
                if let TerrainQuery::Metadata(keys) = q {
                    Some(keys.iter().copied())
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        // Every requested key, in batches. Refining a patch needs its four
        // children, so one batch a plan grew a cover by only ~100 patches per
        // plan and morph: a turned view took tens of seconds to sharpen.
        let keys: Vec<_> = plan
            .requests
            .iter()
            .filter(|k| !queued.contains(k))
            .copied()
            .take(MAX_METADATA.saturating_sub(self.metadata.len() + queued.len()))
            .collect();
        for batch in keys.chunks(world_db::MAX_TERRAIN_NODE_QUERY) {
            self.request(worker, TerrainQuery::Metadata(batch.to_vec()));
        }
        if !plan.balanced {
            stats.status = "balancing coarse terrain cover".into();
        } else if self.draws(&plan) {
            // Nothing to stage. Publishing the same cover ran upload checks, a
            // quadratic removal scan and a full eviction on the plan's frame.
            stats.triangles = plan.stats.triangles;
            if self.metadata.len() > MAX_METADATA - METADATA_CACHE {
                // Descriptors loaded for refinements this cover could not take
                // yet would otherwise fill the table and block every request.
                self.evict(meshes, tracker);
            }
        } else {
            self.target = Some(plan);
        }
    }

    /// Plans a cover for `view` off the main thread, unless the same inputs were planned
    /// last or a plan is not yet due.
    fn start_plan(
        &mut self,
        view: &LodView,
        settings: &LodSettings,
        contacts: &ContactInputs,
        stats: &TerrainLodStats,
        time: &Time,
        cell_size: f32,
    ) {
        let Some(roots) = self.roots.as_ref() else {
            return;
        };
        let identity = PlanIdentity {
            view: view.clone(),
            settings: settings.clone(),
            metadata_revision: self.metadata_revision,
            contacts: contacts.planning.clone(),
        };
        // Ground an actor or grass is waiting for is planned for at once.
        let due = stats.drawn_contact_limited
            || self
                .last_plan_at
                .is_none_or(|at| time.elapsed_secs_f64() - at >= PLAN_INTERVAL_SECONDS);
        if !due || self.last_plan.as_ref() == Some(&identity) {
            return;
        }
        // Off the main thread: a full-budget plan took 6–9 ms of a 16.7 ms frame, ten
        // times a second while moving. Inputs are snapshots; metadata is shared.
        let previous: BTreeSet<_> = self.active.keys().map(|(k, _)| *k).collect();
        let roots = roots.clone();
        let metadata = self.metadata.clone();
        let settings = settings.clone();
        let regions = contacts.planning.clone();
        let view = view.clone();
        let cell_size = cell_size as f64;
        self.last_plan_at = Some(time.elapsed_secs_f64());
        self.planning = Some(PlanTask {
            identity,
            task: AsyncComputeTaskPool::get().spawn(async move {
                let start = std::time::Instant::now();
                let planned = lod::plan_cover_with_contacts(
                    &roots, &metadata, &previous, &view, cell_size, &settings, &regions,
                );
                (planned, start.elapsed().as_secs_f64() * 1000.)
            }),
        });
    }

    /// Prepares the staged target and, once it has uploaded and any morph has finished,
    /// swaps it in for the drawn cover.
    #[allow(clippy::too_many_arguments)]
    fn publish_target(
        &mut self,
        worker: &WorldDatabaseWorker,
        settings: &LodSettings,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        tracker: &UploadTracker,
        contacts: &ContactInputs,
        stats: &mut TerrainLodStats,
        origin: CellCoord,
        cell_size: f32,
        visible: bool,
        delta_seconds: f32,
    ) {
        let Some(plan) = &self.target else {
            return;
        };
        let patches: Vec<_> = plan.patches.iter().map(|(&k, &e)| (k, e)).collect();
        self.prepare_target(
            worker,
            meshes,
            tracker,
            cell_size,
            MAX_NODE_BYTES,
            MAX_MESH_BYTES,
        );
        let uploaded = self.target_uploaded(tracker) && self.materials_ready(tracker);
        let changed = self.active.len() != patches.len()
            || patches.iter().any(|p| !self.active.contains_key(p));
        let transition_done = uploaded
            && (!changed
                || self.active.is_empty()
                || self.advance_transition(
                    settings,
                    commands,
                    meshes,
                    tracker,
                    cell_size,
                    origin,
                    visible,
                    delta_seconds,
                    &contacts.required,
                    &mut stats.contact_handoffs,
                ));
        if !transition_done {
            return;
        }
        if self.active.len() != patches.len()
            || patches.iter().any(|p| !self.active.contains_key(p))
        {
            self.last_plan = None;
        }
        // `patches` comes from an ordered map, so it is sorted.
        let remove: Vec<_> = self
            .active
            .keys()
            .filter(|p| patches.binary_search(p).is_err())
            .copied()
            .collect();
        for p in remove {
            let e = self.active.remove(&p).unwrap();
            commands.entity(e).despawn();
        }
        for patch @ (key, _) in patches {
            if self.active.contains_key(&patch) {
                continue;
            }
            let mesh = &self.meshes[&patch];
            let entity = commands
                .spawn((
                    Mesh3d(mesh.handle.clone()),
                    mesh.bounds,
                    bevy::camera::visibility::NoAutoAabb,
                    MeshMaterial3d(self.patch_material(key)),
                    patch_transform(key, origin, cell_size),
                    GlobalTransform::from(patch_transform(key, origin, cell_size)),
                    if visible {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    },
                    Name::new(format!("Terrain LOD {} ({},{})", key.level, key.x, key.z)),
                ))
                .id();
            self.active.insert(patch, entity);
        }
        stats.triangles = self.target.as_ref().unwrap().stats.triangles;
        self.target = None;
        self.evict(meshes, tracker);
    }
}

/// Publishes the loader's state for F1 and the periodic `TERRAIN_LOD` log.
fn fill_stats(
    stream: &mut TerrainLodStream,
    stats: &mut TerrainLodStats,
    near_stats: &terrain_render::near::NearStats,
    tracker: &UploadTracker,
    time: &Time,
) {
    stats.material_metadata = stream.composites.metadata_len();
    stats.material_detail_tiles = stream.composites.detail.as_ref().map_or(0, |d| d.count());
    stats.material_detail_uploads = stream
        .composites
        .detail
        .as_ref()
        .map_or(0, |d| d.atlas.uploads());
    stats.material_detail_limited = stream.composites.detail.as_ref().is_some_and(|d| d.limited);
    stats.near_material_pages = near_stats.pages;
    stats.near_material_ready_pages = near_stats.ready_pages;
    stats.near_material_bytes = near_stats.control_bytes;
    stats.near_material_limited = near_stats.capacity_limited;
    stats.near_material_error = near_stats.error.clone();
    stats.material_tiles = stream.composites.resident.len();
    stats.material_reserved_bytes = stream.composites.bytes;
    stats.material_status = match stream.composites.available {
        Some(false) => "geometry only; publication has no baked ground",
        Some(true) if stats.material_detail_tiles > 0 => "streamed baked ground",
        Some(true) if stream.materials_ready(tracker) => "coarse baked ground",
        _ => "loading coarse ground materials",
    }
    .into();
    stats.quality_pending = stream.target.is_some();
    stats.live_preview_bytes = stream.overlay.as_ref().map_or(0, |p| p.bytes() as u64);
    stats.live_preview_revision = stream.overlay_revision;
    stats.patches = stream.active.len();
    stats.triangles = stream
        .active
        .keys()
        .map(|(k, _)| 2 * usize::from(stream.metadata[k].resolution - 1).pow(2))
        .sum();
    stats.levels.clear();
    for (k, _) in stream.active.keys() {
        *stats.levels.entry(k.level).or_default() += 1;
    }
    stats.metadata = stream.metadata.len();
    stats.decoded_bytes = stream.decoded_bytes();
    stats.mesh_bytes = stream.mesh_bytes();
    stats.cached_mesh_bytes = stream.cached_mesh_bytes();
    stats.pending = stream.composites.decodes.len()
        + stream.pending.len()
        + stream.decodes.len()
        + stream.builds.len()
        + stream.transition.as_ref().map_or(0, Transition::pending);
    stats.morph_weight = stream
        .transition
        .as_ref()
        .filter(|t| t.running())
        .map(|t| t.weight);
    stats.transition_patches = stream.transition.as_ref().map_or(0, |t| t.patches);
    stats.transition_triangles = stream.transition.as_ref().map_or(0, |t| t.triangles);
    stats.staged = stream.target.as_ref().map_or(0, |p| p.patches.len());
    stats.status = if let Some(e) = &stream.error {
        format!("terrain LOD failed: {e}")
    } else if stats.patches == 0 {
        "loading coarse cover".into()
    } else if stats.morph_weight.is_some() {
        "morphing terrain cover".into()
    } else if stream.transition.is_some() {
        "preparing terrain morph".into()
    } else if stream
        .overlay
        .as_ref()
        .is_some_and(|o| !o.leaves.is_empty())
    {
        "live terrain preview".into()
    } else {
        "published terrain preview".into()
    };
    if time.elapsed_secs_f64() - stream.last_report > 2.0 {
        stream.last_report = time.elapsed_secs_f64();
        debug!("TERRAIN_LOD {stats:?}");
    }
}

/// The most recently used entries whose sizes fit `budget`, newest first.
fn recent_within(
    entries: impl Iterator<Item = (TerrainNodeKey, u64, u64)>,
    budget: u64,
) -> BTreeSet<TerrainNodeKey> {
    let mut entries: Vec<_> = entries.collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut total = 0;
    entries
        .into_iter()
        .take_while(|&(_, _, bytes)| {
            total += bytes;
            total <= budget
        })
        .map(|(key, _, _)| key)
        .collect()
}

/// An actor waiting more than a few seconds for ground is reported with the loader's state,
/// including early exits that skip the periodic report.
fn report_stall(
    stream: &mut TerrainLodStream,
    stats: &TerrainLodStats,
    contacts: &ContactInputs,
    now: f64,
) {
    if stats.blocked_actors == 0 {
        stream.stalled_since = None;
        return;
    }
    let since = *stream.stalled_since.get_or_insert(now);
    if now - since < 5. || now - stream.last_stall_report < 5. {
        return;
    }
    stream.last_stall_report = now;
    warn!(
        "TERRAIN_STALL actor waiting {:.0} s: status={:?} error={:?} contact_error={:?} target={:?} transition={:?} pending={} metadata={} mesh={:.0} MB patches={} plan_budget_limited={} contact_limited={} drawn_contact_limited={} blocked_grass_pages={} plans={} replans={} unmorphed_swaps={}",
        now - since,
        stats.status,
        stream.error,
        contacts.error,
        stream
            .target
            .as_ref()
            .map(|t| (t.patches.len(), t.requests.len(), t.balanced)),
        stream.transition.as_ref().map(|t| t.running()),
        stream.pending.len(),
        stream.metadata.len(),
        stream.mesh_bytes() as f64 / 1048576.,
        stats.patches,
        stats.budget_limited,
        stats.contact_limited,
        stats.drawn_contact_limited,
        stats.blocked_grass_pages,
        stats.plans,
        stream.replans,
        stream.unmorphed_swaps,
    );
}

#[cfg(test)]
mod budget_probe;
#[cfg(test)]
mod tests;

fn near_view(
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    camera: crate::ActiveWorldView,
    mut view: ResMut<terrain_render::near::NearView>,
    stream: Res<TerrainLodStream>,
) {
    view.identity = None;
    let Some(space) = origin.space().and_then(|id| catalog.world_space(id)) else {
        return;
    };
    let Some(camera) = camera.active() else {
        return;
    };
    let p = origin.cell().origin(space.cell_size);
    view.identity = Some((
        format!(
            "{}:preview:{}",
            catalog.generation_id(),
            stream.overlay_revision
        ),
        space.id,
    ));
    view.origin = origin.cell();
    view.eye = camera.transform.translation().as_dvec3() + DVec3::new(p[0], 0., p[1]);
}
