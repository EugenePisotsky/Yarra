//! Stage a balanced coarse cover without changing the active world or its camera.
//! Entry is an atomic cover handoff; ordinary refinement resumes afterwards.
use super::*;
use bevy::{
    asset::RenderAssetUsages, camera::visibility::NoFrustumCulling, mesh::Indices,
    render::render_resource::PrimitiveTopology,
};

const PROBE_BYTES: u64 = 3 * 24 + 3 * 4;

#[derive(Resource, Default)]
pub(in crate::world_streaming) struct TerrainEntry {
    request: Option<(String, WorldSpaceTransition)>,
    stream: TerrainLodStream,
    probe: Option<(Entity, Handle<Mesh>)>,
    ready: bool,
}

impl TerrainEntry {
    #[cfg(test)]
    pub(super) fn test_stream(&self) -> &TerrainLodStream {
        &self.stream
    }
    #[cfg(test)]
    pub(super) fn test_fail(&mut self, error: &str) {
        self.stream.error = Some(error.into());
    }
    pub(super) fn pending(&self) -> bool {
        self.request.is_some()
    }
    pub(in crate::world_streaming) fn ready_for(
        &self,
        generation: &str,
        transition: WorldSpaceTransition,
    ) -> bool {
        self.ready
            && self
                .request
                .as_ref()
                .is_some_and(|(g, t)| g == generation && *t == transition)
    }
    pub(in crate::world_streaming) fn owns_request(&self, id: u64) -> bool {
        self.stream.pending.contains_key(&id)
    }
    pub(in crate::world_streaming) fn receive(
        &mut self,
        id: u64,
        result: Result<TerrainReply, String>,
    ) {
        self.stream.receive(id, result);
    }
    fn release_probe(
        &mut self,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        tracker: &UploadTracker,
    ) {
        if let Some((entity, mesh)) = self.probe.take() {
            commands.entity(entity).despawn();
            meshes.remove(mesh.id());
            let mut uploads = tracker.0.lock().unwrap();
            uploads.entry_probe = None;
            uploads.entry_pipelines_ready = false;
        }
    }
    pub(in crate::world_streaming) fn clear(
        &mut self,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        tracker: &UploadTracker,
    ) {
        self.release_probe(commands, meshes, tracker);
        self.stream.clear(commands, meshes, tracker);
        self.request = None;
        self.ready = false;
    }
    pub(in crate::world_streaming) fn commit(
        &mut self,
        active: &mut TerrainLodStream,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        tracker: &UploadTracker,
        origin: CellCoord,
        cell_size: f32,
    ) {
        assert!(self.ready);
        active.clear(commands, meshes, tracker);
        self.release_probe(commands, meshes, tracker);
        let mut next = std::mem::take(&mut self.stream);
        let cover = next.target.take().unwrap();
        for (key, edges) in cover.patches {
            let patch = (key, edges);
            let transform = patch_transform(key, origin, cell_size);
            let entity = commands
                .spawn((
                    Mesh3d(next.meshes[&patch].handle.clone()),
                    MeshMaterial3d(next.patch_material(key)),
                    transform,
                    GlobalTransform::from(transform),
                    Name::new("Terrain world-entry cover"),
                ))
                .id();
            next.active.insert(patch, entity);
        }
        *active = next;
        self.request = None;
        self.ready = false;
    }
}

pub(in crate::world_streaming) fn prepare(
    mut commands: Commands,
    config: Res<TerrainLodPreview>,
    catalog: Res<WorldCatalog>,
    mut active_space: ResMut<ActiveWorldSpace>,
    worker: Option<Res<WorldDatabaseWorker>>,
    mut entry: ResMut<TerrainEntry>,
    mut active: ResMut<TerrainLodStream>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TerrainCompositeMaterial>>,
    mut images: ResMut<Assets<Image>>,
    origin: Res<WorldOrigin>,
    tracker: Res<UploadTracker>,
    mut stats: ResMut<TerrainLodStats>,
    mut reload: ResMut<WorldGenerationReload>,
) {
    let publication = reload.active();
    let request = if publication {
        reload
            .candidate
            .as_ref()
            .filter(|_| reload.hierarchy && reload.failure.is_none())
            .map(|m| {
                (
                    m.generation_id.clone(),
                    WorldSpaceTransition {
                        space: active_space.current.unwrap_or(m.default_world_space),
                        local_position: [0.; 3],
                    },
                )
            })
    } else {
        active_space
            .requested
            .filter(|t| config.enabled && Some(t.space) != active_space.current)
            .map(|t| (catalog.generation_id().to_owned(), t))
    };
    if entry.request != request {
        entry.clear(&mut commands, &mut meshes, &tracker);
        entry.request = request;
        entry.stream.identity = entry.request.as_ref().map(|(g, t)| (g.clone(), t.space));
        entry.stream.material = active.material.clone();
    }
    stats.entry_material_reserved_bytes = entry.stream.composites.bytes;
    stats.entry_decoded_bytes = entry.stream.decoded_bytes();
    stats.entry_mesh_bytes = entry.stream.mesh_bytes()
        + if entry.probe.is_some() {
            PROBE_BYTES
        } else {
            0
        };
    let Some((_, transition)) = entry.request.clone() else {
        stats.entry_status = active_space.transition_error.clone();
        return;
    };
    stats.entry_status = Some(
        if publication {
            "preparing published terrain; current generation retained"
        } else {
            "uploading destination coarse terrain; current world retained"
        }
        .into(),
    );
    let fail = if !transition.local_position.iter().all(|v| v.is_finite()) {
        Some("destination coordinates must be finite".to_string())
    } else if catalog.world_space(transition.space).is_none() {
        Some("destination world is missing".into())
    } else {
        entry.stream.error.clone()
    };
    if let Some(error) = fail {
        if publication {
            reload.failure = Some(error.clone());
        } else {
            active_space.requested = None;
            active_space.transition_error = Some(error.clone());
        }
        stats.entry_status = Some(format!(
            "{} rejected; current world retained: {error}",
            if publication {
                "publication reload"
            } else {
                "world entry"
            }
        ));
        entry.clear(&mut commands, &mut meshes, &tracker);
        return;
    }
    let Some(worker) = worker else {
        return;
    };
    let size = catalog.world_space(transition.space).unwrap().cell_size;
    // Drain work already admitted by the active cover before admitting more. This
    // keeps the same four IO/decode and two mesh-job limits across both streams.
    active.poll_work(&mut meshes, &tracker);
    let active_size = active
        .identity
        .as_ref()
        .and_then(|(_, s)| catalog.world_space(*s))
        .map_or(size, |s| s.cell_size);
    active.poll_materials(
        &mut materials,
        &mut images,
        &tracker,
        origin.cell(),
        active_size,
    );
    if active.error.is_some() && active.transition.as_ref().is_some_and(|t| !t.running()) {
        active
            .transition
            .take()
            .unwrap()
            .clear(&mut commands, &mut meshes, &tracker);
        active.target = None;
    }
    if !active.pending.is_empty()
        || !active.composites.decodes.is_empty()
        || !active.decodes.is_empty()
        || !active.builds.is_empty()
    {
        return;
    }
    // Let an already-admitted morph finish preparation before freezing its pose;
    // otherwise its asynchronous mesh jobs would overlap the entry's two jobs.
    if active.transition.as_ref().is_some_and(|t| !t.running()) {
        return;
    }
    let stage = &mut entry.stream;
    stage.next_id = active.next_id;
    stage.poll_work(&mut meshes, &tracker);
    stage.poll_materials(&mut materials, &mut images, &tracker, origin.cell(), size);
    stage.prepare_materials(
        &worker,
        material::MAX_MATERIAL_BYTES.saturating_sub(active.composites.bytes),
    );
    if stage.roots.is_none() && stage.pending.is_empty() {
        stage.request(&worker, TerrainQuery::Roots(transition.space));
    }
    if stage.target.is_none()
        && let Some(roots) = &stage.roots
    {
        let settings = LodSettings {
            refine_pixels: f64::MAX,
            collapse_pixels: f64::MAX / 2.,
            exact_radius: 0.,
            contact_radius: 0.,
            ..config.settings.clone()
        };
        let view = LodView {
            clip_from_world: DMat4::IDENTITY,
            viewport: [1, 1],
            contact_position: DVec3::ZERO,
        };
        match lod::plan_cover(
            roots,
            &stage.metadata,
            &BTreeSet::new(),
            &view,
            size as f64,
            &settings,
        ) {
            Ok(plan) if plan.balanced => stage.target = Some(plan),
            Ok(plan) if !plan.requests.is_empty() => {
                if !stage
                    .pending
                    .values()
                    .any(|q| matches!(q, TerrainQuery::Metadata(_)))
                {
                    stage.request(
                        &worker,
                        TerrainQuery::Metadata(plan.requests.into_iter().collect()),
                    );
                }
            }
            Ok(_) => {
                stage.error =
                    Some("destination coarse cover cannot fit the balancing budget".into())
            }
            Err(error) => stage.error = Some(error),
        }
    }
    stage.prepare_target(
        &worker,
        size,
        MAX_NODE_BYTES.saturating_sub(active.decoded_bytes()),
        MAX_MESH_BYTES.saturating_sub(active.mesh_bytes() + PROBE_BYTES),
    );
    active.next_id = stage.next_id;
    if stage.target.as_ref().is_some_and(|p| p.patches.is_empty()) {
        entry.ready = entry.stream.materials_ready(&tracker); // Declared empty worlds still validate material presence.
        return;
    }
    if entry.probe.is_none() && entry.stream.target.is_some() && entry.stream.error.is_none() {
        if active.mesh_bytes() + entry.stream.mesh_bytes() + PROBE_BYTES > MAX_MESH_BYTES {
            entry.stream.error =
                Some("destination pipeline preparation exceeds mesh budget".into());
            return;
        }
        if entry.stream.material.is_none() {
            entry.stream.material = Some(materials.add(TerrainCompositeMaterial::default()));
        }
        let mesh = meshes.add(
            Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.; 3]; 3])
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0., 1., 0.]; 3])
            .with_inserted_indices(Indices::U32(vec![0, 1, 2])),
        );
        let entity = commands
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(entry.stream.material.as_ref().unwrap().clone()),
                Transform::default(),
                NoFrustumCulling,
                Name::new("Terrain entry pipeline preparation"),
            ))
            .id();
        tracker.0.lock().unwrap().entry_probe = Some(entity);
        entry.probe = Some((entity, mesh));
    }
    entry.ready = entry.stream.target_uploaded(&tracker)
        && entry.stream.materials_ready(&tracker)
        && tracker.0.lock().unwrap().entry_pipelines_ready;
}
