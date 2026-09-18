//! World-authoring workspace implementation.

use std::{collections::HashSet, f32::consts::FRAC_PI_4};

use bevy::{
    camera::visibility::RenderLayers,
    core_pipeline::prepass::DepthPrepass,
    gizmos::config::GizmoConfigStore,
    gizmos::transform_gizmo::{
        TransformGizmoAxis, TransformGizmoCamera, TransformGizmoFocus, TransformGizmoMeshMarker,
        TransformGizmoMode, TransformGizmoSettings, TransformGizmoSpace, TransformGizmoState,
    },
    gltf::GltfAssetLabel,
    input::{
        gestures::PinchGesture,
        mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    },
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
    render::view::Msaa,
    window::PrimaryWindow,
    world_serialization::WorldInstance,
};
use engine::{
    StreamedVisualObject, WorldCatalog, WorldEnvironmentCamera, WorldOrigin, WorldStreamingConfig,
    WorldViewCamera, WorldViewpoint,
};
use uuid::Uuid;
use world::{CellCoord, ObjectDefinitionId, StableObjectId, WorldPosition};
use world_db::{
    SourceObjectPaletteRecord, SourceObjectRecord, SourceObjectTransform, SourceObjectViewRecord,
};

use crate::domain_editing::DenseDomainWorkingSets;
use crate::editing::{EditorHistory, EditorObjectWorkingSet, EditorSelection};
use crate::preview::{EditorPreviewMode, PreviewModeState};
use crate::project_store::ProjectEditorStore;
use crate::publication::RuntimePublicationState;
use crate::saving::EditorSaveCoordinator;
use crate::shell::EditorInputCapture;
use crate::tools::{EditorToolRegistry, OBJECT_TOOL};
use crate::vegetation_authoring::VegetationAuthoringState;
use crate::workspaces::EditorWorkspace;

#[cfg(test)]
use crate::{
    shell::{
        AUTHORING_FRAME_RATE, EditorUiCamera, editor_ui_camera, editor_winit_settings,
        setup_editor_shell, sync_workspace_cameras,
    },
    workspaces::{
        AnimationWorkspaceCamera, EditorWorkspacesPlugin, animation::setup_animation_workspace,
    },
};
#[cfg(test)]
use bevy::{camera::CameraOutputMode, winit::UpdateMode};
#[cfg(test)]
use std::time::Duration;

const GRID_RADIUS_CELLS: i32 = 12;
const MIN_CAMERA_DISTANCE: f32 = 1.0;
const MAX_CAMERA_DISTANCE: f32 = 4_000.0;
const MIN_CAMERA_NAVIGATION_SCALE: f32 = 6.0;
const TRANSFORM_GIZMO_DEPTH_BIAS: f32 = 1_000_000.0;
const AUTHORING_PROXY_RADIUS_CELLS: u32 = 4;
const MAX_AUTHORING_PROXIES: usize = 256;

#[derive(Component, Debug)]
pub(crate) struct EditorCamera {
    focus: Option<WorldPosition>,
    pub(crate) distance: f32,
    yaw: f32,
    pitch: f32,
}

#[derive(Resource, Default)]
pub(crate) struct EditorCameraDrag {
    orbiting: bool,
    panning: bool,
    dollying: bool,
}

#[derive(Resource, Default)]
pub(crate) struct EditorCameraFocusRequest(pub(crate) Option<WorldPosition>);

#[derive(Resource, Default)]
pub(crate) struct EditorObjectPalette {
    pub(crate) selected: Option<ObjectDefinitionId>,
}

#[derive(Component, Debug)]
pub(crate) struct PromotedEditorObject {
    id: StableObjectId,
    source_revision: i64,
}

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct EditorHiddenCookedVisual(Visibility);

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

#[derive(Default, Reflect, GizmoConfigGroup)]
pub(crate) struct EditorOverlayGizmos;

pub(crate) fn setup_world_workspace(mut commands: Commands) {
    let controller = EditorCamera {
        focus: None,
        distance: 48.0,
        yaw: FRAC_PI_4,
        pitch: 0.58,
    };
    commands.spawn((
        Camera3d::default(),
        WorldEnvironmentCamera::default(),
        Msaa::Off,
        DepthPrepass,
        Transform::from_xyz(24.0, 26.0, 24.0).looking_at(Vec3::ZERO, Vec3::Y),
        controller,
        WorldViewCamera,
        TransformGizmoCamera,
        Name::new("Editor world camera"),
    ));
}

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

pub(crate) fn suspend_world_workspace_interactions(
    mut camera_drag: ResMut<EditorCameraDrag>,
    mut gizmo_transaction: ResMut<GizmoEditTransaction>,
) {
    *camera_drag = EditorCameraDrag::default();
    *gizmo_transaction = GizmoEditTransaction::default();
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
pub(crate) fn update_editor_camera(
    time: Res<Time>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    mut pinch_gestures: MessageReader<PinchGesture>,
    capture: Res<EditorInputCapture>,
    streaming_config: Res<WorldStreamingConfig>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    mut viewpoint: ResMut<WorldViewpoint>,
    mut drag: ResMut<EditorCameraDrag>,
    mut focus_request: ResMut<EditorCameraFocusRequest>,
    mut camera: Single<(&mut EditorCamera, &mut Transform), With<WorldViewCamera>>,
) {
    if let Some(requested_focus) = focus_request.0.take() {
        camera.0.focus = Some(requested_focus);
    }
    let incoming = viewpoint.position();
    if camera.0.focus.is_none()
        || incoming.is_some_and(|position| {
            camera
                .0
                .focus
                .is_some_and(|focus| focus.space != position.space)
        })
    {
        camera.0.focus = incoming;
    }
    let Some(mut focus) = camera.0.focus else {
        return;
    };
    let Some(space) = catalog.world_space(focus.space) else {
        return;
    };

    if mouse_buttons.just_pressed(MouseButton::Right) && !capture.wants_pointer {
        drag.orbiting = !shift_pressed(&keys);
        drag.panning = shift_pressed(&keys);
        drag.dollying = false;
    }
    if mouse_buttons.just_pressed(MouseButton::Middle) && !capture.wants_pointer {
        drag.dollying = control_pressed(&keys);
        drag.panning = !drag.dollying && shift_pressed(&keys);
        drag.orbiting = !drag.dollying && !drag.panning;
    }
    if mouse_buttons.just_released(MouseButton::Right) {
        drag.orbiting = false;
        if !mouse_buttons.pressed(MouseButton::Middle) {
            drag.panning = false;
            drag.dollying = false;
        }
    }
    if mouse_buttons.just_released(MouseButton::Middle)
        && !mouse_buttons.pressed(MouseButton::Right)
    {
        drag.orbiting = false;
        drag.panning = false;
        drag.dollying = false;
    }

    if drag.orbiting {
        camera.0.yaw -= mouse_motion.delta.x * 0.006;
        camera.0.pitch = (camera.0.pitch + mouse_motion.delta.y * 0.006).clamp(0.08, 1.48);
    }
    if drag.panning {
        let scale = camera_navigation_scale(camera.0.distance) * 0.0018;
        let delta = *camera.1.right() * (-mouse_motion.delta.x * scale)
            + *camera.1.up() * (mouse_motion.delta.y * scale);
        focus = focus.translated(delta.to_array(), space.cell_size);
    }
    if drag.dollying {
        let forward = *camera.1.forward();
        zoom_editor_camera(
            &mut camera.0,
            &mut focus,
            forward,
            -mouse_motion.delta.y * 0.012,
            space.cell_size,
        );
    }

    if !capture.wants_pointer {
        match mouse_scroll.unit {
            MouseScrollUnit::Line if mouse_scroll.delta.y != 0.0 => {
                let forward = *camera.1.forward();
                zoom_editor_camera(
                    &mut camera.0,
                    &mut focus,
                    forward,
                    mouse_scroll.delta.y * 0.12,
                    space.cell_size,
                );
            }
            MouseScrollUnit::Pixel if mouse_scroll.delta != Vec2::ZERO => {
                if shift_pressed(&keys) {
                    camera.0.yaw -= mouse_scroll.delta.x * 0.004;
                    camera.0.pitch =
                        (camera.0.pitch + mouse_scroll.delta.y * 0.004).clamp(0.08, 1.48);
                } else if control_pressed(&keys) {
                    let forward = *camera.1.forward();
                    zoom_editor_camera(
                        &mut camera.0,
                        &mut focus,
                        forward,
                        mouse_scroll.delta.y / 80.0,
                        space.cell_size,
                    );
                } else {
                    let scale = camera_navigation_scale(camera.0.distance) * 0.0018;
                    let delta = *camera.1.right() * (-mouse_scroll.delta.x * scale)
                        + *camera.1.up() * (mouse_scroll.delta.y * scale);
                    focus = focus.translated(delta.to_array(), space.cell_size);
                }
            }
            _ => {}
        }
        let pinch: f32 = pinch_gestures.read().map(|gesture| gesture.0).sum();
        if pinch != 0.0 {
            let forward = *camera.1.forward();
            zoom_editor_camera(
                &mut camera.0,
                &mut focus,
                forward,
                pinch * 1.6,
                space.cell_size,
            );
        }
    } else {
        pinch_gestures.clear();
    }

    if mouse_buttons.pressed(MouseButton::Right) && !capture.wants_keyboard {
        let mut movement = Vec3::ZERO;
        if keys.pressed(KeyCode::KeyW) {
            movement.z -= 1.0;
        }
        if keys.pressed(KeyCode::KeyS) {
            movement.z += 1.0;
        }
        if keys.pressed(KeyCode::KeyA) {
            movement.x -= 1.0;
        }
        if keys.pressed(KeyCode::KeyD) {
            movement.x += 1.0;
        }
        if keys.pressed(KeyCode::KeyE) {
            movement.y += 1.0;
        }
        if keys.pressed(KeyCode::KeyQ) {
            movement.y -= 1.0;
        }
        if movement != Vec3::ZERO {
            let speed = camera_navigation_scale(camera.0.distance) * time.delta_secs() * 0.8;
            let delta = (*camera.1.right() * movement.x
                + Vec3::Y * movement.y
                + *camera.1.forward() * movement.z)
                * speed;
            focus = focus.translated(delta.to_array(), space.cell_size);
        }
    }

    camera.0.focus = Some(focus);
    viewpoint.set(focus);
    let render_origin = anticipated_render_origin(
        origin.cell(),
        focus.cell,
        streaming_config.floating_origin_threshold_cells(),
    );
    *camera.1 = editor_camera_transform(&camera.0, render_origin, space.cell_size);
}

pub(crate) fn reconcile_editor_selection(
    project: Res<ProjectEditorStore>,
    mut objects: ResMut<EditorObjectWorkingSet>,
) {
    let tracked = project
        .objects()
        .iter()
        .filter(|fresh| objects.tracks(fresh.object.id))
        .cloned()
        .collect::<Vec<_>>();
    for fresh in tracked {
        objects.reconcile_source(&fresh);
    }
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
    presets: Res<super::presets::PresetAuthoringState>,
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_promoted_editor_object(
    mut commands: Commands,
    selection: Res<EditorSelection>,
    objects: Res<EditorObjectWorkingSet>,
    asset_server: Res<AssetServer>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    viewpoint: Res<WorldViewpoint>,
    gizmo: Res<TransformGizmoState>,
    mut proxies: Query<(
        Entity,
        &mut PromotedEditorObject,
        &mut Transform,
        Option<&TransformGizmoFocus>,
    )>,
) {
    let selected_id = selection.selected_id();
    let selected_ids = selection.selected_ids();
    let mut desired = viewpoint.position().map_or_else(Vec::new, |viewpoint| {
        let mut desired = objects
            .desired_proxy_views(selected_ids)
            .into_iter()
            .filter(|record| {
                record.object.space == viewpoint.space
                    && record.object.owner_cell.chebyshev_distance(viewpoint.cell)
                        <= AUTHORING_PROXY_RADIUS_CELLS
            })
            .collect::<Vec<_>>();
        desired.sort_by_key(|record| {
            (
                selected_id != Some(record.object.id),
                record.object.owner_cell.chebyshev_distance(viewpoint.cell),
            )
        });
        desired
    });
    desired.truncate(MAX_AUTHORING_PROXIES);

    let mut retained = HashSet::new();
    for (entity, mut proxy, mut current_transform, focused) in &mut proxies {
        if let Some(record) = desired.iter().find(|record| record.object.id == proxy.id)
            && retained.insert(proxy.id)
        {
            proxy.source_revision = record.object.source_revision;
            if !(gizmo.active && selected_id == Some(proxy.id))
                && let Some(space) = catalog.world_space(record.object.space)
            {
                *current_transform =
                    source_object_transform(record, origin.cell(), space.cell_size);
            }
            if selected_id == Some(proxy.id) && focused.is_none() {
                commands.entity(entity).insert(TransformGizmoFocus);
            } else if selected_id != Some(proxy.id) && focused.is_some() {
                commands.entity(entity).remove::<TransformGizmoFocus>();
            }
        } else {
            commands.entity(entity).despawn();
        }
    }

    for record in desired
        .iter()
        .filter(|record| !retained.contains(&record.object.id))
    {
        let Some(space) = catalog.world_space(record.object.space) else {
            continue;
        };
        let transform = source_object_transform(record, origin.cell(), space.cell_size);
        let mut proxy = commands.spawn((
            PromotedEditorObject {
                id: record.object.id,
                source_revision: record.object.source_revision,
            },
            transform,
            Visibility::Visible,
            Name::new(format!(
                "Authoring object {}",
                short_object_id(record.object.id)
            )),
        ));
        if selected_id == Some(record.object.id) {
            proxy.insert(TransformGizmoFocus);
        }
        if let Some(uri) = &record.visual_uri {
            proxy.insert(WorldAssetRoot(
                asset_server.load(GltfAssetLabel::Scene(0).from_asset(uri.clone())),
            ));
        }
    }
}

pub(crate) fn sync_cooked_visual_visibility(
    mut commands: Commands,
    objects: Res<EditorObjectWorkingSet>,
    proxies: Query<(
        &PromotedEditorObject,
        Option<&WorldAssetRoot>,
        Option<&WorldInstance>,
    )>,
    mut cooked_visuals: Query<(
        Entity,
        &StreamedVisualObject,
        &mut Visibility,
        Option<&EditorHiddenCookedVisual>,
    )>,
) {
    let ready_proxies = proxies
        .iter()
        .filter_map(|(proxy, visual, instance)| {
            (visual.is_some() && instance.is_some()).then_some(proxy.id)
        })
        .collect::<HashSet<_>>();

    for (entity, visual, mut visibility, hidden) in &mut cooked_visuals {
        if objects.is_deleted(visual.id) || ready_proxies.contains(&visual.id) {
            if hidden.is_none() {
                commands
                    .entity(entity)
                    .insert(EditorHiddenCookedVisual(*visibility));
            }
            *visibility = Visibility::Hidden;
        } else if let Some(hidden) = hidden {
            *visibility = hidden.0;
            commands.entity(entity).remove::<EditorHiddenCookedVisual>();
        }
    }
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

pub(crate) fn source_object_position(record: &SourceObjectViewRecord) -> WorldPosition {
    WorldPosition {
        space: record.object.space,
        cell: record.object.owner_cell,
        local: record.object.local_translation,
    }
}

fn source_object_transform(
    record: &SourceObjectViewRecord,
    origin_cell: CellCoord,
    cell_size: f32,
) -> Transform {
    Transform {
        translation: Vec3::from_array(
            source_object_position(record).relative_to(origin_cell, cell_size),
        ),
        rotation: Quat::from_rotation_y(record.object.yaw),
        scale: Vec3::splat(record.object.scale),
    }
}

fn anticipated_render_origin(
    current: CellCoord,
    focus: CellCoord,
    threshold: Option<u32>,
) -> CellCoord {
    if threshold.is_some_and(|threshold| current.chebyshev_distance(focus) > threshold) {
        focus
    } else {
        current
    }
}

fn camera_navigation_scale(distance: f32) -> f32 {
    distance.max(MIN_CAMERA_NAVIGATION_SCALE)
}

fn camera_zoom_step(distance: f32, amount: f32) -> (f32, f32) {
    let amount = amount.clamp(-4.0, 4.0);
    let scale = camera_navigation_scale(distance);
    let requested = distance + scale * ((-amount).exp() - 1.0);
    if requested < MIN_CAMERA_DISTANCE {
        (MIN_CAMERA_DISTANCE, MIN_CAMERA_DISTANCE - requested)
    } else {
        (requested.min(MAX_CAMERA_DISTANCE), 0.0)
    }
}

fn zoom_editor_camera(
    camera: &mut EditorCamera,
    focus: &mut WorldPosition,
    forward: Vec3,
    amount: f32,
    cell_size: f32,
) {
    let (distance, forward_travel) = camera_zoom_step(camera.distance, amount);
    camera.distance = distance;
    if forward_travel > 0.0 {
        *focus = focus.translated((forward * forward_travel).to_array(), cell_size);
    }
}

fn editor_camera_transform(
    camera: &EditorCamera,
    origin_cell: CellCoord,
    cell_size: f32,
) -> Transform {
    let focus = Vec3::from_array(
        camera
            .focus
            .expect("editor camera transform requires a logical focus")
            .relative_to(origin_cell, cell_size),
    );
    let horizontal = camera.distance * camera.pitch.cos();
    let offset = Vec3::new(
        horizontal * camera.yaw.sin(),
        camera.distance * camera.pitch.sin(),
        horizontal * camera.yaw.cos(),
    );
    Transform::from_translation(focus + offset).looking_at(focus, Vec3::Y)
}

pub(crate) fn draw_editor_grid(
    mut gizmos: Gizmos,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    viewpoint: Res<WorldViewpoint>,
) {
    let Some(position) = viewpoint.position() else {
        return;
    };
    let Some(space) = catalog.world_space(position.space) else {
        return;
    };
    let center_cell = position.cell;
    let minimum_x = relative_cell_offset(
        center_cell.x.saturating_sub(GRID_RADIUS_CELLS),
        origin.cell().x,
        space.cell_size,
    );
    let maximum_x = relative_cell_offset(
        center_cell.x.saturating_add(GRID_RADIUS_CELLS + 1),
        origin.cell().x,
        space.cell_size,
    );
    let minimum_z = relative_cell_offset(
        center_cell.z.saturating_sub(GRID_RADIUS_CELLS),
        origin.cell().z,
        space.cell_size,
    );
    let maximum_z = relative_cell_offset(
        center_cell.z.saturating_add(GRID_RADIUS_CELLS + 1),
        origin.cell().z,
        space.cell_size,
    );
    let y = 0.03;

    for offset in -GRID_RADIUS_CELLS..=GRID_RADIUS_CELLS + 1 {
        let cell_x = center_cell.x.saturating_add(offset);
        let x = relative_cell_offset(cell_x, origin.cell().x, space.cell_size);
        let color = grid_color(cell_x);
        gizmos.line(
            Vec3::new(x, y, minimum_z),
            Vec3::new(x, y, maximum_z),
            color,
        );

        let cell_z = center_cell.z.saturating_add(offset);
        let z = relative_cell_offset(cell_z, origin.cell().z, space.cell_size);
        let color = grid_color(cell_z);
        gizmos.line(
            Vec3::new(minimum_x, y, z),
            Vec3::new(maximum_x, y, z),
            color,
        );
    }
}

pub(crate) fn draw_promoted_editor_object(
    mut gizmos: Gizmos<EditorOverlayGizmos>,
    proxies: Query<(&PromotedEditorObject, &Transform)>,
    selection: Res<EditorSelection>,
    objects: Res<EditorObjectWorkingSet>,
) {
    for (proxy, transform) in &proxies {
        if !selection.contains(proxy.id) {
            continue;
        }
        let bounds = objects
            .current_view(proxy.id)
            .and_then(|selected| selected.visual_bounds)
            .unwrap_or([1.0, 1.0, 1.0]);
        let corners = visual_bounds_corners(transform, bounds);
        let color = if selection.selected_id() == Some(proxy.id) {
            Color::srgb(0.31, 0.67, 1.0)
        } else {
            Color::srgb(0.95, 0.72, 0.24)
        };
        for (start, end) in [
            (0, 1),
            (1, 3),
            (3, 2),
            (2, 0),
            (4, 5),
            (5, 7),
            (7, 6),
            (6, 4),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ] {
            gizmos.line(corners[start], corners[end], color);
        }
    }
}

fn visual_bounds_corners(transform: &Transform, bounds: [f32; 3]) -> [Vec3; 8] {
    let half = Vec3::from_array(bounds) * 0.5;
    std::array::from_fn(|index| {
        let local = Vec3::new(
            if index & 1 == 0 { -half.x } else { half.x },
            if index & 4 == 0 { 0.0 } else { bounds[1] },
            if index & 2 == 0 { -half.z } else { half.z },
        );
        transform.transform_point(local)
    })
}

pub(crate) fn draw_source_object_handles(
    mut gizmos: Gizmos,
    project: Res<ProjectEditorStore>,
    selection: Res<EditorSelection>,
    objects: Res<EditorObjectWorkingSet>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    viewpoint: Res<WorldViewpoint>,
) {
    if project.desired_window() != project.loaded_window() {
        return;
    }
    let Some(position) = viewpoint.position() else {
        return;
    };
    let Some(space) = catalog.world_space(position.space) else {
        return;
    };
    let color = Color::srgba(0.25, 0.78, 0.92, 0.8);
    for record in project.objects().iter().filter_map(|record| {
        objects.resolve_source_view(record).filter(|record| {
            record.object.space == position.space && !selection.contains(record.object.id)
        })
    }) {
        let center = Vec3::from_array(
            source_object_position(&record).relative_to(origin.cell(), space.cell_size),
        );
        gizmos.line(center - Vec3::Y * 0.25, center + Vec3::Y * 0.75, color);
        gizmos.line(center - Vec3::X * 0.25, center + Vec3::X * 0.25, color);
    }
}

fn relative_cell_offset(cell: i32, origin: i32, cell_size: f32) -> f32 {
    (i64::from(cell) - i64::from(origin)) as f32 * cell_size
}

fn grid_color(coordinate: i32) -> Color {
    if coordinate == 0 {
        Color::srgba(0.75, 0.42, 0.18, 0.8)
    } else if coordinate.rem_euclid(4) == 0 {
        Color::srgba(0.42, 0.46, 0.54, 0.72)
    } else {
        Color::srgba(0.28, 0.31, 0.36, 0.55)
    }
}

pub(crate) fn create_palette_object(
    palette: SourceObjectPaletteRecord,
    position: WorldPosition,
    selection: &mut EditorSelection,
    objects: &mut EditorObjectWorkingSet,
    history: &mut EditorHistory,
) -> bool {
    let id = StableObjectId(*Uuid::new_v4().as_bytes());
    let record = SourceObjectViewRecord {
        object: SourceObjectRecord {
            id,
            space: position.space,
            owner_cell: position.cell,
            definition: palette.definition.id,
            local_translation: position.local,
            yaw: 0.0,
            scale: 1.0,
            source_revision: 0,
        },
        definition: palette.definition,
        visual_uri: palette.visual_uri,
        visual_bounds: palette.visual_bounds,
    };
    history.create(objects, record) && selection.select_id(id, objects)
}

fn short_object_id(id: StableObjectId) -> String {
    id.0[..4].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn shift_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight)
}

fn control_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight)
}

fn command_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight) || control_pressed(keys)
}

fn multi_select_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    shift_pressed(keys) || command_pressed(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn anticipated_origin_keeps_remote_camera_coordinates_small() {
        let far = CellCoord {
            x: 1_000_000,
            z: -1_000_000,
        };
        assert_eq!(
            anticipated_render_origin(CellCoord::ZERO, far, Some(8)),
            far
        );
    }

    #[test]
    fn editor_frame_pacing_is_reactive_and_input_driven() {
        let UpdateMode::Reactive {
            wait,
            react_to_device_events,
            react_to_user_events,
            react_to_window_events,
            ..
        } = editor_winit_settings().focused_mode
        else {
            panic!("the editor should use reactive frame pacing");
        };
        assert_eq!(wait, Duration::from_secs_f64(1.0 / AUTHORING_FRAME_RATE));
        assert!(react_to_device_events);
        assert!(react_to_user_events);
        assert!(react_to_window_events);
    }

    #[test]
    fn workspace_switch_activates_only_its_viewport_camera() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_plugins(EditorWorkspacesPlugin)
            .add_systems(Update, sync_workspace_cameras);
        let world_camera = app
            .world_mut()
            .spawn((Camera::default(), WorldViewCamera))
            .id();
        let fallback_camera = app
            .world_mut()
            .spawn((Camera::default(), AnimationWorkspaceCamera))
            .id();

        app.update();
        assert!(app.world().get::<Camera>(world_camera).unwrap().is_active);
        assert!(
            !app.world()
                .get::<Camera>(fallback_camera)
                .unwrap()
                .is_active
        );

        app.world_mut()
            .resource_mut::<NextState<EditorWorkspace>>()
            .set(EditorWorkspace::Animation);
        app.update();
        assert!(!app.world().get::<Camera>(world_camera).unwrap().is_active);
        assert!(
            app.world()
                .get::<Camera>(fallback_camera)
                .unwrap()
                .is_active
        );
    }

    #[test]
    fn editor_ui_camera_clears_only_its_transparent_intermediate_target() {
        let camera = editor_ui_camera();
        assert!(matches!(
            camera.clear_color,
            ClearColorConfig::Custom(color) if color == Color::NONE
        ));
        assert!(matches!(
            camera.output_mode,
            CameraOutputMode::Write {
                blend_state: Some(_),
                clear_color: ClearColorConfig::None,
            }
        ));
    }

    #[test]
    fn workspace_camera_stack_uses_one_msaa_mode() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_plugins(EditorWorkspacesPlugin)
            .add_systems(
                Startup,
                (
                    setup_editor_shell,
                    setup_world_workspace,
                    setup_animation_workspace,
                ),
            );
        app.update();

        let world = app.world_mut();
        let world_msaa = *world
            .query_filtered::<&Msaa, With<WorldViewCamera>>()
            .single(world)
            .unwrap();
        let fallback_msaa = *world
            .query_filtered::<&Msaa, With<AnimationWorkspaceCamera>>()
            .single(world)
            .unwrap();
        let ui_msaa = *world
            .query_filtered::<&Msaa, With<EditorUiCamera>>()
            .single(world)
            .unwrap();
        assert_eq!(world_msaa, Msaa::Off);
        assert_eq!(fallback_msaa, world_msaa);
        assert_eq!(ui_msaa, world_msaa);
    }

    #[test]
    fn close_camera_zoom_advances_instead_of_stalling_at_the_clamp() {
        let (distance, forward_travel) = camera_zoom_step(MIN_CAMERA_DISTANCE, 0.12);
        assert_eq!(distance, MIN_CAMERA_DISTANCE);
        assert!(forward_travel > 0.5);

        let (distance, forward_travel) = camera_zoom_step(MIN_CAMERA_DISTANCE, -0.12);
        assert!(distance > MIN_CAMERA_DISTANCE);
        assert_eq!(forward_travel, 0.0);
    }

    #[test]
    fn navigation_sensitivity_has_a_close_range_floor() {
        assert_eq!(camera_navigation_scale(1.0), MIN_CAMERA_NAVIGATION_SCALE);
        assert_eq!(camera_navigation_scale(20.0), 20.0);
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
    fn palette_placement_creates_and_selects_a_dirty_stable_object() {
        let definition = ObjectDefinitionId([4; 16]);
        let palette = SourceObjectPaletteRecord {
            definition: world_db::SourceObjectDefinitionRecord {
                id: definition,
                key: "test/tree".into(),
                display_name: "Test tree".into(),
                visual_asset: None,
                activation: world::ObjectActivationPolicy::RenderOnly,
            },
            visual_uri: Some("test/tree.gltf".into()),
            visual_bounds: Some([2.0, 8.0, 2.0]),
        };
        let position = WorldPosition {
            space: world::WorldSpaceId(3),
            cell: CellCoord { x: 7, z: -2 },
            local: [4.0, 0.0, 6.0],
        };
        let mut selection = EditorSelection::default();
        let mut objects = EditorObjectWorkingSet::default();
        let mut history = EditorHistory::default();
        assert!(create_palette_object(
            palette,
            position,
            &mut selection,
            &mut objects,
            &mut history,
        ));
        let selected = selection.selected_id().unwrap();
        let created = objects.current_view(selected).unwrap();
        assert_eq!(created.object.definition, definition);
        assert_eq!(created.object.owner_cell, position.cell);
        assert!(objects.dirty(selected));
        assert_eq!(history.undo_len(), 1);
    }

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
