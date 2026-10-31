//! A character's hair: a style its kit offers, worn on its head joint and fitted to its face.
//!
//! A style is a folder (`content/characters/scripts/hair_fit.py` writes it): `style.hair`, the cards and
//! how they follow the base mesh, and its textures. Only the styles worn load, and a character keeps
//! its old hair until the new one is ready. Its cards turn rigidly with the head, so they need no
//! skinning: the mesh hangs off the head joint by the joint's inverse bind pose.

use atmosphere::environment::EnvironmentMaterialOptIn;
use bevy::{
    asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader},
    gltf::GltfMaterialName,
    image::{
        ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler,
        ImageSamplerDescriptor,
    },
    mesh::{
        Indices, PrimitiveTopology,
        skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    },
    prelude::*,
};

use crate::{
    character_face::{CharacterAppearance, SkinLook},
    character_kit::{
        CharacterKit, CharacterKitRef, HairShape, KitMesh, KitMeshSource, KitStale, PartFitting,
        SOURCE_VERTEX, SkinLayers, validate_part,
    },
};

const VERSION: u32 = 1;

/// A style's files in its folder.
const MESH_FILE: &str = "style.hair";

const COLOR_FILE: &str = "hair_color.ktx2";
const NORMAL_FILE: &str = "hair_normal.ktx2";

/// The joint hair turns with.
const HEAD_JOINT: &str = "head";

/// What `SkinLook::hair_style` holds for a bare head.
pub const NO_HAIR: &str = "None";

/// The material `character_skin` gives a hair's soft cut-out.
pub(crate) const HAIR_MATERIAL: &str = "hair_style";

/// A hair's highlight is wide and weak: on dark hair a narrow, strong one reads as metal.
const HAIR_ROUGHNESS: f32 = 0.72;

const HAIR_REFLECTANCE: f32 = 0.35;

/// The alpha the cards cut out at, in the shadow maps: the main passes dither it.
const HAIR_CUTOFF: f32 = 0.45;

/// A style's cards, and how they follow the base mesh.
#[derive(Asset, TypePath, Debug)]
pub(crate) struct HairStyle {
    pub(crate) fit: PartFitting,
    pub(crate) template: Mesh,
}

/// Takes little-endian numbers off the front of a file.
struct Fields<'a>(&'a [u8]);

impl Fields<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let (head, tail) = self
            .0
            .split_first_chunk::<N>()
            .ok_or("the file ends too soon")?;
        self.0 = tail;
        Ok(*head)
    }

    fn u32(&mut self) -> Result<u32, String> {
        self.take().map(u32::from_le_bytes)
    }

    fn f32(&mut self) -> Result<f32, String> {
        self.take().map(f32::from_le_bytes)
    }

    fn u32s(&mut self, count: usize) -> Result<Vec<u32>, String> {
        (0..count).map(|_| self.u32()).collect()
    }

    fn triples<T>(
        &mut self,
        count: usize,
        mut one: impl FnMut(&mut Self) -> Result<T, String>,
    ) -> Result<Vec<[T; 3]>, String> {
        (0..count)
            .map(|_| Ok([one(self)?, one(self)?, one(self)?]))
            .collect()
    }

    fn vectors(&mut self, count: usize) -> Result<Vec<[f32; 3]>, String> {
        self.triples(count, Self::f32)
    }
}

/// `style.hair`: version, source vertices, vertices, indices, three scales, then per source vertex its
/// base-mesh vertices, weights and offsets, then per vertex its position, normal, UV and source, then
/// the indices.
fn parse_style(bytes: &[u8]) -> Result<HairStyle, String> {
    let mut fields = Fields(bytes);
    let version = fields.u32()?;

    if version != VERSION {
        return Err(format!("hair version {version} (expected {VERSION})"));
    }

    let sources = fields.u32()? as usize;
    let vertices = fields.u32()? as usize;
    let indices = fields.u32()? as usize;
    let mut scale =
        || -> Result<(u32, u32, f32), String> { Ok((fields.u32()?, fields.u32()?, fields.f32()?)) };
    let scales = [scale()?, scale()?, scale()?];
    let fit = PartFitting {
        vertices: fields.triples(sources, Fields::u32)?,
        weights: fields.vectors(sources)?,
        offsets: fields.vectors(sources)?,
        scales,
    };
    let positions = fields.vectors(vertices)?;
    let normals = fields.vectors(vertices)?;
    let uvs = (0..vertices)
        .map(|_| Ok([fields.f32()?, fields.f32()?]))
        .collect::<Result<Vec<_>, String>>()?;
    let source = fields.u32s(vertices)?;
    let triangles = fields.u32s(indices)?;

    if !fields.0.is_empty() {
        return Err("bytes past the file's end".into());
    }

    if source.iter().any(|&s| s as usize >= sources)
        || triangles.iter().any(|&i| i as usize >= vertices)
    {
        return Err("a source vertex or index out of range".into());
    }

    let mut template = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(
        SOURCE_VERTEX,
        source.iter().map(|&s| s as f32).collect::<Vec<_>>(),
    )
    .with_inserted_indices(Indices::U32(triangles));
    template
        .generate_tangents()
        .map_err(|error| error.to_string())?;
    Ok(HairStyle { fit, template })
}

#[derive(Default, TypePath)]
pub(crate) struct HairStyleLoader;

impl AssetLoader for HairStyleLoader {
    type Asset = HairStyle;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        parse_style(&bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    fn extensions(&self) -> &[&str] {
        &["hair"]
    }
}

/// The style a look wears: an unknown one (or a look's empty default) takes the kit's first.
pub(crate) fn worn_shape<'a>(layers: &'a SkinLayers, look: &SkinLook) -> Option<&'a HairShape> {
    if look.hair_style == NO_HAIR {
        return None;
    }

    layers
        .hairs
        .iter()
        .find(|hair| hair.label == look.hair_style)
        .or_else(|| layers.hairs.first())
}

/// A texture that repeats: cards' UVs run past the texture.
fn load_texture(server: &AssetServer, path: String, srgb: bool) -> Handle<Image> {
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
                anisotropy_clamp: 8,
                ..default()
            });
        })
        .load(path)
}

/// On a visual whose hair is on its way.
#[derive(Component)]
pub(crate) struct LoadingHair {
    folder: String,
    style: Handle<HairStyle>,
    color: Handle<Image>,
    normal: Handle<Image>,
}

/// On a visual wearing hair: its folder and mesh.
#[derive(Component)]
pub(crate) struct WornHair {
    folder: String,
    mesh: Entity,
}

/// What a visual's joints say of where hair hangs.
fn head_joint(
    visual: Entity,
    skinned: &Query<(&KitMesh, &SkinnedMesh)>,
    names: &Query<&Name>,
    poses: &Assets<SkinnedMeshInverseBindposes>,
) -> Option<(Entity, Transform)> {
    let (_, skin) = skinned
        .iter()
        .find(|(kit_mesh, _)| kit_mesh.visual == visual)?;
    let index = skin.joints.iter().position(|joint| {
        names
            .get(*joint)
            .is_ok_and(|name| name.as_str() == HEAD_JOINT)
    })?;
    let pose = poses.get(&skin.inverse_bindposes)?.get(index)?;
    Some((skin.joints[index], Transform::from_matrix(*pose)))
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)] // Independent assets and queries.
pub(crate) fn wear_hair(
    mut commands: Commands,
    If(server): If<Res<AssetServer>>,
    If(mut materials): If<ResMut<Assets<StandardMaterial>>>,
    If(poses): If<Res<Assets<SkinnedMeshInverseBindposes>>>,
    If(images): If<Res<Assets<Image>>>,
    styles: Res<Assets<HairStyle>>,
    kits: Res<Assets<CharacterKit>>,
    appearances: Query<&CharacterAppearance>,
    visuals: Query<(
        Entity,
        &CharacterKitRef,
        &ChildOf,
        Option<&WornHair>,
        Option<&LoadingHair>,
    )>,
    skinned: Query<(&KitMesh, &SkinnedMesh)>,
    names: Query<&Name>,
) {
    for (visual, kit_ref, parent, worn, loading) in &visuals {
        let Some(kit) = kits.get(kit_ref.kit()) else {
            continue;
        };
        let Some(layers) = kit.skin_layers() else {
            continue;
        };
        let look = appearances
            .get(parent.parent())
            .map(|appearance| appearance.look.clone())
            .unwrap_or_default();
        let wanted = worn_shape(layers, &look);

        // A hair goes with its head joint, which a character's scene can despawn as it comes up: a
        // style that loaded before then is worn again on the new joint.
        let worn = worn.filter(|worn| names.contains(worn.mesh));

        // Nothing to do, or a bare head wanted.
        if wanted.map(|shape| shape.folder.as_str()) == worn.map(|worn| worn.folder.as_str()) {
            if loading.is_some() {
                commands.entity(visual).remove::<LoadingHair>();
            }

            continue;
        }

        let Some(shape) = wanted else {
            if let Some(worn) = worn {
                commands.entity(worn.mesh).despawn();
            }

            commands.entity(visual).remove::<(WornHair, LoadingHair)>();
            continue;
        };
        let Some(loading) = loading.filter(|loading| loading.folder == shape.folder) else {
            commands.entity(visual).insert(LoadingHair {
                folder: shape.folder.clone(),
                style: server.load(format!("{}/{MESH_FILE}", shape.folder)),
                color: load_texture(&server, format!("{}/{COLOR_FILE}", shape.folder), true),
                normal: load_texture(&server, format!("{}/{NORMAL_FILE}", shape.folder), false),
            });
            continue;
        };
        let (Some(style), true, true) = (
            styles.get(&loading.style),
            images.contains(&loading.color),
            images.contains(&loading.normal),
        ) else {
            continue;
        };

        if let Err(error) = validate_part(&style.fit, kit.base_len()) {
            error_once!("hair style {:?}: {error}", shape.label);
            continue;
        }

        let Some((head, hang)) = head_joint(visual, &skinned, &names, &poses) else {
            continue;
        };
        let [r, g, b] = look.hair.color;
        let material = materials.add(StandardMaterial {
            base_color: Color::srgb(r, g, b),
            base_color_texture: Some(loading.color.clone()),
            normal_map_texture: Some(loading.normal.clone()),
            perceptual_roughness: HAIR_ROUGHNESS,
            reflectance: HAIR_REFLECTANCE,
            alpha_mode: AlphaMode::Mask(HAIR_CUTOFF),
            double_sided: true,
            cull_mode: None,
            ..default()
        });
        let mesh = commands
            .spawn((
                Mesh3d::default(),
                MeshMaterial3d(material),
                hang,
                Visibility::Inherited,
                ChildOf(head),
                GltfMaterialName(HAIR_MATERIAL.into()),
                EnvironmentMaterialOptIn,
                KitMesh {
                    visual,
                    name: format!("hair/{}", shape.label),
                    source: KitMeshSource::Hair(loading.style.clone()),
                    built: None,
                },
                Name::new("Character hair"),
            ))
            .id();

        if let Some(worn) = worn {
            commands.entity(worn.mesh).despawn();
        }

        commands.entity(visual).remove::<LoadingHair>().insert((
            WornHair {
                folder: shape.folder.clone(),
                mesh,
            },
            KitStale,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A style of one triangle over three source vertices, the last split once.
    fn file() -> Vec<u8> {
        let mut out = Vec::new();
        let mut u = |values: &[u32]| values.iter().for_each(|v| out.extend(v.to_le_bytes()));
        u(&[VERSION, 3, 4, 3]);
        let mut bytes = std::mem::take(&mut out);
        let scales: [(u32, u32, f32); 3] = [(0, 1, 1.0), (0, 2, 1.0), (0, 3, 1.0)];

        for (a, b, length) in scales {
            bytes.extend(a.to_le_bytes());
            bytes.extend(b.to_le_bytes());
            bytes.extend(length.to_le_bytes());
        }

        let u32s = |bytes: &mut Vec<u8>, values: &[u32]| {
            values.iter().for_each(|v| bytes.extend(v.to_le_bytes()));
        };
        let f32s = |bytes: &mut Vec<u8>, values: &[f32]| {
            values.iter().for_each(|v| bytes.extend(v.to_le_bytes()));
        };
        u32s(&mut bytes, &[0, 1, 2, 0, 1, 2, 0, 1, 2]);
        f32s(&mut bytes, &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        f32s(&mut bytes, &[0.0; 9]);
        f32s(
            &mut bytes,
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0],
        );
        f32s(&mut bytes, &[0.0, 0.0, 1.0].repeat(4));
        f32s(&mut bytes, &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        u32s(&mut bytes, &[0, 1, 2, 2]);
        u32s(&mut bytes, &[0, 1, 2]);
        bytes
    }

    #[test]
    fn a_style_reads_back_with_its_fit_and_cards() {
        let style = parse_style(&file()).expect("the test style is valid");
        assert_eq!(style.fit.vertices.len(), 3);
        assert_eq!(style.template.count_vertices(), 4);
        assert!(style.template.attribute(Mesh::ATTRIBUTE_TANGENT).is_some());
        assert_eq!(
            style.template.attribute(SOURCE_VERTEX).map(|a| a.len()),
            Some(4)
        );
    }

    #[test]
    fn broken_styles_are_refused() {
        let bytes = file();
        assert!(parse_style(&bytes[..bytes.len() - 1]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(parse_style(&longer).is_err());
        let mut version = bytes;
        version[0] = 9;
        assert!(parse_style(&version).is_err());
    }

    #[test]
    fn a_look_wears_its_style_the_kits_first_or_none() {
        let layers: SkinLayers = serde_json::from_str(
            r#"{"brow_rect": [0, 0, 1, 1], "makeup": "m", "brows": [],
                "hairs": [{"label": "Bob", "folder": "a"}, {"label": "Bun", "folder": "b"}]}"#,
        )
        .expect("the test layers parse");
        let look = |style: &str| SkinLook {
            hair_style: style.into(),
            ..default()
        };
        assert_eq!(worn_shape(&layers, &look("Bun")).unwrap().folder, "b");
        assert_eq!(worn_shape(&layers, &look("")).unwrap().folder, "a");
        assert!(worn_shape(&layers, &look(NO_HAIR)).is_none());
    }
}
