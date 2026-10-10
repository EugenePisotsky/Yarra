//! Impostors: an object's final LOD drawn as one camera-facing quad per instance.
//!
//! The quad samples views baked over the upper hemisphere (YarraVegetation
//! `scripts/bake_impostor.py`, imported by `tools/import_vegetation_bundle.py`): the
//! vertex shader picks the four views nearest the camera's direction and the fragment
//! shader blends them, then lights the result with the trees' crown shading. Impostors are
//! drawn only from far-object blocks (`world_streaming::far_objects`), whose instances of
//! one impostor share a mesh, so distant trees cost no entities of their own. The meshes'
//! final LOD ends where the impostor starts, both dithering over the same band and both
//! limited to `object_lod::IMPOSTOR_HANDOFF_METRES`, so the handover neither gaps nor
//! doubles.
use crate::object_lod::{IMPOSTOR_HANDOFF_METRES, LodProjection};
use crate::{WorldCatalog, WorldOrigin, tree_wind::WindBuffer};
use atmosphere::environment::{EnvironmentAssets, EnvironmentExtension, EnvironmentMaterial};
use bevy::{
    asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader},
    camera::{primitives::Aabb, visibility::NoAutoAabb},
    light::NotShadowCaster,
    math::DVec2,
    mesh::MeshTag,
    mesh::{
        Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexFormat,
    },
    pbr::{
        ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
        MaterialPlugin,
    },
    prelude::*,
    render::{
        render_resource::{
            AsBindGroup, Buffer, BufferDescriptor, BufferUsages, RenderPipelineDescriptor,
            ShaderType, SpecializedMeshPipelineError,
        },
        renderer::{RenderDevice, RenderQueue},
        storage::ShaderBuffer,
    },
    shader::ShaderRef,
};
use serde::Deserialize;
use std::collections::HashMap;

const SHADER: &str = "shaders/tree_impostor.wesl";
/// Per-vertex instance data: yaw, uniform scale, switch (height over threshold) and the
/// quad corner (0-5) plus eight times the instance's index within the batch.
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
            .init_resource::<ImpostorMaterials>()
            .init_resource::<ImpostorFades>()
            .add_observer(release_slots);
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
                crate::forest_shadow::rebuild.after(bevy::transform::TransformSystems::Propagate),
            )
            .add_systems(Last, upload_fades);
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
    /// The foliage, for distant forest shadows (`crate::forest_shadow`).
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
                "invalid impostor descriptor (version 3, 2-32 views, positive radius, crop within a view, 0.5 cut-out, wind)".into(),
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

type ImpostorMaterial = ExtendedMaterial<EnvironmentMaterial, ImpostorExtension>;

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
struct ImpostorExtension {
    // StandardMaterial uses 0-99, tree wind 100-105, EnvironmentMaterial 120-125.
    #[uniform(130)]
    params: ImpostorParams,
    #[texture(131)]
    #[sampler(132)]
    albedo: Handle<Image>,
    #[texture(133)]
    normal: Handle<Image>,
    /// [`ImpostorFades`], shared by every impostor.
    #[storage(134, read_only, buffer)]
    fades: Buffer,
    /// The trees' wind ([`WindBuffer`]), so quads lean as their mesh LODs' trunks do.
    #[storage(135, read_only)]
    wind: Handle<ShaderBuffer>,
}

/// Instances whose fades the buffer holds; beyond it, new batches draw without fades.
const FADE_CAPACITY: u32 = 1 << 18;
/// The mesh tag of a batch that got no slots.
const NO_SLOTS: u32 = u32::MAX;

/// Each drawn impostor instance's fade, which the mesh LODs of its resident object set
/// ([`crate::object_lod`]) and the impostor shader reads: one u32 per instance, 0 while no
/// resident object controls it (drawn), otherwise 64 + its dither level (-16 hidden ... 0
/// drawn). A batch holds a contiguous range of slots, its first in the batch's mesh tag.
/// Objects find their instance's slot by world position.
#[derive(Resource, Default)]
pub(crate) struct ImpostorFades {
    buffer: Option<Buffer>,
    levels: Vec<u32>,
    /// The range of `levels` to upload, `[start, end)`.
    dirty: Option<(usize, usize)>,
    /// Free slot ranges, `[start, end)`.
    free: Vec<(u32, u32)>,
    by_position: HashMap<IVec2, u32>,
    /// Counts batch registrations and releases, so objects look their slot up again.
    generation: u64,
}

/// An object's horizontal world position in 5 cm steps: cells and far-object blocks place
/// the same object a few micrometres apart at most.
pub(crate) fn fade_key(world_xz: DVec2) -> IVec2 {
    let k = (world_xz * 20.0).round();
    IVec2::new(k.x as i32, k.y as i32)
}

impl ImpostorFades {
    fn buffer(&mut self, device: &RenderDevice) -> Buffer {
        if self.buffer.is_none() {
            self.levels = vec![0; FADE_CAPACITY as usize];
            self.free = vec![(0, FADE_CAPACITY)];
            self.buffer = Some(device.create_buffer(&BufferDescriptor {
                label: Some("impostor fades"),
                size: u64::from(FADE_CAPACITY) * 4,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.dirty = Some((0, FADE_CAPACITY as usize));
        }
        self.buffer.clone().unwrap()
    }

    fn allocate(&mut self, count: u32) -> Option<u32> {
        let i = self.free.iter().position(|(s, e)| e - s >= count)?;
        let (start, end) = self.free[i];
        if end - start == count {
            self.free.remove(i);
        } else {
            self.free[i].0 += count;
        }
        Some(start)
    }

    fn release(&mut self, base: u32, count: u32) {
        for slot in base..base + count {
            self.set(slot, None);
        }
        self.free.push((base, base + count));
        self.free.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(self.free.len());
        for (s, e) in self.free.drain(..) {
            match merged.last_mut() {
                Some(last) if last.1 == s => last.1 = e,
                _ => merged.push((s, e)),
            }
        }
        self.free = merged;
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The slot of the impostor instance at `world_xz`, if a drawn batch holds one.
    pub(crate) fn find(&self, world_xz: DVec2) -> Option<u32> {
        let key = fade_key(world_xz);
        self.by_position.get(&key).copied().or_else(|| {
            (-1..=1)
                .flat_map(|x| (-1..=1).map(move |z| key + IVec2::new(x, z)))
                .find_map(|k| self.by_position.get(&k).copied())
        })
    }

    /// Sets an instance's dither level, or releases it to draw on its own (`None`).
    pub(crate) fn set(&mut self, slot: u32, level: Option<i32>) {
        let Some(value) = self.levels.get_mut(slot as usize) else {
            return;
        };
        let new = level.map_or(0, |l| (64 + l.clamp(-16, 16)) as u32);
        if *value != new {
            *value = new;
            let i = slot as usize;
            self.dirty = Some(
                self.dirty
                    .map_or((i, i + 1), |(s, e)| (s.min(i), e.max(i + 1))),
            );
        }
    }
}

/// A batch's slots in [`ImpostorFades`], and the world keys that find them.
#[derive(Component)]
struct ImpostorSlots {
    base: u32,
    count: u32,
    keys: Vec<IVec2>,
}

fn release_slots(
    removed: On<Remove<ImpostorSlots>>,
    slots: Query<&ImpostorSlots>,
    mut fades: ResMut<ImpostorFades>,
) {
    let Ok(slots) = slots.get(removed.entity) else {
        return;
    };
    for (i, key) in slots.keys.iter().enumerate() {
        if fades.by_position.get(key) == Some(&(slots.base + i as u32)) {
            fades.by_position.remove(key);
        }
    }
    fades.release(slots.base, slots.count);
    fades.generation += 1;
}

fn upload_fades(mut fades: ResMut<ImpostorFades>, queue: Option<Res<RenderQueue>>) {
    let (Some(queue), Some((start, end))) = (queue, fades.dirty) else {
        return;
    };
    let Some(buffer) = fades.buffer.as_ref() else {
        return;
    };
    queue.write_buffer(
        buffer,
        start as u64 * 4,
        bytemuck::cast_slice(&fades.levels[start..end]),
    );
    fades.dirty = None;
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
    /// The far mesh LOD's wind profile ([`ImpostorDescriptor::wind_profile`]).
    wind_profile: Vec4,
    /// Its stem height in object metres.
    wind_height: Vec4,
}

impl MaterialExtension for ImpostorExtension {
    // Impostors stand beyond the mesh LODs, where their sway moves them far less than a pixel
    // a frame: MetalFX Temporal's motion for them comes from the final depth and the camera
    // (`upscaling::temporal::CompleteTemporalMotion`), and their alpha-tested cards are not
    // drawn twice. They cast no shadows (`NotShadowCaster`).
    fn enable_prepass() -> bool {
        false
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
            fragment.shader_defs.push("IMPOSTOR_SELF_SHADOW".into());
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
}

/// Keeps impostor switch distances equal to the mesh LODs' as window, field of view or
/// object detail change.
fn follow_projection(
    projection: Res<LodProjection>,
    mut cache: ResMut<ImpostorMaterials>,
    mut materials: ResMut<Assets<ImpostorMaterial>>,
) {
    let (ppm, orthographic) = (projection.pixels_per_metre(), projection.orthographic());
    if ppm == cache.pixels_per_metre && orthographic == cache.orthographic {
        return;
    }
    cache.pixels_per_metre = ppm;
    cache.orthographic = orthographic;
    for handle in cache.by_descriptor.values() {
        if let Some(mut material) = materials.get_mut(handle) {
            let settings = &mut material.extension.params.settings;
            settings.y = ppm;
            settings.z = f32::from(u8::from(orthographic));
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn complete_batches(
    mut commands: Commands,
    server: Res<AssetServer>,
    environment: Option<Res<EnvironmentAssets>>,
    descriptors: Res<Assets<ImpostorDescriptor>>,
    mut cache: ResMut<ImpostorMaterials>,
    mut materials: ResMut<Assets<ImpostorMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    batches: Query<(Entity, &ImpostorBatch, &GlobalTransform), Without<ImpostorBatchDone>>,
    (mut fades, device, origin, catalog, wind): (
        ResMut<ImpostorFades>,
        Res<RenderDevice>,
        Option<Res<WorldOrigin>>,
        Option<Res<WorldCatalog>>,
        Option<Res<WindBuffer>>,
    ),
) {
    // Impostors draw with the cloud-lit trees and their wind (TreeWindPlugin).
    let (Some(environment), Some(wind)) = (environment, wind) else {
        return;
    };
    let fade_buffer = fades.buffer(&device);
    for (entity, batch, transform) in &batches {
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
        let (ppm, orthographic) = (cache.pixels_per_metre, cache.orthographic);
        let material = cache
            .by_descriptor
            .entry(batch.descriptor.id())
            .or_insert_with(|| {
                materials.add(ImpostorMaterial {
                    base: EnvironmentMaterial {
                        base: StandardMaterial {
                            alpha_mode: AlphaMode::Mask(0.5),
                            perceptual_roughness: 0.85,
                            reflectance: 0.25,
                            ..default()
                        },
                        extension: EnvironmentExtension {
                            parameters: environment.parameters.clone(),
                            shadows: environment.shadows.clone(),
                            shelter: environment.shelter.clone(),
                            forest_shadow: environment.forest_shadow.clone(),
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
                                IMPOSTOR_HANDOFF_METRES,
                            ),
                            wind_profile: descriptor.wind_profile,
                            wind_height: Vec4::new(descriptor.wind_height, 0.0, 0.0, 0.0),
                        },
                        albedo: descriptor.albedo.clone(),
                        normal: descriptor.normal.clone(),
                        fades: fade_buffer.clone(),
                        wind: wind.0.clone(),
                    },
                })
            })
            .clone();
        // Register every instance's slot by world position, for its object's mesh LODs.
        let count = batch.instances.len() as u32;
        let base = match fades.allocate(count) {
            Some(base) => {
                let mut keys = Vec::with_capacity(batch.instances.len());
                for (i, instance) in batch.instances.iter().enumerate() {
                    let render = transform.translation() + instance.translation;
                    let key = origin
                        .as_ref()
                        .zip(catalog.as_ref())
                        .and_then(|(o, c)| o.to_world(c, render))
                        .map_or(IVec2::MAX, |(_, w)| fade_key(DVec2::new(w[0], w[2])));
                    fades.by_position.insert(key, base + i as u32);
                    keys.push(key);
                }
                commands
                    .entity(entity)
                    .insert(ImpostorSlots { base, count, keys });
                fades.generation += 1;
                base
            }
            None => {
                warn!("impostor fade slots exhausted; a batch of {count} draws without fades");
                NO_SLOTS
            }
        };
        let (mesh, aabb) = batch_mesh(&batch.instances, descriptor);
        commands.entity(entity).insert((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            MeshTag::new(base),
            aabb,
            NoAutoAabb,
            NotShadowCaster,
            ImpostorBatchDone,
        ));
    }
}

/// Quad vertices per instance: bottom, middle and top rows of two, so the quad bends along
/// its height as a trunk does in the wind, not just shears.
const QUAD_VERTICES: usize = 6;
/// The farthest a point moves in the wind, as a share of its height (the shader's lean is
/// clamped to 0.8 rad, which moves a point on the bent stem at most 0.39 of its height).
const WIND_REACH: f32 = 0.4;

/// [`QUAD_VERTICES`] vertices per instance at its root; the vertex shader spreads them into
/// a quad facing the camera. Bounds hold every quad in any orientation and wind.
fn batch_mesh(instances: &[ImpostorInstance], descriptor: &ImpostorDescriptor) -> (Mesh, Aabb) {
    let mut positions = Vec::with_capacity(instances.len() * QUAD_VERTICES);
    let mut data = Vec::with_capacity(instances.len() * QUAD_VERTICES);
    let mut indices = Vec::with_capacity(instances.len() * 12);
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    let reach = descriptor.centre.length()
        + descriptor.radius * std::f32::consts::SQRT_2
        + WIND_REACH * (descriptor.centre.y + descriptor.radius).max(0.0);
    for (i, instance) in instances.iter().enumerate() {
        let base = (i * QUAD_VERTICES) as u32;
        for corner in 0..QUAD_VERTICES {
            positions.push(instance.translation.to_array());
            // The corner, and the instance within the batch (its fade slot after the batch's).
            data.push([
                instance.yaw,
                instance.scale,
                instance.switch,
                (corner + 8 * i) as f32,
            ]);
        }
        // Corners 0-1 bottom, 2-3 middle, 4-5 top; even ones on the left.
        for row in [base, base + 2] {
            indices.extend([row, row + 3, row + 1, row, row + 2, row + 3]);
        }
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
