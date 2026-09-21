use super::*;

/// A game-owned root whose Transform and MoveIntent use render coordinates.
/// Mark actors, cameras, positional lights and effects here, never their children.
/// Editor entities derived directly from canonical records do not use this marker.
#[derive(Component)]
pub struct WorldRenderRoot;

pub(super) fn sync_vegetation_origin(
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    vegetation: Option<ResMut<vegetation_render::VegetationRenderOrigin>>,
    clouds: Option<ResMut<atmosphere::clouds::CloudOrigin>>,
) {
    if let Some(mut clouds) = clouds {
        clouds.0 = origin
            .space()
            .and_then(|id| catalog.world_space(id))
            .map_or([0.; 2], |space| origin.cell().origin(space.cell_size));
    }
    if let Some(mut vegetation) = vegetation {
        vegetation.world_xz = origin
            .space()
            .and_then(|id| catalog.world_space(id))
            .map_or([0.; 2], |space| origin.cell().origin(space.cell_size));
    }
}

pub(super) type Roots<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        Option<&'static mut GlobalTransform>,
        Option<&'static mut MoveIntent>,
        Has<StreamedPageEntity>,
    ),
    (
        Without<ChildOf>,
        Or<(With<WorldRenderRoot>, With<StreamedPageEntity>)>,
    ),
>;

pub(super) fn effective_config(
    mut config: WorldStreamingConfig,
    hierarchy: bool,
) -> WorldStreamingConfig {
    // Keep normal launches on their existing material/authoring path until its
    // world-space texture and wind phase contracts are implemented.
    if config.gameplay_pages && hierarchy {
        config.floating_origin_threshold_cells = Some(8);
    }
    config
}

pub(super) fn shift_roots(
    roots: &mut Roots,
    old: CellCoord,
    new: CellCoord,
    size: f32,
    keep_sources: bool,
) {
    let shift = Vec3::new(
        ((i64::from(old.x) - i64::from(new.x)) as f64 * f64::from(size)) as f32,
        0.,
        ((i64::from(old.z) - i64::from(new.z)) as f64 * f64::from(size)) as f32,
    );
    for (mut transform, global, intent, source) in roots {
        if source && !keep_sources {
            continue;
        }
        transform.translation += shift;
        // Pointer rays in this Update must see the same origin as terrain queries.
        // Child transforms are propagated normally before rendering.
        if let Some(mut global) = global {
            *global = GlobalTransform::from(*transform);
        }
        if let Some(mut intent) = intent {
            intent.rebase(shift);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::CharacterGait;

    #[test]
    fn game_rebase_keeps_world_positions_intents_children_and_page_residency() {
        let space = WorldSpaceId(1);
        let mut app = App::new();
        app.insert_resource(WorldStreamingConfig::game())
            .insert_resource(TerrainLodPreview {
                enabled: true,
                ..default()
            })
            .insert_resource(ActiveWorldSpace {
                current: Some(space),
                ..default()
            })
            .insert_resource(WorldOrigin {
                space: Some(space),
                cell: CellCoord { x: 10, z: -10 },
            })
            .init_resource::<WorldViewpoint>()
            .init_resource::<SourceResidency>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<TerrainMaterial>>()
            .init_resource::<Assets<Image>>()
            .insert_resource(WorldStream {
                height_only: true,
                index_revision: 7,
                manifest: Some(RuntimeManifest {
                    schema_version: world::RUNTIME_SCHEMA_VERSION,
                    generation_id: "test".into(),
                    content_hash: [0; 32],
                    default_world_space: space,
                    vegetation_catalog: None,
                    world_spaces: vec![world_db::WorldSpaceRecord {
                        atmosphere: Default::default(),
                        atmosphere_revision: 1,
                        id: space,
                        name: "test".into(),
                        cell_size: 32.,
                        minimum_y: 0.,
                        maximum_y: 100.,
                    }],
                }),
                ..default()
            })
            .add_systems(
                Update,
                (sync_stream_focus_to_viewpoint, update_world_origin).chain(),
            );
        let mut intent = MoveIntent::default();
        intent.set_destination(Vec3::new(321., 7., -40.), CharacterGait::Jog);
        let actor = app
            .world_mut()
            .spawn((
                WorldRenderRoot,
                WorldStreamFocus,
                Transform::from_xyz(305., 7., 2.),
                intent,
            ))
            .id();
        let child = app
            .world_mut()
            .spawn((
                WorldRenderRoot,
                Transform::from_xyz(0., 2., 0.),
                ChildOf(actor),
            ))
            .id();
        let camera = app
            .world_mut()
            .spawn((
                WorldRenderRoot,
                Transform::from_xyz(315., 12., 12.),
                GlobalTransform::default(),
            ))
            .id();
        let editor = app.world_mut().spawn(Transform::from_xyz(1., 2., 3.)).id();
        let key = PageKey {
            space,
            cell: CellCoord { x: 19, z: -10 },
            domain: PageDomain::StaticObjects,
            lod: 0,
        };
        let object = app
            .world_mut()
            .spawn((StreamedPageEntity(key), Transform::from_xyz(310., 0., 8.)))
            .id();
        let pending = PageKey {
            domain: PageDomain::TerrainRender,
            ..key
        };
        {
            let mut stream = app.world_mut().resource_mut::<SourceResidency>();
            stream.pages.insert(
                key,
                PageState::Resident(PageAttachment {
                    entities: vec![object],
                    ..default()
                }),
            );
            stream
                .pages
                .insert(pending, PageState::Loading { request_id: 41 });
        }
        app.update();
        let w = app.world();
        assert_eq!(
            w.resource::<WorldOrigin>().cell,
            CellCoord { x: 19, z: -10 }
        );
        assert_eq!(
            w.get::<Transform>(actor).unwrap().translation,
            Vec3::new(17., 7., 2.)
        );
        assert_eq!(
            w.get::<MoveIntent>(actor).unwrap().destination(),
            Some(Vec3::new(33., 7., -40.))
        );
        assert_eq!(
            w.get::<GlobalTransform>(camera).unwrap().translation(),
            Vec3::new(27., 12., 12.)
        );
        assert_eq!(
            w.get::<Transform>(child).unwrap().translation,
            Vec3::new(0., 2., 0.)
        );
        assert_eq!(
            w.get::<Transform>(editor).unwrap().translation,
            Vec3::new(1., 2., 3.)
        );
        assert_eq!(
            w.get::<Transform>(object).unwrap().translation,
            Vec3::new(22., 0., 8.)
        );
        let stream = w.resource::<WorldStream>();
        assert_eq!(stream.index_revision, 7);
        let stream = w.resource::<SourceResidency>();
        assert!(matches!(
            stream.pages[&pending],
            PageState::Loading { request_id: 41 }
        ));
        let position = w.resource::<WorldViewpoint>().position;
        app.update();
        assert_eq!(app.world().resource::<WorldViewpoint>().position, position);
        assert_eq!(
            app.world().get::<Transform>(actor).unwrap().translation.x,
            17.
        );
        // Switching renderer mode must shift game roots even though source pages
        // are being recreated under a different representation contract.
        app.world_mut().resource_mut::<TerrainLodPreview>().enabled = false;
        app.update();
        assert_eq!(app.world().resource::<WorldOrigin>().cell, CellCoord::ZERO);
        assert_eq!(
            app.world().get::<Transform>(actor).unwrap().translation.x,
            625.
        );
        app.world_mut().resource_mut::<TerrainLodPreview>().enabled = true;
        app.update();
        assert_eq!(
            app.world().get::<Transform>(actor).unwrap().translation.x,
            17.
        );
        assert_eq!(app.world().resource::<WorldViewpoint>().position, position);
    }
}
