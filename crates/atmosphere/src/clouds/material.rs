use super::*;
use bevy::{
    pbr::{ExtendedMaterial, MaterialExtension},
    shader::ShaderRef,
};
use std::collections::HashMap;
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct CloudExtension {
    #[storage(120, read_only)]
    pub parameters: Handle<ShaderBuffer>,
    #[texture(121)]
    #[sampler(122)]
    pub shadows: Handle<Image>,
}
impl MaterialExtension for CloudExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/clouds/material.wgsl".into()
    }
}
pub type CloudMaterial = ExtendedMaterial<StandardMaterial, CloudExtension>;
pub struct CloudMaterialPlugin;
#[derive(Component)]
struct OriginalMaterial(Handle<StandardMaterial>);
impl Plugin for CloudMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy::pbr::MaterialPlugin::<CloudMaterial>::default())
            .add_systems(PostUpdate, convert.after(ApplyAtmosphere));
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
    meshes: Query<(Entity, &MeshMaterial3d<StandardMaterial>)>,
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
    if state.owner == AtmosphereOwner::Study {
        return;
    }
    for (e, handle) in &meshes {
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
