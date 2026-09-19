use super::*;
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::NoFrustumCulling,
    mesh::{
        Indices,
        morph::{MeshMorphWeights, MorphAttributes, MorphWeights},
    },
    render::render_resource::PrimitiveTopology,
};
use lod::{MorphMesh, MorphSource};

const MORPH_SECONDS: f32 = 0.25;
enum Progress {
    Pending,
    Publish,
    Replan,
}
const PROBE_BYTES: u64 = 3 * 72 + 3 * 4;

struct MorphJob {
    key: TerrainNodeKey,
    input_bytes: u64,
    task: Task<Result<MorphMesh, String>>,
}
struct MorphResident {
    handle: Handle<Mesh>,
    bounds: [Vec3; 2],
    certificate: lod::contact::ContactCertificate,
}
pub(super) struct Transition {
    old: BTreeMap<TerrainNodeKey, StitchEdges>,
    new: BTreeMap<TerrainNodeKey, StitchEdges>,
    keys: Vec<TerrainNodeKey>,
    jobs: Vec<MorphJob>,
    meshes: BTreeMap<TerrainNodeKey, MorphResident>,
    pub entities: BTreeMap<TerrainNodeKey, Entity>,
    controller: Entity,
    probe: Entity,
    probe_mesh: Handle<Mesh>,
    elapsed: f32,
    pub weight: f32,
    pub bytes: u64,
    pub patches: usize,
    pub triangles: usize,
}
impl Transition {
    fn new(
        stream: &TerrainLodStream,
        settings: &LodSettings,
        commands: &mut Commands,
        assets: &mut Assets<Mesh>,
        tracker: &UploadTracker,
    ) -> Result<Self, String> {
        let old: BTreeMap<_, _> = stream.active.keys().copied().collect();
        let new = stream.target.as_ref().unwrap().patches.clone();
        let common = lod::common_cover(&old, &new)?;
        let triangles = common
            .iter()
            .map(|k| 2 * usize::from(stream.metadata[k].resolution - 1).pow(2))
            .sum::<usize>();
        if common.len() > settings.max_patches.saturating_mul(2)
            || triangles > settings.max_triangles.saturating_mul(2)
        {
            return Err("terrain morph exceeds transient cover budget".into());
        }
        let keys: Vec<_> = common
            .iter()
            .filter(|k| old.get(k) != new.get(k))
            .copied()
            .collect();
        let bytes = PROBE_BYTES
            + keys
                .iter()
                .map(|k| MorphMesh::bytes_estimate(stream.metadata[k].resolution))
                .sum::<u64>();
        if stream.mesh_bytes() + bytes > MAX_MESH_BYTES {
            return Err("terrain morph exceeds geometry budget; old cover retained".into());
        }
        let controller = commands
            .spawn(MorphWeights::new(vec![0.], None).unwrap())
            .id();
        // Zero-area geometry exercises the exact material, morph and prepass/shadow
        // variants without covering a pixel. NoFrustumCulling permits specialization
        // before replacing the visible terrain, including asynchronous compilation.
        let probe_mesh = assets.add(
            Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.; 3]; 3])
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0., 1., 0.]; 3])
            .with_inserted_indices(Indices::U32(vec![0, 1, 2]))
            .with_morph_targets(vec![MorphAttributes::default(); 3]),
        );
        let probe = commands
            .spawn((
                Mesh3d(probe_mesh.clone()),
                MeshMaterial3d(stream.material.as_ref().unwrap().clone()),
                MeshMorphWeights::Reference(controller),
                Transform::default(),
                NoFrustumCulling,
                Name::new("Terrain morph pipeline preparation"),
            ))
            .id();
        let mut upload = tracker.0.lock().unwrap();
        upload.probe = Some(probe);
        upload.pipelines_ready = false;
        Ok(Self {
            old,
            new,
            keys,
            jobs: Vec::new(),
            meshes: BTreeMap::new(),
            entities: BTreeMap::new(),
            controller,
            probe,
            probe_mesh,
            elapsed: 0.,
            weight: 0.,
            bytes,
            patches: common.len(),
            triangles,
        })
    }
    #[cfg(test)]
    pub fn set_fraction(&mut self, fraction: f32) {
        self.elapsed = MORPH_SECONDS * fraction;
    }
    pub fn running(&self) -> bool {
        !self.entities.is_empty()
    }
    pub fn keeps(&self, patch: &Patch) -> bool {
        self.new.get(&patch.0) == Some(&patch.1)
    }
    pub fn input_bytes(&self) -> u64 {
        self.jobs.iter().map(|j| j.input_bytes).sum()
    }
    pub fn pending(&self) -> usize {
        self.keys.len() - self.meshes.len()
    }
    pub fn clear(
        self,
        commands: &mut Commands,
        assets: &mut Assets<Mesh>,
        tracker: &UploadTracker,
    ) {
        for entity in self.entities.values() {
            commands.entity(*entity).despawn();
        }
        commands.entity(self.probe).despawn();
        commands.entity(self.controller).despawn();
        assets.remove(self.probe_mesh.id());
        let mut upload = tracker.0.lock().unwrap();
        upload.probe = None;
        upload.pipelines_ready = false;
        for mesh in self.meshes.values() {
            assets.remove(mesh.handle.id());
            upload.wanted.remove(&mesh.handle.id());
            upload.ready.remove(&mesh.handle.id());
        }
    }
    pub fn certificates(&self) -> impl Iterator<Item = &lod::contact::ContactCertificate> {
        self.meshes.values().map(|m| &m.certificate)
    }
    fn contact_safe(&self, contacts: &[lod::contact::ContactRegion]) -> bool {
        contacts
            .iter()
            .all(|r| self.certificates().all(|c| r.accepts(c)))
    }
    fn update(
        &mut self,
        stream: &TerrainLodStream,
        commands: &mut Commands,
        assets: &mut Assets<Mesh>,
        tracker: &UploadTracker,
        cell_size: f32,
        origin: CellCoord,
        visible: bool,
        delta: f32,
        contacts: &[lod::contact::ContactRegion],
        handoffs: &mut u64,
    ) -> Result<Progress, String> {
        match contact_handoff(
            &self.old,
            &self.new,
            &stream.metadata,
            cell_size as f64,
            contacts,
            stream
                .target
                .as_ref()
                .is_some_and(|p| p.stats.budget_limited),
        ) {
            Some(false) => return Ok(Progress::Replan),
            // The final cover is uploaded. Contact changes during staging/morphing
            // need the same atomic replacement and readiness gates as initial entry.
            Some(true) => {
                *handoffs += 1;
                return Ok(Progress::Publish);
            }
            None => {}
        }
        if self.running() {
            // A moving/teleported consumer can enter a previously distant morph.
            // Its final cover is already uploaded: finish atomically, then replan.
            // Readiness remains closed wherever the final cover still lacks detail.
            if !self.contact_safe(contacts) {
                *handoffs += 1;
                return Ok(Progress::Publish);
            }
            // Keep the exact endpoint for one extraction before removing the transient
            // meshes. A long frame cannot skip the entire animation in one update.
            if self.weight == 1. {
                return Ok(Progress::Publish);
            }
            if visible {
                self.elapsed += delta.clamp(0., 1. / 30.);
            }
            let t = (self.elapsed / MORPH_SECONDS).min(1.);
            self.weight = t * t * (3. - 2. * t);
            commands
                .entity(self.controller)
                .insert(MorphWeights::new(vec![self.weight], None).unwrap());
            return Ok(Progress::Pending);
        }
        let mut i = 0;
        while i < self.jobs.len() {
            if let Some(result) = check_ready(&mut self.jobs[i].task) {
                let job = self.jobs.remove(i);
                let result = result?;
                let min = job.key.cell_bounds().unwrap()[0];
                let shift = DVec3::new(
                    min.x as f64 * cell_size as f64,
                    0.,
                    min.z as f64 * cell_size as f64,
                );
                let bounds = result.bounds.map(|p| p.as_dvec3() + shift);
                let certificate = lod::contact::morph_certificate(
                    bounds,
                    self.old.keys().copied(),
                    &stream.metadata,
                    cell_size as f64,
                );
                let handle = assets.add(result.mesh);
                tracker.0.lock().unwrap().wanted.insert(handle.id());
                self.meshes.insert(
                    job.key,
                    MorphResident {
                        handle,
                        bounds: result.bounds,
                        certificate,
                    },
                );
            } else {
                i += 1;
            }
        }
        for &key in &self.keys {
            if self.jobs.len() >= MAX_BUILDS {
                break;
            }
            if self.meshes.contains_key(&key) || self.jobs.iter().any(|j| j.key == key) {
                continue;
            }
            let sources = |cover: &BTreeMap<_, _>| -> Result<Vec<MorphSource>, String> {
                cover
                    .iter()
                    .filter(|(k, _)| lod::touches(key, **k))
                    .map(|(&key, &edges)| {
                        let field = stream
                            .nodes
                            .get(&key)
                            .and_then(|n| n.heightfield.clone())
                            .ok_or("missing pinned morph samples")?;
                        Ok(MorphSource { key, edges, field })
                    })
                    .collect()
            };
            let input_bytes: u64 = [&self.old, &self.new]
                .into_iter()
                .flat_map(|c| c.keys())
                .filter(|&&k| lod::touches(key, k))
                .map(|k| stream.descriptors[k].decoded_bytes)
                .sum();
            if stream.decoded_bytes() + self.input_bytes() + input_bytes > MAX_NODE_BYTES {
                return Err("terrain morph exceeds sample budget; old cover retained".into());
            }
            let (old, new) = (sources(&self.old)?, sources(&self.new)?);
            self.jobs.push(MorphJob {
                key,
                input_bytes,
                task: AsyncComputeTaskPool::get()
                    .spawn(async move { lod::build_morph_mesh(key, cell_size, &old, &new) }),
            });
        }
        let upload = tracker.0.lock().unwrap();
        let ready = self.meshes.len() == self.keys.len()
            && upload.pipelines_ready
            && self
                .meshes
                .values()
                .all(|m| upload.ready.contains(&m.handle.id()));
        drop(upload);
        if !ready || !visible {
            return Ok(Progress::Pending);
        }
        // Never animate an uncertified intermediate surface beneath a consumer.
        // The bounded interval certificate may be pessimistic; an atomic switch
        // between uploaded endpoints is preferable to floating grass or feet.
        if !self.contact_safe(contacts) {
            *handoffs += 1;
            return Ok(Progress::Publish);
        }
        for (&patch, &entity) in &stream.active {
            if !self.keeps(&patch) {
                commands.entity(entity).insert(Visibility::Hidden);
            }
        }
        for (&key, mesh) in &self.meshes {
            let entity = commands
                .spawn((
                    Mesh3d(mesh.handle.clone()),
                    MeshMaterial3d(stream.patch_material(key)),
                    MeshMorphWeights::Reference(self.controller),
                    patch_transform(key, origin, cell_size),
                    GlobalTransform::from(patch_transform(key, origin, cell_size)),
                    Aabb::from_min_max(mesh.bounds[0], mesh.bounds[1]),
                    Name::new(format!("Terrain morph {} ({},{})", key.level, key.x, key.z)),
                ))
                .id();
            self.entities.insert(key, entity);
        }
        Ok(Progress::Pending)
    }
}
impl TerrainLodStream {
    pub(super) fn advance_transition(
        &mut self,
        settings: &LodSettings,
        commands: &mut Commands,
        assets: &mut Assets<Mesh>,
        tracker: &UploadTracker,
        cell_size: f32,
        origin: CellCoord,
        visible: bool,
        delta: f32,
        contacts: &[lod::contact::ContactRegion],
        handoffs: &mut u64,
    ) -> bool {
        // The final cover is already uploaded when this function is called. A
        // contact refinement from uncertified ground will require an atomic
        // handoff anyway. Do not spend frames building hundreds of transient
        // meshes which cannot be shown beneath those roots/feet.
        if self.transition.is_none() {
            let old: BTreeMap<_, _> = self.active.keys().copied().collect();
            let plan = self.target.as_ref().unwrap();
            match contact_handoff(
                &old,
                &plan.patches,
                &self.metadata,
                cell_size as f64,
                contacts,
                plan.stats.budget_limited,
            ) {
                Some(false) => {
                    self.target = None;
                    self.last_plan = None;
                    self.evict(assets, tracker);
                    return false;
                }
                Some(true) => {
                    *handoffs += 1;
                    return true;
                }
                None => {}
            }
        }
        let mut transition = match self.transition.take() {
            Some(t) => t,
            None => match Transition::new(self, settings, commands, assets, tracker) {
                Ok(t) => t,
                Err(e) => {
                    self.error = Some(e);
                    return false;
                }
            },
        };
        match transition.update(
            self, commands, assets, tracker, cell_size, origin, visible, delta, contacts, handoffs,
        ) {
            Ok(Progress::Publish) => {
                transition.clear(commands, assets, tracker);
                true
            }
            Ok(Progress::Replan) => {
                for &entity in self.active.values() {
                    commands.entity(entity).insert(if visible {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    });
                }
                transition.clear(commands, assets, tracker);
                self.target = None;
                self.last_plan = None;
                self.evict(assets, tracker);
                false
            }
            result => {
                if let Err(e) = result {
                    self.error = Some(e);
                }
                self.transition = Some(transition);
                false
            }
        }
    }
}

/// None permits a morph, true chooses an atomic contact handoff, false replans
/// an obsolete target which would remove an already safe contact surface.
fn contact_handoff(
    old: &BTreeMap<TerrainNodeKey, StitchEdges>,
    new: &BTreeMap<TerrainNodeKey, StitchEdges>,
    metadata: &BTreeMap<TerrainNodeKey, PatchMetadata>,
    size: f64,
    contacts: &[lod::contact::ContactRegion],
    budget_limited: bool,
) -> Option<bool> {
    let mut atomic = false;
    for region in contacts {
        let old_safe = lod::contact::cover_accepts(region, old, metadata, size);
        let new_safe = lod::contact::cover_accepts(region, new, metadata, size);
        if old_safe && !new_safe {
            if region.priority == lod::contact::ContactPriority::Actor || !budget_limited {
                return Some(false);
            }
            // A budget-limited plan can trade lower-priority vegetation for actor
            // ground. Publish atomically; ContactSystems::Publish then hides any
            // grass whose new surface isn't certified, before render extraction.
            // Without this, a previously safe grass page can veto every new plan.
            atomic = true;
        }
        if !old_safe || !new_safe {
            atomic |= old
                .keys()
                .chain(new.keys())
                .any(|k| old.get(k) != new.get(k) && region.intersects(metadata[k].bounds(size)));
        }
    }
    atomic.then_some(true)
}
pub(super) fn patch_transform(key: TerrainNodeKey, origin: CellCoord, cell_size: f32) -> Transform {
    let min = key.cell_bounds().unwrap()[0];
    Transform::from_xyz(
        ((min.x as i64 - origin.x as i64) as f64 * cell_size as f64) as f32,
        0.,
        ((min.z as i64 - origin.z as i64) as f64 * cell_size as f64) as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejected_transition_keeps_cover_and_does_not_allocate() {
        let key = TerrainNodeKey::leaf(WorldSpaceId(1), CellCoord { x: 0, z: 0 });
        let patch = (key, StitchEdges(0));
        let mut stream = TerrainLodStream::default();
        stream.active.insert(patch, Entity::PLACEHOLDER);
        stream.meshes.insert(
            patch,
            ResidentMesh {
                handle: Handle::default(),
                bytes: MAX_MESH_BYTES,
            },
        );
        stream.metadata.insert(
            key,
            PatchMetadata {
                key,
                height_bounds: [0., 1.],
                geometric_error: 0.,
                resolution: 5,
            },
        );
        stream.target = Some(PlannedCover {
            patches: BTreeMap::from([(key, StitchEdges(1))]),
            requests: BTreeSet::new(),
            stats: default(),
            balanced: true,
        });
        let mut world = World::new();
        let initial_entities = world.entities().len();
        let mut assets = Assets::<Mesh>::default();
        let tracker = UploadTracker::default();
        for settings in [
            LodSettings::default(),
            LodSettings {
                max_patches: 0,
                ..default()
            },
        ] {
            let mut queue = bevy::ecs::world::CommandQueue::default();
            let mut commands = Commands::new(&mut queue, &world);
            assert!(
                Transition::new(&stream, &settings, &mut commands, &mut assets, &tracker).is_err()
            );
            queue.apply(&mut world);
            assert_eq!(stream.active[&patch], Entity::PLACEHOLDER);
            assert_eq!(stream.mesh_bytes(), MAX_MESH_BYTES);
            assert!(assets.is_empty());
            assert_eq!(world.entities().len(), initial_entities);
            assert!(tracker.0.lock().unwrap().probe.is_none());
        }
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::*;
    #[test]
    fn budget_pressure_can_retire_grass_but_never_actor_ground() {
        let roots = [0, 2].map(|x| TerrainNodeKey {
            space: WorldSpaceId(1),
            level: 1,
            x,
            z: 0,
        });
        let cover = |fine: usize| {
            roots
                .iter()
                .enumerate()
                .flat_map(|(i, root)| {
                    if i == fine {
                        root.children().unwrap().unwrap().to_vec()
                    } else {
                        vec![*root]
                    }
                })
                .map(|k| (k, StitchEdges::default()))
                .collect::<BTreeMap<_, _>>()
        };
        let old = cover(0);
        let new = cover(1);
        let metadata: BTreeMap<_, _> = roots
            .into_iter()
            .flat_map(|r| [r].into_iter().chain(r.children().unwrap().unwrap()))
            .map(|key| {
                (
                    key,
                    PatchMetadata {
                        key,
                        resolution: 33,
                        height_bounds: [-1., 1.],
                        geometric_error: if key.level == 0 { 0. } else { 1. },
                    },
                )
            })
            .collect();
        let grass = lod::contact::ContactRegion {
            bounds: [DVec3::ZERO, DVec3::ONE],
            exact: true,
            tolerance: 0.,
            priority: lod::contact::ContactPriority::Vegetation,
        };
        let actor = lod::contact::ContactRegion {
            bounds: grass.bounds.map(|p| p + DVec3::X * 33.),
            priority: lod::contact::ContactPriority::Actor,
            ..grass.clone()
        };
        for contacts in [[grass.clone(), actor.clone()], [actor, grass]] {
            assert_eq!(
                contact_handoff(&old, &new, &metadata, 8., &contacts, false),
                Some(false)
            );
            assert_eq!(
                contact_handoff(&old, &new, &metadata, 8., &contacts, true),
                Some(true)
            );
            assert_eq!(
                contact_handoff(&new, &old, &metadata, 8., &contacts, true),
                Some(false)
            );
        }
    }
    #[test]
    fn contact_refinement_skips_unused_morphs_but_never_publishes_an_unsafe_coarsening() {
        let root = TerrainNodeKey {
            space: WorldSpaceId(1),
            level: 1,
            x: 0,
            z: 0,
        };
        let children = root.children().unwrap().unwrap();
        let old = BTreeMap::from([(root, StitchEdges::default())]);
        let new: BTreeMap<_, _> = children
            .into_iter()
            .map(|k| (k, StitchEdges::default()))
            .collect();
        let metadata: BTreeMap<_, _> = [root]
            .into_iter()
            .chain(children)
            .map(|key| {
                (
                    key,
                    PatchMetadata {
                        key,
                        resolution: 33,
                        height_bounds: [-1., 1.],
                        geometric_error: if key.level == 0 { 0. } else { 1. },
                    },
                )
            })
            .collect();
        let contact = lod::contact::ContactRegion {
            bounds: [DVec3::ZERO, DVec3::ONE],
            exact: true,
            tolerance: 0.,
            priority: lod::contact::ContactPriority::Actor,
        };
        assert_eq!(contact_handoff(&old, &new, &metadata, 8., &[], false), None);
        assert_eq!(
            contact_handoff(
                &old,
                &new,
                &metadata,
                8.,
                std::slice::from_ref(&contact),
                false
            ),
            Some(true)
        );
        assert_eq!(
            contact_handoff(
                &new,
                &old,
                &metadata,
                8.,
                std::slice::from_ref(&contact),
                false
            ),
            Some(false)
        );
        assert_eq!(
            contact_handoff(
                &new,
                &new,
                &metadata,
                8.,
                std::slice::from_ref(&contact),
                false
            ),
            None
        );
        let distant = lod::contact::ContactRegion {
            bounds: contact.bounds.map(|p| p + DVec3::splat(100.)),
            ..contact
        };
        assert_eq!(
            contact_handoff(&old, &new, &metadata, 8., &[distant], false),
            None
        );
    }
}
