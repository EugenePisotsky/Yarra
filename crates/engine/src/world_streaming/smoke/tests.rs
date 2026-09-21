use super::super::{WorldDebugControls, WorldStreamingPlugin, test_world_resources};
use super::*;
use std::time::Duration;

fn key(space: i64, x: i32) -> PageKey {
    PageKey {
        space: WorldSpaceId(space),
        cell: CellCoord { x, z: 0 },
        domain: world::PageDomain::StaticObjects,
        lod: 0,
    }
}

#[test]
fn ordinary_world_streaming_composes_object_lod_without_smoke_state_or_plugin() {
    for plugin in [
        WorldStreamingPlugin::game("unused.sqlite"),
        WorldStreamingPlugin::editor("unused.sqlite"),
    ] {
        let mut app = App::new();
        app.add_plugins(plugin);
        assert!(app.is_plugin_added::<crate::ObjectLodPlugin>());
        assert!(!app.is_plugin_added::<StreamingSmokePlugin>());
        assert!(!app.world().contains_resource::<StreamingSmokeState>());
        assert!(!app.world().resource::<WorldDebugControls>().world_switch);
    }
}

#[test]
fn traversal_uses_canonical_position_and_exact_demand_instead_of_a_fixed_cell_radius() {
    let position = WorldPosition {
        space: WorldSpaceId(1),
        cell: CellCoord { x: 105, z: 0 },
        local: [0.; 3],
    };
    let nearby = key(1, 105);
    let camera_source = key(1, 110);
    let desired = BTreeSet::from([nearby, camera_source]);
    assert_traversal_sources(
        position,
        position.space,
        position.cell,
        &desired,
        [nearby, camera_source],
    );
    for bad in [key(1, 0), key(2, 105)] {
        assert!(
            std::panic::catch_unwind(|| assert_traversal_sources(
                position,
                position.space,
                position.cell,
                &desired,
                [bad]
            ))
            .is_err()
        );
    }
    let wrong_position = WorldPosition {
        cell: CellCoord::ZERO,
        ..position
    };
    assert!(
        std::panic::catch_unwind(|| assert_traversal_sources(
            wrong_position,
            position.space,
            position.cell,
            &desired,
            [nearby]
        ))
        .is_err()
    );
}

#[test]
fn opt_in_smoke_transitions_after_streaming_and_emits_success_only_once() {
    let mut app = App::new();
    let (mut catalog, origin) =
        test_world_resources(WorldSpaceId(1), CellCoord { x: 105, z: 0 }, None);
    let mut second = catalog.world_spaces[0].clone();
    second.id = WorldSpaceId(2);
    catalog.world_spaces.push(second);
    app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()))
        .init_resource::<Time>()
        .init_resource::<SourceResidency>()
        .insert_resource(origin)
        .insert_resource(WorldViewpoint {
            position: Some(WorldPosition {
                space: WorldSpaceId(1),
                cell: CellCoord { x: 105, z: 0 },
                local: [0.; 3],
            }),
        })
        .insert_resource(ActiveWorldSpace {
            current: Some(WorldSpaceId(1)),
            ..default()
        })
        .insert_resource(WorldStream {
            manifest: Some(world_db::RuntimeManifest {
                schema_version: world::RUNTIME_SCHEMA_VERSION,
                generation_id: "smoke-test".into(),
                content_hash: [0; 32],
                default_world_space: WorldSpaceId(1),
                vegetation_catalog: None,
                world_spaces: catalog
                    .world_spaces
                    .iter()
                    .map(|s| world_db::WorldSpaceRecord {
                        id: s.id,
                        name: s.name.clone(),
                        cell_size: s.cell_size,
                        minimum_y: s.minimum_y,
                        maximum_y: s.maximum_y,
                        atmosphere: s.atmosphere.clone(),
                        atmosphere_revision: 1,
                    })
                    .collect(),
            }),
            ..default()
        })
        .init_resource::<StreamingStats>()
        .add_message::<AppExit>()
        .add_plugins(StreamingSmokePlugin);
    {
        let mut stats = app.world_mut().resource_mut::<StreamingStats>();
        stats.status = "ready".into();
        stats.demanded = 1;
        stats.resident = 1;
        stats.owned_entities = 1;
    }
    app.world_mut()
        .spawn((WorldStreamFocus, Transform::default()));
    let page = key(1, 110);
    let entity = app.world_mut().spawn(StreamedPageEntity(page)).id();
    app.world_mut()
        .resource_mut::<SourceResidency>()
        .desired
        .insert(page);
    *app.world_mut().resource_mut::<StreamingSmokeState>() = StreamingSmokeState::Traversal {
        expected_space: WorldSpaceId(1),
        expected_cell: CellCoord { x: 105, z: 0 },
    };
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(7.5));
    app.update();
    assert_eq!(
        app.world()
            .resource::<ActiveWorldSpace>()
            .requested
            .unwrap()
            .space,
        WorldSpaceId(2)
    );
    assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    // Simulate the streaming coordinator committing the requested world this frame.
    fn commit_transition(
        mut active: ResMut<ActiveWorldSpace>,
        mut stats: ResMut<StreamingStats>,
        mut pages: Query<&mut StreamedPageEntity>,
    ) {
        active.current = Some(WorldSpaceId(2));
        active.requested = None;
        stats.gameplay_objects = 1;
        stats.cached_definitions = 1;
        for mut page in &mut pages {
            page.0.space = WorldSpaceId(2);
        }
    }
    app.add_systems(Update, commit_transition.in_set(WorldStreamingSystems));
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(3.5));
    app.update();
    assert_eq!(
        app.world()
            .get::<StreamedPageEntity>(entity)
            .unwrap()
            .0
            .space,
        WorldSpaceId(2)
    );
    let mut cursor = bevy::ecs::message::MessageCursor::<AppExit>::default();
    assert_eq!(
        cursor
            .read(app.world().resource::<Messages<AppExit>>())
            .count(),
        1
    );
    app.update();
    assert_eq!(
        cursor
            .read(app.world().resource::<Messages<AppExit>>())
            .count(),
        0
    );
    assert!(matches!(
        app.world().resource::<StreamingSmokeState>(),
        StreamingSmokeState::Complete
    ));
}
