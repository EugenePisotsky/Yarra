//! Optional CPU-only probe of real cooked terrain. No recook or world mutation.
use super::*;
use bevy::camera::CameraProjection;
use lod::contact::{ContactPriority, ContactRegion, cover_accepts};
use world_db::RuntimeReader;

#[test]
#[ignore = "set YARRA_TEST_WORLD_DB and YARRA_TEST_START_VIEW to a cooked landscape and bookmark"]
fn published_landscape_contact_budget_probe() {
    let path = std::env::var_os("YARRA_TEST_WORLD_DB").expect("YARRA_TEST_WORLD_DB");
    let view_path = std::env::var_os("YARRA_TEST_START_VIEW").expect("YARRA_TEST_START_VIEW");
    let bookmark: world::WorldViewBookmark =
        ron::from_str(&std::fs::read_to_string(view_path).unwrap()).unwrap();
    bookmark.validate().unwrap();
    let reader = RuntimeReader::open_immutable(std::path::Path::new(&path)).unwrap();
    let space = reader
        .manifest()
        .world_space(reader.manifest().default_world_space)
        .unwrap();
    let size = f64::from(space.cell_size);
    let roots = reader.read_terrain_roots(space.id).unwrap();
    let keys: Vec<_> = roots.iter().map(|d| d.key).collect();
    let mut metadata = BTreeMap::new();
    let mut pending = roots;
    while !pending.is_empty() {
        let mut children = vec![];
        for descriptor in pending {
            if let Some(keys) = descriptor.key.children().unwrap() {
                children.extend(keys);
            }
            metadata.insert(
                descriptor.key,
                PatchMetadata {
                    key: descriptor.key,
                    resolution: descriptor.resolution.unwrap(),
                    height_bounds: descriptor.height_bounds,
                    geometric_error: descriptor.geometric_error,
                },
            );
        }
        pending = children
            .chunks(world_db::MAX_TERRAIN_NODE_QUERY)
            .flat_map(|batch| {
                reader
                    .read_terrain_node_descriptors(batch)
                    .unwrap()
                    .into_iter()
                    .map(Option::unwrap)
            })
            .collect();
    }
    let position = Vec3::from_array(bookmark.position);
    let actor = ContactRegion {
        bounds: [
            position.as_dvec3() - DVec3::new(0.35, 10000., 0.35),
            position.as_dvec3() + DVec3::new(0.35, 10000., 0.35),
        ],
        exact: true,
        tolerance: 0.,
        priority: ContactPriority::Actor,
    };
    let actor_guard = ContactRegion {
        bounds: [
            actor.bounds[0] - DVec3::new(24., 0., 24.),
            actor.bounds[1] + DVec3::new(24., 0., 24.),
        ],
        ..actor.clone()
    };
    for pitch in [bookmark.pitch_degrees, 70.] {
        let mut bookmark = bookmark.clone();
        bookmark.pitch_degrees = pitch;
        let camera = crate::WorldStartView::camera_at(&bookmark, position);
        let projection = PerspectiveProjection {
            aspect_ratio: 16. / 9.,
            far: bookmark.fog_visibility,
            ..default()
        };
        let view = LodView {
            clip_from_world: projection.get_clip_from_view().as_dmat4()
                * camera.to_matrix().as_dmat4().inverse(),
            viewport: [2560, 1440],
            contact_position: camera.translation.as_dvec3(),
        };
        // Conservative full-meadow stress: demand every nearby leaf, including
        // cells that the actual authored coverage might leave empty.
        let mut contacts = vec![actor_guard.clone()];
        contacts.extend(
            metadata
                .values()
                .filter(|m| m.key.level == 0)
                .filter_map(|m| {
                    let bounds = m.bounds(size);
                    (view
                        .contact_position
                        .distance(view.contact_position.clamp(bounds[0], bounds[1]))
                        <= 112.)
                        .then_some(ContactRegion {
                            bounds: [
                                bounds[0] - DVec3::new(16., 0., 16.),
                                bounds[1] + DVec3::new(16., 0., 16.),
                            ],
                            exact: true,
                            tolerance: 0.,
                            priority: ContactPriority::Vegetation,
                        })
                }),
        );
        assert!(contacts.len() <= lod::contact::MAX_CONTACT_REGIONS);
        // The default budget, a phone-sized one and two larger desktop candidates.
        for (max_patches, max_triangles) in [
            (512, 1_048_576),
            (512, 262_144),
            (1024, 2_097_152),
            (2048, 4_194_304),
        ] {
            let settings = LodSettings {
                max_patches,
                max_triangles,
                ..default()
            };
            let start = std::time::Instant::now();
            let plan = lod::plan_cover_with_contacts(
                &keys,
                &metadata,
                &BTreeSet::new(),
                &view,
                size,
                &settings,
                &contacts,
            )
            .unwrap();
            let actor_ready = cover_accepts(&actor, &plan.patches, &metadata, size);
            let grass_ready = contacts
                .iter()
                .skip(1)
                .filter(|r| cover_accepts(r, &plan.patches, &metadata, size))
                .count();
            // Identifies the exact cover and its stitched edges across planner changes.
            let cover_hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::hash::DefaultHasher::new();
                for (key, edges) in &plan.patches {
                    (key.level, key.x, key.z, edges.0).hash(&mut hasher);
                }
                hasher.finish()
            };
            println!(
                "budget={max_patches}/{max_triangles} pitch={pitch} patches={} triangles={} work={} actor_ready={actor_ready} grass_regions={grass_ready}/{} limited={} error_px={:.2} elapsed_ms={:.3} cover={cover_hash:016x}",
                plan.patches.len(),
                plan.stats.triangles,
                plan.stats.work,
                contacts.len() - 1,
                plan.stats.budget_limited,
                plan.stats.maximum_visible_error,
                start.elapsed().as_secs_f64() * 1000.,
            );
            assert!(plan.balanced && actor_ready);
            assert!(plan.patches.len() <= settings.max_patches);
            assert!(plan.stats.triangles <= settings.max_triangles);
            assert!(plan.stats.work <= settings.max_work);
            assert!(plan.requests.is_empty());
        }
    }
}

/// Replays the engine's streaming loop from the roots alone: plan, load the first metadata
/// batch the plan requests, repeat. The actor's ground must arrive within a few plans even
/// where the view keeps requesting more fine terrain than one batch can load.
#[test]
#[ignore = "set YARRA_TEST_WORLD_DB and YARRA_TEST_START_VIEW to a cooked landscape and bookmark"]
fn streamed_actor_contact_arrives_within_a_few_plans() {
    let path = std::env::var_os("YARRA_TEST_WORLD_DB").expect("YARRA_TEST_WORLD_DB");
    let view_path = std::env::var_os("YARRA_TEST_START_VIEW").expect("YARRA_TEST_START_VIEW");
    let bookmark: world::WorldViewBookmark =
        ron::from_str(&std::fs::read_to_string(view_path).unwrap()).unwrap();
    let reader = RuntimeReader::open_immutable(std::path::Path::new(&path)).unwrap();
    let space = reader
        .manifest()
        .world_space(reader.manifest().default_world_space)
        .unwrap();
    let size = f64::from(space.cell_size);
    let roots = reader.read_terrain_roots(space.id).unwrap();
    let keys: Vec<_> = roots.iter().map(|d| d.key).collect();
    let describe = |d: &world_db::TerrainNodeDescriptor| PatchMetadata {
        key: d.key,
        resolution: d.resolution.unwrap_or(2),
        height_bounds: d.height_bounds,
        geometric_error: d.geometric_error,
    };
    let mut metadata: BTreeMap<_, _> = roots.iter().map(|d| (d.key, describe(d))).collect();
    let position = Vec3::from_array(bookmark.position);
    let actor = ContactRegion {
        bounds: [
            position.as_dvec3() - DVec3::new(0.35, 10000., 0.35),
            position.as_dvec3() + DVec3::new(0.35, 10000., 0.35),
        ],
        exact: true,
        tolerance: 0.,
        priority: ContactPriority::Actor,
    };
    let camera = crate::WorldStartView::camera_at(&bookmark, position);
    let projection = PerspectiveProjection {
        aspect_ratio: 16. / 9.,
        far: bookmark.fog_visibility,
        ..default()
    };
    let view = LodView {
        clip_from_world: projection.get_clip_from_view().as_dmat4()
            * camera.to_matrix().as_dmat4().inverse(),
        viewport: [2560, 1440],
        contact_position: camera.translation.as_dvec3(),
    };
    let settings = LodSettings::default();
    let mut previous = BTreeSet::new();
    for plan_number in 0..16 {
        let plan = lod::plan_cover_with_contacts(
            &keys,
            &metadata,
            &previous,
            &view,
            size,
            &settings,
            std::slice::from_ref(&actor),
        )
        .unwrap();
        if cover_accepts(&actor, &plan.patches, &metadata, size) {
            println!(
                "actor ground after {plan_number} plans, {} patches",
                plan.patches.len()
            );
            return;
        }
        // As the engine does: one bounded batch of the requests, in the plan's order.
        let batch: Vec<_> = plan
            .requests
            .iter()
            .take(world_db::MAX_TERRAIN_NODE_QUERY)
            .copied()
            .collect();
        for (key, descriptor) in batch
            .iter()
            .zip(reader.read_terrain_node_descriptors(&batch).unwrap())
        {
            if let Some(descriptor) = descriptor {
                metadata.insert(*key, describe(&descriptor));
            }
        }
        if plan.balanced {
            previous = plan.patches.keys().copied().collect();
        }
    }
    panic!("the actor's ground did not arrive within 16 plans");
}

/// Main-thread costs at the desktop budget: steady re-plans with the metadata a settled
/// view keeps, a turn, and the morph's common cover. Hashes identify exact results, so an
/// optimisation can be checked for identical covers.
#[test]
#[ignore = "set YARRA_TEST_WORLD_DB and YARRA_TEST_START_VIEW to a cooked landscape and bookmark"]
fn planner_cost_probe() {
    let path = std::env::var_os("YARRA_TEST_WORLD_DB").expect("YARRA_TEST_WORLD_DB");
    let view_path = std::env::var_os("YARRA_TEST_START_VIEW").expect("YARRA_TEST_START_VIEW");
    let bookmark: world::WorldViewBookmark =
        ron::from_str(&std::fs::read_to_string(view_path).unwrap()).unwrap();
    let reader = RuntimeReader::open_immutable(std::path::Path::new(&path)).unwrap();
    let space = reader
        .manifest()
        .world_space(reader.manifest().default_world_space)
        .unwrap();
    let size = f64::from(space.cell_size);
    let roots = reader.read_terrain_roots(space.id).unwrap();
    let keys: Vec<_> = roots.iter().map(|d| d.key).collect();
    let mut full = BTreeMap::new();
    let mut pending = roots;
    while !pending.is_empty() {
        let mut children = vec![];
        for d in pending {
            if let Some(keys) = d.key.children().unwrap() {
                children.extend(keys);
            }
            full.insert(
                d.key,
                PatchMetadata {
                    key: d.key,
                    resolution: d.resolution.unwrap(),
                    height_bounds: d.height_bounds,
                    geometric_error: d.geometric_error,
                },
            );
        }
        pending = children
            .chunks(world_db::MAX_TERRAIN_NODE_QUERY)
            .flat_map(|batch| {
                reader
                    .read_terrain_node_descriptors(batch)
                    .unwrap()
                    .into_iter()
                    .map(Option::unwrap)
            })
            .collect();
    }
    let position = Vec3::from_array(bookmark.position);
    let actor = ContactRegion {
        bounds: [
            position.as_dvec3() - DVec3::new(0.35, 10000., 0.35),
            position.as_dvec3() + DVec3::new(0.35, 10000., 0.35),
        ],
        exact: true,
        tolerance: 0.,
        priority: ContactPriority::Actor,
    };
    // Grass pages near the actor, as coalesced 32 m regions with a reach margin.
    let mut contacts = vec![actor.clone()];
    contacts.extend(
        full.values()
            .filter(|m| m.key.level == 0)
            .filter_map(|m| {
                let bounds = m.bounds(size);
                let at = position.as_dvec3();
                (at.distance(at.clamp(bounds[0], bounds[1])) <= 64.).then_some(ContactRegion {
                    bounds: [
                        bounds[0] - DVec3::new(2., 0., 2.),
                        bounds[1] + DVec3::new(2., 0., 2.),
                    ],
                    exact: false,
                    tolerance: 0.01,
                    priority: ContactPriority::Vegetation,
                })
            })
            .take(lod::contact::MAX_CONTACT_REGIONS - 1),
    );
    let settings = LodSettings::default();
    let view_at = |yaw: f32| {
        let mut bookmark = bookmark.clone();
        bookmark.yaw_degrees += yaw;
        let camera = crate::WorldStartView::camera_at(&bookmark, position);
        let projection = PerspectiveProjection {
            aspect_ratio: 16. / 9.,
            far: bookmark.fog_visibility,
            ..default()
        };
        LodView {
            clip_from_world: projection.get_clip_from_view().as_dmat4()
                * camera.to_matrix().as_dmat4().inverse(),
            viewport: [2560, 1440],
            contact_position: camera.translation.as_dvec3(),
        }
    };
    let hash = |cover: &BTreeMap<TerrainNodeKey, StitchEdges>| {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        for (key, edges) in cover {
            (key.level, key.x, key.z, edges.0).hash(&mut hasher);
        }
        hasher.finish()
    };
    // What evict keeps for a settled cover: the cover, its ancestors, direct children, roots.
    let settled = |cover: &BTreeMap<TerrainNodeKey, StitchEdges>| {
        let mut kept: BTreeSet<_> = keys.iter().copied().collect();
        for key in cover.keys() {
            kept.insert(*key);
            kept.extend(key.children().unwrap().into_iter().flatten());
            let mut node = *key;
            while let Some(parent) = node.parent().unwrap() {
                kept.insert(parent);
                node = parent;
            }
        }
        kept.into_iter()
            .filter_map(|k| full.get(&k).map(|m| (k, m.clone())))
            .collect::<BTreeMap<_, _>>()
    };
    fn median(mut f: impl FnMut() -> f64) -> f64 {
        let mut times: Vec<f64> = (0..7).map(|_| f()).collect();
        times.sort_by(f64::total_cmp);
        times[3]
    }
    let mut covers = Vec::new();
    for (name, yaw) in [("start", 0.), ("turned", 135.), ("behind", 180.)] {
        let view = view_at(yaw);
        let plan = lod::plan_cover_with_contacts(
            &keys,
            &full,
            &BTreeSet::new(),
            &view,
            size,
            &settings,
            &contacts,
        )
        .unwrap();
        let metadata = settled(&plan.patches);
        let previous: BTreeSet<_> = plan.patches.keys().copied().collect();
        let mut steady = None;
        let steady_ms = median(|| {
            let start = std::time::Instant::now();
            steady = Some(
                lod::plan_cover_with_contacts(
                    &keys, &metadata, &previous, &view, size, &settings, &contacts,
                )
                .unwrap(),
            );
            start.elapsed().as_secs_f64() * 1000.
        });
        let steady = steady.unwrap();
        // For a sampling profiler: YARRA_PROBE_LOOP_SECONDS repeats the steady plan.
        if name == "start"
            && let Some(seconds) = std::env::var("YARRA_PROBE_LOOP_SECONDS")
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
        {
            let start = std::time::Instant::now();
            while start.elapsed().as_secs_f64() < seconds {
                lod::plan_cover_with_contacts(
                    &keys, &metadata, &previous, &view, size, &settings, &contacts,
                )
                .unwrap();
            }
        }
        let actor_only_ms = median(|| {
            let start = std::time::Instant::now();
            lod::plan_cover_with_contacts(
                &keys,
                &metadata,
                &previous,
                &view,
                size,
                &settings,
                std::slice::from_ref(&actor),
            )
            .unwrap();
            start.elapsed().as_secs_f64() * 1000.
        });
        println!(
            "view={name} patches={} steady_patches={} metadata={} contacts={} steady_plan_ms={steady_ms:.3} actor_only_plan_ms={actor_only_ms:.3} work={} requests={} cover={:016x} steady={:016x}",
            plan.patches.len(),
            steady.patches.len(),
            metadata.len(),
            contacts.len(),
            steady.stats.work,
            steady.requests.len(),
            hash(&plan.patches),
            hash(&steady.patches),
        );
        covers.push((name, view, plan.patches, metadata));
    }
    for (a, b) in [(0, 1), (1, 0), (0, 2)] {
        let (from, _, old, old_metadata) = &covers[a];
        let (to, view, new, new_metadata) = &covers[b];
        let mut metadata = old_metadata.clone();
        metadata.extend(new_metadata.iter().map(|(k, m)| (*k, m.clone())));
        let previous: BTreeSet<_> = old.keys().copied().collect();
        let turn_ms = median(|| {
            let start = std::time::Instant::now();
            lod::plan_cover_with_contacts(
                &keys, &metadata, &previous, view, size, &settings, &contacts,
            )
            .unwrap();
            start.elapsed().as_secs_f64() * 1000.
        });
        let mut common = Vec::new();
        let common_ms = median(|| {
            let start = std::time::Instant::now();
            common = lod::common_cover(old, new).unwrap();
            start.elapsed().as_secs_f64() * 1000.
        });
        let handoff_ms = median(|| {
            let start = std::time::Instant::now();
            transition::HandoffIndex::new(old, new, &metadata, size);
            start.elapsed().as_secs_f64() * 1000.
        });
        let common_hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::hash::DefaultHasher::new();
            common.hash(&mut hasher);
            hasher.finish()
        };
        println!(
            "turn={from}->{to} turn_plan_ms={turn_ms:.3} common_cover_ms={common_ms:.3} common={} common_hash={common_hash:016x} handoff_index_ms={handoff_ms:.3}",
            common.len()
        );
    }
}
