//! Coarse fallback and bounded ground detail, selected independently of mesh LOD.
use super::*;
use terrain_render::TerrainCompositeMaterial;
use world::{TerrainComposite, TerrainMaterialKey};
use world_db::{EncodedTerrainComposite, TerrainCompositeDescriptor};

pub(super) const MAX_MATERIAL_BYTES: u64 = 32 * 1024 * 1024;
pub(super) const MAX_MATERIAL_TILES: usize = 512;
const MAX_MATERIAL_METADATA: usize = 2048;
mod detail;
mod selection;

#[derive(Clone, Debug)]
pub(in crate::world_streaming) enum Query {
    Presence(WorldSpaceId),
    Descriptors(Vec<TerrainMaterialKey>),
    Tile(TerrainMaterialKey),
}
#[derive(Debug)]
pub(in crate::world_streaming) enum Reply {
    Presence(bool),
    Descriptors(Vec<TerrainCompositeDescriptor>),
    Tile(EncodedTerrainComposite),
}
pub(super) fn read(reader: &RuntimeReader, query: Query) -> Result<Reply, String> {
    match query {
        Query::Presence(space) => reader
            .has_terrain_composites(space)
            .map(Reply::Presence)
            .map_err(|e| e.to_string()),
        Query::Descriptors(keys) => reader
            .read_terrain_composite_descriptors(&keys)
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .map(Reply::Descriptors)
            .ok_or("missing declared terrain composite".into()),
        Query::Tile(key) => reader
            .read_terrain_composite(key)
            .map_err(|e| e.to_string())?
            .map(Reply::Tile)
            .ok_or("missing declared terrain composite".into()),
    }
}

#[derive(Default)]
pub(super) struct Cover {
    pub available: Option<bool>,
    pub bytes: u64,
    pub detail: Option<detail::Detail>,
    pub(super) descriptors: BTreeMap<TerrainMaterialKey, TerrainCompositeDescriptor>,
    pub decodes: BTreeMap<TerrainMaterialKey, Task<Result<TerrainComposite, String>>>,
    pub resident: BTreeMap<TerrainMaterialKey, Handle<TerrainCompositeMaterial>>,
}
impl Cover {
    pub(super) fn metadata_len(&self) -> usize {
        self.descriptors.len()
    }
    fn receive(&mut self, query: Query, reply: Reply) -> Result<(), String> {
        match (query, reply) {
            (Query::Presence(_), Reply::Presence(present)) => self.available = Some(present),
            (Query::Descriptors(keys), Reply::Descriptors(descriptors))
                if keys.len() == descriptors.len()
                    && keys.iter().zip(&descriptors).all(|(k, d)| *k == d.key) =>
            {
                if self.descriptors.len()
                    + descriptors
                        .iter()
                        .filter(|d| !self.descriptors.contains_key(&d.key))
                        .count()
                    > MAX_MATERIAL_METADATA
                {
                    return Err("material metadata admission limit".into());
                }
                for d in descriptors {
                    if !d.height_bounds.iter().all(|v| v.is_finite())
                        || d.height_bounds[0] > d.height_bounds[1]
                        || d.gpu_bytes != TerrainComposite::gpu_bytes() as u64
                        || !(1..=world::MAX_TERRAIN_COMPOSITE_BYTES as u64)
                            .contains(&d.decoded_bytes)
                        || !(1..=world::MAX_TERRAIN_COMPOSITE_BYTES as u64)
                            .contains(&d.encoded_bytes)
                    {
                        return Err("invalid composite admission descriptor".into());
                    }
                    self.descriptors.insert(d.key, d);
                }
            }
            (Query::Tile(key), Reply::Tile(encoded))
                if self.descriptors.get(&key) == Some(&encoded.descriptor) =>
            {
                self.decodes.insert(
                    key,
                    AsyncComputeTaskPool::get()
                        .spawn(async move { encoded.decode().map_err(|e| e.to_string()) }),
                );
            }
            _ => return Err("terrain composite response identity mismatch".into()),
        }
        Ok(())
    }
    pub fn clear(&mut self, tracker: &UploadTracker) {
        let mut uploads = tracker.0.lock().unwrap();
        for material in self.resident.values() {
            uploads.material_wanted.remove(&material.id());
            uploads.material_ready.remove(&material.id());
        }
        // Strong handles in drawn entities are released at the same command boundary.
        *self = Self::default();
    }
}

impl TerrainLodStream {
    pub(super) fn receive_material(&mut self, query: Query, mut reply: Reply) {
        if let Reply::Descriptors(descriptors) = &mut reply {
            for d in descriptors {
                if let Some(node) = self.overlay.as_ref().and_then(|o| o.nodes.get(&d.key.0)) {
                    d.height_bounds = node.height_bounds;
                }
            }
        }
        if let Err(error) = self.composites.receive(query, reply) {
            self.error = Some(error);
        }
    }
    pub(super) fn poll_materials(
        &mut self,
        materials: &mut Assets<TerrainCompositeMaterial>,
        images: &mut Assets<Image>,
        tracker: &UploadTracker,
        origin: CellCoord,
        size: f32,
    ) {
        let finished: Vec<_> = self
            .composites
            .decodes
            .iter_mut()
            .filter_map(|(&key, task)| check_ready(task).map(|r| (key, r)))
            .collect();
        for (key, result) in finished {
            self.composites.decodes.remove(&key);
            if !self
                .roots
                .as_ref()
                .is_some_and(|roots| roots.contains(&key.0))
            {
                match result {
                    Ok(tile) => {
                        if let Some(cache) = &mut self.composites.detail {
                            cache.cpu.insert(key, tile);
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
                continue;
            }
            match result.and_then(|tile| {
                TerrainCompositeMaterial::from_composite(tile, origin, size, images)
            }) {
                Ok(material) => {
                    let handle = materials.add(material);
                    tracker
                        .0
                        .lock()
                        .unwrap()
                        .material_wanted
                        .insert(handle.id());
                    self.composites.resident.insert(key, handle);
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    pub(super) fn prepare_materials(&mut self, worker: &WorldDatabaseWorker, budget: u64) {
        let Some(roots) = self.roots.clone() else {
            return;
        };
        if self.composites.available.is_none() {
            if !self
                .pending
                .values()
                .any(|q| matches!(q, TerrainQuery::Material(Query::Presence(_))))
            {
                self.request(
                    worker,
                    TerrainQuery::Material(Query::Presence(self.identity.as_ref().unwrap().1)),
                );
            }
            return;
        }
        if self.composites.available == Some(false) {
            return;
        }
        let bytes = roots.len() as u64 * TerrainComposite::gpu_bytes() as u64
            + self
                .composites
                .detail
                .as_ref()
                .map_or(0, |_| terrain_render::composite::atlas::DETAIL_BYTES);
        if roots.len() > MAX_MATERIAL_TILES || bytes > budget {
            self.error = Some("coarse ground materials exceed shared residency budget".into());
            return;
        }
        // Reserve the complete cover before any payload IO, including in-flight tiles.
        self.composites.bytes = bytes;
        let queued: BTreeSet<_> = self
            .pending
            .values()
            .filter_map(|q| match q {
                TerrainQuery::Material(Query::Descriptors(keys)) => Some(keys.iter().copied()),
                _ => None,
            })
            .flatten()
            .collect();
        let missing: Vec<_> = roots
            .iter()
            .copied()
            .map(TerrainMaterialKey)
            .filter(|k| !self.composites.descriptors.contains_key(k) && !queued.contains(k))
            .take(128)
            .collect();
        if !missing.is_empty() {
            self.request(worker, TerrainQuery::Material(Query::Descriptors(missing)));
        }
        for key in roots.into_iter().map(TerrainMaterialKey) {
            if self.composites.descriptors.contains_key(&key)
                && !self.composites.resident.contains_key(&key)
                && !self.composites.decodes.contains_key(&key)
                && !self
                    .pending
                    .values()
                    .any(|q| matches!(q, TerrainQuery::Material(Query::Tile(k)) if *k == key))
            {
                self.request(worker, TerrainQuery::Material(Query::Tile(key)));
            }
        }
    }
    pub(super) fn materials_ready(&self, tracker: &UploadTracker) -> bool {
        match self.composites.available {
            Some(false) => true, // Explicit geometry diagnostic for publications without a bake.
            Some(true) => {
                let uploads = tracker.0.lock().unwrap();
                self.roots.as_ref().is_some_and(|roots| {
                    roots.iter().all(|k| {
                        self.composites
                            .resident
                            .get(&TerrainMaterialKey(*k))
                            .is_some_and(|m| uploads.material_ready.contains(&m.id()))
                    })
                })
            }
            None => false,
        }
    }
    pub(super) fn patch_material(
        &self,
        mut key: TerrainNodeKey,
    ) -> Handle<TerrainCompositeMaterial> {
        // Each patch/morph lies within one immutable material root. Changing mesh
        // resolution cannot change the sampled material or its world projection.
        loop {
            if let Some(material) = self.composites.resident.get(&TerrainMaterialKey(key)) {
                return material.clone();
            }
            match key.parent().ok().flatten() {
                Some(parent) => key = parent,
                None => break,
            }
        }
        self.material.as_ref().unwrap().clone()
    }
    pub(super) fn sync_material_origin(
        &self,
        materials: &mut Assets<TerrainCompositeMaterial>,
        origin: CellCoord,
        size: f32,
    ) {
        for handle in self.composites.resident.values() {
            if let Some(mut material) = materials.get_mut(handle) {
                material.set_origin(origin, size);
            }
        }
    }
}

#[cfg(test)]
mod tests;
