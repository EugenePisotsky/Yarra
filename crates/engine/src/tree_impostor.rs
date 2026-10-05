//! Impostors: an object's final LOD drawn as one camera-facing quad per instance.
//!
//! The quad samples views baked over the upper hemisphere (YarraVegetation
//! `scripts/bake_impostor.py`, imported by `tools/import_vegetation_bundle.py`): the
//! vertex shader picks the four views nearest the camera's direction and the fragment
//! shader blends them, then lights the result with the trees' crown shading. Impostors are
//! drawn only from far-object blocks (`world_streaming::far_objects`), whose instances of
//! one impostor share a mesh, so distant trees cost no entities of their own. The meshes'
//! final LOD ends where the impostor starts, both dithering over the same band and both
//! limited to the [`ImpostorHandoff`] ([`crate::object_lod`]), so the handover neither
//! gaps nor doubles.
use crate::object_lod::{ImpostorHandoff, LodProjection};
use atmosphere::clouds::{CloudAssets, CloudExtension, CloudMaterial};
use bevy::{
    asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader},
    camera::{primitives::Aabb, visibility::NoAutoAabb},
    light::NotShadowCaster,
    mesh::{
        Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexFormat,
    },
    pbr::{
        ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
        MaterialPlugin,
    },
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    },
    shader::ShaderRef,
};
use serde::Deserialize;
use std::collections::HashMap;

const SHADER: &str = "shaders/tree_impostor.wgsl";
/// Per-vertex instance data: yaw, uniform scale, switch (height over threshold) and the
/// quad corner (0-3). Custom, so the main pass and the prepass read the same location.
const ATTRIBUTE_INSTANCE: MeshVertexAttribute = MeshVertexAttribute::new(
    "Impostor_Instance",
    0x5952_5241_494d_0001,
    VertexFormat::Float32x4,
);
const INSTANCE_LOCATION: u32 = 10;

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
            .add_systems(PostUpdate, (follow_projection, complete_batches).chain())
            .add_systems(
                PostUpdate,
                crate::forest_shadow::rebuild.after(bevy::transform::TransformSystems::Propagate),
            );
    }
}

/// Baked views of one asset: `views` x `views` cells over the upper hemisphere, each an
/// orthographic square of side `2 * radius` around `centre` (object space). No view covers
/// anything outside `crop` (u0, v0, u1, v1 across a view, v down), so quads span only that.
#[derive(Asset, TypePath, Debug)]
pub struct ImpostorDescriptor {
    pub views: u32,
    pub centre: Vec3,
    pub radius: f32,
    pub crop: Vec4,
    /// The foliage, for distant forest shadows (`crate::forest_shadow`).
    pub crown: ImpostorCrown,
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
    /// Older bakes have no crown: the upper 65% of the views' coverage, half opaque.
    fn from_crop(centre: Vec3, radius: f32, crop: [f32; 4]) -> Self {
        let top = centre.y + (1.0 - 2.0 * crop[1]) * radius;
        let base = centre.y + (1.0 - 2.0 * crop[3]) * radius;
        Self {
            centre: Vec2::new(centre.x, centre.z),
            bottom: base + 0.35 * (top - base),
            top,
            radius: (crop[2] - crop[0]) * radius * 0.8,
            opacity: 0.5,
        }
    }
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
struct DescriptorFile {
    version: u32,
    views: u32,
    centre: [f32; 3],
    radius: f32,
    #[serde(default = "full_view")]
    crop: [f32; 4],
    #[serde(default)]
    crown: Option<CrownFile>,
    alpha_cutoff: f32,
    albedo: String,
    normal: String,
}

fn full_view() -> [f32; 4] {
    [0.0, 0.0, 1.0, 1.0]
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
        if file.version != 1
            || !(2..=32).contains(&file.views)
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
        {
            return Err(invalid(
                "invalid impostor descriptor (version 1, 2-32 views, positive radius, crop within a view, 0.5 cut-out)".into(),
            ));
        }
        let crown = file.crown.as_ref().map_or_else(
            || ImpostorCrown::from_crop(Vec3::from(file.centre), file.radius, file.crop),
            |c| ImpostorCrown {
                centre: Vec2::from(c.centre),
                bottom: c.bottom,
                top: c.top,
                radius: c.radius,
                opacity: c.opacity,
            },
        );
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
            crown,
            albedo: context.load(folder.join(file.albedo)),
            normal: context.load(folder.join(file.normal)),
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
    /// Object height over the projected height where the impostor takes over. Times the
    /// LOD projection's pixels per metre, it is the switch distance.
    pub(crate) switch: f32,
}

/// A block's instances of one impostor; drawn once its descriptor and textures have loaded.
#[derive(Component)]
pub(crate) struct ImpostorBatch {
    pub(crate) descriptor: Handle<ImpostorDescriptor>,
    pub(crate) instances: Vec<ImpostorInstance>,
}

/// An [`ImpostorBatch`] whose descriptor loaded: drawn and feeding the forest shadows.
#[derive(Component)]
pub(crate) struct ImpostorBatchDone;

type ImpostorMaterial = ExtendedMaterial<CloudMaterial, ImpostorExtension>;

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
struct ImpostorExtension {
    // StandardMaterial uses 0-99, tree wind 100-105, CloudMaterial 120-123.
    #[uniform(130)]
    params: ImpostorParams,
    #[texture(131)]
    #[sampler(132)]
    albedo: Handle<Image>,
    #[texture(133)]
    normal: Handle<Image>,
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct ImpostorParams {
    /// Object-space centre of the baked views and their half size.
    centre_radius: Vec4,
    /// The part of every view any view covers: u0, v0, u1, v1 (v down).
    crop: Vec4,
    /// Views per side, pixels per metre of the LOD projection, 1 for an orthographic
    /// camera, and the farthest hand-off from the mesh LODs in metres.
    settings: Vec4,
}

impl MaterialExtension for ImpostorExtension {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }
    fn prepass_vertex_shader() -> ShaderRef {
        SHADER.into()
    }
    fn prepass_fragment_shader() -> ShaderRef {
        SHADER.into()
    }
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let attributes = layout
            .0
            .get_layout(&[ATTRIBUTE_INSTANCE.at_shader_location(INSTANCE_LOCATION)])?;
        if let Some(buffer) = descriptor.vertex.buffers.first_mut() {
            buffer.attributes.extend(attributes.attributes);
        }
        // Quads always face the camera; lit with the trees' crown shading.
        descriptor.primitive.cull_mode = None;
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.push("TREE_CROWN_SHADING".into());
        }
        Ok(())
    }
}

/// Materials by impostor, so every block's batch of an impostor shares one.
#[derive(Resource, Default)]
struct ImpostorMaterials {
    by_descriptor: HashMap<AssetId<ImpostorDescriptor>, Handle<ImpostorMaterial>>,
    pixels_per_metre: f32,
    orthographic: bool,
    handoff: f32,
}

/// Keeps impostor switch distances equal to the mesh LODs' as window, field of view,
/// object detail or the hand-off change.
fn follow_projection(
    projection: Res<LodProjection>,
    handoff: Res<ImpostorHandoff>,
    mut cache: ResMut<ImpostorMaterials>,
    mut materials: ResMut<Assets<ImpostorMaterial>>,
) {
    let (ppm, orthographic) = (projection.pixels_per_metre(), projection.orthographic());
    let handoff = handoff.metres();
    if ppm == cache.pixels_per_metre
        && orthographic == cache.orthographic
        && handoff == cache.handoff
    {
        return;
    }
    cache.pixels_per_metre = ppm;
    cache.orthographic = orthographic;
    cache.handoff = handoff;
    for handle in cache.by_descriptor.values() {
        if let Some(mut material) = materials.get_mut(handle) {
            let settings = &mut material.extension.params.settings;
            settings.y = ppm;
            settings.z = f32::from(u8::from(orthographic));
            settings.w = handoff;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn complete_batches(
    mut commands: Commands,
    server: Res<AssetServer>,
    clouds: Option<Res<CloudAssets>>,
    descriptors: Res<Assets<ImpostorDescriptor>>,
    mut cache: ResMut<ImpostorMaterials>,
    mut materials: ResMut<Assets<ImpostorMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    batches: Query<(Entity, &ImpostorBatch), Without<ImpostorBatchDone>>,
) {
    let Some(clouds) = clouds else {
        return;
    };
    for (entity, batch) in &batches {
        match server.recursive_dependency_load_state(&batch.descriptor) {
            bevy::asset::RecursiveDependencyLoadState::Loaded => {}
            bevy::asset::RecursiveDependencyLoadState::Failed(error) => {
                warn!(
                    "impostor {:?} failed to load: {error}",
                    batch.descriptor.path()
                );
                commands.entity(entity).insert(ImpostorBatchDone);
                continue;
            }
            _ => continue,
        }
        let Some(descriptor) = descriptors.get(&batch.descriptor) else {
            continue;
        };
        let (ppm, orthographic, handoff) =
            (cache.pixels_per_metre, cache.orthographic, cache.handoff);
        let material = cache
            .by_descriptor
            .entry(batch.descriptor.id())
            .or_insert_with(|| {
                materials.add(ImpostorMaterial {
                    base: CloudMaterial {
                        base: StandardMaterial {
                            alpha_mode: AlphaMode::Mask(0.5),
                            perceptual_roughness: 0.85,
                            reflectance: 0.25,
                            ..default()
                        },
                        extension: CloudExtension {
                            parameters: clouds.parameters.clone(),
                            shadows: clouds.shadows.clone(),
                            shelter: clouds.shelter.clone(),
                            forest_shadow: clouds.forest_shadow.clone(),
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
                                handoff,
                            ),
                        },
                        albedo: descriptor.albedo.clone(),
                        normal: descriptor.normal.clone(),
                    },
                })
            })
            .clone();
        let (mesh, aabb) = batch_mesh(&batch.instances, descriptor);
        commands.entity(entity).insert((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            aabb,
            NoAutoAabb,
            NotShadowCaster,
            ImpostorBatchDone,
        ));
    }
}

/// Four vertices per instance at its root; the vertex shader spreads them into a quad
/// facing the camera. Bounds hold every quad in any orientation.
fn batch_mesh(instances: &[ImpostorInstance], descriptor: &ImpostorDescriptor) -> (Mesh, Aabb) {
    let mut positions = Vec::with_capacity(instances.len() * 4);
    let mut data = Vec::with_capacity(instances.len() * 4);
    let mut indices = Vec::with_capacity(instances.len() * 6);
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    let reach = descriptor.centre.length() + descriptor.radius * std::f32::consts::SQRT_2;
    for (i, instance) in instances.iter().enumerate() {
        let base = (i * 4) as u32;
        for corner in 0..4 {
            positions.push(instance.translation.to_array());
            data.push([instance.yaw, instance.scale, instance.switch, corner as f32]);
        }
        indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
        let r = Vec3::splat(reach * instance.scale);
        lo = lo.min(instance.translation - r);
        hi = hi.max(instance.translation + r);
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(ATTRIBUTE_INSTANCE, data);
    mesh.insert_indices(Indices::U32(indices));
    (mesh, Aabb::from_min_max(lo, hi))
}
