//! World shortcuts, source selection and interaction cancellation.
use crate::{
    domain_editing::DenseDomainWorkingSets,
    editing::{EditorHistory, EditorObjectWorkingSet, EditorSelection},
    project_store::ProjectEditorStore,
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::EditorInputCapture,
    tools::{EditorToolRegistry, OBJECT_TOOL},
    vegetation_authoring::VegetationAuthoringState,
    workspaces::{
        EditorWorkspace,
        world::{
            camera::EditorCameraDrag,
            gizmo::GizmoEditTransaction,
            objects::{
                AUTHORING_PROXY_RADIUS_CELLS, PromotedEditorObject, source_object_transform,
                visual_bounds_corners,
            },
        },
    },
};
use bevy::{
    gizmos::transform_gizmo::{TransformGizmoMode, TransformGizmoSettings, TransformGizmoState},
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
    window::PrimaryWindow,
};
use engine::{StreamedVisualObject, WorldCatalog, WorldOrigin, WorldViewCamera, WorldViewpoint};
use std::collections::HashSet;
use world::StableObjectId;

pub(crate) fn suspend_world_workspace_interactions(
    mut camera_drag: ResMut<EditorCameraDrag>,
    mut gizmo_transaction: ResMut<GizmoEditTransaction>,
) {
    *camera_drag = EditorCameraDrag::default();
    *gizmo_transaction = GizmoEditTransaction::default();
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_editor_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    capture: Res<EditorInputCapture>,
    gizmo: Res<TransformGizmoState>,
    mut gizmo_settings: ResMut<TransformGizmoSettings>,
    mut selection: ResMut<EditorSelection>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut dense_domains: ResMut<DenseDomainWorkingSets>,
    vegetation: Res<VegetationAuthoringState>,
    mut history: ResMut<EditorHistory>,
    publication: Res<RuntimePublicationState>,
    mut save: ResMut<EditorSaveCoordinator>,
    tools: Res<EditorToolRegistry>,
    paint: Res<crate::environment_paint::EnvironmentPaintState>,
    presets: Res<crate::workspaces::presets::PresetAuthoringState>,
) {
    let object_tool_active = tools
        .active(EditorWorkspace::World)
        .is_some_and(|tool| tool.id == OBJECT_TOOL.id);
    if capture.wants_keyboard
        || paint.has_unapplied_changes()
        || presets.dirty()
        || (object_tool_active && gizmo.active)
        || objects.saving()
        || dense_domains.saving()
        || dense_domains.gesture_active
        || dense_domains.atmospheres.gesture.is_some()
        || publication.active()
        || vegetation.saving()
        || save.active()
    {
        return;
    }

    if object_tool_active && keys.just_pressed(KeyCode::Digit1) {
        gizmo_settings.mode = TransformGizmoMode::Translate;
    } else if object_tool_active && keys.just_pressed(KeyCode::Digit2) {
        gizmo_settings.mode = TransformGizmoMode::Rotate;
    } else if object_tool_active && keys.just_pressed(KeyCode::Digit3) {
        gizmo_settings.mode = TransformGizmoMode::Scale;
    }

    if command_pressed(&keys) && keys.just_pressed(KeyCode::KeyZ) {
        if shift_pressed(&keys) {
            history.redo(&mut objects, &mut dense_domains);
        } else {
            history.undo(&mut objects, &mut dense_domains);
        }
    } else if control_pressed(&keys) && keys.just_pressed(KeyCode::KeyY) {
        history.redo(&mut objects, &mut dense_domains);
    } else if command_pressed(&keys) && keys.just_pressed(KeyCode::KeyS) && !publication.active() {
        if objects.dirty_count() + dense_domains.dirty_count() + vegetation.dirty_count() > 0 {
            save.request_save();
        }
    } else if (keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace))
        && object_tool_active
        && !selection.selected_ids().is_empty()
        && history.delete_many(&mut objects, selection.selected_ids())
    {
        selection.clear(&objects);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn pick_source_object(
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<WorldViewCamera>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    capture: Res<EditorInputCapture>,
    project: Res<ProjectEditorStore>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    viewpoint: Res<WorldViewpoint>,
    gizmo: Res<TransformGizmoState>,
    mut mesh_ray_cast: MeshRayCast,
    parents: Query<&ChildOf>,
    cooked_roots: Query<&StreamedVisualObject>,
    promoted_roots: Query<&PromotedEditorObject>,
    mut selection: ResMut<EditorSelection>,
    mut objects: ResMut<EditorObjectWorkingSet>,
) {
    if !mouse_buttons.just_pressed(MouseButton::Left)
        || capture.wants_pointer
        || !selection.can_change_selection(&objects)
        || gizmo.active
        || gizmo.hovered_axis.is_some()
    {
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Some(position) = viewpoint.position() else {
        return;
    };
    let Some(space) = catalog.world_space(position.space) else {
        return;
    };
    let mut candidates = project
        .objects()
        .iter()
        .filter_map(|record| objects.resolve_source_view(record))
        .collect::<Vec<_>>();
    let source_ids = candidates
        .iter()
        .map(|record| record.object.id)
        .collect::<HashSet<_>>();
    candidates.extend(objects.current_views().into_iter().filter(|record| {
        !source_ids.contains(&record.object.id)
            && record.object.space == position.space
            && record.object.owner_cell.chebyshev_distance(position.cell)
                <= AUTHORING_PROXY_RADIUS_CELLS
    }));
    let mesh_picked = camera
        .0
        .viewport_to_world(camera.1, cursor)
        .ok()
        .and_then(|ray| {
            mesh_ray_cast
                .cast_ray(ray, &MeshRayCastSettings::default().never_early_exit())
                .iter()
                .find_map(|(entity, _)| {
                    stable_object_ancestor(*entity, &parents, &cooked_roots, &promoted_roots)
                })
        });
    let mesh_record = mesh_picked.and_then(|picked| {
        candidates
            .iter()
            .find(|record| record.object.id == picked)
            .cloned()
    });
    let bounds_record = || {
        candidates
            .into_iter()
            .filter(|record| record.object.space == position.space)
            .filter_map(|record| {
                let transform = source_object_transform(&record, origin.cell(), space.cell_size);
                let center = transform.translation;
                if let Some(bounds) = record.visual_bounds {
                    let mut minimum = Vec2::splat(f32::INFINITY);
                    let mut maximum = Vec2::splat(f32::NEG_INFINITY);
                    for corner in visual_bounds_corners(&transform, bounds) {
                        let screen = camera.0.world_to_viewport(camera.1, corner).ok()?;
                        minimum = minimum.min(screen);
                        maximum = maximum.max(screen);
                    }
                    let padding = Vec2::splat(6.0);
                    if cursor.cmpge(minimum - padding).all()
                        && cursor.cmple(maximum + padding).all()
                    {
                        Some((center.distance_squared(camera.1.translation()), record))
                    } else {
                        None
                    }
                } else {
                    let screen = camera.0.world_to_viewport(camera.1, center).ok()?;
                    let distance = screen.distance(cursor);
                    (distance <= 18.0)
                        .then_some((center.distance_squared(camera.1.translation()), record))
                }
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, record)| record)
    };
    if let Some(record) = mesh_record.or_else(bounds_record) {
        if multi_select_pressed(&keys) {
            selection.toggle(record, &mut objects);
        } else {
            selection.select(record, &mut objects);
        }
    }
}

fn stable_object_ancestor(
    mut entity: Entity,
    parents: &Query<&ChildOf>,
    cooked_roots: &Query<&StreamedVisualObject>,
    promoted_roots: &Query<&PromotedEditorObject>,
) -> Option<StableObjectId> {
    for _ in 0..64 {
        if let Ok(root) = promoted_roots.get(entity) {
            return Some(root.id);
        }
        if let Ok(root) = cooked_roots.get(entity) {
            return Some(root.id);
        }
        entity = parents.get(entity).ok()?.parent();
    }
    None
}

pub(super) fn shift_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight)
}

pub(super) fn control_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight)
}

pub(super) fn command_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight) || control_pressed(keys)
}

pub(super) fn multi_select_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    shift_pressed(keys) || command_pressed(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::world::objects::PromotedEditorObject;

    use engine::StreamedVisualObject;
    use world::StableObjectId;

    #[test]
    fn mesh_descendants_resolve_to_their_stable_streamed_object() {
        use bevy::ecs::system::SystemState;

        let mut world = World::new();
        let id = StableObjectId([9; 16]);
        let root = world.spawn(StreamedVisualObject { id }).id();
        let scene_node = world.spawn(ChildOf(root)).id();
        let mesh = world.spawn(ChildOf(scene_node)).id();
        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&StreamedVisualObject>,
            Query<&PromotedEditorObject>,
        )> = SystemState::new(&mut world);
        let (parents, cooked, promoted) = state.get(&world).unwrap();
        assert_eq!(
            stable_object_ancestor(mesh, &parents, &cooked, &promoted),
            Some(id)
        );
    }
}
