//! Recently drawn GPU meshes stay reusable without keeping off-screen entities alive.
use super::*;

impl TerrainLodStream {
    fn mesh_is_required(&self, patch: &Patch) -> bool {
        self.active.contains_key(patch)
            || self
                .target
                .as_ref()
                .is_some_and(|p| p.patches.get(&patch.0) == Some(&patch.1))
    }

    pub(super) fn cached_mesh_bytes(&self) -> u64 {
        self.meshes
            .iter()
            .filter(|(p, _)| !self.mesh_is_required(p))
            .map(|(_, m)| m.bytes)
            .sum()
    }

    /// Replacements, entry and morphs get their space before optional cached meshes.
    pub(super) fn trim_mesh_cache(
        &mut self,
        assets: &mut Assets<Mesh>,
        tracker: &UploadTracker,
        cache_limit: u64,
        total_limit: u64,
    ) {
        let mut cached: Vec<_> = self
            .meshes
            .iter()
            .filter(|(p, _)| !self.mesh_is_required(p))
            .map(|(&p, m)| {
                (
                    p,
                    self.mesh_last_used.get(&p).copied().unwrap_or(0),
                    m.bytes,
                )
            })
            .collect();
        let required = self.mesh_bytes() - cached.iter().map(|v| v.2).sum::<u64>();
        let mut remaining = cache_limit.min(total_limit.saturating_sub(required));
        cached.sort_by_key(|(p, age, _)| (std::cmp::Reverse(*age), p.0, p.1.0));
        let mut uploads = tracker.0.lock().unwrap();
        for (patch, _, bytes) in cached {
            if bytes <= remaining {
                remaining -= bytes;
            } else {
                let mesh = self.meshes.remove(&patch).unwrap();
                assets.remove(mesh.handle.id());
                uploads.wanted.remove(&mesh.handle.id());
                uploads.ready.remove(&mesh.handle.id());
            }
        }
        self.mesh_last_used
            .retain(|p, _| self.meshes.contains_key(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returning_mesh_is_reused_and_pressure_evicts_only_optional_meshes() {
        let mut stream = TerrainLodStream::default();
        let mut assets = Assets::<Mesh>::default();
        let tracker = UploadTracker::default();
        let patches: Vec<_> = (0..4)
            .map(|x| {
                (
                    TerrainNodeKey::leaf(WorldSpaceId(1), CellCoord { x, z: 0 }),
                    StitchEdges(0),
                )
            })
            .collect();
        let mut handles = vec![];
        let field = world::TerrainHeightfield::from_heights(3, &[0.; 9], 0., 0., 32.).unwrap();
        for (i, &patch) in patches.iter().enumerate() {
            stream
                .nodes
                .insert(patch.0, TerrainNode::leaf(patch.0, &field, 3).unwrap());
            let handle = assets.add(Mesh::new(
                bevy::render::render_resource::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::default(),
            ));
            handles.push(handle.id());
            tracker.0.lock().unwrap().ready.insert(handle.id());
            stream.meshes.insert(
                patch,
                ResidentMesh {
                    handle,
                    bytes: 100,
                    bounds: Aabb::from_min_max(Vec3::ZERO, Vec3::ONE),
                },
            );
            stream.mesh_last_used.insert(patch, i as u64);
        }
        stream.active.insert(patches[0], Entity::PLACEHOLDER);
        stream.target = Some(PlannedCover {
            patches: BTreeMap::from([patches[1]]),
            requests: vec![],
            stats: default(),
            balanced: true,
        });
        stream.trim_mesh_cache(&mut assets, &tracker, 200, 300);
        assert_eq!(stream.mesh_bytes(), 300);
        assert_eq!(stream.cached_mesh_bytes(), 100);
        assert!(!stream.meshes.contains_key(&patches[2]));
        assert!(!tracker.0.lock().unwrap().ready.contains(&handles[2]));
        // A returning target reuses the uploaded allocation and its acknowledgement.
        stream
            .target
            .as_mut()
            .unwrap()
            .patches
            .insert(patches[3].0, patches[3].1);
        assert!(stream.target_uploaded(&tracker));
        let node = stream.nodes.remove(&patches[3].0).unwrap();
        assert!(
            !stream.target_uploaded(&tracker),
            "a GPU cache hit still needs morph samples"
        );
        stream.nodes.insert(patches[3].0, node);
        assert_eq!(stream.meshes[&patches[3]].handle.id(), handles[3]);
        // Optional residency cannot evict a drawn or staged endpoint, even under an
        // impossible limit. Its caller must reject the replacement in that case.
        stream.trim_mesh_cache(&mut assets, &tracker, 0, 0);
        assert_eq!(stream.meshes.len(), 3);
        stream.target = None;
        stream.trim_mesh_cache(&mut assets, &tracker, 0, 100);
        assert_eq!(stream.meshes.len(), 1);
        assert!(stream.meshes.contains_key(&patches[0]));
    }
}
