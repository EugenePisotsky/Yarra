//! Transform-gizmo rendering and grouped undoable edit transactions.
use crate::{
    editing::{EditorHistory, EditorObjectWorkingSet, EditorSelection},
    preview::{EditorPreviewMode, PreviewModeState},
    tools::{EditorToolRegistry, OBJECT_TOOL},
    workspaces::{
        EditorWorkspace,
        world::{objects::PromotedEditorObject, overlay::EditorOverlayGizmos},
    },
};
use bevy::{
    camera::visibility::RenderLayers,
    gizmos::{
        config::GizmoConfigStore,
        transform_gizmo::{
            TransformGizmoAxis, TransformGizmoFocus, TransformGizmoMeshMarker, TransformGizmoMode,
            TransformGizmoSettings, TransformGizmoSpace, TransformGizmoState,
        },
    },
    prelude::*,
};
use engine::{WorldCatalog, WorldOrigin, WorldViewCamera};
use world::{StableObjectId, WorldPosition};
use world_db::SourceObjectTransform;

const TRANSFORM_GIZMO_DEPTH_BIAS: f32 = 1_000_000.0;

#[derive(Resource, Default)]
pub(crate) struct GizmoEditTransaction {
    was_active: bool,
    before: Vec<(StableObjectId, SourceObjectTransform)>,
}

#[derive(Default)]
pub(crate) struct BuiltinGizmoRenderSetup {
    overlay_removed: bool,
    camera_layers_configured: bool,
    materials_configured: bool,
}
type BuiltinGizmoOverlayCameras<'world, 'state> = Query<
    'world,
    'state,
    (Entity, &'static Camera, &'static RenderLayers),
    (With<Camera3d>, Without<WorldViewCamera>),
>;
type BuiltinGizmoMeshes<'world, 'state> = Query<
    'world,
    'state,
    (
        &'static MeshMaterial3d<StandardMaterial>,
        &'static RenderLayers,
    ),
    With<TransformGizmoMeshMarker>,
>;
pub(crate) fn configure_transform_gizmo(
    mut settings: ResMut<TransformGizmoSettings>,
    mut gizmo_configs: ResMut<GizmoConfigStore>,
) {
    settings.mode = TransformGizmoMode::Translate;
    settings.space = TransformGizmoSpace::World;
    settings.screen_scale_factor = 0.12;
    settings.axis_hit_distance = 10.0;
    // Cursor confinement falls back to locking on macOS, which breaks ray-based gizmo dragging.
    settings.confine_cursor = false;
    let (overlay, _) = gizmo_configs.config_mut::<EditorOverlayGizmos>();
    overlay.depth_bias = -1.0;
    overlay.line.width = 3.0;
}

pub(crate) fn editor_gizmo_enabled(
    workspace: Res<State<EditorWorkspace>>,
    preview: Res<PreviewModeState>,
    tools: Res<EditorToolRegistry>,
    selection: Res<EditorSelection>,
    objects: Res<EditorObjectWorkingSet>,
) -> bool {
    *workspace.get() == EditorWorkspace::World
        && preview.active() == Some(EditorPreviewMode::Authoring)
        && tools
            .active(EditorWorkspace::World)
            .is_some_and(|tool| tool.id == OBJECT_TOOL.id)
        && selection.can_edit(&objects)
}

pub(crate) fn prepare_builtin_transform_gizmo_renderer(
    mut commands: Commands,
    overlay_cameras: BuiltinGizmoOverlayCameras,
    gizmo_meshes: BuiltinGizmoMeshes,
    world_camera: Single<Entity, With<WorldViewCamera>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut setup: Local<BuiltinGizmoRenderSetup>,
) {
    let Some((_, gizmo_layers)) = gizmo_meshes.iter().next() else {
        return;
    };

    if !setup.camera_layers_configured {
        commands
            .entity(*world_camera)
            .insert(editor_world_render_layers(gizmo_layers));
        setup.camera_layers_configured = true;
    }

    if !setup.overlay_removed {
        for (entity, camera, render_layers) in &overlay_cameras {
            if camera.order == 1 && render_layers == gizmo_layers {
                commands.entity(entity).despawn();
                setup.overlay_removed = true;
            }
        }
    }

    if !setup.materials_configured && !gizmo_meshes.is_empty() {
        let mut all_materials_available = true;
        for (handle, _) in &gizmo_meshes {
            let Some(mut material) = materials.get_mut(handle) else {
                all_materials_available = false;
                continue;
            };
            // The stock overlay normally ignores scene depth. Once routed through the
            // clearing world camera, a strong positive bias preserves that behavior.
            material.depth_bias = TRANSFORM_GIZMO_DEPTH_BIAS;
        }
        setup.materials_configured = all_materials_available;
    }
}

fn editor_world_render_layers(gizmo_layers: &RenderLayers) -> RenderLayers {
    gizmo_layers
        .iter()
        .fold(RenderLayers::layer(0), RenderLayers::with)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_promoted_gizmo(
    gizmo: Res<TransformGizmoState>,
    settings: Res<TransformGizmoSettings>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    mut proxies: Query<(&PromotedEditorObject, &mut Transform), With<TransformGizmoFocus>>,
    mut transaction: ResMut<GizmoEditTransaction>,
    selection: Res<EditorSelection>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut history: ResMut<EditorHistory>,
) {
    let Some((proxy, mut transform)) = proxies.iter_mut().next() else {
        transaction.was_active = false;
        transaction.before.clear();
        return;
    };
    let Some(_) = selection
        .selected_view(&objects)
        .filter(|selected| selected.object.id == proxy.id)
    else {
        transaction.was_active = false;
        transaction.before.clear();
        return;
    };

    if gizmo.active {
        if !transaction.was_active {
            transaction.before = selection
                .selected_ids()
                .iter()
                .filter_map(|object| {
                    objects
                        .current_view(*object)
                        .map(|view| (*object, SourceObjectTransform::from(&view.object)))
                })
                .collect();
        }
        let Some((_, active_before)) = transaction
            .before
            .iter()
            .find(|(object, _)| *object == proxy.id)
            .copied()
        else {
            return;
        };
        let Some(space) = catalog.world_space(active_before.space) else {
            return;
        };
        let mut active_preview = active_before;
        match settings.mode {
            TransformGizmoMode::Translate => {
                let origin_world = origin.cell().origin(space.cell_size);
                let position = WorldPosition::from_world(
                    active_before.space,
                    [
                        origin_world[0] + f64::from(transform.translation.x),
                        f64::from(transform.translation.y),
                        origin_world[1] + f64::from(transform.translation.z),
                    ],
                    space.cell_size,
                );
                active_preview.owner_cell = position.cell;
                active_preview.local_translation = position.local;
            }
            TransformGizmoMode::Rotate => {
                if gizmo.axis == Some(TransformGizmoAxis::Y) {
                    let (yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
                    active_preview.yaw = yaw.rem_euclid(std::f32::consts::TAU);
                }
            }
            TransformGizmoMode::Scale => {
                let scale = match gizmo.axis {
                    Some(TransformGizmoAxis::X) => transform.scale.x,
                    Some(TransformGizmoAxis::Y) => transform.scale.y,
                    Some(TransformGizmoAxis::Z) => transform.scale.z,
                    Some(TransformGizmoAxis::View) | None => transform.scale.x,
                };
                active_preview.scale = scale.max(0.001);
            }
        }
        let previews = group_gizmo_targets(
            settings.mode,
            proxy.id,
            active_preview,
            &transaction.before,
            space.cell_size,
        );
        objects.set_transforms(&previews);
        transform.rotation = Quat::from_rotation_y(active_preview.yaw);
        transform.scale = Vec3::splat(active_preview.scale);
    } else if transaction.was_active {
        history.record_previews(&objects, &transaction.before);
        transaction.before.clear();
    }
    transaction.was_active = gizmo.active;
}

fn group_gizmo_targets(
    mode: TransformGizmoMode,
    active: StableObjectId,
    active_target: SourceObjectTransform,
    before: &[(StableObjectId, SourceObjectTransform)],
    cell_size: f32,
) -> Vec<(StableObjectId, SourceObjectTransform)> {
    let Some((_, active_before)) = before.iter().find(|(object, _)| *object == active).copied()
    else {
        return Vec::new();
    };

    let translation_delta = if mode == TransformGizmoMode::Translate {
        let initial = WorldPosition {
            space: active_before.space,
            cell: active_before.owner_cell,
            local: active_before.local_translation,
        }
        .world(cell_size);
        let target = WorldPosition {
            space: active_target.space,
            cell: active_target.owner_cell,
            local: active_target.local_translation,
        }
        .world(cell_size);
        [
            target[0] - initial[0],
            target[1] - initial[1],
            target[2] - initial[2],
        ]
    } else {
        [0.0; 3]
    };
    let yaw_delta = active_target.yaw - active_before.yaw;
    let scale_factor = if active_before.scale.is_finite() && active_before.scale > 0.0 {
        active_target.scale / active_before.scale
    } else {
        1.0
    };

    before
        .iter()
        .filter(|(_, transform)| transform.space == active_before.space)
        .map(|(object, initial)| {
            if *object == active {
                return (*object, active_target);
            }
            let mut target = *initial;
            match mode {
                TransformGizmoMode::Translate => {
                    let initial_world = WorldPosition {
                        space: initial.space,
                        cell: initial.owner_cell,
                        local: initial.local_translation,
                    }
                    .world(cell_size);
                    let position = WorldPosition::from_world(
                        initial.space,
                        [
                            initial_world[0] + translation_delta[0],
                            initial_world[1] + translation_delta[1],
                            initial_world[2] + translation_delta[2],
                        ],
                        cell_size,
                    );
                    target.owner_cell = position.cell;
                    target.local_translation = position.local;
                }
                TransformGizmoMode::Rotate => {
                    target.yaw = (initial.yaw + yaw_delta).rem_euclid(std::f32::consts::TAU);
                }
                TransformGizmoMode::Scale => {
                    target.scale = (initial.scale * scale_factor).max(0.001);
                }
            }
            (*object, target)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        camera::visibility::RenderLayers,
        gizmos::transform_gizmo::{TransformGizmoMeshMarker, TransformGizmoMode},
    };
    use engine::WorldViewCamera;
    use world::{CellCoord, StableObjectId};
    use world_db::SourceObjectTransform;

    fn test_transform(
        space: world::WorldSpaceId,
        cell: CellCoord,
        local: [f32; 3],
        yaw: f32,
        scale: f32,
    ) -> SourceObjectTransform {
        SourceObjectTransform {
            space,
            owner_cell: cell,
            local_translation: local,
            yaw,
            scale,
        }
    }
    #[test]
    fn group_translation_preserves_offsets_across_cell_boundaries() {
        let space = world::WorldSpaceId(3);
        let other_space = world::WorldSpaceId(4);
        let active = StableObjectId([1; 16]);
        let companion = StableObjectId([2; 16]);
        let excluded = StableObjectId([3; 16]);
        let before = [
            (
                active,
                test_transform(space, CellCoord::ZERO, [31.0, 1.0, 8.0], 0.0, 1.0),
            ),
            (
                companion,
                test_transform(space, CellCoord::ZERO, [30.0, 0.0, 6.0], 0.5, 2.0),
            ),
            (
                excluded,
                test_transform(other_space, CellCoord::ZERO, [2.0, 0.0, 2.0], 0.0, 1.0),
            ),
        ];
        let active_target =
            test_transform(space, CellCoord { x: 1, z: 0 }, [3.0, 2.0, 8.0], 0.0, 1.0);

        let targets = group_gizmo_targets(
            TransformGizmoMode::Translate,
            active,
            active_target,
            &before,
            32.0,
        );
        assert_eq!(targets.len(), 2);
        assert!(!targets.iter().any(|(object, _)| *object == excluded));
        let companion_target = targets
            .iter()
            .find(|(object, _)| *object == companion)
            .unwrap()
            .1;
        assert_eq!(companion_target.owner_cell, CellCoord { x: 1, z: 0 });
        assert_eq!(companion_target.local_translation, [2.0, 1.0, 6.0]);
        assert_eq!(companion_target.yaw, 0.5);
        assert_eq!(companion_target.scale, 2.0);
    }
    #[test]
    fn group_rotation_applies_the_active_yaw_delta_in_place() {
        let space = world::WorldSpaceId(3);
        let active = StableObjectId([1; 16]);
        let companion = StableObjectId([2; 16]);
        let before = [
            (
                active,
                test_transform(space, CellCoord::ZERO, [1.0, 0.0, 1.0], 0.25, 1.0),
            ),
            (
                companion,
                test_transform(space, CellCoord::ZERO, [4.0, 0.0, 5.0], 1.0, 1.0),
            ),
        ];
        let active_target = test_transform(space, CellCoord::ZERO, [1.0, 0.0, 1.0], 0.75, 1.0);

        let targets = group_gizmo_targets(
            TransformGizmoMode::Rotate,
            active,
            active_target,
            &before,
            32.0,
        );
        let companion_target = targets
            .iter()
            .find(|(object, _)| *object == companion)
            .unwrap()
            .1;
        assert!((companion_target.yaw - 1.5).abs() < f32::EPSILON);
        assert_eq!(companion_target.local_translation, [4.0, 0.0, 5.0]);
    }
    #[test]
    fn group_scale_applies_the_active_uniform_factor_in_place() {
        let space = world::WorldSpaceId(3);
        let active = StableObjectId([1; 16]);
        let companion = StableObjectId([2; 16]);
        let before = [
            (
                active,
                test_transform(space, CellCoord::ZERO, [1.0, 0.0, 1.0], 0.0, 2.0),
            ),
            (
                companion,
                test_transform(space, CellCoord::ZERO, [4.0, 0.0, 5.0], 0.0, 4.0),
            ),
        ];
        let active_target = test_transform(space, CellCoord::ZERO, [1.0, 0.0, 1.0], 0.0, 3.0);

        let targets = group_gizmo_targets(
            TransformGizmoMode::Scale,
            active,
            active_target,
            &before,
            32.0,
        );
        let companion_target = targets
            .iter()
            .find(|(object, _)| *object == companion)
            .unwrap()
            .1;
        assert!((companion_target.scale - 6.0).abs() < f32::EPSILON);
        assert_eq!(companion_target.local_translation, [4.0, 0.0, 5.0]);
    }
    #[test]
    fn stock_gizmo_uses_the_clearing_world_camera() {
        use bevy::gizmos::transform_gizmo::{TransformGizmoAxis, TransformGizmoRoot};

        const DISCOVERED_GIZMO_LAYER: usize = 27;

        let mut app = App::new();
        app.insert_resource(Assets::<StandardMaterial>::default());
        app.add_systems(Update, prepare_builtin_transform_gizmo_renderer);

        let root = app.world_mut().spawn(TransformGizmoRoot).id();
        let material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        app.world_mut().spawn((
            TransformGizmoMeshMarker {
                axis: TransformGizmoAxis::X,
                mode: TransformGizmoMode::Translate,
            },
            MeshMaterial3d(material.clone()),
            RenderLayers::layer(DISCOVERED_GIZMO_LAYER),
        ));
        let overlay = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera {
                    order: 1,
                    ..default()
                },
                RenderLayers::layer(DISCOVERED_GIZMO_LAYER),
            ))
            .id();
        let auxiliary = app
            .world_mut()
            .spawn((Camera3d::default(), RenderLayers::default()))
            .id();
        let world_view = app
            .world_mut()
            .spawn((Camera3d::default(), WorldViewCamera))
            .id();

        app.update();

        assert!(app.world().get_entity(root).is_ok());
        assert!(app.world().get_entity(overlay).is_err());
        assert!(app.world().get_entity(auxiliary).is_ok());
        assert!(app.world().get_entity(world_view).is_ok());
        assert_eq!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&material)
                .expect("stock gizmo material should remain loaded")
                .depth_bias,
            TRANSFORM_GIZMO_DEPTH_BIAS
        );

        let layers = app
            .world()
            .get::<RenderLayers>(world_view)
            .expect("world camera should receive the discovered stock gizmo layer");
        assert!(layers.intersects(&RenderLayers::layer(0)));
        assert!(layers.intersects(&RenderLayers::layer(DISCOVERED_GIZMO_LAYER)));
    }
}
