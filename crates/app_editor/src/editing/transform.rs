//! The transform inspector's draft and normalized source transforms.

use bevy::prelude::*;
use std::f32::consts::TAU;
use world::{StableObjectId, WorldPosition};
use world_db::SourceObjectTransform;

use super::{objects::EditorObjectWorkingSet, selection::EditorSelection};

#[derive(Resource, Default)]
pub(crate) struct TransformInspectorDraft {
    pub(crate) object: Option<StableObjectId>,
    pub(crate) edit_revision: u64,
    pub(crate) transform: Option<SourceObjectTransform>,
}

impl TransformInspectorDraft {
    pub(crate) fn sync(&mut self, selection: &EditorSelection, objects: &EditorObjectWorkingSet) {
        let object = selection.selected_id();
        if self.object == object && self.edit_revision == objects.edit_revision() {
            return;
        }
        self.object = object;
        self.edit_revision = objects.edit_revision();
        self.transform = selection
            .selected_view(objects)
            .map(|selected| SourceObjectTransform::from(&selected.object));
    }
}

pub(crate) fn normalize_transform(
    mut transform: SourceObjectTransform,
    cell_size: f32,
) -> Option<SourceObjectTransform> {
    if !cell_size.is_finite()
        || cell_size <= 0.0
        || !transform.local_translation.into_iter().all(f32::is_finite)
        || !transform.yaw.is_finite()
        || !transform.scale.is_finite()
        || transform.scale <= 0.0
    {
        return None;
    }
    let origin = transform.owner_cell.origin(cell_size);
    let world = [
        origin[0] + f64::from(transform.local_translation[0]),
        f64::from(transform.local_translation[1]),
        origin[1] + f64::from(transform.local_translation[2]),
    ];
    let minimum = f64::from(i32::MIN) * f64::from(cell_size);
    let maximum = (f64::from(i32::MAX) + 1.0) * f64::from(cell_size);
    if world[0] < minimum || world[0] >= maximum || world[2] < minimum || world[2] >= maximum {
        return None;
    }
    let position = WorldPosition::from_world(transform.space, world, cell_size);
    transform.owner_cell = position.cell;
    transform.local_translation = position.local;
    transform.yaw = transform.yaw.rem_euclid(TAU);
    Some(transform)
}
