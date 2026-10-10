//! Two-layer tree bark. A glTF bark material tagged `yarra_bark: "blend_v1"` blends from
//! its own textures to an upper bark, whose textures `yarra_bark_upper` lists by asset
//! path, by the vertex colour's alpha. A pine trunk turns from plated bark at its foot
//! to thin orange bark higher up without per-tree textures; the generator writes the
//! weight (see TREE_BARK_BLEND in `shaders/lighting/material.wesl`).
use atmosphere::environment::{
    EnvironmentMaterial, EnvironmentMaterialOptIn, EnvironmentMaterialSystems,
};
use bevy::{
    asset::AssetEventSystems,
    gltf::GltfMaterialExtras,
    image::{
        ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler,
        ImageSamplerDescriptor,
    },
    mesh::MeshVertexBufferLayoutRef,
    pbr::{
        ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
        MaterialPlugin,
    },
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    },
};
use std::collections::{HashMap, HashSet};

type TreeBarkMaterial = ExtendedMaterial<EnvironmentMaterial, TreeBarkExtension>;
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
struct TreeBarkExtension {
    // StandardMaterial uses 0–99, tree wind 100 and EnvironmentMaterial 120–123.
    #[texture(101)]
    #[sampler(102)]
    upper_color: Handle<Image>,
    #[texture(103)]
    upper_normal: Handle<Image>,
    #[texture(104)]
    upper_metallic_roughness: Handle<Image>,
}
impl MaterialExtension for TreeBarkExtension {
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.push("TREE_BARK_BLEND".into());
        }
        Ok(())
    }
}

/// Asset paths of the upper bark's textures.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct UpperBark {
    pub(super) color: String,
    pub(super) normal: String,
    pub(super) metallic_roughness: String,
}

pub(super) fn upper_bark(extras: &str) -> Option<UpperBark> {
    let value = serde_json::from_str::<serde_json::Value>(extras).ok()?;
    if value.get("yarra_bark")?.as_str()? != "blend_v1" {
        return None;
    }
    let upper = value.get("yarra_bark_upper")?;
    let path = |key: &str| upper.get(key)?.as_str().map(str::to_owned);
    Some(UpperBark {
        color: path("color")?,
        normal: path("normal")?,
        metallic_roughness: path("metallic_roughness")?,
    })
}

pub(super) struct TreeBarkPlugin;
impl Plugin for TreeBarkPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<TreeBarkMaterial>::default())
            .add_systems(PostUpdate, opt_in.before(EnvironmentMaterialSystems))
            .add_systems(
                PostUpdate,
                convert
                    .after(EnvironmentMaterialSystems)
                    .before(AssetEventSystems),
            );
    }
}

#[derive(Component)]
struct BarkChecked;
#[derive(Component)]
struct SourceMaterial(Handle<EnvironmentMaterial>);

// Collections in the isolated Presets workspace need the composed material too.
#[allow(clippy::type_complexity)]
fn opt_in(
    mut commands: Commands,
    candidates: Query<
        (Entity, &GltfMaterialExtras),
        (
            With<MeshMaterial3d<StandardMaterial>>,
            Without<EnvironmentMaterialOptIn>,
        ),
    >,
) {
    for (entity, extras) in &candidates {
        if upper_bark(&extras.value).is_some() {
            commands.entity(entity).insert(EnvironmentMaterialOptIn);
        }
    }
}

pub(super) fn load_tiling(server: &AssetServer, path: &str, srgb: bool) -> Handle<Image> {
    server
        .load_builder()
        .with_settings(move |settings: &mut ImageLoaderSettings| {
            settings.is_srgb = srgb;
            settings.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                mipmap_filter: ImageFilterMode::Linear,
                ..default()
            });
        })
        .load(path.to_owned())
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn convert(
    mut commands: Commands,
    server: Res<AssetServer>,
    source: Res<Assets<EnvironmentMaterial>>,
    mut target: ResMut<Assets<TreeBarkMaterial>>,
    mut events: MessageReader<AssetEvent<EnvironmentMaterial>>,
    candidates: Query<
        (
            Entity,
            &GltfMaterialExtras,
            &MeshMaterial3d<EnvironmentMaterial>,
        ),
        Without<BarkChecked>,
    >,
    retained: Query<&SourceMaterial>,
    mut cache: Local<HashMap<AssetId<EnvironmentMaterial>, Handle<TreeBarkMaterial>>>,
) {
    for event in events.read() {
        if let AssetEvent::Modified { id } = event
            && let (Some(material), Some(handle)) = (source.get(*id), cache.get(id))
            && let Some(mut converted) = target.get_mut(handle)
        {
            converted.base = material.clone();
        }
    }
    // Only live scene instances retain material handles; unloading releases them.
    let live: HashSet<_> = retained.iter().map(|m| m.0.id()).collect();
    cache.retain(|id, _| live.contains(id));
    for (entity, extras, handle) in &candidates {
        let Some(material) = source.get(handle) else {
            continue;
        };
        commands.entity(entity).insert(BarkChecked);
        if super::material::structural_tag(&extras.value) {
            continue;
        }
        let Some(upper) = upper_bark(&extras.value) else {
            continue;
        };
        let converted = cache
            .entry(handle.id())
            .or_insert_with(|| {
                target.add(TreeBarkMaterial {
                    base: material.clone(),
                    extension: TreeBarkExtension {
                        upper_color: load_tiling(&server, &upper.color, true),
                        upper_normal: load_tiling(&server, &upper.normal, false),
                        upper_metallic_roughness: load_tiling(
                            &server,
                            &upper.metallic_roughness,
                            false,
                        ),
                    },
                })
            })
            .clone();
        // Like tree wind: publish before AssetEventSystems so the replacement material
        // is prepared in the same frame and the trunk never drops out.
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<EnvironmentMaterial>>()
            .insert((SourceMaterial(handle.0.clone()), MeshMaterial3d(converted)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_needs_the_versioned_tag_and_all_three_upper_textures() {
        let full = r#"{"yarra_bark":"blend_v1","yarra_bark_upper":{"color":"t/c.ktx2","normal":"t/n.ktx2","metallic_roughness":"t/mr.ktx2"}}"#;
        assert_eq!(
            upper_bark(full),
            Some(UpperBark {
                color: "t/c.ktx2".into(),
                normal: "t/n.ktx2".into(),
                metallic_roughness: "t/mr.ktx2".into(),
            })
        );
        for text in [
            "",
            "{}",
            r#"{"yarra_bark":"blend_v2","yarra_bark_upper":{"color":"c","normal":"n","metallic_roughness":"m"}}"#,
            r#"{"yarra_bark":"blend_v1","yarra_bark_upper":{"color":"c","normal":"n"}}"#,
            r#"{"yarra_wind":"foliage_uv1_v1"}"#,
        ] {
            assert_eq!(upper_bark(text), None, "{text}");
        }
    }
}
