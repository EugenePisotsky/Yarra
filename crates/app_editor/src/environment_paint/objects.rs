//! Disposable generated-object visuals. Manual editor proxies and source objects are untouched.
use super::preview::EnvironmentPreview;
use crate::workspaces::EditorWorkspace;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstance;
use engine::{GeneratedEnvironmentObject, WorldCatalog, WorldOrigin};
use std::collections::{BTreeMap, BTreeSet};
use world::{CellCoord, StableObjectId, WorldSpaceId};

#[derive(Component)]
pub(super) struct GeneratedPreview {
    id: StableObjectId,
    asset: world::AssetId,
}
#[derive(Component)]
pub(super) struct HiddenGenerated(Visibility);

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn sync(
    mut commands: Commands,
    preview: Res<EnvironmentPreview>,
    workspace: Res<State<EditorWorkspace>>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    server: Res<AssetServer>,
    mut proxies: Query<(
        Entity,
        &GeneratedPreview,
        &mut Transform,
        Option<&WorldInstance>,
    )>,
    mut cooked: Query<
        (
            Entity,
            &GeneratedEnvironmentObject,
            &mut Visibility,
            Option<&HiddenGenerated>,
        ),
        Without<GeneratedPreview>,
    >,
) {
    let mut desired = BTreeMap::new();
    let mut ready = BTreeSet::<(WorldSpaceId, CellCoord)>::new();
    if *workspace.get() == EditorWorkspace::World {
        for (key, (_, cell)) in preview.visible_cells() {
            if origin.space() != Some(cell.space) {
                continue;
            }
            ready.insert(*key);
            for o in &cell.objects {
                desired.insert(o.id, (*key, o));
            }
        }
    }
    let mut retained = BTreeSet::new();
    let transform = |key: (WorldSpaceId, CellCoord), o: &world::StaticObjectInstance| {
        let size = catalog
            .world_space(key.0)
            .map_or(0.0, |s| f64::from(s.cell_size));
        Transform::from_xyz(
            ((f64::from(key.1.x) - f64::from(origin.cell().x)) * size) as f32 + o.translation[0],
            o.translation[1],
            ((f64::from(key.1.z) - f64::from(origin.cell().z)) * size) as f32 + o.translation[2],
        )
        .with_rotation(Quat::from_rotation_y(o.yaw))
        .with_scale(Vec3::splat(o.scale))
    };
    for (entity, proxy, mut current, instance) in &mut proxies {
        if let Some(&(key, o)) = desired.get(&proxy.id)
            && o.asset == proxy.asset
        {
            let next = transform(key, o);
            if *current != next {
                *current = next;
            }
            retained.insert(proxy.id);
            if instance.is_none() {
                ready.remove(&key);
            }
        } else {
            commands.entity(entity).despawn();
        }
    }
    let mut spawned = 0;
    for (id, &(key, o)) in &desired {
        if retained.contains(id) {
            continue;
        }
        ready.remove(&key);
        if spawned >= 64 {
            continue;
        }
        let Some(asset) = preview.assets.get(&o.asset) else {
            continue;
        };
        let entity =
            engine::spawn_collection_visual(&mut commands, &server, transform(key, o), asset);
        commands.entity(entity).insert(GeneratedPreview {
            id: *id,
            asset: o.asset,
        });
        spawned += 1;
    }
    for (entity, generated, mut visibility, hidden) in &mut cooked {
        if ready.contains(&(generated.space, generated.cell)) {
            if hidden.is_none() {
                commands.entity(entity).insert(HiddenGenerated(*visibility));
            }
            *visibility = Visibility::Hidden;
        } else if let Some(hidden) = hidden {
            *visibility = hidden.0;
            commands.entity(entity).remove::<HiddenGenerated>();
        }
    }
}
