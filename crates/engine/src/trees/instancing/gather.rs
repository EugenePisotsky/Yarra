//! Each frame's instances: the main view's visible tree meshes, and for every shadow cascade the
//! meshes of the LOD each tree casts from.
use super::{
    DRAWS, Form, Group, Instanced, TREE_LAYER, TreeInstance, TreeInstanceFrame, TreeInstancing,
};
use crate::object_lod::{LodScene, ScreenSpaceLod, TAG_BIAS, TIMED_RANGE};
use crate::trees::material::TreeWindMaterial;
use bevy::{
    camera::{
        primitives::{Aabb, CascadesFrusta, Frustum},
        visibility::{RenderLayers, VisibilityRange},
    },
    math::Vec3A,
    mesh::MeshTag,
    prelude::*,
    render::{sync_world::MainEntity, view::RetainedViewEntity},
    shape::ViewFrustum,
};
use std::{collections::HashMap, sync::Arc};

/// Instances of one view, by mesh and material.
pub(super) type Groups = HashMap<Form, Vec<TreeInstance>>;

/// Appends a view's instances to `instances`, group after group, and empties `groups` for the
/// next frame (keeping their allocations).
pub(super) fn flatten(groups: &mut Groups, instances: &mut Vec<TreeInstance>) -> Vec<Group> {
    groups.retain(|_, list| !list.is_empty());
    let mut drawn = Vec::with_capacity(groups.len().min(DRAWS));
    for (&(mesh, material), list) in groups.iter().take(DRAWS) {
        drawn.push(Group {
            mesh,
            material,
            first: instances.len() as u32,
            count: list.len() as u32,
        });
        instances.extend_from_slice(list);
    }
    for list in groups.values_mut() {
        list.clear();
    }
    drawn
}

/// The LOD scenes (by variant index) an object casts its shadow from, with their dither
/// levels: each drawn mesh LOD casts from the one `bias` steps coarser, at most the last mesh
/// LOD. Two drawn LODs casting from the same scene, in the middle of a fade between them, cast
/// it whole, so the shadow does not change while the tree dissolves.
pub(super) fn shadow_scenes(lod: &ScreenSpaceLod, bias: usize, casts: &mut Vec<(usize, i32)>) {
    casts.clear();
    let variants = lod.variants();
    let Some(last) = variants.iter().rposition(|v| v.scene.is_some()) else {
        return;
    };
    for index in 0..=last {
        let Some(level) = lod.timed_level(index) else {
            continue;
        };
        let cast = (index + bias).min(last);
        match casts.iter_mut().find(|(scene, _)| *scene == cast) {
            Some(both) => both.1 = 0,
            None => casts.push((cast, level)),
        }
    }
}

/// Whether a sphere touches a shadow cascade, whose near plane does not cull: a caster
/// between the light and the cascade still shades it.
pub(super) fn in_cascade(frustum: &Frustum, centre: Vec3A, radius: f32) -> bool {
    let centre = centre.extend(1.0);
    frustum.half_spaces.iter().enumerate().all(|(i, half)| {
        i == ViewFrustum::NEAR_PLANE_IDX || half.normal_d().dot(centre) + radius > 0.0
    })
}

/// One shadow cascade being gathered.
struct Cascade {
    view: RetainedViewEntity,
    frustum: Frustum,
    groups: Groups,
}

#[derive(Default)]
pub(super) struct Gathering {
    main: Groups,
    cascades: Vec<Cascade>,
    /// Tree mesh entities under each LOD scene, found once its meshes exist.
    scene_meshes: HashMap<Entity, Vec<Entity>>,
    casts: Vec<(usize, i32)>,
    log_in: u32,
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)] // Tree meshes, LODs, lights.
pub(super) fn collect(
    mut commands: Commands,
    settings: Res<TreeInstancing>,
    mut frame: ResMut<TreeInstanceFrame>,
    world_view: crate::ActiveWorldView,
    trees: Query<(
        Entity,
        &Mesh3d,
        &MeshMaterial3d<TreeWindMaterial>,
        &GlobalTransform,
        &InheritedVisibility,
        Option<&Aabb>,
        Option<&MeshTag>,
        Has<Instanced>,
        Option<&VisibilityRange>,
        &Visibility,
    )>,
    objects: Query<(
        &ScreenSpaceLod,
        &Children,
        &GlobalTransform,
        &InheritedVisibility,
    )>,
    scenes: Query<&LodScene>,
    descendants: Query<&Children>,
    lights: Query<(Entity, &DirectionalLight, &CascadesFrusta, &ViewVisibility)>,
    mut gathering: Local<Gathering>,
) {
    if !settings.enabled {
        return;
    }
    let gathering = &mut *gathering;
    let view = world_view.active().map(|view| (view.entity, view.frustum));

    // The main view: every visible mesh of an object that fades over time (others keep Bevy's
    // per-view distance crossfades, so they stay entities).
    for (entity, mesh, material, transform, visible, aabb, tag, instanced, range, _) in &trees {
        if range != Some(&TIMED_RANGE) {
            continue;
        }
        if !instanced {
            commands
                .entity(entity)
                .insert((Instanced, RenderLayers::layer(TREE_LAYER)));
        }
        let Some((_, frustum)) = view else {
            continue;
        };
        if !visible.get()
            || aabb
                .is_some_and(|aabb| !frustum.intersects_obb(aabb, &transform.affine(), true, false))
        {
            continue;
        }
        gathering
            .main
            .entry((mesh.id(), material.id()))
            .or_default()
            .push(TreeInstance::new(transform, tag.map_or(0, |t| t.value)));
    }

    // Shadows: the world view's cascades of every light that casts.
    gathering.cascades.retain(|_| false);
    if let Some((camera, _)) = view {
        for (light, directional, frusta, light_visible) in &lights {
            if !directional.shadow_maps_enabled || !light_visible.get() {
                continue;
            }
            for (index, frustum) in frusta.frusta.get(&camera).into_iter().flatten().enumerate() {
                gathering.cascades.push(Cascade {
                    view: RetainedViewEntity::new(
                        MainEntity::from(light),
                        Some(MainEntity::from(camera)),
                        index as u32,
                    ),
                    frustum: *frustum,
                    groups: Groups::default(),
                });
            }
        }
    }
    let mut casting = 0;
    if !gathering.cascades.is_empty() {
        for (lod, children, transform, visible) in &objects {
            if !visible.get() {
                continue;
            }
            // A sphere round the whole tree, as wide as it is tall, picks the cascades it
            // may shade; each of its meshes is then tested as Bevy tests casters.
            let (scale, _, translation) = transform.to_scale_rotation_translation();
            let height = lod.height(scale);
            let centre = Vec3A::from(translation + Vec3::Y * height * 0.5);
            let mut touched = 0u32;
            for (index, cascade) in gathering.cascades.iter().enumerate() {
                if in_cascade(&cascade.frustum, centre, height) {
                    touched |= 1 << index;
                }
            }
            if touched == 0 {
                continue;
            }
            shadow_scenes(lod, settings.shadow_lod, &mut gathering.casts);
            if !gathering.casts.is_empty() {
                casting += 1;
            }
            for &(index, level) in &gathering.casts {
                let Some(scene) = children
                    .iter()
                    .find(|child| scenes.get(*child).is_ok_and(|scene| scene.0 == index))
                else {
                    continue;
                };
                let meshes = gathering.scene_meshes.entry(scene).or_default();
                if meshes.is_empty() {
                    meshes.extend(
                        descendants
                            .iter_descendants(scene)
                            .filter(|entity| trees.contains(*entity)),
                    );
                }
                let tag = (TAG_BIAS + level) as u32;
                for &entity in meshes.iter() {
                    // Timed LOD hides whole scenes, which may still cast; a mesh hidden itself
                    // (F1 Objects off, the Ground and Grass scenes) casts nothing, as entities do.
                    let Ok((_, mesh, material, transform, _, aabb, .., visibility)) =
                        trees.get(entity)
                    else {
                        continue;
                    };
                    if *visibility == Visibility::Hidden {
                        continue;
                    }
                    let affine = transform.affine();
                    let instance = TreeInstance::new(transform, tag);
                    for (index, cascade) in gathering.cascades.iter_mut().enumerate() {
                        if touched & (1 << index) == 0
                            || aabb.is_some_and(|aabb| {
                                !cascade.frustum.intersects_obb(aabb, &affine, false, true)
                            })
                        {
                            continue;
                        }
                        cascade
                            .groups
                            .entry((mesh.id(), material.id()))
                            .or_default()
                            .push(instance);
                    }
                }
            }
        }
    }

    let mut instances = Vec::new();
    let groups = flatten(&mut gathering.main, &mut instances);
    let main_instances = instances.len();
    let mut cascades = HashMap::with_capacity(gathering.cascades.len());
    let mut per_cascade = Vec::with_capacity(gathering.cascades.len());
    for cascade in &mut gathering.cascades {
        let first = instances.len();
        cascades.insert(cascade.view, flatten(&mut cascade.groups, &mut instances));
        per_cascade.push(instances.len() - first);
    }
    gathering.log_in = gathering.log_in.saturating_sub(1);
    if gathering.log_in == 0 {
        gathering.log_in = 600;
        // Scenes despawned with their cells leave the cache here.
        gathering
            .scene_meshes
            .retain(|scene, _| scenes.contains(*scene));
        debug!(
            "TREE_INSTANCING groups={} instances={} entities={} casting={} cascades={:?} shadow_lod={}",
            groups.len(),
            main_instances,
            trees.iter().len(),
            casting,
            per_cascade,
            settings.shadow_lod,
        );
    }
    *frame = TreeInstanceFrame {
        instances: Arc::new(instances),
        groups: Arc::new(groups),
        cascades: Arc::new(cascades),
    };
}
