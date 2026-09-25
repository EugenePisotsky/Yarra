use super::*;
use bevy::{
    asset::AssetEventSystems,
    pbr::{ExtendedMaterial, MaterialExtension},
    shader::ShaderRef,
};
use std::collections::HashMap;
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone, Default)]
pub struct CloudExtension {
    #[storage(120, read_only)]
    pub parameters: Handle<ShaderBuffer>,
    #[texture(121)]
    #[sampler(122)]
    pub shadows: Handle<Image>,
    #[texture(123, sample_type = "float", filterable = false)]
    pub shelter: Handle<Image>,
}
impl MaterialExtension for CloudExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/clouds/material.wgsl".into()
    }
}
pub type CloudMaterial = ExtendedMaterial<StandardMaterial, CloudExtension>;
pub struct CloudMaterialPlugin;
/// Scene-material conversion completes before optional surface effects compose with it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CloudMaterialSystems;
/// Request this material pipeline in isolated studies for extensions that compose
/// with it. Study atmosphere already disables cloud transmission in the shared data.
#[derive(Component)]
pub struct CloudMaterialOptIn;
#[derive(Component)]
struct OriginalMaterial(Handle<StandardMaterial>);
impl Plugin for CloudMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy::pbr::MaterialPlugin::<CloudMaterial>::default())
            .add_systems(
                PostUpdate,
                convert
                    .in_set(CloudMaterialSystems)
                    .after(ApplyAtmosphere)
                    .before(AssetEventSystems),
            );
    }
}
// Keep source handles alive and mirror edits. Unlit/editor overlay materials retain
// their normal path. Study meshes are never converted while a study owns lighting.
fn convert(
    mut commands: Commands,
    assets: Option<Res<CloudAssets>>,
    state: Res<AtmosphereState>,
    mut events: MessageReader<AssetEvent<StandardMaterial>>,
    source: Res<Assets<StandardMaterial>>,
    mut target: ResMut<Assets<CloudMaterial>>,
    meshes: Query<(
        Entity,
        &MeshMaterial3d<StandardMaterial>,
        Has<CloudMaterialOptIn>,
    )>,
    retained: Query<&OriginalMaterial>,
    mut cache: Local<HashMap<AssetId<StandardMaterial>, Handle<CloudMaterial>>>,
) {
    let Some(assets) = assets else {
        return;
    };
    for event in events.read() {
        if let AssetEvent::Modified { id } = event {
            if let (Some(p), Some(handle)) = (source.get(*id), cache.get(id)) {
                if let Some(mut material) = target.get_mut(handle) {
                    material.base = p.clone();
                }
            }
        }
    }
    let live: std::collections::HashSet<_> = retained.iter().map(|p| p.0.id()).collect();
    cache.retain(|id, _| live.contains(id));
    for (e, handle, opt_in) in &meshes {
        if state.owner == AtmosphereOwner::Study && !opt_in {
            continue;
        }
        let Some(base) = source.get(handle) else {
            continue;
        };
        if base.unlit {
            continue;
        }
        let material = cache
            .entry(handle.id())
            .or_insert_with(|| {
                target.add(CloudMaterial {
                    base: base.clone(),
                    extension: CloudExtension {
                        parameters: assets.parameters.clone(),
                        shadows: assets.shadows.clone(),
                        shelter: assets.shelter.clone(),
                    },
                })
            })
            .clone();
        commands
            .entity(e)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert((OriginalMaterial(handle.0.clone()), MeshMaterial3d(material)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn study_conversion_requires_explicit_composition_opt_in() {
        let mut app = App::new();
        app.init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<CloudMaterial>>()
            .add_message::<AssetEvent<StandardMaterial>>()
            .insert_resource(AtmosphereState {
                owner: AtmosphereOwner::Study,
                ..default()
            })
            .insert_resource(CloudAssets {
                parameters: default(),
                shadows: default(),
                noise: default(),
                shelter: default(),
            })
            .add_systems(Update, convert);
        let material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let ordinary = app.world_mut().spawn(MeshMaterial3d(material.clone())).id();
        let composed = app
            .world_mut()
            .spawn((MeshMaterial3d(material), CloudMaterialOptIn))
            .id();
        app.update();
        assert!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(ordinary)
                .is_some()
        );
        assert!(
            app.world()
                .get::<MeshMaterial3d<CloudMaterial>>(composed)
                .is_some()
        );
    }
}
