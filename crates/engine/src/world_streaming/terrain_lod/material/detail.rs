use super::*;
use bevy::render::storage::ShaderBuffer;
use terrain_render::composite::atlas::{self, CompositeUploadHub, DETAIL_SLOTS, DetailAtlas};

struct Slot {
    layer: u32,
    fade: f32,
    last_used: u64,
}
pub(in crate::world_streaming::terrain_lod) struct Detail {
    pub atlas: DetailAtlas,
    slots: BTreeMap<TerrainMaterialKey, Slot>,
    pub cpu: BTreeMap<TerrainMaterialKey, TerrainComposite>,
    desired: BTreeSet<TerrainMaterialKey>,
    retiring: BTreeSet<TerrainMaterialKey>,
    epoch: u64,
    bound: bool,
    pub limited: bool,
    last_table: Vec<atlas::DetailEntry>,
}
impl Detail {
    fn new(
        images: &mut Assets<Image>,
        buffers: &mut Assets<ShaderBuffer>,
        hub: &CompositeUploadHub,
    ) -> Self {
        Self {
            atlas: DetailAtlas::new(images, buffers, hub),
            slots: BTreeMap::new(),
            cpu: BTreeMap::new(),
            desired: BTreeSet::new(),
            retiring: BTreeSet::new(),
            epoch: 0,
            bound: false,
            limited: false,
            last_table: vec![atlas::DetailEntry::default(); atlas::TABLE_SIZE],
        }
    }
    pub fn count(&self) -> usize {
        self.slots.len()
    }
    fn set_desired(&mut self, desired: BTreeSet<TerrainMaterialKey>, capacity: usize) {
        if self.desired == desired {
            return;
        }
        self.desired = desired;
        self.epoch += 1;
        for (key, slot) in &mut self.slots {
            if self.desired.contains(key) {
                slot.last_used = self.epoch;
            }
        }
        self.retiring.clear();
        let missing = self
            .desired
            .iter()
            .filter(|k| !self.slots.contains_key(k))
            .count();
        let retire_count = (self.slots.len() + missing).saturating_sub(capacity);
        // Keep spare tiles warm. Under pressure retire oldest leaves first, keeping
        // their ancestors available throughout the fade and the next upload.
        let mut children = BTreeMap::<TerrainMaterialKey, usize>::new();
        for key in self.slots.keys() {
            if let Some(parent) = key.0.parent().ok().flatten() {
                *children.entry(TerrainMaterialKey(parent)).or_default() += 1;
            }
        }
        let mut leaves: BTreeSet<_> = self
            .slots
            .iter()
            .filter(|(key, _)| !self.desired.contains(key) && !children.contains_key(key))
            .map(|(&key, slot)| (slot.last_used, key))
            .collect();
        while self.retiring.len() < retire_count {
            let Some((_, key)) = leaves.pop_first() else {
                break;
            };
            self.retiring.insert(key);
            if let Some(parent) = key.0.parent().ok().flatten().map(TerrainMaterialKey) {
                let count = children.get_mut(&parent).unwrap();
                *count -= 1;
                if *count == 0
                    && !self.desired.contains(&parent)
                    && let Some(slot) = self.slots.get(&parent)
                {
                    leaves.insert((slot.last_used, parent));
                }
            }
        }
    }
    fn animate(&mut self, roots: &[TerrainNodeKey], dt: f32) {
        let step = dt.clamp(0., 1. / 30.) / 0.3;
        // Ancestors must be stable before a child appears. Retiring descendants
        // disappear first, preserving a fully weighted fallback at every step.
        let keys: Vec<_> = self.slots.keys().copied().collect();
        let parents: BTreeSet<_> = keys
            .iter()
            .filter_map(|k| k.0.parent().ok().flatten().map(TerrainMaterialKey))
            .collect();
        for key in keys.iter().rev() {
            let wanted = !self.retiring.contains(key);
            let parent_ready = key.0.parent().ok().flatten().is_some_and(|p| {
                roots.contains(&p)
                    || self
                        .slots
                        .get(&TerrainMaterialKey(p))
                        .is_some_and(|s| s.fade >= 1.)
            });
            let has_child = parents.contains(key);
            let slot = self.slots.get_mut(key).unwrap();
            if wanted && parent_ready {
                slot.fade = (slot.fade + step).min(1.);
            } else if !wanted && !has_child {
                slot.fade = (slot.fade - step).max(0.);
            }
        }
        self.slots
            .retain(|k, s| s.fade > 0. || !self.retiring.contains(k));
    }
}

impl TerrainLodStream {
    pub(in crate::world_streaming::terrain_lod) fn update_material_detail(
        &mut self,
        worker: &WorldDatabaseWorker,
        view: &LodView,
        size: f32,
        images: &mut Assets<Image>,
        buffers: &mut Assets<ShaderBuffer>,
        materials: &mut Assets<TerrainCompositeMaterial>,
        hub: &CompositeUploadHub,
        dt: f32,
    ) {
        if self.active.is_empty()
            || self.composites.available != Some(true)
            || self.composites.resident.len() != self.roots.as_ref().map_or(0, Vec::len)
        {
            return;
        }
        let roots = self.roots.clone().unwrap();
        if roots.iter().all(|k| k.level == 0) {
            return;
        }
        if self.composites.detail.is_none() {
            if self.composites.bytes + atlas::DETAIL_BYTES > MAX_MATERIAL_BYTES {
                return;
            }
            self.composites.detail = Some(Detail::new(images, buffers, hub));
            self.composites.bytes += atlas::DETAIL_BYTES;
        }
        let cache = self.composites.detail.as_mut().unwrap();
        if !cache.atlas.ready() || !cache.atlas.idle() {
            return;
        }
        if !cache.bound {
            for handle in self.composites.resident.values() {
                if let Some(mut material) = materials.get_mut(handle) {
                    material.set_detail_atlas(&cache.atlas);
                }
            }
            cache.bound = true;
        }
        let plan = super::selection::plan(
            &roots,
            &self.composites.descriptors,
            &cache.desired,
            view,
            size as f64,
            DETAIL_SLOTS,
            self.composites.minimum_level,
        );
        cache.set_desired(plan.keys.iter().copied().collect(), DETAIL_SLOTS);
        cache.limited = plan.limited;
        cache.animate(&roots, dt);
        cache.cpu.retain(|k, _| cache.desired.contains(k));
        let mut uploads = vec![];
        for key in &plan.keys {
            // One atlas transaction carries at most four tiles; the rest wait a frame.
            if uploads.len() == atlas::MAX_UPLOADS {
                break;
            }
            if cache.slots.contains_key(key) {
                continue;
            }
            let Some(layer) =
                (0..DETAIL_SLOTS as u32).find(|i| cache.slots.values().all(|s| s.layer != *i))
            else {
                break;
            };
            if let Some(tile) = cache.cpu.remove(key) {
                cache.slots.insert(
                    *key,
                    Slot {
                        layer,
                        fade: 0.,
                        last_used: cache.epoch,
                    },
                );
                uploads.push((layer, tile));
            }
        }
        let table = atlas::table(
            cache
                .slots
                .iter()
                .filter(|(_, s)| s.fade > 0.)
                .map(|(&k, s)| (k, s.layer, s.fade)),
        );
        if table != cache.last_table || !uploads.is_empty() {
            cache.last_table = table.clone();
            cache.atlas.submit(table, uploads);
        }
        let queued: BTreeSet<_> = self
            .pending
            .values()
            .filter_map(|q| match q {
                TerrainQuery::Material(Query::Descriptors(keys)) => Some(keys.iter().copied()),
                _ => None,
            })
            .flatten()
            .collect();
        // Retain only local traversal, residency and in-flight metadata, including
        // candidate children. This bound is unrelated to authored world area.
        let mut keep: BTreeSet<_> = roots
            .iter()
            .copied()
            .map(TerrainMaterialKey)
            .chain(cache.slots.keys().copied())
            .chain(cache.desired.iter().copied())
            .chain(self.composites.decodes.keys().copied())
            .chain(cache.cpu.keys().copied())
            .collect();
        for key in keep.clone() {
            if let Some(children) = key.0.children().ok().flatten() {
                keep.extend(children.map(TerrainMaterialKey));
            }
        }
        keep.extend(&queued);
        keep.extend(&plan.metadata);
        for query in self.pending.values() {
            if let TerrainQuery::Material(Query::Tile(k)) = query {
                keep.insert(*k);
            }
        }
        self.composites.descriptors.retain(|k, _| keep.contains(k));
        let missing: Vec<_> = plan
            .metadata
            .into_iter()
            .filter(|k| !queued.contains(k))
            .take(
                128.min(
                    MAX_MATERIAL_METADATA
                        .saturating_sub(self.composites.descriptors.len() + queued.len()),
                ),
            )
            .collect();
        // Cached residents yield to desired requests. CPU payloads remain bounded by
        // the desired capacity while their GPU predecessors finish fading out.
        let occupied = cache
            .slots
            .keys()
            .filter(|k| cache.desired.contains(k))
            .count()
            + cache.cpu.len()
            + self.composites.decodes.len()
            + self
                .pending
                .values()
                .filter(|q| matches!(q, TerrainQuery::Material(Query::Tile(_))))
                .count();
        let requests: Vec<_> = plan
            .keys
            .into_iter()
            .filter(|k| {
                self.composites.descriptors.contains_key(k)
                    && !cache.slots.contains_key(k)
                    && !cache.cpu.contains_key(k)
                    && !self.composites.decodes.contains_key(k)
                    && !self
                        .pending
                        .values()
                        .any(|q| matches!(q, TerrainQuery::Material(Query::Tile(t)) if t == k))
            })
            .take(DETAIL_SLOTS.saturating_sub(occupied))
            .collect();
        if !missing.is_empty() {
            self.request(worker, TerrainQuery::Material(Query::Descriptors(missing)));
        }
        for key in requests {
            self.request(worker, TerrainQuery::Material(Query::Tile(key)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_back_keeps_spare_tiles_and_pressure_retires_the_oldest() {
        let root = TerrainNodeKey {
            space: WorldSpaceId(1),
            level: 1,
            x: 0,
            z: 0,
        };
        let [a, b, c, d] = root.children().unwrap().unwrap().map(TerrainMaterialKey);
        let mut cache = Detail::new(
            &mut Assets::default(),
            &mut Assets::default(),
            &CompositeUploadHub::default(),
        );
        cache.slots.insert(
            a,
            Slot {
                layer: 0,
                fade: 1.,
                last_used: 0,
            },
        );
        cache.slots.insert(
            b,
            Slot {
                layer: 1,
                fade: 1.,
                last_used: 0,
            },
        );
        cache.set_desired(BTreeSet::from([a]), 3);
        cache.set_desired(BTreeSet::from([b]), 3);
        for _ in 0..40 {
            cache.animate(&[root], 1. / 30.);
        }
        assert_eq!(
            cache.slots[&a].fade, 1.,
            "looking away alone must not discard detail"
        );
        cache.set_desired(BTreeSet::from([b, c, d]), 3);
        assert_eq!(cache.retiring, BTreeSet::from([a]));
        cache.animate(&[root], 1. / 30.);
        assert!(cache.slots[&a].fade < 1.);
        cache.set_desired(BTreeSet::from([a]), 3);
        for _ in 0..10 {
            cache.animate(&[root], 1. / 30.);
        }
        assert_eq!(
            cache.slots[&a].fade, 1.,
            "return during retirement must reuse the tile"
        );
        assert_eq!(cache.slots[&a].layer, 0);
        assert!(cache.retiring.is_empty());
    }

    #[test]
    fn parents_are_ready_before_children_fade_in_and_retire_after_them() {
        let root = TerrainNodeKey {
            space: WorldSpaceId(1),
            level: 2,
            x: -1,
            z: 0,
        };
        let parent = TerrainMaterialKey(root.children().unwrap().unwrap()[0]);
        let child = TerrainMaterialKey(parent.0.children().unwrap().unwrap()[0]);
        let mut images = Assets::default();
        let mut buffers = Assets::default();
        let mut cache = Detail::new(&mut images, &mut buffers, &CompositeUploadHub::default());
        cache.slots.insert(
            parent,
            Slot {
                layer: 0,
                fade: 0.,
                last_used: 0,
            },
        );
        cache.slots.insert(
            child,
            Slot {
                layer: 1,
                fade: 0.,
                last_used: 0,
            },
        );
        cache.desired.extend([parent, child]);
        cache.animate(&[root], 1. / 30.);
        assert!(cache.slots[&parent].fade > 0.);
        assert_eq!(cache.slots[&child].fade, 0.);
        for _ in 0..20 {
            cache.animate(&[root], 1. / 30.);
        }
        assert_eq!(cache.slots[&child].fade, 1.);
        cache.set_desired(BTreeSet::new(), 0);
        cache.animate(&[root], 1. / 30.);
        assert_eq!(cache.slots[&parent].fade, 1.);
        assert!(cache.slots[&child].fade < 1.);
        for _ in 0..30 {
            cache.animate(&[root], 1. / 30.);
        }
        assert!(cache.slots.is_empty());
    }
}
