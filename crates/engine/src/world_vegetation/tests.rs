use super::*;
use crate::world_streaming::test_world_resources;
use vegetation_render::{VegetationProfileMode, VegetationSettings};

pub(super) fn surface(space: i64, x: i32, height: f32) -> StreamedTerrainSurface {
    StreamedTerrainSurface {
        key: PageKey {
            space: WorldSpaceId(space),
            cell: CellCoord { x, z: 0 },
            domain: world::PageDomain::Terrain,
        },
        cell_size: 16.,
        heightfield: world::TerrainHeightfield::from_heights(2, &[height; 4], height, height, 16.)
            .unwrap(),
    }
}

fn fields(terrain: &StreamedTerrainSurface) -> StreamedVegetationFieldPage {
    StreamedVegetationFieldPage {
        key: PageKey {
            domain: world::PageDomain::Vegetation,
            ..terrain.key
        },
        cell_size: terrain.cell_size,
        data: VegetationFieldPageData {
            fields: vegetation::fixtures::reference_page([0.; 2]).fields,
        },
    }
}

fn app() -> App {
    let (catalog, origin) = test_world_resources(
        WorldSpaceId(1),
        CellCoord::ZERO,
        Some(vegetation::fixtures::reference_catalog()),
    );
    let mut app = App::new();
    app.insert_resource(catalog)
        .insert_resource(origin)
        .init_resource::<Time>()
        .init_resource::<WorldViewpoint>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<terrain_render::TerrainMaterial>>()
        .add_plugins(WorldVegetationPlugin);
    app
}

#[test]
fn terrain_conversion_joins_domains_and_preserves_relief_after_rebase() {
    let terrain = surface(1, 10, 7.5);
    let mut field = fields(&terrain);
    assert_ne!(terrain.key.domain, field.key.domain);
    assert!(terrain.matches_vegetation(&field));
    let page = terrain.vegetation_page(CellCoord { x: 9, z: -2 }, &field.data);
    assert_eq!(page.origin_xz, [16., 32.]);
    assert_eq!(page.surface.heights, vec![7.5; 4]);
    assert_eq!(page.surface.normals_oct, terrain.heightfield.normals_oct);
    assert_eq!(page.fields, field.data.fields);
    field.key.space = WorldSpaceId(2);
    assert!(!terrain.matches_vegetation(&field));
    field.key.space = WorldSpaceId(1);
    field.cell_size = 8.;
    assert!(!terrain.matches_vegetation(&field));
}

#[test]
fn only_joined_source_changes_invalidate_the_scene_and_missing_sources_clear_it() {
    let mut app = app();
    let terrain = surface(1, 0, 1.);
    let field = app.world_mut().spawn(fields(&terrain)).id();
    let ground = app.world_mut().spawn(terrain).id();
    app.update();
    let revision = app.world().resource::<VegetationSceneState>().revision();
    let unrelated = app.world_mut().spawn(surface(1, 100, 0.)).id();
    app.update();
    assert_eq!(
        app.world().resource::<VegetationSceneState>().revision(),
        revision
    );
    app.world_mut()
        .get_mut::<StreamedTerrainSurface>(unrelated)
        .unwrap()
        .heightfield
        .heights[0] = 5.;
    app.update();
    assert_eq!(
        app.world().resource::<VegetationSceneState>().revision(),
        revision
    );
    app.world_mut()
        .get_mut::<StreamedTerrainSurface>(ground)
        .unwrap()
        .heightfield
        .heights
        .fill(4.);
    app.update();
    assert_eq!(
        app.world().resource::<VegetationSceneState>().scene().pages[0]
            .surface
            .heights,
        vec![4.; 4]
    );
    app.world_mut()
        .get_mut::<StreamedVegetationFieldPage>(field)
        .unwrap()
        .data
        .fields[0]
        .coverage
        .fill(0);
    app.update();
    assert!(
        app.world().resource::<VegetationSceneState>().scene().pages[0].fields[0]
            .coverage
            .iter()
            .all(|&v| v == 0)
    );
    app.world_mut().despawn(ground);
    app.update();
    assert!(
        app.world()
            .resource::<VegetationSceneState>()
            .scene()
            .pages
            .is_empty()
    );
    app.world_mut().spawn(surface(1, 0, 8.));
    app.update();
    assert_eq!(
        app.world()
            .resource::<VegetationSceneState>()
            .scene()
            .pages
            .len(),
        1
    );
    app.insert_resource(WorldCatalog::default());
    app.update();
    assert!(
        app.world()
            .resource::<VegetationSceneState>()
            .scene()
            .pages
            .is_empty()
    );
}

#[test]
fn disabled_grass_stays_current_through_rebase_world_change_and_resume() {
    let mut app = app();

    for (space, x, height) in [(1, 10, 1.), (2, 0, 9.)] {
        let terrain = surface(space, x, height);
        app.world_mut().spawn(fields(&terrain));
        app.world_mut().spawn(terrain);
    }

    app.world_mut()
        .resource_mut::<WorldViewpoint>()
        .set(world::WorldPosition::from_world(
            WorldSpaceId(1),
            [163., 2., 4.],
            16.,
        ));
    app.update();
    assert_eq!(
        app.world().resource::<VegetationSceneState>().scene().pages[0].origin_xz,
        [160., 0.]
    );
    app.world_mut()
        .resource_mut::<VegetationSettings>()
        .profile_mode = VegetationProfileMode::Disabled;
    let (catalog, origin) = test_world_resources(
        WorldSpaceId(1),
        CellCoord { x: 10, z: 0 },
        Some(vegetation::fixtures::reference_catalog()),
    );
    app.insert_resource(catalog).insert_resource(origin);
    app.update();
    assert_eq!(
        app.world().resource::<VegetationSceneState>().scene().pages[0].origin_xz,
        [0., 0.]
    );
    assert_eq!(
        app.world().resource::<VegetationLodFocus>().position,
        Some(Vec3::new(3., 2., 4.))
    );
    let mut updated = vegetation::fixtures::reference_catalog();
    updated.grass[0].colors.tip = [0.5, 0.4, 0.1];
    let (catalog, origin) =
        test_world_resources(WorldSpaceId(2), CellCoord::ZERO, Some(updated.clone()));
    app.insert_resource(catalog).insert_resource(origin);
    app.update();
    let scene = app.world().resource::<VegetationSceneState>();
    assert_eq!(scene.scene().catalog, updated);
    assert_eq!(scene.scene().pages.len(), 1);
    assert_eq!(scene.scene().pages[0].surface.heights, vec![9.; 4]);
    assert_eq!(app.world().resource::<VegetationLodFocus>().position, None);
    let revision = scene.revision();
    app.world_mut()
        .resource_mut::<VegetationSettings>()
        .profile_mode = VegetationProfileMode::Full;
    app.update();
    assert_eq!(
        app.world().resource::<VegetationSceneState>().revision(),
        revision
    );
    app.insert_resource(WorldOrigin::default());
    app.update();
    assert!(
        app.world()
            .resource::<VegetationSceneState>()
            .scene()
            .pages
            .is_empty()
    );
}

#[test]
fn characters_press_grass_along_their_motion_nearest_first() {
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<VegetationLodFocus>()
        .init_resource::<vegetation_render::GrassBenders>()
        .add_systems(Update, list_grass_benders);
    app.world_mut()
        .resource_mut::<VegetationLodFocus>()
        .position = Some(Vec3::ZERO);

    for x in 0..vegetation_render::MAX_GRASS_BENDERS + 4 {
        let motion = crate::actor::CharacterMotion {
            displacement: Vec2::new(0.0, 0.05),
            speed_mps: 1.5,
            ..default()
        };
        app.world_mut().spawn((
            GlobalTransform::from_translation(Vec3::new(x as f32, 0.0, 0.0)),
            motion,
        ));
    }

    app.update();
    let benders = &app
        .world()
        .resource::<vegetation_render::GrassBenders>()
        .benders;
    assert_eq!(benders.len(), vegetation_render::MAX_GRASS_BENDERS);
    assert!(
        benders
            .iter()
            .all(|b| b.position.x < vegetation_render::MAX_GRASS_BENDERS as f32)
    );
    assert!((benders[0].velocity - Vec2::new(0.0, 1.5)).length() < 1e-5);
}

#[test]
fn grass_waits_for_its_leaf_and_for_any_ground() {
    let mut app = app();

    for x in [0, 1] {
        let terrain = surface(1, x, 1.);
        app.world_mut().spawn(fields(&terrain));
        app.world_mut().spawn(terrain);
    }

    app.insert_resource(ClipmapTerrain::test_flat(
        WorldSpaceId(1),
        CellCoord::ZERO,
        16.,
        1.,
    ));
    app.update();
    let scene = app.world().resource::<VegetationSceneState>().scene();
    assert_eq!(scene.pages.len(), 2);
    let waiting = scene
        .pages
        .iter()
        .find(|page| page.origin_xz == [16., 0.])
        .map(VegetationTerrainGate::page_id)
        .unwrap();
    let gate = app.world().resource::<VegetationTerrainGate>();
    assert!(!gate.block_all);
    assert_eq!(gate.blocked_pages, [waiting].into());
    app.insert_resource(ClipmapTerrain::default());
    app.update();
    assert!(app.world().resource::<VegetationTerrainGate>().block_all);
}
