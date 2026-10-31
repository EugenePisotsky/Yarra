//! A character's skin paints brows and makeup over itself as it draws: masks in the head's texture
//! layout (`content/characters/scripts/skin_layers_bake.py`), a colour and strength each. A brow
//! follows every face slider, being in the skin's own layout, and a character's colours cost it no
//! texture of its own. Each head gets its own material, in place of the one its glb's instances
//! share. The masks load before a head takes them: a material whose texture is not ready does not
//! draw. Head and body light as skin (`SKIN_LIGHTING`), and lashes keep at least a pixel's width in a
//! vertex shader of their own.

use atmosphere::environment::{EnvironmentMaterial, EnvironmentMaterialSystems};
use bevy::{
    asset::AssetEventSystems,
    ecs::system::SystemParam,
    gltf::GltfMaterialName,
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
    render::{
        RenderApp,
        render_resource::{
            AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
        },
    },
    shader::ShaderRef,
};

use crate::{
    character_face::{CharacterAppearance, SkinLook, SkinPaint},
    character_hair::{HAIR_MATERIAL, NO_HAIR, worn_shape},
    character_kit::{CharacterKit, CharacterKitRef, KitMesh, STRAND_ACROSS, SkinLayers},
};

/// The material of a character's head (`content/characters/scripts/skin_export.py`).
const HEAD_MATERIAL: &str = "skin_head";

/// How bright a hair's grey cards are on the whole: the skin under them, painted the hair's colour,
/// should look like them.
const HAIR_CARD_LEVEL: f32 = 0.55;

/// Lash strands' `across`, after Bevy's own vertex inputs (0–7).
const STRAND_LOCATION: u32 = 10;

type SkinMaterial = ExtendedMaterial<EnvironmentMaterial, SkinLayersExtension>;

#[derive(Clone, Debug, Default, PartialEq, ShaderType)]
struct SkinLayerParams {
    brow_rect: Vec4,
    brow: Vec4,
    hairline: Vec4,
    eyeshadow: Vec4,
    liner: Vec4,
    blush: Vec4,
    lipstick: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
struct SkinLayersExtension {
    // StandardMaterial uses 0–99 and EnvironmentMaterial 120–129.
    #[uniform(101)]
    params: SkinLayerParams,
    #[texture(102)]
    #[sampler(103)]
    brow_mask: Handle<Image>,
    #[texture(104)]
    makeup: Handle<Image>,
    #[texture(105)]
    hairline: Handle<Image>,
}

impl MaterialExtension for SkinLayersExtension {
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.push("CHARACTER_SKIN".into());
            fragment.shader_defs.push("SKIN_LIGHTING".into());
        }

        Ok(())
    }
}

/// A material a mesh of a character takes in place of the plain one its glb names.
trait KitMeshExtension: MaterialExtension + Default {
    const MATERIAL: &'static str;
}

/// The body: skin's lighting and nothing to paint.
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug, Default)]
struct BodySkinExtension {}

impl MaterialExtension for BodySkinExtension {
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.push("SKIN_LIGHTING".into());
        }

        Ok(())
    }
}

impl KitMeshExtension for BodySkinExtension {
    const MATERIAL: &'static str = "skin_body";
}

/// A bought hair's cards: their soft cut-out and softened strand normals.
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug, Default)]
struct HairExtension {}

impl MaterialExtension for HairExtension {
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.push("HAIR_CARDS".into());
        }

        Ok(())
    }
}

impl KitMeshExtension for HairExtension {
    const MATERIAL: &'static str = HAIR_MATERIAL;
}

/// Lash strands: their vertex shader and its attribute.
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug, Default)]
struct LashExtension {}

impl MaterialExtension for LashExtension {
    fn vertex_shader() -> ShaderRef {
        "shaders/lighting/lash_strand.wesl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let across = layout
            .0
            .get_layout(&[STRAND_ACROSS.at_shader_location(STRAND_LOCATION)])?;

        if let Some(buffer) = descriptor.vertex.buffers.first_mut() {
            buffer.attributes.extend(across.attributes);
        }

        Ok(())
    }
}

impl KitMeshExtension for LashExtension {
    const MATERIAL: &'static str = "eye_lash";
}

pub(crate) struct CharacterSkinPlugin;

impl Plugin for CharacterSkinPlugin {
    fn build(&self, app: &mut App) {
        // Nothing to paint in apps without a renderer (headless gameplay).
        if app.get_sub_app(RenderApp).is_none() {
            return;
        }

        app.add_plugins((
            MaterialPlugin::<SkinMaterial>::default(),
            MaterialPlugin::<ExtendedMaterial<EnvironmentMaterial, BodySkinExtension>>::default(),
            MaterialPlugin::<ExtendedMaterial<EnvironmentMaterial, LashExtension>>::default(),
            MaterialPlugin::<ExtendedMaterial<EnvironmentMaterial, HairExtension>>::default(),
        ))
        .add_systems(Startup, make_blank_mask)
        .add_systems(
            PostUpdate,
            (
                take_skin_material,
                paint_heads,
                take_kit_material::<BodySkinExtension>,
                take_kit_material::<LashExtension>,
                take_kit_material::<HairExtension>,
                tint_hair,
            )
                .chain()
                .after(EnvironmentMaterialSystems)
                .before(AssetEventSystems),
        );
    }
}

/// A head with its own material, and what that material shows.
#[derive(Component)]
struct PaintedHead {
    material: Handle<SkinMaterial>,
    /// Keeps the glb's material alive, as it is what the head's own was made from.
    _source: Handle<EnvironmentMaterial>,
    shown: SkinLook,
    /// The brow and hairline masks the look wants, until each has loaded.
    wanted_mask: Option<Handle<Image>>,
    wanted_hairline: Option<Handle<Image>>,
}

/// The hairline mask of a bare head: its strength is zero, and a material needs some texture.
#[derive(Resource)]
struct BlankMask(Handle<Image>);

fn make_blank_mask(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(BlankMask(images.add(Image::default())));
}

/// What loads a head's masks.
#[derive(SystemParam)]
struct MaskLoader<'w> {
    server: Res<'w, AssetServer>,
    blank: Res<'w, BlankMask>,
}

impl MaskLoader<'_> {
    /// The hairline mask a look wears (`hair_fit.py` writes one beside each style).
    fn hairline(&self, layers: &SkinLayers, look: &SkinLook) -> Handle<Image> {
        match worn_shape(layers, look) {
            Some(shape) => load_mask(&self.server, &shape.hairline_path()),
            None => self.blank.0.clone(),
        }
    }
}

/// A head waiting for its first masks.
#[derive(Component)]
struct UnpaintedHead {
    brow_mask: Handle<Image>,
    makeup: Handle<Image>,
    hairline: Handle<Image>,
}

fn load_mask(server: &AssetServer, path: &str) -> Handle<Image> {
    server
        .load_builder()
        .with_settings(|settings: &mut ImageLoaderSettings| {
            settings.is_srgb = false;
            settings.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::ClampToEdge,
                address_mode_v: ImageAddressMode::ClampToEdge,
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                mipmap_filter: ImageFilterMode::Linear,
                anisotropy_clamp: 8,
                ..default()
            });
        })
        .load(path.to_owned())
}

/// The kit's mask for this look's brow: an unknown brow takes its first.
fn brow_mask_path<'a>(layers: &'a SkinLayers, look: &SkinLook) -> Option<&'a str> {
    layers
        .brows
        .iter()
        .find(|brow| brow.label == look.brow)
        .or_else(|| layers.brows.first())
        .map(|brow| brow.mask.as_str())
}

fn linear(paint: SkinPaint) -> Vec4 {
    let [r, g, b] = paint.color;
    let color = LinearRgba::from(Color::srgb(r, g, b));
    Vec4::new(color.red, color.green, color.blue, paint.strength)
}

/// The hair's colour on the skin, as its cards look on the whole. A bare head paints none.
fn hairline_paint(look: &SkinLook) -> Vec4 {
    let paint = linear(look.hair);
    let strength = if look.hair_style == NO_HAIR {
        0.0
    } else {
        paint.w
    };
    (paint.truncate() * HAIR_CARD_LEVEL).extend(strength)
}

fn params(look: &SkinLook, rect: [f32; 4]) -> SkinLayerParams {
    SkinLayerParams {
        brow_rect: Vec4::from_array(rect),
        brow: linear(look.brow_paint),
        hairline: hairline_paint(look),
        eyeshadow: linear(look.eyeshadow),
        liner: linear(look.liner),
        blush: linear(look.blush),
        lipstick: linear(look.lipstick),
    }
}

/// Where a head's masks and look come from.
#[derive(SystemParam)]
struct HeadSources<'w, 's> {
    kits: Res<'w, Assets<CharacterKit>>,
    visuals: Query<'w, 's, (&'static CharacterKitRef, &'static ChildOf)>,
    appearances: Query<'w, 's, &'static CharacterAppearance>,
}

impl HeadSources<'_, '_> {
    fn layers(&self, visual: Entity) -> Option<&SkinLayers> {
        let (kit, _) = self.visuals.get(visual).ok()?;
        self.kits.get(kit.kit())?.skin_layers()
    }

    /// The look of the character this visual belongs to.
    fn look(&self, visual: Entity) -> SkinLook {
        self.visuals
            .get(visual)
            .ok()
            .and_then(|(_, parent)| self.appearances.get(parent.parent()).ok())
            .map(|appearance| appearance.look.clone())
            .unwrap_or_default()
    }
}

/// Heads still drawn with the material their glb's instances share.
type SharedHeads<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static KitMesh,
        &'static GltfMaterialName,
        &'static MeshMaterial3d<EnvironmentMaterial>,
        Option<&'static UnpaintedHead>,
    ),
    Without<PaintedHead>,
>;

/// Gives a head its own material once its masks have loaded.
fn take_skin_material(
    mut commands: Commands,
    loader: MaskLoader,
    source: Res<Assets<EnvironmentMaterial>>,
    images: Res<Assets<Image>>,
    mut target: ResMut<Assets<SkinMaterial>>,
    heads: SharedHeads,
    sources: HeadSources,
) {
    for (entity, kit_mesh, name, material, unpainted) in &heads {
        if name.0 != HEAD_MATERIAL {
            continue;
        }

        let Some(layers) = sources.layers(kit_mesh.visual) else {
            continue;
        };
        let look = sources.look(kit_mesh.visual);
        let Some(unpainted) = unpainted else {
            if let Some(path) = brow_mask_path(layers, &look) {
                commands.entity(entity).insert(UnpaintedHead {
                    brow_mask: load_mask(&loader.server, path),
                    makeup: load_mask(&loader.server, &layers.makeup),
                    hairline: loader.hairline(layers, &look),
                });
            }

            continue;
        };
        let masks = [&unpainted.brow_mask, &unpainted.makeup, &unpainted.hairline];
        let (Some(base), true) = (
            source.get(material),
            masks.into_iter().all(|mask| images.contains(mask)),
        ) else {
            continue;
        };
        let painted = target.add(SkinMaterial {
            base: base.clone(),
            extension: SkinLayersExtension {
                params: params(&look, layers.brow_rect),
                brow_mask: unpainted.brow_mask.clone(),
                makeup: unpainted.makeup.clone(),
                hairline: unpainted.hairline.clone(),
            },
        });
        commands
            .entity(entity)
            .remove::<(MeshMaterial3d<EnvironmentMaterial>, UnpaintedHead)>()
            .insert((
                PaintedHead {
                    material: painted.clone(),
                    _source: material.0.clone(),
                    shown: look,
                    wanted_mask: None,
                    wanted_hairline: None,
                },
                MeshMaterial3d(painted),
            ));
    }
}

/// Gives a character's meshes of this material the extension, once the kit has built them: a lash
/// mesh has its strand attribute only then.
fn take_kit_material<E: KitMeshExtension>(
    mut commands: Commands,
    source: Res<Assets<EnvironmentMaterial>>,
    mut target: ResMut<Assets<ExtendedMaterial<EnvironmentMaterial, E>>>,
    meshes: Query<(
        Entity,
        &KitMesh,
        &GltfMaterialName,
        &MeshMaterial3d<EnvironmentMaterial>,
    )>,
) where
    ExtendedMaterial<EnvironmentMaterial, E>: Material,
{
    for (entity, kit_mesh, name, material) in &meshes {
        if name.0 != E::MATERIAL || kit_mesh.built.is_none() {
            continue;
        }

        let Some(base) = source.get(material) else {
            continue;
        };
        let taken = target.add(ExtendedMaterial {
            base: base.clone(),
            extension: E::default(),
        });
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<EnvironmentMaterial>>()
            .insert(MeshMaterial3d(taken));
    }
}

/// Tints each character's hair cards its look's colour, which their grey texture multiplies.
fn tint_hair(
    sources: HeadSources,
    meshes: Query<(
        &KitMesh,
        &MeshMaterial3d<ExtendedMaterial<EnvironmentMaterial, HairExtension>>,
    )>,
    mut materials: ResMut<Assets<ExtendedMaterial<EnvironmentMaterial, HairExtension>>>,
) {
    for (kit_mesh, material) in &meshes {
        let [r, g, b] = sources.look(kit_mesh.visual).hair.color;
        let color = Color::srgb(r, g, b);

        // Read first: a material marked as changed is sent to the GPU again.
        if materials
            .get(&material.0)
            .is_some_and(|m| m.base.base.base_color != color)
            && let Some(mut changed) = materials.get_mut(&material.0)
        {
            changed.base.base.base_color = color;
        }
    }
}

/// Follows each head's look. A new brow's mask shows once it has loaded.
fn paint_heads(
    loader: MaskLoader,
    images: Res<Assets<Image>>,
    mut materials: ResMut<Assets<SkinMaterial>>,
    mut heads: Query<(&KitMesh, &mut PaintedHead)>,
    sources: HeadSources,
) {
    for (kit_mesh, mut head) in &mut heads {
        let Some(layers) = sources.layers(kit_mesh.visual) else {
            continue;
        };
        let look = sources.look(kit_mesh.visual);

        if look != head.shown {
            if look.brow != head.shown.brow
                && let Some(path) = brow_mask_path(layers, &look)
            {
                head.wanted_mask = Some(load_mask(&loader.server, path));
            }

            if look.hair_style != head.shown.hair_style {
                head.wanted_hairline = Some(loader.hairline(layers, &look));
            }

            if let Some(mut material) = materials.get_mut(&head.material) {
                material.extension.params = params(&look, layers.brow_rect);
            }

            head.shown = look;
        }

        if head
            .wanted_mask
            .as_ref()
            .is_some_and(|mask| images.contains(mask))
            && let Some(mask) = head.wanted_mask.take()
            && let Some(mut material) = materials.get_mut(&head.material)
        {
            material.extension.brow_mask = mask;
        }

        if head
            .wanted_hairline
            .as_ref()
            .is_some_and(|mask| images.contains(mask))
            && let Some(mask) = head.wanted_hairline.take()
            && let Some(mut material) = materials.get_mut(&head.material)
        {
            material.extension.hairline = mask;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layers() -> SkinLayers {
        serde_json::from_str(
            r#"{"brow_rect": [0.1, 0.2, 0.5, 0.1], "makeup": "m.ktx2", "hairs": [],
                "brows": [{"label": "Soft", "mask": "a.ktx2"}, {"label": "Bold", "mask": "b.ktx2"}]}"#,
        )
        .expect("the test layers parse")
    }

    #[test]
    fn a_look_finds_its_brow_or_takes_the_kits_first() {
        let layers = layers();
        let look = |brow: &str| SkinLook {
            brow: brow.into(),
            ..default()
        };
        assert_eq!(brow_mask_path(&layers, &look("Bold")), Some("b.ktx2"));
        assert_eq!(brow_mask_path(&layers, &look("")), Some("a.ktx2"));
    }

    #[test]
    fn paints_reach_the_shader_linear_with_their_strength() {
        let look = SkinLook {
            lipstick: SkinPaint {
                color: [1.0, 0.5, 0.0],
                strength: 0.25,
            },
            ..default()
        };
        let params = params(&look, [0.1, 0.2, 0.5, 0.1]);
        assert_eq!(params.brow_rect, Vec4::new(0.1, 0.2, 0.5, 0.1));
        assert!((params.lipstick.x - 1.0).abs() < 1e-5);
        assert!((params.lipstick.y - 0.214).abs() < 1e-2);
        assert_eq!(params.lipstick.w, 0.25);
    }
}
