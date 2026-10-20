use super::*;
use environment::fixtures::*;

/// Tufts grow on the dry ground and meadow grass on the green.
fn plants(green_grows: bool) -> vegetation::VegetationCatalog {
    let grows = |surface: u8, grass: vegetation::GrassType| vegetation::SurfaceGrass {
        surface: [surface; 16],
        grass: vec![vegetation::GrassAmount {
            grass: grass.id(),
            amount: 1.0,
        }],
    };
    let mut surfaces = vec![grows(1, vegetation::fixtures::tufts())];

    if green_grows {
        surfaces.push(grows(2, vegetation::fixtures::meadow()));
    }

    vegetation::VegetationCatalog {
        surfaces,
        ..vegetation::fixtures::reference_catalog()
    }
}

fn request() -> fixture::Request {
    let dry = TerrainSurfaceId([1; 16]);
    let green = TerrainSurfaceId([2; 16]);
    fixture::Request {
        road: None,
        library: meadow_library(dry, green),
        plants: plants(true),
        preset: GREEN_MEADOW,
        base: dry,
        surfaces: vec![dry, green],
        worn: vec![],
        size: 8,
        footprint: Footprint::Patch,
        underlay: None,
    }
}

#[test]
fn names_do_not_invalidate_preview_but_apply_still_validates_them() {
    let original = request();
    let mut renamed = original.clone();
    renamed.library.revision += 1;

    for preset in &mut renamed.library.presets {
        preset.name.clear(); // A text field can be empty between Select All and typing.
        preset.revision += 1;

        if let environment::PresetKind::Composition(uses) = &mut preset.kind {
            for child in uses {
                child.name.clear();
            }
        }
    }

    assert!(visual_changes::same_library(
        &original.library,
        &renamed.library
    ));
    assert!(
        Draft {
            base: original.library.clone(),
            library: renamed.library.clone()
        }
        .dirty()
    );
    let before = original.clone().compile().unwrap();
    let after = renamed.clone().compile().unwrap();
    assert_eq!(before.scene, after.scene);
    assert_eq!(before.ground, after.ground);
    assert!(
        renamed.library.validate().is_err(),
        "Apply still validates authoring names"
    );

    let p = renamed
        .library
        .presets
        .iter_mut()
        .find(|p| p.id == GREEN_MEADOW)
        .unwrap();
    let environment::PresetKind::Composition(children) = &mut p.kind else {
        panic!()
    };
    children[0].overrides.push(environment::PresetOverride {
        path: vec![],
        value: environment::QuickValue::GroundInfluence(0.25),
    });
    assert!(!visual_changes::same_library(
        &original.library,
        &renamed.library
    ));
}

#[test]
fn road_style_renaming_keeps_preview_geometry_and_refreshes_the_authoring_name() {
    let original = road_request();
    let mut renamed = original.clone();
    let p = &mut renamed.road.as_mut().unwrap().profile;
    p.name.push_str(" renamed");
    p.revision += 1;
    assert!(visual_changes::same_road(
        original.road.as_ref().map(|r| &r.profile),
        Some(p)
    ));
    let before = original.clone().compile().unwrap();
    let after = renamed.clone().compile().unwrap();
    assert_eq!(before.terrain, after.terrain);
    assert_eq!(before.ground, after.ground);
    assert_eq!(before.scene, after.scene);

    let mut unnamed = renamed.clone();
    unnamed.road.as_mut().unwrap().profile.name.clear();
    assert!(unnamed.road.as_ref().unwrap().profile.validate().is_err());
    assert!(
        unnamed.compile().is_ok(),
        "unfinished names cannot invalidate visual previews"
    );

    let mut styles = road_styles::Styles::default();
    let mut roads = crate::road_authoring::working::RoadWorkingSet::default();
    let original_profile = original.road.unwrap().profile;
    let key = world_db::RoadRecordKey::Profile(original_profile.id);
    roads
        .apply(&[crate::road_authoring::working::RoadChange {
            key,
            record: Some(world_db::RoadSourceRecord::Profile(original_profile)),
        }])
        .unwrap();
    assert!(styles.refresh(&roads));
    let renamed_profile = renamed.road.unwrap().profile;
    roads
        .apply(&[crate::road_authoring::working::RoadChange {
            key,
            record: Some(world_db::RoadSourceRecord::Profile(renamed_profile.clone())),
        }])
        .unwrap();
    assert!(!styles.refresh(&roads));
    assert_eq!(
        styles.draft.as_ref().unwrap().value.name,
        renamed_profile.name
    );
    styles.draft.as_mut().unwrap().value.name.push_str(" draft");
    assert!(styles.dirty());
    styles.draft.as_mut().unwrap().value.relief.road_depth += 0.1;
    assert!(!visual_changes::same_road(
        Some(&renamed_profile),
        styles.draft.as_ref().map(|d| &d.value)
    ));
}

#[test]
fn production_patch_is_deterministic_and_coverage_controls_ground_and_grass() {
    let r = request();
    let a = r.clone().compile().unwrap();
    let b = r.clone().compile().unwrap();
    assert_eq!(a.scene, b.scene);
    assert_eq!(a.ground, b.ground);
    assert!(a.candidates > 0);
    assert!(
        a.scene.pages[0]
            .fields
            .iter()
            .any(|f| f.coverage.iter().any(|d| *d > 0))
    );
    let full = fixture::Request {
        footprint: Footprint::Full,
        ..r.clone()
    }
    .compile()
    .unwrap();
    let hole = fixture::Request {
        footprint: Footprint::Hole,
        ..r
    }
    .compile()
    .unwrap();
    assert_ne!(a.scene.pages, full.scene.pages);
    assert_ne!(a.scene.pages, hole.scene.pages);
    assert_ne!(a.ground, full.ground);
    assert_ne!(a.ground, hole.ground);
}

#[test]
fn exclusions_clear_the_grass_of_the_ground_beneath() {
    let r = fixture::Request {
        preset: CLEAR_GRASS,
        ..request()
    };
    let cleared = fixture::Request {
        underlay: Some(DRY_MEADOW),
        footprint: Footprint::Full,
        ..r.clone()
    }
    .compile()
    .unwrap();
    assert!(cleared.scene.pages[0].fields.is_empty());
    let patch = fixture::Request {
        underlay: Some(DRY_MEADOW),
        ..r
    }
    .compile()
    .unwrap();
    let field = &patch.scene.pages[0].fields[0];
    let n = usize::from(field.resolution);
    assert_eq!(field.coverage[n / 2 * n + n / 2], 0, "the patch is cleared");
    assert!(
        field.coverage.iter().any(|&v| v > 0),
        "grass grows around it"
    );
}

#[test]
fn preview_rejects_unbounded_patch_and_groundless_underlay() {
    assert!(
        fixture::Request {
            size: 128,
            ..request()
        }
        .compile()
        .is_err()
    );
    assert!(
        fixture::Request {
            underlay: Some(CLEAR_GRASS),
            ..request()
        }
        .compile()
        .is_err()
    );
}

#[test]
fn preset_workspace_restores_scene_lighting_wind_and_camera_ownership() {
    use bevy::camera::visibility::RenderLayers;
    use vegetation_render::*;
    let mut app = App::new();
    let original = VegetationSceneState::reference();
    let mut wind = VegetationWind::default();
    wind.set_phase_seconds(12.0);
    let transform = Transform::from_xyz(1.0, 2.0, 3.0);
    app.add_plugins(bevy::state::app::StatesPlugin)
        .add_plugins(super::super::EditorWorkspacesPlugin)
        .init_resource::<PresetAuthoringState>()
        .init_resource::<viewport::PreviewState>()
        .insert_resource(original.clone())
        .insert_resource(wind)
        .init_resource::<VegetationSettings>()
        .init_resource::<VegetationLighting>()
        .init_resource::<GlobalAmbientLight>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<terrain_render::clipmap::ClipmapTerrainMaterial>>()
        .init_resource::<terrain_render::clipmap::ClipmapUploads>()
        .add_systems(OnEnter(EditorWorkspace::Presets), viewport::enter)
        .add_systems(OnExit(EditorWorkspace::Presets), viewport::leave)
        .add_systems(Update, crate::shell::sync_workspace_cameras);
    let sun = app
        .world_mut()
        .spawn((
            engine::WorldSun,
            transform,
            DirectionalLight::default(),
            RenderLayers::layer(0),
        ))
        .id();
    let world = app
        .world_mut()
        .spawn((Camera::default(), engine::WorldViewCamera))
        .id();
    let preview = app
        .world_mut()
        .spawn((Camera::default(), PresetWorkspaceCamera))
        .id();
    let other = app
        .world_mut()
        .spawn((Camera::default(), super::super::AnimationWorkspaceCamera))
        .id();
    app.update();

    for destination in [
        EditorWorkspace::World,
        EditorWorkspace::Animation,
        EditorWorkspace::World,
    ] {
        app.world_mut()
            .resource_mut::<NextState<EditorWorkspace>>()
            .set(EditorWorkspace::Presets);
        app.update();
        assert!(app.world().get::<Camera>(preview).unwrap().is_active);
        assert!(!app.world().get::<Camera>(world).unwrap().is_active);
        assert!(!app.world().get::<Camera>(other).unwrap().is_active);
        app.world_mut()
            .resource_mut::<VegetationLighting>()
            .diffuse_strength = 0.987;
        app.world_mut()
            .resource_mut::<NextState<EditorWorkspace>>()
            .set(destination);
        app.update();
        assert!(!app.world().get::<Camera>(preview).unwrap().is_active);
        assert_eq!(
            app.world().resource::<VegetationSceneState>().scene(),
            original.scene()
        );
        assert_eq!(
            app.world().resource::<VegetationWind>().phase_seconds(),
            12.0
        );
        assert!(!app.world().resource::<VegetationWind>().externally_driven);
        assert_eq!(
            app.world()
                .resource::<VegetationLighting>()
                .diffuse_strength,
            VegetationLighting::default().diffuse_strength
        );
        assert_eq!(*app.world().get::<Transform>(sun).unwrap(), transform);
        assert_eq!(
            app.world().get::<RenderLayers>(sun),
            Some(&RenderLayers::layer(0))
        );
    }
}

pub(super) fn road_request() -> fixture::Request {
    let mut r = request();
    r.base = TerrainSurfaceId([2; 16]);

    // Roads wear the green base down to dry ground.
    r.worn = vec![environment::WornGround {
        surface: TerrainSurfaceId([2; 16]),
        ground: vec![environment::SurfaceWeight {
            surface: TerrainSurfaceId([1; 16]),
            weight: 1.0,
        }],
        trampled: None,
    }];

    // The hole in the reference ground shows bare base ground.
    r.plants = plants(false);
    r.underlay = Some(DRY_MEADOW);
    r.footprint = Footprint::Full;
    r.road = Some(fixture::RoadFixture {
        profile: crate::road_authoring::commands::default_profile(
            DRY_GROUND,
            environment::GRASS_CHANNEL,
        ),
        curved: false,
        width: 3.5,
        traffic: environment::roads::DEFAULT_TRAFFIC,
    });
    r
}

#[test]
fn road_relief_preview_uses_the_same_heightfield_for_ground_and_grass() {
    let mut r = road_request();
    let p = &mut r.road.as_mut().unwrap().profile;
    p.relief.road_depth = 0.15;
    p.relief.track_depth = 0.08;
    let output = r.compile().unwrap();
    let h = output.terrain.as_ref().unwrap();
    let grass = &output.scene.pages[0].surface;
    assert_eq!(h.resolution, grass.resolution);
    assert_eq!(h.normals_oct, grass.normals_oct);
    let n = usize::from(h.resolution);

    for i in 0..n * n {
        assert_eq!(h.height_at(i % n, i / n), grass.heights[i]);
    }

    assert!(h.heights.iter().any(|&height| height < -0.1));
}

#[test]
fn road_preview_uses_production_wear_and_preserves_unpainted_holes() {
    for size in [8, 16] {
        for curved in [false, true] {
            let mut r = road_request();
            r.size = size;
            r.road.as_mut().unwrap().curved = curved;
            let full = r.clone().compile().unwrap();
            let repeated = r.clone().compile().unwrap();
            assert_eq!(full.scene, repeated.scene);
            assert_eq!(full.ground, repeated.ground);

            // A quiet road leaves the strip between its tracks, over the hole, unworn.
            r.footprint = Footprint::Hole;
            r.road.as_mut().unwrap().traffic = 0.1;
            let worn = r.clone().compile().unwrap();

            // Keep the exact road fixture and grid, but make it clear no grass.
            let p = &mut r.road.as_mut().unwrap().profile;
            p.vegetation_channel = environment::ChannelId([0; 16]);
            let untouched = r.compile().unwrap();
            assert_ne!(full.scene.pages, worn.scene.pages);

            for (w, u) in worn.scene.pages[0]
                .fields
                .iter()
                .zip(&untouched.scene.pages[0].fields)
            {
                assert_eq!(w.grass, u.grass);
                assert!(w.coverage.iter().zip(&u.coverage).all(|(w, u)| w <= u));
                let n = (w.coverage.len() as f64).sqrt() as usize;
                assert_eq!(
                    w.coverage[n / 2 * n + n / 2],
                    0,
                    "the center hole must remain empty"
                );
            }

            assert_eq!(
                worn.ground, untouched.ground,
                "grass retention must not change the material mixture"
            );
        }
    }
}

#[test]
fn road_preview_wears_the_ground_and_its_grass_follows() {
    let mut r = road_request();
    r.underlay = None;
    let worn = r.clone().compile().unwrap();

    // Only the worn dry ground grows anything over the bare base.
    assert_eq!(worn.scene.pages[0].fields.len(), 1);

    // A world whose ground wears to itself shows its road in neither ground nor grass.
    let mut unworn = r.clone();
    unworn.worn.clear();
    let unworn = unworn.compile().unwrap();
    assert_ne!(worn.ground, unworn.ground);
    assert!(unworn.scene.pages[0].fields.is_empty());
    r.road.as_mut().unwrap().width = 1.0;
    assert!(r.compile().is_err());
}

#[test]
fn road_style_draft_participates_in_global_save_guard() {
    let mut state = PresetAuthoringState::default();
    let mut roads = crate::road_authoring::working::RoadWorkingSet::default();
    let p = road_request().road.unwrap().profile;
    roads
        .apply(&[crate::road_authoring::working::RoadChange {
            key: world_db::RoadRecordKey::Profile(p.id),
            record: Some(world_db::RoadSourceRecord::Profile(p)),
        }])
        .unwrap();
    state.styles.refresh(&roads);
    assert!(!state.dirty());
    state.styles.draft.as_mut().unwrap().value.breakup = 0.1;
    assert!(state.dirty());
    state.open(WorldSpaceId(1), Some(DRY_GROUND));
    assert!(
        state.dirty(),
        "switching to Environment preserves and guards the road draft"
    );
    state.styles.discard();
    assert!(!state.dirty());
}
