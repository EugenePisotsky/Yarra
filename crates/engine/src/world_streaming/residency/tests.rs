use super::*;
use crate::world_streaming::StreamedTerrainSurface;
use attachment::attach_height_source;
use bevy::ecs::system::RunSystemOnce;
use world::{CellCoord, PagePayload, TerrainHeightfield, WorldSpaceId};
use world_db::RuntimeManifest;
fn space(size: f32) -> world_db::WorldSpaceRecord {
    world_db::WorldSpaceRecord {
        atmosphere: Default::default(),
        atmosphere_revision: 1,
        id: WorldSpaceId(1),
        name: "test".into(),
        cell_size: size,
        minimum_y: -100.,
        maximum_y: 100.,
    }
}
pub(in crate::world_streaming) fn height_page(key: PageKey) -> PreparedPage {
    PreparedPage {
        height_only: true,
        terrain: None,
        definitions: vec![],
        dependencies: vec![],
        decoded: DecodedPage {
            key,
            decoded_bytes: 64,
            gpu_bytes_estimate: 0,
            payload: PagePayload::TerrainHeightfield(world::TerrainHeightfieldPage {
                heightfield: TerrainHeightfield::from_heights(2, &[1., 2., 4., 8.], 1., 8., 8.)
                    .unwrap(),
                surfaces: vec![],
                weight_pages: vec![],
            }),
        },
    }
}
#[test]
fn height_sources_preserve_relief_without_allocating_render_assets() {
    let mut world = World::new();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, &world);
    let key = PageKey {
        space: WorldSpaceId(1),
        cell: CellCoord { x: -3, z: 2 },
        domain: PageDomain::TerrainRender,
        lod: 0,
    };
    let attachment = attach_height_source(&mut commands, height_page(key), 8.).unwrap();
    queue.apply(&mut world);
    assert_eq!(attachment.gpu_bytes_estimate, 0);
    assert_eq!(attachment.height_only_pages, 1);
    assert!(
        attachment.owned_terrain_meshes.is_empty()
            && attachment.owned_terrain_materials.is_empty()
            && attachment.owned_terrain_images.is_empty()
            && attachment.terrain_texture_set.is_none()
    );
    let entity = attachment.entities[0];
    assert!(world.get::<Mesh3d>(entity).is_none());
    let surface = world.get::<StreamedTerrainSurface>(entity).unwrap();
    assert_eq!(surface.key, key);
    assert_eq!(surface.heightfield.heights, vec![1., 2., 4., 8.]);
    assert!((surface.sample_world([-22., 22.]).height - 4.25).abs() < 1e-5);
}
fn attachment_app(resident_bytes: u64) -> App {
    let mut app = App::new();
    app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()))
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<TerrainMaterial>>()
        .init_asset::<Image>()
        .init_asset::<WorldAsset>()
        .init_resource::<TerrainMacroVariation>()
        .init_resource::<WorldOrigin>()
        .init_resource::<SourceResidency>()
        .insert_resource(WorldRenderAssets {
            unit_plane: Handle::default(),
        })
        .insert_resource(StreamingStats {
            decoded_bytes: resident_bytes,
            ..default()
        })
        .insert_resource(WorldStream {
            manifest: Some(RuntimeManifest {
                schema_version: world::RUNTIME_SCHEMA_VERSION,
                generation_id: "attachment-test".into(),
                content_hash: [0; 32],
                default_world_space: WorldSpaceId(1),
                world_spaces: vec![space(8.)],
                vegetation_catalog: None,
            }),
            ..default()
        })
        .add_systems(Update, attach_prepared_pages);
    app
}

fn queue_height(app: &mut App, order: i32, bytes: u64) -> PageKey {
    let key = PageKey {
        space: WorldSpaceId(1),
        cell: CellCoord { x: order, z: 0 },
        domain: PageDomain::TerrainRender,
        lod: 0,
    };
    let mut page = height_page(key);
    // Account large pages without allocating irrelevant payloads in this scheduling test.
    page.decoded.decoded_bytes = bytes;
    let mut stream = app.world_mut().resource_mut::<SourceResidency>();
    stream.desired.insert(key);
    stream.priorities.insert(key, (0, f64::from(order)));
    stream.pages.insert(key, PageState::Prepared(page));
    key
}

#[test]
fn budget_blocked_pages_do_not_starve_smaller_prepared_pages() {
    let mut app = attachment_app(MAX_RESIDENT_DECODED_BYTES - 128);
    let blocked = [
        queue_height(&mut app, 0, 256),
        queue_height(&mut app, 1, 256),
    ];
    let small = [queue_height(&mut app, 2, 64), queue_height(&mut app, 3, 64)];
    app.update();
    let stream = app.world().resource::<SourceResidency>();
    for key in blocked {
        assert!(matches!(stream.pages[&key], PageState::Prepared(_)));
    }
    for key in small {
        assert!(
            matches!(stream.pages[&key], PageState::Resident(_)),
            "a fitting page must pass blocked larger pages"
        );
    }
    assert_eq!(stream.admission_blocked, 2);
}

#[test]
fn attachment_limit_still_bounds_successful_work_per_frame() {
    let mut app = attachment_app(0);
    let keys: Vec<_> = (0..4).map(|i| queue_height(&mut app, i, 64)).collect();
    app.update();
    let stream = app.world().resource::<SourceResidency>();
    for key in &keys[..MAX_ATTACHMENTS_PER_FRAME] {
        assert!(matches!(stream.pages[key], PageState::Resident(_)));
    }
    for key in &keys[MAX_ATTACHMENTS_PER_FRAME..] {
        assert!(matches!(stream.pages[key], PageState::Prepared(_)));
    }
}

fn key(domain: PageDomain, x: i32) -> PageKey {
    PageKey {
        space: WorldSpaceId(1),
        cell: CellCoord { x, z: 0 },
        domain,
        lod: 0,
    }
}

fn queue_page(app: &mut App, page: PreparedPage) {
    let key = page.decoded.key;
    let mut residency = app.world_mut().resource_mut::<SourceResidency>();
    residency.desired.insert(key);
    residency.priorities.insert(key, (0, 0.));
    residency.pages.insert(key, PageState::Prepared(page));
}

fn assert_failed_without_entities(app: &mut App, key: PageKey, message: &str) {
    app.update();
    let PageState::Failed(error) = &app.world().resource::<SourceResidency>().pages[&key] else {
        panic!("invalid page must fail attachment");
    };
    assert!(error.contains(message), "{error}");
    let world = app.world_mut();
    assert_eq!(
        world
            .query::<&crate::world_streaming::StreamedPageEntity>()
            .iter(world)
            .count(),
        0,
        "failed attachment must not leave unowned entities"
    );
    assert!(world.resource::<Assets<Mesh>>().is_empty());
    assert!(world.resource::<Assets<TerrainMaterial>>().is_empty());
    assert!(world.resource::<Assets<Image>>().is_empty());
}

#[test]
fn invalid_later_gameplay_object_does_not_leave_earlier_entities() {
    for missing_definition in [true, false] {
        let mut app = attachment_app(0);
        let key = key(PageDomain::GameplayObjects, 0);
        let mut page = height_page(key);
        page.height_only = false;
        let definition = RuntimeObjectDefinition {
            id: ObjectDefinitionId([1; 16]),
            key: "valid".into(),
            display_name: "Valid".into(),
            visual_asset: None,
            activation: world::ObjectActivationPolicy::Proximity,
        };
        page.definitions.push(definition.clone());
        if !missing_definition {
            page.definitions.push(RuntimeObjectDefinition {
                id: ObjectDefinitionId([2; 16]),
                activation: world::ObjectActivationPolicy::RenderOnly,
                ..definition
            });
        }
        page.decoded.payload = PagePayload::GameplayObjects(world::GameplayObjectsPage {
            instances: (1..=2)
                .map(|n| world::GameplayObjectInstance {
                    id: world::StableObjectId([n; 16]),
                    definition: ObjectDefinitionId([n; 16]),
                    translation: [0.; 3],
                    yaw: 0.,
                    scale: 1.,
                })
                .collect(),
        });
        queue_page(&mut app, page);
        assert_failed_without_entities(
            &mut app,
            key,
            if missing_definition {
                "no fetched definition"
            } else {
                "render-only"
            },
        );
    }
}

#[test]
fn invalid_later_visual_object_does_not_leave_earlier_entities() {
    for failure in 0..3 {
        let mut app = attachment_app(0);
        let key = key(PageDomain::StaticObjects, 0);
        let mut page = height_page(key);
        page.height_only = false;
        let dependency = PageDependency {
            asset: world::AssetId([1; 32]),
            asset_lod: 0,
            kind: "gltf-scene".into(),
            uri: "fixture.glb".into(),
            bounds: [1.; 3],
            gpu_bytes_estimate: 0,
            shadow_policy: 0,
            minimum_screen_height: 0.,
        };
        page.dependencies.push(dependency.clone());
        if failure > 0 {
            page.dependencies.push(PageDependency {
                asset: world::AssetId([2; 32]),
                kind: if failure == 1 {
                    "unsupported".into()
                } else {
                    "gltf-scene".into()
                },
                ..dependency.clone()
            });
        }
        if failure == 2 {
            page.dependencies.push(PageDependency {
                asset: world::AssetId([2; 32]),
                asset_lod: 1,
                minimum_screen_height: 1.,
                ..dependency
            });
        }
        page.decoded.payload = PagePayload::StaticObjects(world::StaticObjectsPage {
            instances: (1..=2)
                .map(|n| world::StaticObjectInstance {
                    id: world::StableObjectId([n; 16]),
                    asset: world::AssetId([n; 32]),
                    generated: false,
                    translation: [0.; 3],
                    yaw: 0.,
                    scale: 1.,
                })
                .collect(),
        });
        queue_page(&mut app, page);
        assert_failed_without_entities(
            &mut app,
            key,
            [
                "no cooked asset dependency",
                "unsupported kind",
                "not descending",
            ][failure],
        );
    }
}

fn rendered_height_page(key: PageKey) -> PreparedPage {
    let mut page = height_page(key);
    page.height_only = false;
    let id = world::TerrainSurfaceId([1; 16]);
    let textures = world::TerrainTextureSet {
        id: TerrainTextureSetId([2; 16]),
        key: "fixture".into(),
        base_color_universal_uri: "fixture.png".into(),
        normal_material_universal_uri: "fixture.png".into(),
        macro_variation_universal_uri: "fixture.png".into(),
        base_color_astc_uri: "fixture.png".into(),
        normal_material_astc_uri: "fixture.png".into(),
        macro_variation_astc_uri: "fixture.png".into(),
        universal_gpu_bytes: 100,
        astc_gpu_bytes: 100,
    };
    page.terrain = Some(TerrainRenderResources {
        profile: world::TerrainProfile {
            space: key.space,
            texture_set: textures.id,
            weight_resolution: 1,
            macro_scales: [1.; 3],
            macro_contrast: 1.,
            macro_albedo_strength: 0.,
        },
        texture_set: textures,
        surfaces: vec![world_db::RuntimeTerrainSurface {
            layer: 0,
            surface: world::TerrainSurface {
                id,
                key: "fixture".into(),
                display_name: "Fixture".into(),
                tile_size: 1.,
                anti_tiling: false,
                normal_y_sign: 1.,
                normal_strength: 1.,
                roughness_min: 0.5,
                roughness_max: 1.,
            },
        }],
    });
    let PagePayload::TerrainHeightfield(ref mut terrain) = page.decoded.payload else {
        unreachable!()
    };
    terrain.surfaces.push(id);
    page
}

#[test]
fn invalid_flat_relief_does_not_allocate_terrain_assets() {
    let mut app = attachment_app(0);
    let key = key(PageDomain::TerrainRender, 0);
    let mut page = rendered_height_page(key);
    page.decoded.payload = PagePayload::TerrainRender(world::TerrainRenderPage {
        height: f32::NAN,
        surfaces: vec![world::TerrainSurfaceId([1; 16])],
        weight_pages: vec![],
    });
    queue_page(&mut app, page);
    assert_failed_without_entities(&mut app, key, "heightfield");
}

fn lifecycle_app() -> App {
    let mut app = attachment_app(0);
    app.init_resource::<Time>()
        .init_resource::<ActiveWorldSpace>()
        .init_resource::<WorldGenerationReload>()
        .add_systems(
            Update,
            (cool_and_remove_pages, update_streaming_stats)
                .chain()
                .after(attach_prepared_pages),
        );
    app
}

#[test]
fn cooling_revives_then_releases_owned_assets_and_keeps_shared_textures_accounted_once() {
    let mut app = lifecycle_app();
    let first = key(PageDomain::TerrainRender, 0);
    let second = key(PageDomain::TerrainRender, 1);
    queue_page(&mut app, rendered_height_page(first));
    queue_page(&mut app, rendered_height_page(second));
    app.update();
    let (entity, meshes, materials, images) =
        match &app.world().resource::<SourceResidency>().pages[&first] {
            PageState::Resident(a) => (
                a.entities[0],
                a.owned_terrain_meshes.clone(),
                a.owned_terrain_materials.clone(),
                a.owned_terrain_images.clone(),
            ),
            _ => panic!("terrain must attach"),
        };
    let stats = app.world().resource::<StreamingStats>();
    assert_eq!(stats.resident, 2);
    assert_eq!(
        stats.gpu_bytes_estimate, 100,
        "shared texture set counts once"
    );
    app.world_mut()
        .resource_mut::<SourceResidency>()
        .desired
        .remove(&first);
    app.update();
    assert_eq!(app.world().resource::<StreamingStats>().cooling, 1);
    assert_eq!(
        app.world().resource::<StreamingStats>().gpu_bytes_estimate,
        100
    );
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs(1));
    app.world_mut()
        .resource_mut::<SourceResidency>()
        .desired
        .insert(first);
    app.update();
    assert!(
        app.world().get_entity(entity).is_ok(),
        "revival preserves the same entity"
    );
    assert_eq!(app.world().resource::<StreamingStats>().resident, 2);
    app.world_mut()
        .resource_mut::<SourceResidency>()
        .desired
        .remove(&first);
    app.update();
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(COOLING_SECONDS));
    app.update();
    assert!(app.world().get_entity(entity).is_err());
    assert!(
        meshes
            .iter()
            .all(|h| !app.world().resource::<Assets<Mesh>>().contains(h.id()))
    );
    assert!(materials.iter().all(|h| {
        !app.world()
            .resource::<Assets<TerrainMaterial>>()
            .contains(h.id())
    }));
    assert!(
        images
            .iter()
            .all(|h| !app.world().resource::<Assets<Image>>().contains(h.id()))
    );
    let stats = app.world().resource::<StreamingStats>();
    assert_eq!(stats.resident, 1);
    assert_eq!(stats.cooling, 0);
    assert_eq!(
        stats.gpu_bytes_estimate, 100,
        "the other page still owns the shared texture set"
    );
}

fn clear_sources(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut stream: ResMut<WorldStream>,
    mut residency: ResMut<SourceResidency>,
) {
    crate::world_streaming::clear_streamed_pages(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        &mut stream,
        &mut residency,
    );
}

#[test]
fn clearing_sources_releases_resident_and_cooling_pages_and_rejects_old_replies() {
    let mut app = lifecycle_app();
    let first = key(PageDomain::TerrainRender, 0);
    let second = key(PageDomain::TerrainRender, 1);
    queue_page(&mut app, rendered_height_page(first));
    queue_page(&mut app, rendered_height_page(second));
    app.update();
    app.world_mut()
        .resource_mut::<SourceResidency>()
        .desired
        .remove(&first);
    app.update();
    let (worker, requests, _replies) = WorldDatabaseWorker::test_channel_pair(2, 1);
    let pending = key(PageDomain::TerrainRender, 2);
    {
        let mut residency = app.world_mut().resource_mut::<SourceResidency>();
        residency.set_demand(BTreeMap::from([(pending, (0, 0.))]));
        residency.request_missing(&worker, "old", true).unwrap();
        // A pending decode also belongs to the old source lifetime.
        residency.decode_tasks.push(DecodeTask {
            key: first,
            request_id: 10,
            task: AsyncComputeTaskPool::get().spawn(std::future::pending()),
        });
    }
    let DatabaseRequest::ReadPage {
        request_id: old_id, ..
    } = requests.recv().unwrap()
    else {
        unreachable!()
    };
    {
        let mut stream = app.world_mut().resource_mut::<WorldStream>();
        stream.index_revision = 8;
        stream.requested_index = Some((9, WorldSpaceId(1)));
    }
    app.world_mut().run_system_once(clear_sources).unwrap();
    let world = app.world_mut();
    assert_eq!(
        world
            .query::<&crate::world_streaming::StreamedPageEntity>()
            .iter(world)
            .count(),
        0
    );
    assert!(world.resource::<Assets<Mesh>>().is_empty());
    assert!(world.resource::<Assets<TerrainMaterial>>().is_empty());
    // Asset-server-owned texture handles may still be pending; only page images are owned here.
    assert!(world.resource::<Assets<Image>>().is_empty());
    let stream = world.resource::<WorldStream>();
    assert_eq!(stream.index_revision, 9);
    assert!(stream.requested_index.is_none());
    let mut residency = world.resource_mut::<SourceResidency>();
    assert!(
        residency.pages.is_empty()
            && residency.desired.is_empty()
            && residency.decode_tasks.is_empty()
    );
    residency.set_demand(BTreeMap::from([(pending, (0, 0.))]));
    residency.request_missing(&worker, "new", true).unwrap();
    let DatabaseRequest::ReadPage {
        request_id: new_id, ..
    } = requests.recv().unwrap()
    else {
        unreachable!()
    };
    assert_ne!(
        old_id, new_id,
        "clear must not reuse in-flight request identities"
    );
    residency.receive_page(old_id, pending, Err("old generation".into()));
    assert!(
        matches!(residency.pages[&pending], PageState::Loading { request_id } if request_id == new_id)
    );
    residency.receive_page(new_id, pending, Ok(None));
    assert!(matches!(&residency.pages[&pending], PageState::Failed(e) if e == "page is missing"));
}

#[test]
fn decode_completion_requires_current_identity_and_demand_before_caching_or_preparing() {
    let mut app = App::new();
    app.add_plugins(bevy::app::TaskPoolPlugin::default())
        .init_resource::<SourceResidency>()
        .add_systems(Update, receive_decode_results);
    {
        let mut residency = app.world_mut().resource_mut::<SourceResidency>();
        for n in 0..3 {
            let key = key(PageDomain::TerrainRender, n);
            let mut page = height_page(key);
            page.definitions.push(RuntimeObjectDefinition {
                id: ObjectDefinitionId([n as u8; 16]),
                key: n.to_string(),
                display_name: n.to_string(),
                visual_asset: None,
                activation: world::ObjectActivationPolicy::Proximity,
            });
            if n != 1 {
                residency.desired.insert(key);
            }
            residency.pages.insert(
                key,
                PageState::Decoding {
                    request_id: if n == 0 { 2 } else { 1 },
                },
            );
            residency.decode_tasks.push(DecodeTask {
                key,
                request_id: 1,
                task: AsyncComputeTaskPool::get().spawn(async move { Ok(page) }),
            });
        }
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !app
        .world()
        .resource::<SourceResidency>()
        .decode_tasks
        .is_empty()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "decode completion timed out"
        );
        app.update();
        std::thread::yield_now();
    }
    let residency = app.world().resource::<SourceResidency>();
    assert!(matches!(
        residency.pages[&key(PageDomain::TerrainRender, 0)],
        PageState::Decoding { request_id: 2 }
    ));
    assert!(
        !residency
            .pages
            .contains_key(&key(PageDomain::TerrainRender, 1))
    );
    assert!(matches!(
        residency.pages[&key(PageDomain::TerrainRender, 2)],
        PageState::Prepared(_)
    ));
    assert_eq!(residency.definition_cache.len(), 1);
    assert!(
        residency
            .definition_cache
            .contains_key(&ObjectDefinitionId([2; 16]))
    );
}
