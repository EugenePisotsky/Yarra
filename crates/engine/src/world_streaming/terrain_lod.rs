//! Opt-in geometry validation path, shared by the editor and game. This consumes
//! published terrain only; normal authoring stays on its detailed, editable meshes.
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
use terrain_render::lod::{self, LodSettings, LodView, PatchMetadata, PlannedCover, StitchEdges};
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
        Self {
            enabled: std::env::args().any(|a| a == "--terrain-lod"),
            settings: LodSettings::default(),
        }
    }
}
#[derive(Resource, Clone, Debug, Default)]
pub struct TerrainLodStats {
    pub status: String,
    pub patches: usize,
    pub triangles: usize,
    pub levels: BTreeMap<u8, usize>,
    pub maximum_visible_error: f64,
    pub budget_limited: bool,
    pub contact_limited: bool,
    pub metadata: usize,
    pub decoded_bytes: u64,
    pub mesh_bytes: u64,
    pub pending: usize,
    pub staged: usize,
}
#[derive(Clone, Debug)]
pub(super) enum TerrainQuery {
    Roots(WorldSpaceId),
    Metadata(Vec<TerrainNodeKey>),
    Node(TerrainNodeKey),
}
#[derive(Debug)]
pub(super) enum TerrainReply {
    Metadata(Vec<TerrainNodeDescriptor>),
    Node(EncodedTerrainNode),
}
pub(super) fn read(reader: &RuntimeReader, query: TerrainQuery) -> Result<TerrainReply, String> {
    match query {
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
    wanted: std::collections::HashSet<bevy::asset::AssetId<Mesh>>,
    ready: std::collections::HashSet<bevy::asset::AssetId<Mesh>>,
}
#[derive(Resource, Clone, Default)]
struct UploadTracker(Arc<Mutex<Uploads>>);
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
            meshes.get(id).is_some()
                && allocator.mesh_vertex_slice(&id).is_some()
                && allocator.mesh_index_slice(&id).is_some()
        })
        .collect();
}
pub(super) fn install(app: &mut App) {
    let tracker = UploadTracker::default();
    app.insert_resource(tracker.clone())
        .init_resource::<TerrainLodPreview>()
        .init_resource::<TerrainLodStats>()
        .init_resource::<TerrainLodStream>()
        .add_systems(PostUpdate, update.before(TransformSystems::Propagate));
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.insert_resource(tracker).add_systems(
            Render,
            acknowledge_uploads.after(RenderSystems::PrepareMeshes),
        );
    }
}

#[derive(PartialEq)]
struct PlanIdentity {
    view: LodView,
    settings: LodSettings,
    metadata_revision: u64,
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
pub(super) struct TerrainLodStream {
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
    material: Option<Handle<StandardMaterial>>,
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
                for d in descriptors {
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
        if self.pending.len() + self.decodes.len() >= MAX_REQUESTS {
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
        keys.iter().map(|k| self.descriptors[k].decoded_bytes).sum()
    }
    fn mesh_bytes(&self) -> u64 {
        self.meshes.values().map(|m| m.bytes).sum::<u64>()
            + self.builds.iter().map(|b| b.bytes).sum::<u64>()
    }
    fn clear(
        &mut self,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        tracker: &UploadTracker,
    ) {
        for (_, entity) in std::mem::take(&mut self.active) {
            commands.entity(entity).despawn();
        }
        for (_, mesh) in std::mem::take(&mut self.meshes) {
            meshes.remove(mesh.handle.id());
        }
        *tracker.0.lock().unwrap() = Uploads::default();
        let next_id = self.next_id;
        let material = self.material.take();
        *self = Self {
            next_id,
            material,
            ..default()
        };
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
    mut materials: ResMut<Assets<StandardMaterial>>,
    tracker: Res<UploadTracker>,
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
        stream.material = Some(materials.add(StandardMaterial {
            base_color: Color::srgb(0.37, 0.43, 0.29),
            perceptual_roughness: 1.0,
            ..default()
        }));
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
        for (&(key, _), &entity) in &stream.active {
            let min = key.cell_bounds().unwrap()[0];
            commands.entity(entity).insert((
                Transform::from_xyz(
                    ((min.x as i64 - origin.cell().x as i64) as f64 * info.cell_size as f64) as f32,
                    0.0,
                    ((min.z as i64 - origin.cell().z as i64) as f64 * info.cell_size as f64) as f32,
                ),
                if visible {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                },
            ));
        }
        stream.position_origin = Some(origin.cell());
        stream.draw_visible = visible;
    }
    if let Some(error) = &stream.error {
        stats.status = format!("terrain LOD failed (cover retained): {error}");
        return;
    }
    if stream.roots.is_none() && stream.pending.is_empty() {
        stream.request(&worker, TerrainQuery::Roots(space));
    }
    let finished: Vec<_> = stream
        .decodes
        .iter_mut()
        .filter_map(|(&key, task)| check_ready(task).map(|r| (key, r)))
        .collect();
    for (key, result) in finished {
        stream.decodes.remove(&key);
        match result {
            Ok(node) => {
                stream.nodes.insert(key, node);
            }
            Err(e) => stream.error = Some(e),
        }
    }
    let mut i = 0;
    while i < stream.builds.len() {
        if let Some(result) = check_ready(&mut stream.builds[i].task) {
            let job = stream.builds.remove(i);
            match result {
                Ok(mesh) => {
                    let handle = meshes.add(mesh);
                    tracker.0.lock().unwrap().wanted.insert(handle.id());
                    stream.meshes.insert(
                        job.patch,
                        ResidentMesh {
                            handle,
                            bytes: job.bytes,
                        },
                    );
                }
                Err(e) => stream.error = Some(e),
            }
        } else {
            i += 1;
        }
    }
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
        };
        if stream.last_plan.as_ref() != Some(&identity) {
            let previous = stream.active.keys().map(|(k, _)| *k).collect();
            match lod::plan_cover(
                roots,
                &stream.metadata,
                &previous,
                &view,
                info.cell_size as f64,
                &config.settings,
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
        for patch @ (key, edges) in &patches {
            if stream.meshes.contains_key(patch) || stream.builds.iter().any(|b| b.patch == *patch)
            {
                continue;
            }
            if let Some(field) = stream.nodes.get(key).and_then(|n| n.heightfield.as_ref()) {
                if stream.builds.len() >= MAX_BUILDS {
                    continue;
                }
                let bytes = stream.descriptors[key].gpu_bytes_estimate;
                if stream.mesh_bytes() + bytes > MAX_MESH_BYTES {
                    stream.error = Some("terrain replacement exceeds mesh budget".into());
                    break;
                }
                let field = field.clone();
                let extent = info.cell_size * (1_u32 << key.level) as f32;
                let edges = *edges;
                stream.builds.push(MeshJob {
                    patch: *patch,
                    bytes,
                    task: AsyncComputeTaskPool::get()
                        .spawn(async move { lod::build_patch_mesh(&field, extent, edges) }),
                });
            } else if !stream.decodes.contains_key(key)
                && !stream
                    .pending
                    .values()
                    .any(|q| matches!(q,TerrainQuery::Node(k) if k==key))
            {
                if stream.decoded_bytes() + stream.descriptors[key].decoded_bytes > MAX_NODE_BYTES {
                    stream.error = Some("terrain replacement exceeds sample budget".into());
                    break;
                }
                stream.request(&worker, TerrainQuery::Node(*key));
            }
        }
        let uploads = tracker.0.lock().unwrap();
        let uploaded = patches.iter().all(|p| {
            stream
                .meshes
                .get(p)
                .is_some_and(|m| uploads.ready.contains(&m.handle.id()))
        });
        drop(uploads);
        if uploaded {
            if stream.active.len() != patches.len()
                || patches.iter().any(|p| !stream.active.contains_key(p))
            {
                stream.last_plan = None;
            }
            let material = stream.material.as_ref().unwrap().clone();
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
                let min = key.cell_bounds().unwrap()[0];
                let entity = commands
                    .spawn((
                        Mesh3d(stream.meshes[&patch].handle.clone()),
                        MeshMaterial3d(material.clone()),
                        Transform::from_xyz(
                            ((min.x as i64 - origin.cell().x as i64) as f64 * info.cell_size as f64)
                                as f32,
                            0.0,
                            ((min.z as i64 - origin.cell().z as i64) as f64 * info.cell_size as f64)
                                as f32,
                        ),
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
    stats.patches = stream.active.len();
    stats.levels.clear();
    for (k, _) in stream.active.keys() {
        *stats.levels.entry(k.level).or_default() += 1;
    }
    stats.metadata = stream.metadata.len();
    stats.decoded_bytes = stream.decoded_bytes();
    stats.mesh_bytes = stream.mesh_bytes();
    stats.pending = stream.pending.len() + stream.decodes.len() + stream.builds.len();
    stats.staged = stream.target.as_ref().map_or(0, |p| p.patches.len());
    stats.status = if let Some(e) = &stream.error {
        format!("terrain LOD failed: {e}")
    } else if stats.patches == 0 {
        "loading coarse cover".into()
    } else {
        "published geometry preview".into()
    };
    if time.elapsed_secs_f64() - stream.last_report > 2.0 {
        stream.last_report = time.elapsed_secs_f64();
        info!("TERRAIN_LOD {stats:?}");
    }
}

#[cfg(test)]
mod tests;
