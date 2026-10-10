use super::*;
use bevy::{
    asset::AssetEventSystems,
    mesh::MeshVertexBufferLayoutRef,
    pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline},
    render::render_resource::{RenderPipelineDescriptor, SpecializedMeshPipelineError},
    shader::ShaderRef,
};
use std::collections::HashMap;
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone, Default)]
pub struct EnvironmentExtension {
    #[storage(120, read_only)]
    pub parameters: Handle<ShaderBuffer>,
    #[texture(121)]
    #[sampler(122)]
    pub shadows: Handle<Image>,
    #[texture(123, sample_type = "float", filterable = false)]
    pub shelter: Handle<Image>,
    #[texture(124)]
    #[sampler(125)]
    pub forest_shadow: Handle<Image>,
}
impl MaterialExtension for EnvironmentExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/lighting/material.wesl".into()
    }
    /// With 4x MSAA, LOD crossfades cover samples instead of dithering whole pixels
    /// (`shaders/crossfade.wesl`); the main pass resolves them into a smooth blend.
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.mesh_key.msaa_samples() == 4
            && let Some(fragment) = descriptor.fragment.as_mut()
        {
            fragment.shader_defs.push("CROSSFADE_SAMPLE_MASK".into());
        }
        Ok(())
    }
}
pub type EnvironmentMaterial = ExtendedMaterial<StandardMaterial, EnvironmentExtension>;
pub struct EnvironmentMaterialPlugin;
/// Scene-material conversion completes before optional surface effects compose with it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnvironmentMaterialSystems;
/// Request this material pipeline in isolated workspaces for extensions that compose
/// with it. An isolated atmosphere already disables cloud transmission in the shared data.
#[derive(Component)]
pub struct EnvironmentMaterialOptIn;
#[derive(Component)]
struct OriginalMaterial(Handle<StandardMaterial>);
impl Plugin for EnvironmentMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy::pbr::MaterialPlugin::<EnvironmentMaterial>::default())
            .add_systems(
                PostUpdate,
                convert
                    .in_set(EnvironmentMaterialSystems)
                    .after(ApplyAtmosphere)
                    .before(AssetEventSystems),
            );
    }
}
// Keep source handles alive and mirror edits. Unlit/editor overlay materials retain
// their normal path. Other meshes are never converted while an isolated workspace owns lighting.
fn convert(
    mut commands: Commands,
    assets: Option<Res<EnvironmentAssets>>,
    state: Res<AtmosphereState>,
    mut events: MessageReader<AssetEvent<StandardMaterial>>,
    source: Res<Assets<StandardMaterial>>,
    mut target: ResMut<Assets<EnvironmentMaterial>>,
    meshes: Query<(
        Entity,
        &MeshMaterial3d<StandardMaterial>,
        Has<EnvironmentMaterialOptIn>,
    )>,
    retained: Query<&OriginalMaterial>,
    mut cache: Local<HashMap<AssetId<StandardMaterial>, Handle<EnvironmentMaterial>>>,
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
        if state.owner == AtmosphereOwner::Isolated && !opt_in {
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
                target.add(EnvironmentMaterial {
                    base: base.clone(),
                    extension: EnvironmentExtension {
                        parameters: assets.parameters.clone(),
                        shadows: assets.shadows.clone(),
                        shelter: assets.shelter.clone(),
                        forest_shadow: assets.forest_shadow.clone(),
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
    fn isolated_conversion_requires_explicit_composition_opt_in() {
        let mut app = App::new();
        app.init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<EnvironmentMaterial>>()
            .add_message::<AssetEvent<StandardMaterial>>()
            .insert_resource(AtmosphereState {
                owner: AtmosphereOwner::Isolated,
                ..default()
            })
            .insert_resource(EnvironmentAssets {
                parameters: default(),
                shadows: default(),
                noise: default(),
                shelter: default(),
                forest_shadow: default(),
                mist: default(),
                shore: default(),
            })
            .add_systems(Update, convert);
        let material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let ordinary = app.world_mut().spawn(MeshMaterial3d(material.clone())).id();
        let composed = app
            .world_mut()
            .spawn((MeshMaterial3d(material), EnvironmentMaterialOptIn))
            .id();
        app.update();
        assert!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(ordinary)
                .is_some()
        );
        assert!(
            app.world()
                .get::<MeshMaterial3d<EnvironmentMaterial>>(composed)
                .is_some()
        );
    }
}
