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
        for max_triangles in [1_048_576, 262_144] {
            let settings = LodSettings {
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
            println!(
                "budget={max_triangles} pitch={pitch} patches={} triangles={} work={} actor_ready={actor_ready} grass_regions={grass_ready}/{} limited={} elapsed_ms={:.3}",
                plan.patches.len(),
                plan.stats.triangles,
                plan.stats.work,
                contacts.len() - 1,
                plan.stats.budget_limited,
                start.elapsed().as_secs_f64() * 1000.
            );
            assert!(plan.balanced && actor_ready);
            assert!(plan.patches.len() <= settings.max_patches);
            assert!(plan.stats.triangles <= settings.max_triangles);
            assert!(plan.stats.work <= settings.max_work);
            assert!(plan.requests.is_empty());
        }
    }
}
