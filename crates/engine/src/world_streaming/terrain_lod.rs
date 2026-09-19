//! Default terrain hierarchy shared by the editor and game, with bounded material
//! streaming and regional live authoring. Legacy rendering is a diagnostic option.
use super::*;
use bevy::{
    math::{DMat4, DVec3},
    render::{
        Render, RenderApp, RenderSystems,
        mesh::{RenderMesh, allocator::MeshAllocator},
        render_asset::RenderAssets,
    },
};
use std::sync::{Arc, Mutex};
mod contact;
mod material;
use terrain_render::TerrainCompositeMaterial;
pub(super) mod entry;
mod transition;
pub use contact::TerrainContactReadiness;
mod authoring;
pub use authoring::{LiveTerrainPreview, TerrainPreviewRequest};
use contact::{ContactInputs, ContactSystems};
use terrain_render::lod::{self, LodSettings, LodView, PatchMetadata, PlannedCover, StitchEdges};
use transition::{Transition, patch_transform};
use world::{TerrainNode, TerrainNodeKey};
use world_db::{EncodedTerrainNode, TerrainNodeDescriptor};

const MAX_METADATA: usize = 4096;
const MAX_NODE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_MESH_BYTES: u64 = 128 * 1024 * 1024;
const MAX_REQUESTS: usize = 4;
const MAX_BUILDS: usize = 2;
type Patch = (TerrainNodeKey, StitchEdges);

#[derive(Resource, Clone)]
pub struct TerrainLodPreview {
    pub enabled: bool,
    pub settings: LodSettings,
}
impl Default for TerrainLodPreview {
    fn default() -> Self {
        Self::from_args(std::env::args_os())
    }
}
impl TerrainLodPreview {
    fn from_args(args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>) -> Self {
        Self {
            enabled: !args.into_iter().any(|a| a.as_ref() == "--terrain-legacy"),
            settings: LodSettings::default(),
        }
    }
}
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
    /// Cumulative source-height certifications, excluding reused certificates.
    pub contact_source_checks: u64,
    pub contact_source_samples: u64,
    pub metadata: usize,
    pub decoded_bytes: u64,
    pub mesh_bytes: u64,
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
    /// A whole replacement group shares this weight; None means no visible morph.
    pub morph_weight: Option<f32>,
    pub transition_patches: usize,
    pub transition_triangles: usize,
}
#[derive(Clone, Debug)]
pub(super) enum TerrainQuery {
    Roots(WorldSpaceId),
    Metadata(Vec<TerrainNodeKey>),
    Node(TerrainNodeKey),
    Material(material::Query),
}
#[derive(Debug)]
pub(super) enum TerrainReply {
    Metadata(Vec<TerrainNodeDescriptor>),
    Node(EncodedTerrainNode),
    Material(material::Reply),
}
pub(super) fn read(reader: &RuntimeReader, query: TerrainQuery) -> Result<TerrainReply, String> {
    match query {
        TerrainQuery::Material(query) => material::read(reader, query).map(TerrainReply::Material),
        TerrainQuery::Roots(space) => reader
            .read_terrain_roots(space)
            .map(TerrainReply::Metadata)
            .map_err(|e| e.to_string()),
        TerrainQuery::Metadata(keys) => reader
            .read_terrain_node_descriptors(&keys)
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .map(TerrainReply::Metadata)
            .ok_or("missing required terrain metadata".into()),
        TerrainQuery::Node(key) => reader
            .read_terrain_node(key)
            .map_err(|e| e.to_string())?
            .map(TerrainReply::Node)
            .ok_or("missing required terrain node".into()),
    }
}

#[derive(Default)]
struct Uploads {
    #[cfg(test)]
    pause_acknowledgements: bool,
    #[cfg(test)]
    pause_material_acknowledgements: bool,
    wanted: std::collections::HashSet<bevy::asset::AssetId<Mesh>>,
    ready: std::collections::HashSet<bevy::asset::AssetId<Mesh>>,
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
    tracker.ready = tracker
        .wanted
        .iter()
        .copied()
        .filter(|&id| {
            meshes.get(id).is_some_and(|m| {
                !m.has_morph_targets() || allocator.mesh_morph_target_slice(&id).is_some()
            }) && allocator.mesh_vertex_slice(&id).is_some()
                && allocator.mesh_index_slice(&id).is_some()
        })
        .collect();
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
                        .filter_map(|v| v.get(&entity).map(|(p, _)| *p)),
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
        .init_resource::<TerrainLodPreview>()
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
}
struct MeshJob {
    patch: Patch,
    bytes: u64,
    task: Task<Result<Mesh, String>>,
}
#[derive(Resource, Default)]
pub(crate) struct TerrainLodStream {
    overlay: Option<Arc<world::TerrainPreviewProducts>>,
    overlay_revision: u64,
    identity: Option<(String, WorldSpaceId)>,
    next_id: u64,
    pending: BTreeMap<u64, TerrainQuery>,
    roots: Option<Vec<TerrainNodeKey>>,
    descriptors: BTreeMap<TerrainNodeKey, TerrainNodeDescriptor>,
    metadata: BTreeMap<TerrainNodeKey, PatchMetadata>,
    nodes: BTreeMap<TerrainNodeKey, TerrainNode>,
    decodes: BTreeMap<TerrainNodeKey, Task<Result<TerrainNode, String>>>,
    builds: Vec<MeshJob>,
    meshes: BTreeMap<Patch, ResidentMesh>,
    active: BTreeMap<Patch, Entity>,
    target: Option<PlannedCover>,
    transition: Option<Transition>,
    material: Option<Handle<TerrainCompositeMaterial>>,
    composites: material::Cover,
    error: Option<String>,
    last_report: f64,
    last_plan: Option<PlanIdentity>,
    metadata_revision: u64,
    position_origin: Option<CellCoord>,
    draw_visible: bool,
}
impl TerrainLodStream {
    pub(super) fn receive(&mut self, request_id: u64, reply: Result<TerrainReply, String>) {
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
                    self.metadata.insert(
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
        let id = self.next_id.wrapping_add(1).max(1);
        self.next_id = id;
        match worker.requests.try_send(DatabaseRequest::Terrain {
            request_id: id,
            generation: generation.clone(),
            query: query.clone(),
        }) {
            Ok(()) => {
                self.pending.insert(id, query);
            }
            Err(TrySendError::Full(_)) => (),
            Err(TrySendError::Disconnected(_)) => {
                self.error = Some("terrain database worker stopped".into())
            }
        }
    }
    fn decoded_bytes(&self) -> u64 {
        let mut keys: BTreeSet<_> = self
            .nodes
            .keys()
            .chain(self.decodes.keys())
            .copied()
            .collect();
        for query in self.pending.values() {
            if let TerrainQuery::Node(key) = query {
                keys.insert(*key);
            }
        }
        keys.iter()
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
        let next_id = self.next_id;
        let material = self.material.take();
        *self = Self {
            next_id,
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
                    Ok(mesh) => {
                        let handle = meshes.add(mesh);
                        tracker.0.lock().unwrap().wanted.insert(handle.id());
                        self.meshes.insert(
                            job.patch,
                            ResidentMesh {
                                handle,
                                bytes: job.bytes,
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
        cell_size: f32,
        node_limit: u64,
        mesh_limit: u64,
    ) {
        let Some(plan) = &self.target else {
            return;
        };
        let patches: Vec<_> = plan.patches.iter().map(|(&k, &e)| (k, e)).collect();
        for patch @ (key, edges) in &patches {
            if self.meshes.contains_key(patch) || self.builds.iter().any(|b| b.patch == *patch) {
                continue;
            }
            if let Some(field) = self.nodes.get(key).and_then(|n| n.heightfield.as_ref()) {
                if self.builds.len() >= MAX_BUILDS {
                    continue;
                }
                let bytes = self.descriptors[key].gpu_bytes_estimate;
                if self.mesh_bytes() + bytes > mesh_limit {
                    self.error = Some("terrain replacement exceeds mesh budget".into());
                    break;
                }
                let field = field.clone();
                let extent = cell_size * (1_u32 << key.level) as f32;
                let edges = *edges;
                self.builds.push(MeshJob {
                    patch: *patch,
                    bytes,
                    task: AsyncComputeTaskPool::get()
                        .spawn(async move { lod::build_patch_mesh(&field, extent, edges) }),
                });
            } else if !self.decodes.contains_key(key)
                && !self
                    .pending
                    .values()
                    .any(|q| matches!(q,TerrainQuery::Node(k) if k==key))
            {
                if self.decoded_bytes() + self.descriptors[key].decoded_bytes > node_limit {
                    self.error = Some("terrain replacement exceeds sample budget".into());
                    break;
                }
                self.request(worker, TerrainQuery::Node(*key));
            }
        }
    }
    fn target_uploaded(&self, tracker: &UploadTracker) -> bool {
        let Some(plan) = &self.target else {
            return false;
        };
        let patches: Vec<_> = plan.patches.iter().map(|(&k, &e)| (k, e)).collect();
        let uploads = tracker.0.lock().unwrap();
        patches.iter().all(|p| {
            self.meshes
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
        self.meshes.retain(|patch, mesh| {
            if wanted.contains(patch) {
                true
            } else {
                assets.remove(mesh.handle.id());
                let mut t = tracker.0.lock().unwrap();
                t.wanted.remove(&mesh.handle.id());
                t.ready.remove(&mesh.handle.id());
                false
            }
        });
        let keys: BTreeSet<_> = wanted
            .iter()
            .map(|(k, _)| *k)
            .chain(self.roots.iter().flatten().copied())
            .collect();
        self.nodes.retain(|key, _| keys.contains(key));
        // Keep ancestors for hysteresis/balancing, direct children for pending refinements,
        // and all in-flight descriptors. Discard old distant branches after movement.
        let mut metadata = keys.clone();
        for key in keys {
            let mut ancestor = key;
            while let Some(parent) = ancestor.parent().ok().flatten() {
                if !self.metadata.contains_key(&parent) {
                    break;
                }
                metadata.insert(parent);
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
        self.metadata.retain(|k, _| metadata.contains(k));
        self.descriptors.retain(|k, _| metadata.contains(k));
    }
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    config: Res<TerrainLodPreview>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    active_space: Res<ActiveWorldSpace>,
    worker: Option<Res<WorldDatabaseWorker>>,
    time: Res<Time>,
    camera: Query<(&Camera, &GlobalTransform), With<WorldViewCamera>>,
    mut leaves: Query<&mut Visibility, With<StreamedTerrainSurface>>,
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
    if !config.enabled {
        if stream.identity.is_some() {
            stream.clear(&mut commands, &mut meshes, &tracker);
            for mut v in &mut leaves {
                *v = Visibility::Inherited;
            }
        }
        stats.status = "disabled".into();
        return;
    }
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
    let visible = camera.iter().any(|(c, _)| c.is_active);
    let ready = stream.roots.is_some()
        && (!stream.active.is_empty() || stream.roots.as_ref().is_some_and(Vec::is_empty));
    for mut visibility in &mut leaves {
        let desired = if ready && visible {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        if *visibility != desired {
            *visibility = desired;
        }
    }
    // Position from canonical keys each frame; rebasing never requires terrain reload.
    if stream.position_origin != Some(origin.cell()) || stream.draw_visible != visible {
        stream.sync_material_origin(&mut materials, origin.cell(), info.cell_size);
        for (&patch, &entity) in &stream.active {
            let show = visible
                && stream
                    .transition
                    .as_ref()
                    .is_none_or(|t| !t.running() || t.keeps(&patch));
            commands.entity(entity).insert((
                patch_transform(patch.0, origin.cell(), info.cell_size),
                GlobalTransform::from(patch_transform(patch.0, origin.cell(), info.cell_size)),
                if show {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                },
            ));
        }
        if let Some(t) = &stream.transition {
            for (&key, &entity) in &t.entities {
                commands.entity(entity).insert((
                    patch_transform(key, origin.cell(), info.cell_size),
                    GlobalTransform::from(patch_transform(key, origin.cell(), info.cell_size)),
                    if visible {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    },
                ));
            }
        }
        stream.position_origin = Some(origin.cell());
        stream.draw_visible = visible;
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
    if let Some(error) = &stream.error {
        stats.status = format!("terrain LOD failed (cover retained): {error}");
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
    // Freeze a replacement while it uploads; changing camera demand cannot continually
    // cancel the last missing child and starve publication.
    if stream.target.is_none()
        && visible
        && let Some((camera, transform)) = camera.iter().find(|(c, _)| c.is_active)
        && let Some(size) = camera.physical_viewport_size()
        && let Some(roots) = stream.roots.as_ref()
    {
        let shift = DVec3::new(
            origin.cell().x as f64 * info.cell_size as f64,
            0.0,
            origin.cell().z as f64 * info.cell_size as f64,
        );
        let view = LodView {
            clip_from_world: camera.clip_from_view().as_dmat4()
                * transform.to_matrix().as_dmat4().inverse()
                * DMat4::from_translation(-shift),
            viewport: [size.x, size.y],
            contact_position: transform.translation().as_dvec3() + shift,
        };
        let identity = PlanIdentity {
            view: view.clone(),
            settings: config.settings.clone(),
            metadata_revision: stream.metadata_revision,
            contacts: contacts.planning.clone(),
        };
        if stream.last_plan.as_ref() != Some(&identity) {
            let previous = stream.active.keys().map(|(k, _)| *k).collect();
            match lod::plan_cover_with_contacts(
                roots,
                &stream.metadata,
                &previous,
                &view,
                info.cell_size as f64,
                &config.settings,
                &contacts.planning,
            ) {
                Ok(plan) => {
                    if plan.requests.is_empty() {
                        stream.last_plan = Some(identity);
                    }
                    stats.maximum_visible_error = plan.stats.maximum_visible_error;
                    stats.budget_limited = plan.stats.budget_limited;
                    stats.contact_limited = plan.stats.contact_limited;
                    let queued: BTreeSet<_> = stream
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
                    let keys: Vec<_> =
                        plan.requests
                            .iter()
                            .filter(|k| !queued.contains(k))
                            .copied()
                            .take(world_db::MAX_TERRAIN_NODE_QUERY.min(
                                MAX_METADATA.saturating_sub(stream.metadata.len() + queued.len()),
                            ))
                            .collect();
                    if !keys.is_empty() {
                        stream.request(&worker, TerrainQuery::Metadata(keys));
                    }
                    if plan.balanced {
                        stream.target = Some(plan);
                    } else {
                        stats.status = "balancing coarse terrain cover".into();
                    }
                }
                Err(e) => stream.error = Some(e),
            }
        }
    }
    if let Some(plan) = &stream.target {
        let patches: Vec<_> = plan.patches.iter().map(|(&k, &e)| (k, e)).collect();
        stream.prepare_target(&worker, info.cell_size, MAX_NODE_BYTES, MAX_MESH_BYTES);
        let uploaded = stream.target_uploaded(&tracker) && stream.materials_ready(&tracker);
        let changed = stream.active.len() != patches.len()
            || patches.iter().any(|p| !stream.active.contains_key(p));
        let transition_done = uploaded
            && (!changed
                || stream.active.is_empty()
                || stream.advance_transition(
                    &config.settings,
                    &mut commands,
                    &mut meshes,
                    &tracker,
                    info.cell_size,
                    origin.cell(),
                    visible,
                    time.delta_secs(),
                    &contacts.required,
                    &mut stats.contact_handoffs,
                ));
        if transition_done {
            if stream.active.len() != patches.len()
                || patches.iter().any(|p| !stream.active.contains_key(p))
            {
                stream.last_plan = None;
            }
            let remove: Vec<_> = stream
                .active
                .keys()
                .filter(|p| !patches.contains(p))
                .copied()
                .collect();
            for p in remove {
                let e = stream.active.remove(&p).unwrap();
                commands.entity(e).despawn();
            }
            for patch @ (key, _) in patches {
                if stream.active.contains_key(&patch) {
                    continue;
                }
                let entity = commands
                    .spawn((
                        Mesh3d(stream.meshes[&patch].handle.clone()),
                        MeshMaterial3d(stream.patch_material(key)),
                        patch_transform(key, origin.cell(), info.cell_size),
                        GlobalTransform::from(patch_transform(key, origin.cell(), info.cell_size)),
                        if visible {
                            Visibility::Inherited
                        } else {
                            Visibility::Hidden
                        },
                        Name::new(format!("Terrain LOD {} ({},{})", key.level, key.x, key.z)),
                    ))
                    .id();
                stream.active.insert(patch, entity);
            }
            stats.triangles = stream.target.as_ref().unwrap().stats.triangles;
            stream.target = None;
            stream.evict(&mut meshes, &tracker);
            // Same deferred-command boundary as the complete replacement group.
            for mut visibility in &mut leaves {
                *visibility = if visible {
                    Visibility::Hidden
                } else {
                    Visibility::Inherited
                };
            }
        }
    }
    if stream.materials_ready(&tracker)
        && let Some((camera, transform)) = camera.iter().find(|(c, _)| c.is_active)
        && let Some(viewport) = camera.physical_viewport_size()
    {
        let shift = DVec3::new(
            origin.cell().x as f64 * info.cell_size as f64,
            0.,
            origin.cell().z as f64 * info.cell_size as f64,
        );
        let view = LodView {
            clip_from_world: camera.clip_from_view().as_dmat4()
                * transform.to_matrix().as_dmat4().inverse()
                * DMat4::from_translation(-shift),
            viewport: [viewport.x, viewport.y],
            contact_position: transform.translation().as_dvec3() + shift,
        };
        stream.update_material_detail(
            &worker,
            &view,
            info.cell_size,
            &mut images,
            &mut buffers,
            &mut materials,
            &upload_hub,
            time.delta_secs(),
        );
    }

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
        Some(true) if stream.materials_ready(&tracker) => "coarse baked ground",
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
        info!("TERRAIN_LOD {stats:?}");
    }
}

#[cfg(test)]
mod budget_probe;
#[cfg(test)]
mod tests;

fn near_view(
    config: Res<TerrainLodPreview>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    cameras: Query<(&Camera, &GlobalTransform), With<WorldViewCamera>>,
    mut view: ResMut<terrain_render::near::NearView>,
    stream: Res<TerrainLodStream>,
) {
    view.identity = None;
    if !config.enabled {
        return;
    }
    let Some(space) = origin.space().and_then(|id| catalog.world_space(id)) else {
        return;
    };
    let Some((_, t)) = cameras.iter().find(|(c, _)| c.is_active) else {
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
    view.eye = t.translation().as_dvec3() + DVec3::new(p[0], 0., p[1]);
}
