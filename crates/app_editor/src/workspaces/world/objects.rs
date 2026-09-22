//! Authored object proxies, source transforms and cooked-visual visibility.
use crate::{
    editing::{EditorHistory, EditorObjectWorkingSet, EditorSelection},
    project_store::ProjectEditorStore,
};
use bevy::{
    gizmos::transform_gizmo::{TransformGizmoFocus, TransformGizmoState},
    gltf::GltfAssetLabel,
    prelude::*,
    world_serialization::WorldInstance,
};
use engine::{StreamedVisualObject, WorldCatalog, WorldOrigin, WorldViewpoint};
use std::collections::HashSet;
use uuid::Uuid;
use world::{CellCoord, ObjectDefinitionId, StableObjectId, WorldPosition};
use world_db::{SourceObjectPaletteRecord, SourceObjectRecord, SourceObjectViewRecord};

pub(super) const AUTHORING_PROXY_RADIUS_CELLS: u32 = 4;
const MAX_AUTHORING_PROXIES: usize = 256;

#[derive(Resource, Default)]
pub(crate) struct EditorObjectPalette {
    pub(crate) selected: Option<ObjectDefinitionId>,
}

#[derive(Component, Debug)]
pub(crate) struct PromotedEditorObject {
    pub(super) id: StableObjectId,
    source_revision: i64,
}

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct EditorHiddenCookedVisual(Visibility);
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

#[allow(clippy::type_complexity)]
pub(crate) fn sync_cooked_visual_visibility(
    mut commands: Commands,
    objects: Res<EditorObjectWorkingSet>,
    proxies: Query<(
        &PromotedEditorObject,
        Option<&WorldAssetRoot>,
        Option<&WorldInstance>,
    )>,
    mut cooked_visuals: Query<
        (
            Entity,
            &StreamedVisualObject,
            &mut Visibility,
            Option<&EditorHiddenCookedVisual>,
        ),
        Without<engine::GeneratedEnvironmentObject>,
    >,
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

pub(crate) fn source_object_position(record: &SourceObjectViewRecord) -> WorldPosition {
    WorldPosition {
        space: record.object.space,
        cell: record.object.owner_cell,
        local: record.object.local_translation,
    }
}

pub(super) fn source_object_transform(
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

pub(super) fn visual_bounds_corners(transform: &Transform, bounds: [f32; 3]) -> [Vec3; 8] {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::{EditorHistory, EditorObjectWorkingSet, EditorSelection};

    use world::{CellCoord, ObjectDefinitionId, WorldPosition};
    use world_db::SourceObjectPaletteRecord;

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
}
