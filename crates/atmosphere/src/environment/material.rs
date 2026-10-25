use super::*;
use crate::AtmosphereState;

use bevy::{
    asset::AssetEventSystems,
    material::OpaqueRendererMethod,
    pbr::{ExtendedMaterial, MaterialExtension},
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
    #[texture(126)]
    #[sampler(127)]
    pub terrain_shadow: Handle<Image>,
    #[texture(128, dimension = "2d_array")]
    #[sampler(129)]
    pub ground_albedo: Handle<Image>,
}

impl MaterialExtension for EnvironmentExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/lighting/material.wesl".into()
    }

    // Writes our G-buffer (`shaders/lighting/gbuffer.wesl`), not Bevy's.
    fn deferred_fragment_shader() -> ShaderRef {
        "shaders/lighting/material.wesl".into()
    }
}

/// The source material as environment materials draw it: into the G-buffer on deferred views
/// (docs/DEFERRED_LIGHTING.md), lit there by the lighting kernels; views without a G-buffer draw
/// it lit in the main pass.
fn deferred(source: &StandardMaterial) -> StandardMaterial {
    StandardMaterial {
        opaque_render_method: OpaqueRendererMethod::Deferred,
        ..source.clone()
    }
}

pub type EnvironmentMaterial = ExtendedMaterial<StandardMaterial, EnvironmentExtension>;

pub(crate) struct EnvironmentMaterialPlugin;

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
#[allow(clippy::too_many_arguments)] // Independent asset collections and events.
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
        if let AssetEvent::Modified { id } = event
            && let (Some(p), Some(handle)) = (source.get(*id), cache.get(id))
            && let Some(mut material) = target.get_mut(handle)
        {
            material.base = deferred(p);
        }
    }

    let live: std::collections::HashSet<_> = retained.iter().map(|p| p.0.id()).collect();
    cache.retain(|id, _| live.contains(id));

    for (e, handle, opt_in) in &meshes {
        if state.isolated() && !opt_in {
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
                    base: deferred(base),
                    extension: EnvironmentExtension {
                        parameters: assets.parameters.clone(),
                        shadows: assets.shadows.clone(),
                        shelter: assets.shelter.clone(),
                        forest_shadow: assets.forest_shadow.clone(),
                        terrain_shadow: assets.terrain_shadow.clone(),
                        ground_albedo: assets.ground_albedo.clone(),
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
    use crate::AtmosphereOwner;

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
                cloud_shape: default(),
                cloud_detail: default(),
                shelter: default(),
                forest_shadow: default(),
                terrain_shadow: default(),
                terrain_heights: default(),
                ground_albedo: default(),
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
