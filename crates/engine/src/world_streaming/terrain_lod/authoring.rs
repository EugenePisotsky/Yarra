//! Two-phase editor handoff: upload a complete affected region, then let the
//! editor commit its matching grass/source cells at the same frame boundary.
use super::*;
use std::sync::Arc;
use world::{TerrainMaterialKey, TerrainPreviewProducts};

#[derive(Clone)]
pub struct TerrainPreviewRequest {
    pub revision: u64,
    pub generation: String,
    pub space: WorldSpaceId,
    pub products: Arc<TerrainPreviewProducts>,
}
#[derive(Resource, Default)]
pub struct LiveTerrainPreview {
    pub request: Option<TerrainPreviewRequest>,
    pub ready: Option<u64>,
    pub commit: Option<u64>,
    pub applied: Option<u64>,
    pub error: Option<String>,
    stage: Option<Stage>,
}
#[derive(Default)]
struct Stage {
    appearance_changed: bool,
    revision: u64,
    todo: Vec<Patch>,
    jobs: Vec<MeshJob>,
    meshes: BTreeMap<Patch, ResidentMesh>,
    materials: BTreeMap<TerrainMaterialKey, Handle<TerrainCompositeMaterial>>,
    bytes: u64,
}
impl Stage {
    fn clear(self, meshes: &mut Assets<Mesh>, tracker: &UploadTracker) {
        let mut t = tracker.0.lock().unwrap();
        for mesh in self.meshes.values() {
            meshes.remove(mesh.handle.id());
            t.wanted.remove(&mesh.handle.id());
            t.ready.remove(&mesh.handle.id());
        }
        for handle in self.materials.values() {
            t.material_wanted.remove(&handle.id());
            t.material_ready.remove(&handle.id());
        }
    }
}

pub(super) fn descriptor(node: &TerrainNode) -> TerrainNodeDescriptor {
    TerrainNodeDescriptor {
        key: node.key,
        child_mask: node.child_mask,
        height_bounds: node.height_bounds,
        geometric_error: node.geometric_error,
        resolution: node.heightfield.as_ref().map(|h| h.resolution),
        decoded_bytes: node
            .heightfield
            .as_ref()
            .map_or(128, |h| 128 + h.heights.len() as u64 * 8),
        gpu_bytes_estimate: node.gpu_bytes_estimate(),
        checksum: [0; 32],
    }
}
impl TerrainLodStream {
    /// Overlay reads have no IO and never compete with published replies.
    pub(super) fn preview_request(&mut self, query: &TerrainQuery) -> bool {
        let Some(overlay) = &self.overlay else {
            return false;
        };
        match query {
            TerrainQuery::Node(key) => {
                if let Some(node) = overlay.nodes.get(key) {
                    self.nodes.insert(*key, (**node).clone());
                    return true;
                }
            }
            TerrainQuery::Material(material::Query::Tile(key)) => {
                if let Some(tile) = overlay.composites.get(key) {
                    let tile = tile.clone();
                    self.composites.decodes.insert(
                        *key,
                        AsyncComputeTaskPool::get().spawn(async move { Ok((*tile).clone()) }),
                    );
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update(
    mut commands: Commands,
    mut live: ResMut<LiveTerrainPreview>,
    config: Res<TerrainLodPreview>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    mut stream: ResMut<TerrainLodStream>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TerrainCompositeMaterial>>,
    mut images: ResMut<Assets<Image>>,
    tracker: Res<UploadTracker>,
    entry: Res<entry::TerrainEntry>,
) {
    let request = live.request.clone().filter(|r| {
        config.enabled && r.generation == catalog.generation_id() && Some(r.space) == origin.space()
    });
    if live
        .stage
        .as_ref()
        .is_some_and(|s| request.as_ref().is_none_or(|r| r.revision != s.revision))
    {
        live.stage.take().unwrap().clear(&mut meshes, &tracker);
        live.ready = None;
    }
    let Some(request) = request else { return };
    if live.applied == Some(request.revision) || live.error.is_some() {
        return;
    }
    if (entry.pending() && live.commit != Some(request.revision))
        || stream.identity.as_ref() != Some(&(request.generation.clone(), request.space))
        || stream.active.is_empty()
        || !stream.materials_ready(&tracker)
    {
        return;
    }
    // Finish an already drawn morph. Unshown replacements may be cancelled safely.
    if stream.transition.as_ref().is_some_and(|t| t.running()) {
        return;
    }
    if let Some(t) = stream.transition.take() {
        t.clear(&mut commands, &mut meshes, &tracker);
    }
    stream.target = None;
    stream.builds.clear();
    stream.evict(&mut meshes, &tracker);
    let size = catalog.world_space(request.space).unwrap().cell_size;
    if live.stage.is_none() {
        let appearance_changed = stream
            .overlay
            .as_ref()
            .map_or(!request.products.composites.is_empty(), |o| {
                o.composites != request.products.composites
            });
        let mut stage = Stage {
            revision: request.revision,
            appearance_changed,
            ..default()
        };
        for &patch in stream.active.keys() {
            if let Some(node) = request.products.nodes.get(&patch.0)
                && stream.nodes.get(&patch.0) != Some(node.as_ref())
            {
                stage.bytes += node.gpu_bytes_estimate();
                stage.todo.push(patch);
            }
        }
        if stage.bytes + stream.mesh_bytes() > MAX_MESH_BYTES {
            live.error = Some(
                "Live terrain replacement exceeds mesh memory budget; previous preview retained"
                    .into(),
            );
            return;
        }
        let root_tiles: Vec<_> = stream
            .roots
            .iter()
            .flatten()
            .filter_map(|k| {
                let key = TerrainMaterialKey(*k);
                request
                    .products
                    .composites
                    .get(&key)
                    .filter(|t| {
                        stream.overlay.as_ref().and_then(|o| o.composites.get(&key)) != Some(t)
                    })
                    .map(|t| (key, t.clone()))
            })
            .collect();
        if stream.composites.bytes
            + root_tiles.len() as u64 * world::TerrainComposite::gpu_bytes() as u64
            > material::MAX_MATERIAL_BYTES
        {
            live.error=Some("Live terrain replacement exceeds material memory budget; previous preview retained".into());
            return;
        }
        for (key, tile) in root_tiles {
            match TerrainCompositeMaterial::from_composite(
                (*tile).clone(),
                origin.cell(),
                size,
                &mut images,
            ) {
                Ok(m) => {
                    let h = materials.add(m);
                    tracker.0.lock().unwrap().material_wanted.insert(h.id());
                    stage.materials.insert(key, h);
                }
                Err(e) => {
                    stage.clear(&mut meshes, &tracker);
                    live.error = Some(e);
                    return;
                }
            }
        }
        live.stage = Some(stage);
    }
    let stage = live.stage.as_mut().unwrap();
    let mut i = 0;
    while i < stage.jobs.len() {
        if let Some(result) = check_ready(&mut stage.jobs[i].task) {
            let job = stage.jobs.remove(i);
            match result {
                Ok(mesh) => {
                    let handle = meshes.add(mesh);
                    tracker.0.lock().unwrap().wanted.insert(handle.id());
                    stage.meshes.insert(
                        job.patch,
                        ResidentMesh {
                            handle,
                            bytes: job.bytes,
                        },
                    );
                }
                Err(e) => {
                    live.error = Some(e);
                    return;
                }
            }
        } else {
            i += 1;
        }
    }
    while stage.jobs.len() < MAX_BUILDS {
        let Some(patch) = stage.todo.pop() else { break };
        let node = request.products.nodes[&patch.0].clone();
        let bytes = node.gpu_bytes_estimate();
        stage.jobs.push(MeshJob {
            patch,
            bytes,
            task: AsyncComputeTaskPool::get().spawn(async move {
                lod::build_patch_mesh(
                    node.heightfield
                        .as_ref()
                        .ok_or("missing live terrain samples")?,
                    size * (1_u32 << patch.0.level) as f32,
                    patch.1,
                )
            }),
        });
    }
    let ready = {
        let t = tracker.0.lock().unwrap();
        stage.todo.is_empty()
            && stage.jobs.is_empty()
            && stage
                .meshes
                .values()
                .all(|m| t.ready.contains(&m.handle.id()))
            && stage
                .materials
                .values()
                .all(|m| t.material_ready.contains(&m.id()))
    };
    if !ready {
        return;
    }
    live.ready = Some(request.revision);
    if live.commit != Some(request.revision) {
        return;
    }
    let stage = live.stage.take().unwrap();
    // Cancel pre-edit IO/decode completions before changing metadata. Future reads
    // consult this immutable overlay, including when the camera revisits a region.
    stream.pending.clear();
    stream.decodes.clear();
    stream.composites.decodes.clear();
    if stage.appearance_changed {
        stream.composites.detail = None;
    }
    for (key, handle) in stage.materials {
        if let Some(old) = stream.composites.resident.insert(key, handle) {
            let mut t = tracker.0.lock().unwrap();
            t.material_wanted.remove(&old.id());
            t.material_ready.remove(&old.id());
        }
    }
    for handle in stream.composites.resident.values() {
        if let Some(mut m) = materials.get_mut(handle) {
            if stage.appearance_changed {
                m.clear_streamed_inputs();
            }
            m.set_origin(origin.cell(), size);
        }
    }
    for (patch, mesh) in stage.meshes {
        let entity = stream.active[&patch];
        commands
            .entity(entity)
            .insert(Mesh3d(mesh.handle.clone()))
            .remove::<bevy::camera::primitives::Aabb>();
        if let Some(old) = stream.meshes.insert(patch, mesh) {
            meshes.remove(old.handle.id());
            let mut t = tracker.0.lock().unwrap();
            t.wanted.remove(&old.handle.id());
            t.ready.remove(&old.handle.id());
        }
    }
    for (&patch, &entity) in &stream.active {
        commands
            .entity(entity)
            .insert(MeshMaterial3d(stream.patch_material(patch.0)));
    }
    for (key, node) in &request.products.nodes {
        if let Some(d) = stream
            .composites
            .descriptors
            .get_mut(&TerrainMaterialKey(*key))
        {
            d.height_bounds = node.height_bounds;
        }
        if stream.metadata.contains_key(key) {
            let d = descriptor(node);
            stream.metadata.insert(
                *key,
                PatchMetadata {
                    key: *key,
                    resolution: d.resolution.unwrap(),
                    height_bounds: d.height_bounds,
                    geometric_error: d.geometric_error,
                },
            );
            stream.descriptors.insert(*key, d);
        }
        if stream.nodes.contains_key(key) {
            stream.nodes.insert(*key, (**node).clone());
        }
    }
    stream.overlay = Some(request.products);
    if stage.appearance_changed {
        stream.overlay_revision = request.revision;
    }
    stream.metadata_revision = stream.metadata_revision.wrapping_add(1);
    stream.last_plan = None;
    live.applied = Some(request.revision);
    live.commit = None;
}
