use super::{MAX_DISPLACEMENT, WindBuffer};
use atmosphere::clouds::{CloudMaterial, CloudMaterialOptIn, CloudMaterialSystems};
use bevy::{
    asset::AssetEventSystems,
    camera::{primitives::Aabb, visibility::VisibilitySystems},
    gltf::GltfMaterialExtras,
    material::AlphaMode,
    math::Vec3A,
    mesh::MeshVertexBufferLayoutRef,
    pbr::{
        ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
        MaterialPlugin,
    },
    prelude::*,
    render::{
        render_resource::{AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError},
        storage::ShaderBuffer,
    },
    shader::ShaderRef,
};
use std::collections::{HashMap, HashSet};

type TreeWindMaterial = ExtendedMaterial<CloudMaterial, TreeWindExtension>;
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
#[bind_group_data(TreeWindKey)]
struct TreeWindExtension {
    // StandardMaterial uses 0–99; CloudMaterial uses 120–122.
    #[storage(100, read_only)]
    wind: Handle<ShaderBuffer>,
    crown_shading: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TreeWindKey {
    crown_shading: bool,
}
impl From<&TreeWindExtension> for TreeWindKey {
    fn from(material: &TreeWindExtension) -> Self {
        Self {
            crown_shading: material.crown_shading,
        }
    }
}
impl MaterialExtension for TreeWindExtension {
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.bind_group_data.crown_shading
            && let Some(fragment) = descriptor.fragment.as_mut()
        {
            fragment.shader_defs.push("TREE_CROWN_SHADING".into());
        }
        Ok(())
    }
    fn vertex_shader() -> ShaderRef {
        "shaders/tree_wind.wgsl".into()
    }
    fn prepass_vertex_shader() -> ShaderRef {
        "shaders/tree_wind.wgsl".into()
    }
    fn deferred_vertex_shader() -> ShaderRef {
        "shaders/tree_wind.wgsl".into()
    }
}

pub(super) struct TreeWindMaterialPlugin;
impl Plugin for TreeWindMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<TreeWindMaterial>::default())
            .add_systems(PostUpdate, opt_in.before(CloudMaterialSystems))
            .add_systems(
                PostUpdate,
                convert
                    .after(CloudMaterialSystems)
                    .before(AssetEventSystems),
            )
            .add_systems(
                PostUpdate,
                expand_bounds
                    .after(VisibilitySystems::CalculateBounds)
                    .before(VisibilitySystems::CheckVisibility),
            );
    }
}

#[derive(Component)]
struct WindChecked;
#[derive(Component)]
struct WindBoundsPending;
#[derive(Component)]
struct SourceMaterial(Handle<CloudMaterial>);

fn extra_is(extras: &str, key: &str, value: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(extras)
        .ok()
        .and_then(|v| v.get(key).and_then(|v| v.as_str()).map(|v| v == value))
        .unwrap_or(false)
}

fn uses_wind(extras: &str) -> bool {
    extra_is(extras, "yarra_wind", "foliage_uv1_v1")
}

// Crown-shaded foliage (`yarra_shading: "crown_v1"`) is lit by where a card sits in
// the crown: both sides keep the authored crown normal, and vertex colour is crown
// occlusion for indirect light (see TREE_CROWN_SHADING in the cloud material shaders).
// Back faces are still drawn; only Bevy's two-sided normal flip is turned off.
fn crown_shading(extras: &str) -> bool {
    extra_is(extras, "yarra_shading", "crown_v1")
}

fn foliage_base(source: &CloudMaterial, crown_shading: bool) -> CloudMaterial {
    let mut material = source.clone();
    if crown_shading {
        material.base.double_sided = false;
        material.base.cull_mode = None;
    }
    material
}

// Collections in the Presets workspace also need the composed material while
// study lighting disables cloud transmission. Do not change unrelated study meshes.
fn opt_in(
    mut commands: Commands,
    candidates: Query<
        (Entity, &GltfMaterialExtras),
        (
            With<MeshMaterial3d<StandardMaterial>>,
            Without<CloudMaterialOptIn>,
        ),
    >,
) {
    for (entity, extras) in &candidates {
        if uses_wind(&extras.value) {
            commands.entity(entity).insert(CloudMaterialOptIn);
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn convert(
    mut commands: Commands,
    buffer: Option<Res<WindBuffer>>,
    meshes: Res<Assets<Mesh>>,
    source: Res<Assets<CloudMaterial>>,
    mut target: ResMut<Assets<TreeWindMaterial>>,
    mut events: MessageReader<AssetEvent<CloudMaterial>>,
    candidates: Query<
        (
            Entity,
            &GltfMaterialExtras,
            &Mesh3d,
            &MeshMaterial3d<CloudMaterial>,
        ),
        Without<WindChecked>,
    >,
    retained: Query<&SourceMaterial>,
    mut cache: Local<HashMap<AssetId<CloudMaterial>, (Handle<TreeWindMaterial>, bool)>>,
) {
    let Some(buffer) = buffer else {
        return;
    };
    for event in events.read() {
        if let AssetEvent::Modified { id } = event
            && let (Some(material), Some(&(ref handle, crown))) = (source.get(*id), cache.get(id))
            && let Some(mut converted) = target.get_mut(handle)
        {
            converted.base = foliage_base(material, crown);
        }
    }
    // Only live scene instances retain material handles; unloading pages/LODs releases them.
    let live: HashSet<_> = retained.iter().map(|m| m.0.id()).collect();
    cache.retain(|id, _| live.contains(id));
    for (entity, extras, mesh, handle) in &candidates {
        let (Some(mesh), Some(material)) = (meshes.get(mesh), source.get(handle)) else {
            continue;
        };
        commands.entity(entity).insert(WindChecked);
        if !uses_wind(&extras.value) {
            continue;
        }
        if !mesh.contains_attribute(Mesh::ATTRIBUTE_UV_1)
            || mesh.contains_attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
            || mesh.has_morph_targets()
            || !matches!(material.base.alpha_mode, AlphaMode::Mask(_))
        {
            warn!(
                "Tree wind requires a static alpha-masked mesh with authored UV1 weights: {entity:?}"
            );
            continue;
        }
        let converted = cache
            .entry(handle.id())
            .or_insert_with(|| {
                let crown = crown_shading(&extras.value);
                let material = target.add(TreeWindMaterial {
                    base: foliage_base(material, crown),
                    extension: TreeWindExtension {
                        wind: buffer.0.clone(),
                        crown_shading: crown,
                    },
                });
                (material, crown)
            })
            .0
            .clone();
        // Publish the final material before AssetEventSystems, so its GPU asset and
        // specialization are available in the same frame as the replacement mesh.
        // CalculateBounds runs after those events; waiting for Aabb here used to
        // leave a bare trunk for a frame on every tree LOD switch.
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<CloudMaterial>>()
            .insert((
                SourceMaterial(handle.0.clone()),
                MeshMaterial3d(converted),
                WindBoundsPending,
            ));
    }
}

fn expand_bounds(
    mut commands: Commands,
    mut meshes: Query<(Entity, &GlobalTransform, &mut Aabb), With<WindBoundsPending>>,
) {
    for (entity, transform, mut bounds) in &mut meshes {
        // Bounds are calculated after transform propagation. Keep this separate
        // from early material conversion and expand exactly once, before culling.
        let scale = transform
            .to_scale_rotation_translation()
            .0
            .abs()
            .min_element()
            .max(0.001);
        bounds.half_extents += Vec3A::splat(MAX_DISPLACEMENT / scale);
        commands.entity(entity).remove::<WindBoundsPending>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_opt_in_is_explicit_and_versioned() {
        assert!(uses_wind(r#"{"yarra_wind":"foliage_uv1_v1","other":true}"#));
        for text in [
            "",
            "{}",
            r#"{"yarra_wind":"foliage_uv1_v2"}"#,
            r#"{"name":"tree"}"#,
        ] {
            assert!(!uses_wind(text));
        }
    }

    #[test]
    fn crown_shading_keeps_back_faces_without_flipping() {
        let source = CloudMaterial {
            base: StandardMaterial {
                double_sided: true,
                cull_mode: None,
                ..default()
            },
            ..default()
        };
        let shared = foliage_base(
            &source,
            crown_shading(r#"{"yarra_wind":"foliage_uv1_v1","yarra_shading":"crown_v1"}"#),
        );
        assert!(!shared.base.double_sided);
        assert_eq!(shared.base.cull_mode, None);
        let flipped = foliage_base(&source, crown_shading(r#"{"yarra_wind":"foliage_uv1_v1"}"#));
        assert!(flipped.base.double_sided);
    }

    #[test]
    fn scene_instances_share_materials_and_lod_replacements_keep_cloud_updates() {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<CloudMaterial>>()
            .init_resource::<Assets<TreeWindMaterial>>()
            .add_message::<AssetEvent<CloudMaterial>>()
            .insert_resource(WindBuffer(Handle::default()))
            .add_systems(Update, (convert, expand_bounds).chain());
        let mesh = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0., 0., 0.]; 3])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, vec![[0.4, 1.]; 3]);
        let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
        let material = app
            .world_mut()
            .resource_mut::<Assets<CloudMaterial>>()
            .add(CloudMaterial {
                base: StandardMaterial {
                    alpha_mode: AlphaMode::Mask(0.5),
                    ..default()
                },
                ..default()
            });
        let spawn = |app: &mut App, tag: &str| {
            app.world_mut()
                .spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    GlobalTransform::IDENTITY,
                    Aabb {
                        center: Vec3A::ZERO,
                        half_extents: Vec3A::ONE,
                    },
                    GltfMaterialExtras { value: tag.into() },
                ))
                .id()
        };
        let a = spawn(&mut app, r#"{"yarra_wind":"foliage_uv1_v1"}"#);
        let b = spawn(&mut app, r#"{"yarra_wind":"foliage_uv1_v1"}"#);
        let unrelated = spawn(&mut app, "{}");
        app.update();
        let handle = app
            .world()
            .get::<MeshMaterial3d<TreeWindMaterial>>(a)
            .unwrap()
            .0
            .clone();
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<TreeWindMaterial>>(b)
                .unwrap()
                .0,
            handle
        );
        assert!(
            app.world()
                .get::<MeshMaterial3d<CloudMaterial>>(unrelated)
                .is_some()
        );
        assert!(app.world().get::<Aabb>(a).unwrap().half_extents.x > 2.);
        app.world_mut().despawn(a);
        let new_lod = spawn(&mut app, r#"{"yarra_wind":"foliage_uv1_v1"}"#);
        app.world_mut()
            .resource_mut::<Assets<CloudMaterial>>()
            .get_mut(&material)
            .unwrap()
            .base
            .perceptual_roughness = 0.17;
        app.world_mut()
            .write_message(AssetEvent::<CloudMaterial>::Modified { id: material.id() });
        app.update();
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<TreeWindMaterial>>(new_lod)
                .unwrap()
                .0,
            handle
        );
        assert_eq!(
            app.world()
                .resource::<Assets<TreeWindMaterial>>()
                .get(&handle)
                .unwrap()
                .base
                .base
                .perceptual_roughness,
            0.17
        );
    }
}
