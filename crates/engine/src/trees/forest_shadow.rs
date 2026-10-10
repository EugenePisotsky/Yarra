//! Feeds the distant forest shadows (`atmosphere::forest_shadow`) with every far-object tree:
//! each drawn impostor batch contributes its instances' crowns. The map is rasterized off the
//! main thread whenever batches come or go, a rebase moves them, or the camera leaves the
//! middle of the covered square.
use super::impostor::{ImpostorBatch, ImpostorBatchDone, ImpostorDescriptor};
use crate::ActiveWorldView;
use atmosphere::forest_shadow::{ForestCrown, ForestShadow, ForestShadowMap};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};

/// The map recentres once the camera is this far from its centre: well inside the 4 km
/// square, so the 2 km far-object range stays covered.
const RECENTRE_METRES: f32 = 512.0;

/// An impostor batch left out of the forest shadows, e.g. a LOD lab tree's forced impostor,
/// which duplicates the batch that shadows, or a hidden lab tree.
#[derive(Component)]
pub(crate) struct NoForestShadow;

#[derive(Default)]
pub(crate) struct ForestShadowBuilder {
    task: Option<Task<ForestShadowMap>>,
    dirty: bool,
    centre: Option<Vec2>,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn rebuild(
    mut builder: Local<ForestShadowBuilder>,
    forest: Option<ResMut<ForestShadow>>,
    view: ActiveWorldView,
    batches: Query<
        (&ImpostorBatch, &GlobalTransform),
        (With<ImpostorBatchDone>, Without<NoForestShadow>),
    >,
    added: Query<(), Or<(Added<ImpostorBatchDone>, Added<NoForestShadow>)>>,
    moved: Query<(), (With<ImpostorBatchDone>, Changed<GlobalTransform>)>,
    mut removed: RemovedComponents<ImpostorBatchDone>,
    mut restored: RemovedComponents<NoForestShadow>,
    descriptors: Res<Assets<ImpostorDescriptor>>,
) {
    let Some(mut forest) = forest else {
        return;
    };
    if !added.is_empty()
        || !moved.is_empty()
        || removed.read().count() > 0
        || restored.read().count() > 0
    {
        builder.dirty = true;
    }
    let Some(view) = view.active() else {
        return;
    };
    let eye = view.transform.translation().xz();
    if builder
        .centre
        .is_none_or(|centre| centre.distance(eye) > RECENTRE_METRES)
    {
        builder.dirty = true;
    }
    if let Some(task) = &mut builder.task {
        let Some(map) = check_ready(task) else {
            return;
        };
        forest.publish(map);
        builder.task = None;
    }
    if !builder.dirty {
        return;
    }
    builder.dirty = false;
    let mut crowns = Vec::new();
    for (batch, transform) in &batches {
        let Some(descriptor) = descriptors.get(&batch.descriptor) else {
            continue;
        };
        let crown = descriptor.crown;
        for instance in &batch.instances {
            let root = transform.transform_point(instance.translation);
            let offset = Quat::from_rotation_y(instance.yaw)
                * Vec3::new(crown.centre.x, 0.0, crown.centre.y)
                * instance.scale;
            crowns.push(ForestCrown {
                centre: (root + offset).xz(),
                radius: crown.radius * instance.scale,
                bottom: root.y + crown.bottom * instance.scale,
                top: root.y + crown.top * instance.scale,
                opacity: crown.opacity,
            });
        }
    }
    if crowns.is_empty() {
        forest.disable();
        builder.centre = None;
        return;
    }
    builder.centre = Some(eye);
    builder.task = Some(
        AsyncComputeTaskPool::get().spawn(async move { ForestShadowMap::rasterize(eye, crowns) }),
    );
}
