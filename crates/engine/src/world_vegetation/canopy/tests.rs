use super::*;
use crate::world_streaming::test_world_resources;
use crate::world_vegetation::tests::surface;
use terrain_render::TerrainMaterialKey;
use vegetation::VegetationScene;
use world::{CellCoord, WorldSpaceId};

fn app() -> (App, Entity, Handle<TerrainMaterial>) {
    let (catalog, origin) = test_world_resources(
        WorldSpaceId(1),
        CellCoord::ZERO,
        Some(vegetation::fixtures::reference_catalog()),
    );
    let mut app = App::new();
    app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()))
        .init_asset::<Image>()
        .insert_resource(catalog)
        .insert_resource(origin)
        .init_resource::<Time>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<TerrainMaterial>>()
        .insert_resource(VegetationLighting {
            canopy: vegetation::CanopyShading {
                enabled: true,
                strength: 1.,
                ground_amount: 1.,
                ..default()
            },
            ..default()
        })
        .insert_resource(
            VegetationSceneState::new(VegetationScene {
                catalog: vegetation::fixtures::reference_catalog(),
                pages: vec![vegetation::fixtures::reference_page([0.; 2])],
            })
            .unwrap(),
        )
        .add_plugins(GroundCanopyPlugin);
    let material = material(&mut app);
    let entity = app
        .world_mut()
        .spawn((surface(1, 0, 0.), MeshMaterial3d(material.clone())))
        .id();
    app.update();
    (app, entity, material)
}
fn material(app: &mut App) -> Handle<TerrainMaterial> {
    // Exercise the real material API without requiring a GPU or decoded textures.
    let id = world::TerrainSurfaceId([1; 16]);
    let textures = world::TerrainTextureSet {
        id: world::TerrainTextureSetId([2; 16]),
        key: "fixture".into(),
        base_color_universal_uri: "canopy-fixture.png".into(),
        normal_material_universal_uri: "canopy-fixture.png".into(),
        macro_variation_universal_uri: "canopy-fixture.png".into(),
        base_color_astc_uri: "canopy-fixture.png".into(),
        normal_material_astc_uri: "canopy-fixture.png".into(),
        macro_variation_astc_uri: "canopy-fixture.png".into(),
        universal_gpu_bytes: 0,
        astc_gpu_bytes: 0,
    };
    let profile = world::TerrainProfile {
        space: WorldSpaceId(1),
        texture_set: textures.id,
        weight_resolution: 1,
        macro_scales: [1.; 3],
        macro_contrast: 1.,
        macro_albedo_strength: 0.,
    };
    let layers = [terrain_render::TerrainSurfaceLayer {
        layer: 0,
        surface: world::TerrainSurface {
            id,
            key: "fixture".into(),
            display_name: "fixture".into(),
            tile_size: 1.,
            anti_tiling: false,
            normal_y_sign: 1.,
            normal_strength: 1.,
            roughness_min: 0.5,
            roughness_max: 1.,
        },
    }];
    app.world_mut()
        .resource_scope(|world, mut images: Mut<Assets<Image>>| {
            let server = world.resource::<AssetServer>().clone();
            let mut materials = world.resource_mut::<Assets<TerrainMaterial>>();
            terrain_render::prepare_terrain_material(
                terrain_render::PrepareTerrainMaterialContext {
                    asset_server: &server,
                    images: &mut images,
                    materials: &mut materials,
                    cell: CellCoord::ZERO,
                    origin_cell: CellCoord::ZERO,
                    cell_size: 16.,
                    page_surfaces: &[id],
                    weight_pages: &[],
                    profile: &profile,
                    texture_set: &textures,
                    surfaces: &layers,
                    macro_variation: default(),
                },
            )
            .unwrap()
            .material
        })
}
fn attached(app: &App, material: &Handle<TerrainMaterial>) -> bool {
    let assets = app.world().resource::<Assets<TerrainMaterial>>();
    let actual = assets.get(material).unwrap();
    let mut without_canopy = actual.clone();
    without_canopy.set_canopy_coverage(None, Vec4::ZERO);
    TerrainMaterialKey::from(actual) != TerrainMaterialKey::from(&without_canopy)
}
fn ready_mask(app: &mut App, entity: Entity) -> Handle<Image> {
    let image = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::default());
    {
        let mut cache = app.world_mut().resource_mut::<GroundCanopyTiles>();
        let tile = cache.entries.get_mut(&entity).unwrap();
        tile.image = Some(image.clone());
        tile.needs_bake = false;
        tile.bounds = Vec4::new(0., 0., 16., 16.);
    }
    app.update();
    image
}
fn replace_scene(app: &mut App, mutate: impl FnOnce(&mut VegetationScene)) {
    let mut scene = app
        .world()
        .resource::<VegetationSceneState>()
        .scene()
        .clone();
    mutate(&mut scene);
    app.world_mut()
        .resource_mut::<VegetationSceneState>()
        .replace(scene)
        .unwrap();
}
fn pending_job() -> Task<canopy::Bake> {
    AsyncComputeTaskPool::get_or_init(|| bevy::tasks::TaskPoolBuilder::new().num_threads(2).build())
        .spawn(async {
            canopy::Bake {
                image: Image::default(),
                bounds: Vec4::splat(123.),
            }
        })
}

#[test]
fn canopy_reuses_unrelated_sources_but_rejects_changed_relief_and_stale_jobs() {
    let (mut app, entity, material) = app();
    let image = ready_mask(&mut app, entity);
    assert!(attached(&app, &material));
    replace_scene(&mut app, |scene| {
        scene
            .pages
            .insert(0, vegetation::fixtures::reference_page([1600., 0.]))
    });
    app.update();
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&image)
            .is_some()
    );
    assert!(attached(&app, &material));
    assert_eq!(
        app.world().resource::<GroundCanopyTiles>().counts(),
        (1, 1, 0)
    );
    app.world_mut()
        .resource_mut::<GroundCanopyTiles>()
        .entries
        .get_mut(&entity)
        .unwrap()
        .task = Some(pending_job());
    replace_scene(&mut app, |scene| scene.pages[1].surface.heights.fill(5.));
    app.update();
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&image)
            .is_none()
    );
    assert!(!attached(&app, &material));
    assert_eq!(
        app.world().resource::<GroundCanopyTiles>().counts(),
        (0, 1, 0)
    );
    let tile = &app.world().resource::<GroundCanopyTiles>().entries[&entity];
    assert!(tile.needs_bake && tile.task.is_none());
    assert_eq!(tile.bounds, Vec4::ZERO);
}

#[test]
fn disabling_cancels_jobs_reuses_valid_masks_and_processes_edits_before_resume() {
    let (mut app, entity, material) = app();
    let image = ready_mask(&mut app, entity);
    app.world_mut()
        .resource_mut::<VegetationLighting>()
        .canopy
        .enabled = false;
    app.update();
    assert!(!attached(&app, &material));
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&image)
            .is_some()
    );
    app.world_mut()
        .resource_mut::<VegetationLighting>()
        .canopy
        .enabled = true;
    app.update();
    assert!(attached(&app, &material));
    assert_eq!(
        app.world().resource::<GroundCanopyTiles>().counts(),
        (1, 1, 0)
    );
    app.world_mut()
        .resource_mut::<GroundCanopyTiles>()
        .entries
        .get_mut(&entity)
        .unwrap()
        .task = Some(pending_job());
    app.world_mut()
        .resource_mut::<VegetationLighting>()
        .canopy
        .ground_amount = 0.;
    app.update();
    assert_eq!(app.world().resource::<GroundCanopyTiles>().counts().2, 0);
    replace_scene(&mut app, |scene| scene.pages[0].fields[0].coverage.fill(0));
    app.update();
    app.world_mut()
        .resource_mut::<VegetationLighting>()
        .canopy
        .ground_amount = 1.;
    app.update();
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&image)
            .is_none()
    );
    assert!(!attached(&app, &material));
}

#[test]
fn material_replacement_unload_and_world_change_release_images_and_jobs() {
    let (mut app, entity, material) = app();
    let image = ready_mask(&mut app, entity);
    let replacement = self::material(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(MeshMaterial3d(replacement.clone()));
    app.update();
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&image)
            .is_none()
    );
    assert!(!attached(&app, &material));
    let image = ready_mask(&mut app, entity);
    app.world_mut()
        .resource_mut::<GroundCanopyTiles>()
        .entries
        .get_mut(&entity)
        .unwrap()
        .task = Some(pending_job());
    let (catalog, origin) = test_world_resources(
        WorldSpaceId(2),
        CellCoord::ZERO,
        Some(vegetation::fixtures::reference_catalog()),
    );
    app.insert_resource(catalog).insert_resource(origin);
    app.update();
    assert_eq!(
        app.world().resource::<GroundCanopyTiles>().counts(),
        (0, 0, 0)
    );
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&image)
            .is_none()
    );
    assert!(!attached(&app, &replacement));
    let (catalog, origin) = test_world_resources(
        WorldSpaceId(1),
        CellCoord::ZERO,
        Some(vegetation::fixtures::reference_catalog()),
    );
    app.insert_resource(catalog).insert_resource(origin);
    app.update();
    let image = ready_mask(&mut app, entity);
    app.world_mut()
        .resource_mut::<VegetationLighting>()
        .canopy
        .enabled = false;
    app.world_mut().despawn(entity);
    app.update();
    assert_eq!(
        app.world().resource::<GroundCanopyTiles>().counts(),
        (0, 0, 0)
    );
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&image)
            .is_none()
    );
    assert!(!attached(&app, &replacement));
}

#[test]
fn rebase_and_catalog_changes_retire_masks_before_accepting_results() {
    let (mut app, entity, material) = app();
    let image = ready_mask(&mut app, entity);
    app.world_mut()
        .resource_mut::<GroundCanopyTiles>()
        .entries
        .get_mut(&entity)
        .unwrap()
        .task = Some(pending_job());
    let (_, origin) = test_world_resources(WorldSpaceId(1), CellCoord { x: 10, z: 0 }, None);
    app.insert_resource(origin);
    replace_scene(&mut app, |scene| scene.pages[0].origin_xz = [-160., 0.]);
    app.update();
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&image)
            .is_none()
    );
    assert!(!attached(&app, &material));
    assert_eq!(
        app.world().resource::<VegetationLighting>().canopy_origin,
        [160., 0.]
    );
    assert_eq!(
        app.world().resource::<GroundCanopyTiles>().entries[&entity].origin,
        [-160., 0.]
    );
    let image = ready_mask(&mut app, entity);
    replace_scene(&mut app, |scene| {
        scene.catalog.species[0].key.push_str("_draft")
    });
    app.update();
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&image)
            .is_none()
    );
    assert!(!attached(&app, &material));
}

#[test]
fn empty_sources_do_not_bake_and_active_jobs_are_bounded() {
    let (mut app, _, _) = app();
    for x in [1, 2, 3] {
        let material = material(&mut app);
        app.world_mut()
            .spawn((surface(1, x, 0.), MeshMaterial3d(material)));
    }
    replace_scene(&mut app, |scene| {
        for x in [1, 2, 3] {
            scene
                .pages
                .push(vegetation::fixtures::reference_page([x as f32 * 16., 0.]));
        }
    });
    app.update();
    AsyncComputeTaskPool::get_or_init(|| {
        bevy::tasks::TaskPoolBuilder::new().num_threads(2).build()
    });
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs_f32(0.4));
    app.update();
    assert_eq!(app.world().resource::<GroundCanopyTiles>().counts().2, 2);
    replace_scene(&mut app, |scene| scene.pages.clear());
    app.update();
    assert_eq!(
        app.world().resource::<GroundCanopyTiles>().counts(),
        (0, 4, 0)
    );
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs_f32(0.4));
    app.update();
    assert_eq!(
        app.world().resource::<GroundCanopyTiles>().counts(),
        (0, 4, 0)
    );
}
