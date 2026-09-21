use super::*;
use crossbeam_channel::Receiver;

fn fixture() -> (
    TerrainLodStream,
    WorldDatabaseWorker,
    Receiver<DatabaseRequest>,
) {
    let key = TerrainNodeKey::leaf(WorldSpaceId(1), CellCoord { x: -2, z: 1 });
    let (worker, receiver, _) = WorldDatabaseWorker::test_channel_pair(8, 8);
    (
        TerrainLodStream {
            identity: Some(("test-generation".into(), key.space)),
            roots: Some(vec![key]),
            ..default()
        },
        worker,
        receiver,
    )
}

#[test]
fn composite_admission_precedes_payload_io_and_shares_the_entry_budget() {
    let (mut stream, worker, requests) = fixture();
    stream.composites.available = Some(true);
    stream.prepare_materials(&worker, TerrainComposite::gpu_bytes() as u64 - 1);
    assert!(stream.error.as_ref().unwrap().contains("budget"));
    assert!(requests.is_empty());
    assert_eq!(stream.composites.bytes, 0);
    stream.error = None;
    stream.prepare_materials(&worker, MAX_MATERIAL_BYTES);
    let DatabaseRequest::Terrain {
        request_id,
        query: TerrainQuery::Material(Query::Descriptors(_keys)),
        ..
    } = requests.try_recv().unwrap()
    else {
        panic!()
    };
    assert_eq!(
        stream.composites.bytes,
        TerrainComposite::gpu_bytes() as u64
    );
    assert!(
        requests.is_empty(),
        "no payload before its admission descriptor"
    );
    stream.receive(request_id, Err("missing declared terrain composite".into()));
    assert!(stream.error.is_some());
    assert!(!stream.materials_ready(&UploadTracker::default()));
}

#[test]
fn declared_absence_is_distinct_from_missing_tiles_and_stale_replies() {
    let (mut stream, worker, requests) = fixture();
    stream.prepare_materials(&worker, MAX_MATERIAL_BYTES);
    let DatabaseRequest::Terrain { request_id, .. } = requests.try_recv().unwrap() else {
        panic!()
    };
    stream.receive(
        request_id + 10,
        Ok(TerrainReply::Material(Reply::Presence(true))),
    );
    assert_eq!(stream.composites.available, None);
    stream.receive(
        request_id,
        Ok(TerrainReply::Material(Reply::Presence(false))),
    );
    stream.prepare_materials(&worker, MAX_MATERIAL_BYTES);
    assert!(requests.is_empty());
    assert!(stream.materials_ready(&UploadTracker::default()));
    assert_eq!(stream.composites.bytes, 0);
}

#[test]
fn composite_cover_requires_every_prepared_binding_and_keeps_mesh_lod_independent() {
    let (mut stream, _, _) = fixture();
    let root = TerrainNodeKey {
        level: 2,
        x: -1,
        z: 0,
        space: WorldSpaceId(1),
    };
    stream.roots = Some(vec![root]);
    stream.composites.available = Some(true);
    let mut materials = Assets::<TerrainCompositeMaterial>::default();
    let material = materials.add(TerrainCompositeMaterial::default());
    stream.material = Some(materials.add(TerrainCompositeMaterial::default()));
    stream
        .composites
        .resident
        .insert(TerrainMaterialKey(root), material.clone());
    let tracker = UploadTracker::default();
    assert!(!stream.materials_ready(&tracker));
    tracker
        .0
        .lock()
        .unwrap()
        .material_ready
        .insert(material.id());
    assert!(stream.materials_ready(&tracker));
    for child in root.children().unwrap().unwrap() {
        assert_eq!(stream.patch_material(child), material);
        for leaf in child.children().unwrap().unwrap() {
            assert_eq!(stream.patch_material(leaf), material);
        }
    }
    stream.composites.clear(&tracker);
    assert!(tracker.0.lock().unwrap().material_ready.is_empty());
    assert!(!stream.materials_ready(&tracker));
}

#[test]
fn texture_demand_refines_flat_geometry_and_respects_altitude_and_capacity() {
    let root = TerrainMaterialKey(TerrainNodeKey {
        space: WorldSpaceId(1),
        x: -1,
        z: 0,
        level: 3,
    });
    let mut descriptors = BTreeMap::new();
    let mut pending = vec![root];
    while let Some(key) = pending.pop() {
        if let Some(children) = key.0.children().unwrap() {
            pending.extend(children.map(TerrainMaterialKey));
        }
        descriptors.insert(
            key,
            TerrainCompositeDescriptor {
                key,
                fingerprint: [0; 32],
                checksum: [0; 32],
                encoded_bytes: 1,
                decoded_bytes: 1,
                gpu_bytes: TerrainComposite::gpu_bytes() as u64,
                height_bounds: [0., 0.],
            },
        );
    }
    let view = |height| {
        let eye = DVec3::new(-64., height, 64.);
        LodView {
            clip_from_world: DMat4::perspective_rh(1., 1., 0.1, 10000.)
                * DMat4::look_at_rh(eye, DVec3::new(-64., 0., 64.), DVec3::Z),
            viewport: [800, 800],
            contact_position: eye,
        }
    };
    let low = selection::plan(
        &[root.0],
        &descriptors,
        &BTreeSet::new(),
        &view(40.),
        32.,
        128,
    );
    let high = selection::plan(
        &[root.0],
        &descriptors,
        &BTreeSet::new(),
        &view(8000.),
        32.,
        128,
    );
    assert!(low.keys.iter().any(|k| k.0.level == 0));
    assert!(
        high.keys.is_empty(),
        "altitude must not load detailed valley materials"
    );
    let limited = selection::plan(
        &[root.0],
        &descriptors,
        &BTreeSet::new(),
        &view(40.),
        32.,
        4,
    );
    assert!(limited.limited && limited.keys.len() <= 4);
    for key in &low.keys {
        assert!(key.0.x < 0);
        let parent = TerrainMaterialKey(key.0.parent().unwrap().unwrap());
        assert!(
            parent == root || low.keys.contains(&parent),
            "every detail tile keeps its ancestry"
        );
    }
    descriptors.retain(|k, _| *k == root);
    let loading = selection::plan(
        &[root.0],
        &descriptors,
        &BTreeSet::new(),
        &view(40.),
        32.,
        128,
    );
    assert!(loading.keys.is_empty());
    assert_eq!(loading.metadata.len(), 4);
}
