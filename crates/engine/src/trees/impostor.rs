//! Impostors: an object's final LOD drawn as one camera-facing quad per instance.
//!
//! The quad samples views baked over the upper hemisphere (YarraVegetation
//! `scripts/bake_impostor.py`, imported by `tools/import_vegetation_bundle.py`): the
//! vertex shader picks the four views nearest the camera's direction and the fragment
//! shader blends them, then lights the result with the trees' crown shading. Impostors come
//! from far-object blocks (`world_streaming::far_objects`) as batches, which the GPU trees
//! ([`super::gpu`]) draw with every other representation of their objects, the impostors
//! through this material from the trees' instance records.
use super::wind::WindBuffer;
use crate::object_lod::LodProjection;
use atmosphere::environment::{EnvironmentAssets, EnvironmentExtension, EnvironmentMaterial};

use bevy::{
    asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader},
    material::OpaqueRendererMethod,
    math::DVec2,
    mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology},
    pbr::{
        ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
        MaterialPlugin,
    },
    prelude::*,
    render::{
        render_resource::{
            AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
        },
        storage::ShaderBuffer,
    },
    shader::ShaderRef,
};

use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};

const SHADER: &str = "shaders/tree/impostor.wesl";

pub(crate) struct TreeImpostorPlugin;

impl Plugin for TreeImpostorPlugin {
    fn build(&self, app: &mut App) {
        // Headless LOD tests run without assets or rendering; there is nothing to draw.
        if !app.world().contains_resource::<AssetServer>() {
            return;
        }

        app.init_asset::<ImpostorDescriptor>()
            .init_asset_loader::<ImpostorLoader>()
            .init_resource::<ImpostorMaterials>();

        if app.get_sub_app(bevy::render::RenderApp).is_none() {
            return;
        }

        app.add_plugins(MaterialPlugin::<ImpostorMaterial>::default())
            .add_systems(
                PostUpdate,
                (follow_projection, complete_batches)
                    .chain()
                    .after(bevy::transform::TransformSystems::Propagate),
            )
            .add_systems(
                PostUpdate,
                super::forest_shadow::rebuild.after(bevy::transform::TransformSystems::Propagate),
            );
    }
}

/// Baked views of one asset: `views` x `views` cells over the upper hemisphere, each an
/// orthographic square of side `2 * radius` around `centre` (object space). No view covers
/// anything outside `crop` (u0, v0, u1, v1 across a view, v down), so quads span only that
/// and the atlases store only that part of each view, as equal tiles. The normal atlas
/// holds the octahedral normal (RG), the surface's depth toward the viewer (B, of `radius`)
/// and crown occlusion (A).
#[derive(Asset, TypePath, Debug)]
pub struct ImpostorDescriptor {
    pub views: u32,
    pub centre: Vec3,
    pub radius: f32,
    pub crop: Vec4,
    /// Texels per object metre in every view: a view's `cell` texels span `2 * radius`.
    pub texels_per_metre: f32,
    /// The foliage, for distant forest shadows (`super::forest_shadow`).
    pub crown: ImpostorCrown,
    /// The far mesh LOD's structural wind profile and stem height (object metres), so the
    /// quad leans as that trunk does (`shaders/tree/wind.wesl`, `sway_point_of_trunk`).
    pub wind_profile: Vec4,
    pub wind_height: f32,
    pub albedo: Handle<Image>,
    pub normal: Handle<Image>,
}

/// A crown as an upright ellipsoid of foliage, in object space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImpostorCrown {
    /// Horizontal centre (X, Z).
    pub centre: Vec2,
    pub bottom: f32,
    pub top: f32,
    pub radius: f32,
    /// Share of the crown's silhouette that foliage covers.
    pub opacity: f32,
}

impl ImpostorCrown {
    fn valid(&self) -> bool {
        [
            self.centre.x,
            self.centre.y,
            self.bottom,
            self.top,
            self.radius,
        ]
        .iter()
        .all(|v| v.is_finite())
            && self.top > self.bottom
            && self.radius > 0.0
            && (0.0..=1.0).contains(&self.opacity)
    }
}

#[derive(Deserialize)]
struct CrownFile {
    centre: [f32; 2],
    bottom: f32,
    top: f32,
    radius: f32,
    opacity: f32,
}

#[derive(Deserialize)]
struct WindFile {
    profile: [f32; 4],
    height: f32,
}

#[derive(Deserialize)]
struct DescriptorFile {
    version: u32,
    views: u32,
    cell: u32,
    centre: [f32; 3],
    radius: f32,
    crop: [f32; 4],
    crown: CrownFile,
    wind: WindFile,
    alpha_cutoff: f32,
    albedo: String,
    normal: String,
}

#[derive(Default, TypePath)]
struct ImpostorLoader;

impl AssetLoader for ImpostorLoader {
    type Asset = ImpostorDescriptor;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let invalid =
            |message: String| std::io::Error::new(std::io::ErrorKind::InvalidData, message);
        let file: DescriptorFile =
            serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;

        if file.version != 3
            || !(2..=32).contains(&file.views)
            || file.cell == 0
            || !file.radius.is_finite()
            || file.radius <= 0.0
            || file.centre.iter().any(|v| !v.is_finite())
            || !(0.0..=1.0).contains(&file.crop[0])
            || !(0.0..=1.0).contains(&file.crop[1])
            || !(file.crop[0] < file.crop[2] && file.crop[2] <= 1.0)
            || !(file.crop[1] < file.crop[3] && file.crop[3] <= 1.0)
            || file.alpha_cutoff != 0.5
            || file.albedo.contains(['/', '\\'])
            || file.normal.contains(['/', '\\'])
            || file.wind.profile.iter().any(|v| !v.is_finite())
            || !(file.wind.height.is_finite() && file.wind.height > 0.0)
        {
            return Err(invalid(
                "invalid impostor descriptor (version 3, 2-32 views, a view's size, positive radius, crop within a view, 0.5 cut-out, wind)".into(),
            ));
        }

        let c = &file.crown;
        let crown = ImpostorCrown {
            centre: Vec2::from(c.centre),
            bottom: c.bottom,
            top: c.top,
            radius: c.radius,
            opacity: c.opacity,
        };

        if !crown.valid() {
            return Err(invalid("invalid impostor crown".into()));
        }

        let folder = context
            .path()
            .path()
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        Ok(ImpostorDescriptor {
            views: file.views,
            centre: Vec3::from(file.centre),
            radius: file.radius,
            crop: Vec4::from(file.crop),
            texels_per_metre: file.cell as f32 / (2.0 * file.radius),
            crown,
            wind_profile: Vec4::from(file.wind.profile),
            wind_height: file.wind.height,
            albedo: context.load(folder.join(file.albedo)),
            // Normal, depth and occlusion are data: a UASTC atlas decodes as sRGB unless told
            // otherwise, whatever its transfer function says.
            normal: context
                .load_builder()
                .with_settings(|settings: &mut bevy::image::ImageLoaderSettings| {
                    settings.is_srgb = false;
                })
                .load(folder.join(file.normal)),
        })
    }

    fn extensions(&self) -> &[&str] {
        &["impostor.json"]
    }
}

/// One object's impostor within a batch, relative to the batch entity.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ImpostorInstance {
    pub(crate) translation: Vec3,
    pub(crate) yaw: f32,
    pub(crate) scale: f32,
}

/// A block's instances of one impostor; drawn once its descriptor and textures have loaded.
#[derive(Component)]
pub(crate) struct ImpostorBatch {
    pub(crate) descriptor: Handle<ImpostorDescriptor>,
    pub(crate) instances: Vec<ImpostorInstance>,
    /// The batch's origin in world metres (x, z), by which its instances are matched to the
    /// placed objects they draw ([`super::gpu::DrawnOnGpu`]); `None` for a lab tree.
    pub(crate) world: Option<DVec2>,
    /// The object's whole LOD chain, which the GPU trees draw ([`super::gpu`]).
    pub(crate) form: Arc<TreeFormSpec>,
}

/// An object's LOD chain as a far-object block carries it: its impostor's descriptor after
/// mesh LODs, and its height at unit scale.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TreeFormSpec {
    pub(crate) impostor: String,
    pub(crate) height: f32,
    /// Each mesh LOD's scene and minimum projected height, nearest first.
    pub(crate) meshes: Vec<(String, f32)>,
}

impl TreeFormSpec {
    /// Each representation's minimum projected height, nearest first, the impostor's 0
    /// last. The impostor takes over at `handoff` ([`LodProjection::impostor_threshold`]),
    /// in place of the last mesh LOD's own threshold, which only the cells' entities use; a
    /// mesh LOD whose threshold lies below it gets no band.
    pub(crate) fn thresholds(&self, handoff: f32) -> Vec<f32> {
        let last = self.meshes.len().saturating_sub(1);
        self.meshes
            .iter()
            .enumerate()
            .map(|(lod, &(_, threshold))| {
                if lod == last {
                    handoff
                } else {
                    threshold.max(handoff)
                }
            })
            .chain([0.0])
            .collect()
    }
}

/// An [`ImpostorBatch`] whose descriptor loaded: feeding the forest shadows.
#[derive(Component)]
pub(crate) struct ImpostorBatchDone;

pub(crate) type ImpostorMaterial = ExtendedMaterial<EnvironmentMaterial, ImpostorExtension>;

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub(crate) struct ImpostorExtension {
    // StandardMaterial uses 0-99, tree wind 100-105, EnvironmentMaterial 120-129.
    #[uniform(130)]
    params: ImpostorParams,
    #[texture(131)]
    #[sampler(132)]
    albedo: Handle<Image>,
    #[texture(133)]
    normal: Handle<Image>,
    /// The trees' wind ([`WindBuffer`]), so quads lean as their mesh LODs' trunks do.
    #[storage(135, read_only)]
    wind: Handle<ShaderBuffer>,
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct ImpostorParams {
    /// Object-space centre of the baked views and their half size.
    centre_radius: Vec4,
    /// The part of every view any view covers: u0, v0, u1, v1 (v down).
    crop: Vec4,
    /// Views per side, pixels per metre of the LOD projection, 1 for an orthographic
    /// camera, unused.
    settings: Vec4,
    /// The far mesh LOD's wind profile ([`ImpostorDescriptor::wind_profile`]).
    wind_profile: Vec4,
    /// Its stem height in object metres, the views' texels per metre and the LOD
    /// projection's output pixels per pixel: instanced quads take their hand-off at unit
    /// scale (output pixels over texels, [`LodProjection::impostor_threshold`]) from here
    /// rather than from their vertices.
    wind_height: Vec4,
}

impl MaterialExtension for ImpostorExtension {
    // Impostors draw into the G-buffer of deferred views (`gpu::render` queues them; never into
    // a depth prepass, so their alpha-tested quads are not drawn twice). They cast no shadows
    // (`NotShadowCaster`).
    fn enable_prepass() -> bool {
        true
    }

    fn enable_shadows() -> bool {
        false
    }

    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn deferred_vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn deferred_fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // The quad's corner is its vertex position's x; everything else is per instance.
        descriptor.layout.push(super::gpu::instance_layout());

        // Quads always face the camera; lit with the trees' crown shading.
        descriptor.primitive.cull_mode = None;

        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.push("TREE_CROWN_SHADING".into());

            // Impostors carry their mesh's crown occlusion: linear sky visibility since every
            // impostor-baked tree exports crown_sky_v2.
            fragment.shader_defs.push("TREE_CROWN_SKY".into());
            fragment.shader_defs.push("IMPOSTOR_SELF_SHADOW".into());
        }

        Ok(())
    }
}

/// Materials by impostor, so every block's batch of an impostor shares one.
#[derive(Resource, Default)]
pub(crate) struct ImpostorMaterials {
    by_descriptor: HashMap<AssetId<ImpostorDescriptor>, Handle<ImpostorMaterial>>,
    pixels_per_metre: f32,
    orthographic: bool,
    output_scale: f32,
}

/// What every impostor material is made with.
pub(crate) struct ImpostorMaterialInputs<'a> {
    pub(crate) environment: &'a EnvironmentAssets,
    pub(crate) wind: &'a WindBuffer,
}

impl ImpostorMaterials {
    /// The material of `descriptor`'s impostor, made on first use.
    pub(crate) fn get_or_add(
        &mut self,
        materials: &mut Assets<ImpostorMaterial>,
        handle: &Handle<ImpostorDescriptor>,
        descriptor: &ImpostorDescriptor,
        inputs: &ImpostorMaterialInputs,
    ) -> Handle<ImpostorMaterial> {
        let (ppm, orthographic, output_scale) =
            (self.pixels_per_metre, self.orthographic, self.output_scale);
        self.by_descriptor
            .entry(handle.id())
            .or_insert_with(|| {
                let environment = inputs.environment;
                materials.add(ImpostorMaterial {
                    base: EnvironmentMaterial {
                        base: StandardMaterial {
                            alpha_mode: AlphaMode::Mask(0.5),
                            perceptual_roughness: 0.85,
                            reflectance: 0.25,
                            opaque_render_method: OpaqueRendererMethod::Deferred,
                            ..default()
                        },
                        extension: EnvironmentExtension {
                            parameters: environment.parameters.clone(),
                            shadows: environment.shadows.clone(),
                            shelter: environment.shelter.clone(),
                            forest_shadow: environment.forest_shadow.clone(),
                            terrain_shadow: environment.terrain_shadow.clone(),
                            ground_albedo: environment.ground_albedo.clone(),
                        },
                    },
                    extension: ImpostorExtension {
                        params: ImpostorParams {
                            centre_radius: descriptor.centre.extend(descriptor.radius),
                            crop: descriptor.crop,
                            settings: Vec4::new(
                                descriptor.views as f32,
                                ppm,
                                f32::from(u8::from(orthographic)),
                                0.0,
                            ),
                            wind_profile: descriptor.wind_profile,
                            wind_height: Vec4::new(
                                descriptor.wind_height,
                                descriptor.texels_per_metre,
                                output_scale,
                                0.0,
                            ),
                        },
                        albedo: descriptor.albedo.clone(),
                        normal: descriptor.normal.clone(),
                        wind: inputs.wind.0.clone(),
                    },
                })
            })
            .clone()
    }
}

/// Keeps impostor switch distances equal to the mesh LODs' as window, display, field of
/// view or object detail change.
fn follow_projection(
    projection: Res<LodProjection>,
    mut cache: ResMut<ImpostorMaterials>,
    mut materials: ResMut<Assets<ImpostorMaterial>>,
) {
    let (ppm, orthographic, output_scale) = (
        projection.pixels_per_metre(),
        projection.orthographic(),
        projection.output_scale(),
    );

    if ppm == cache.pixels_per_metre
        && orthographic == cache.orthographic
        && output_scale == cache.output_scale
    {
        return;
    }

    cache.pixels_per_metre = ppm;
    cache.orthographic = orthographic;
    cache.output_scale = output_scale;

    for handle in cache.by_descriptor.values() {
        if let Some(mut material) = materials.get_mut(handle) {
            let params = &mut material.extension.params;
            params.settings.y = ppm;
            params.settings.z = f32::from(u8::from(orthographic));
            params.wind_height.z = output_scale;
        }
    }
}

/// Marks batches whose descriptor loaded, for the forest shadows; the GPU trees draw them.
fn complete_batches(
    mut commands: Commands,
    server: Res<AssetServer>,
    batches: Query<(Entity, &ImpostorBatch), Without<ImpostorBatchDone>>,
) {
    for (entity, batch) in &batches {
        match server.recursive_dependency_load_state(&batch.descriptor) {
            bevy::asset::RecursiveDependencyLoadState::Loaded => {}
            bevy::asset::RecursiveDependencyLoadState::Failed(error) => {
                warn!(
                    "impostor {:?} failed to load: {error}",
                    batch.descriptor.path()
                );
            }
            _ => continue,
        }

        commands.entity(entity).insert(ImpostorBatchDone);
    }
}

/// Quad vertices per instance: bottom, middle and top rows of two, so the quad bends along
/// its height as a trunk does in the wind, not just shears.
const QUAD_VERTICES: usize = 6;

/// The farthest a point moves in the wind, as a share of its height (the shader's lean is
/// clamped to 0.8 rad, which moves a point on the bent stem at most 0.39 of its height).
const WIND_REACH: f32 = 0.4;

/// One instance's quad for the GPU trees: the corner index as each vertex's x, the rest from
/// the instance (`shaders/tree/impostor.wesl`).
pub(crate) fn instanced_quad() -> Mesh {
    let positions: Vec<[f32; 3]> = (0..QUAD_VERTICES).map(|i| [i as f32, 0.0, 0.0]).collect();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_indices(Indices::U32(vec![0, 3, 1, 0, 2, 3, 2, 5, 3, 2, 4, 5]));
    mesh
}

/// The farthest an impostor's quad reaches from its root at unit scale, in any orientation
/// and wind.
pub(crate) fn quad_reach(descriptor: &ImpostorDescriptor) -> f32 {
    descriptor.centre.length()
        + descriptor.radius * std::f32::consts::SQRT_2
        + WIND_REACH * (descriptor.centre.y + descriptor.radius).max(0.0)
}
