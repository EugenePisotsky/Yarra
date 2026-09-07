//! Optional shared albedo bakes plus bounded, page-local weight/macro textures.
//! The reference material remains available until both resources are usable.
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use super::{TerrainMaterial, TerrainShadingMode, load_repeat_image};
use bevy::{
    asset::{AssetId, AssetLoader, LoadContext, RenderAssetUsages, io::Reader},
    core_pipeline::schedule::camera_driver,
    image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::*,
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        texture::GpuImage,
    },
};
use serde::Deserialize;

const MANIFEST: &str = "packs/terrain/prepared.terrain-prepared";
const SIDE: u32 = 272;
const MIPS: u32 = 4;
const PAGE_BYTES: u64 = (272 * 272 + 136 * 136 + 68 * 68 + 34 * 34) * 4;
const BUILDS_PER_FRAME: usize = 2;

#[derive(Resource, Clone, Debug)]
pub struct TerrainPreparedSettings {
    pub enabled: bool,
    /// Page texture payload only; shared albedo arrays and driver overhead are additional.
    pub max_bytes: u64,
}
impl Default for TerrainPreparedSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            max_bytes: 24 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct TerrainPreparedSnapshot {
    pub enabled: bool,
    pub pages: usize,
    pub active: usize,
    pub bytes: u64,
    pub builds: u64,
}
#[derive(Default)]
struct Shared {
    ready: HashSet<AssetId<Image>>,
    snapshot: TerrainPreparedSnapshot,
}
#[derive(Resource, Clone, Default)]
pub struct TerrainPreparedStats(Arc<Mutex<Shared>>);
impl TerrainPreparedStats {
    pub fn snapshot(&self) -> TerrainPreparedSnapshot {
        self.0.lock().unwrap().snapshot
    }
}

#[derive(Asset, TypePath, Debug, Deserialize)]
struct PreparedManifest {
    version: u32,
    entries: Vec<ManifestEntry>,
}
#[derive(Debug, Deserialize)]
struct ManifestEntry {
    source: String,
    image: String,
    period: f32,
}
#[derive(Default, TypePath)]
struct ManifestLoader;
#[derive(Debug, thiserror::Error)]
enum ManifestError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Ron(#[from] ron::error::SpannedError),
    #[error("invalid terrain prepared manifest (version, period, or duplicate source)")]
    Invalid,
}
impl AssetLoader for ManifestLoader {
    type Asset = PreparedManifest;
    type Settings = ();
    type Error = ManifestError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let manifest: PreparedManifest = ron::de::from_bytes(&bytes)?;
        let mut sources = HashSet::new();
        if manifest.version != 1
            || manifest.entries.iter().any(|e| {
                !e.period.is_finite()
                    || !(2.0..=16.0).contains(&e.period)
                    || !sources.insert(&e.source)
            })
        {
            return Err(ManifestError::Invalid);
        }
        Ok(manifest)
    }
    fn extensions(&self) -> &[&str] {
        &["terrain-prepared"]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Inputs {
    weights: AssetId<Image>,
    macro_image: AssetId<Image>,
    minimum: Vec2,
    extent: Vec2,
    scales: Vec3,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TerrainMaterialUniform;

    #[test]
    fn only_control_inputs_invalidate_and_non_finite_pages_fall_back() {
        let mut material = TerrainMaterial {
            shading_mode: TerrainShadingMode::Production,
            stochastic_cached: false,
            prepared: false,
            source_weights: Handle::default(),
            source_base_color_array: Handle::default(),
            weights: Handle::default(),
            base_color_array: Handle::default(),
            normal_material_array: Handle::default(),
            macro_variation: Handle::default(),
            stochastic_cache: Handle::default(),
            settings: TerrainMaterialUniform {
                chunk_minimum: Vec2::new(-32.0, 64.0),
                chunk_extent: Vec2::splat(32.0),
                surface_layers: Vec4::ONE,
                tile_sizes: Vec4::ONE,
                normal_settings: Vec4::ONE,
                roughness_ranges: Vec4::ONE,
                macro_scales: Vec4::ONE,
                macro_settings: Vec4::ONE,
                cache_origins: Vec4::ZERO,
                cache_size: UVec4::ZERO,
            },
        };
        let inputs = Inputs::from_material(&material).unwrap();
        material.settings.macro_scales.w = 0.4;
        material.settings.macro_settings = Vec4::ZERO;
        material.settings.tile_sizes *= 2.0;
        assert_eq!(Inputs::from_material(&material), Some(inputs));
        material.settings.chunk_minimum.x += 32.0;
        assert_ne!(Inputs::from_material(&material), Some(inputs));
        material.settings.chunk_extent.x = f32::NAN;
        assert!(Inputs::from_material(&material).is_none());
    }
}
impl Inputs {
    fn from_material(m: &TerrainMaterial) -> Option<Self> {
        let s = &m.settings;
        (s.chunk_minimum.is_finite()
            && s.chunk_extent.is_finite()
            && s.chunk_extent.min_element() > 0.0
            && s.macro_scales.xyz().is_finite()
            && s.macro_scales.xyz().min_element() > 0.0)
            .then_some(Self {
                weights: m.source_weights.id(),
                macro_image: m.macro_variation.id(),
                minimum: s.chunk_minimum,
                extent: s.chunk_extent,
                scales: s.macro_scales.xyz(),
            })
    }
}
struct Entry {
    inputs: Inputs,
    image: Handle<Image>,
}
#[derive(Resource, Default)]
struct Entries {
    pages: HashMap<AssetId<TerrainMaterial>, Entry>,
    manifest: Option<Handle<PreparedManifest>>,
    albedos: HashMap<String, (Handle<Image>, f32)>,
}
#[derive(Clone)]
struct Request {
    inputs: Inputs,
    output: AssetId<Image>,
    albedo: AssetId<Image>,
}
#[derive(Resource, Default, Clone, ExtractResource)]
struct Requests(Vec<Request>);

pub(super) fn install(app: &mut App) {
    let stats = TerrainPreparedStats::default();
    app.init_resource::<TerrainPreparedSettings>()
        .init_resource::<Entries>()
        .init_resource::<Requests>()
        .init_asset::<PreparedManifest>()
        .init_asset_loader::<ManifestLoader>()
        .insert_resource(stats.clone())
        .add_plugins(ExtractResourcePlugin::<Requests>::default())
        .add_systems(
            PostUpdate,
            maintain.in_set(super::TerrainMaterialPreparation),
        );
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .insert_resource(stats)
            .init_resource::<Built>()
            .add_systems(RenderStartup, initialize)
            .add_systems(RenderGraph, build_controls.before(camera_driver));
    }
}

fn control_image() -> Image {
    let mut image = Image::new_target_texture(SIDE, SIDE, TextureFormat::Rgba8Unorm, None);
    image.data = None;
    image.copy_on_resize = false;
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING | TextureUsages::COPY_SRC;
    image.texture_descriptor.label = Some("terrain prepared weights and macro");
    image.texture_descriptor.mip_level_count = MIPS;
    image.asset_usage = RenderAssetUsages::RENDER_WORLD;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        lod_max_clamp: (MIPS - 1) as f32,
        ..default()
    });
    image
}

#[allow(clippy::too_many_arguments)]
fn maintain(
    config: Res<TerrainPreparedSettings>,
    stats: Res<TerrainPreparedStats>,
    server: Res<AssetServer>,
    manifests: Res<Assets<PreparedManifest>>,
    mut entries: ResMut<Entries>,
    mut requests: ResMut<Requests>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut events: MessageReader<AssetEvent<Image>>,
) {
    if config.enabled && entries.manifest.is_none() {
        entries.manifest = Some(server.load(MANIFEST));
    }
    let manifest = entries.manifest.as_ref().and_then(|h| manifests.get(h));
    if manifests.is_changed() {
        entries.albedos.clear();
    }
    if let Some(manifest) = manifest {
        for (_, material) in materials.iter() {
            let Some(path) = material.source_base_color_array.path() else {
                continue;
            };
            let source = path.to_string();
            if !entries.albedos.contains_key(&source)
                && let Some(entry) = manifest.entries.iter().find(|e| e.source == source)
            {
                // Resolve only the format used by resident materials. Loading every
                // manifest entry would allocate both desktop and iOS albedo arrays.
                entries.albedos.insert(
                    source,
                    (load_repeat_image(&server, &entry.image, true), entry.period),
                );
            }
        }
    }
    let modified: HashSet<_> = events
        .read()
        .filter_map(|e| match e {
            AssetEvent::Modified { id } => Some(*id),
            _ => None,
        })
        .collect();
    let mut wanted = HashMap::new();
    if !entries.albedos.is_empty() {
        for (id, m) in materials.iter() {
            // These bakes replace stochastic albedo, not deliberately plain surfaces.
            if m.settings.macro_settings.z < 0.5
                || (m.settings.surface_layers.z > 1.5 && m.settings.macro_settings.w < 0.5)
                || !matches!(
                    m.shading_mode,
                    TerrainShadingMode::Production | TerrainShadingMode::SurfaceUnlit
                )
            {
                continue;
            }
            let Some(path) = m.source_base_color_array.path() else {
                continue;
            };
            let Some((albedo, period)) = entries.albedos.get(&path.to_string()) else {
                continue;
            };
            if let Some(inputs) = Inputs::from_material(m) {
                wanted.insert(id, (inputs, albedo.clone(), *period));
            }
        }
    }
    let mut bytes = 0;
    entries.pages.retain(|id, entry| {
        let keep = wanted
            .get(id)
            .is_some_and(|(inputs, _, _)| *inputs == entry.inputs)
            && !modified.contains(&entry.inputs.weights)
            && !modified.contains(&entry.inputs.macro_image)
            && bytes + PAGE_BYTES <= config.max_bytes;
        if keep {
            bytes += PAGE_BYTES;
        } else {
            images.remove(entry.image.id());
        }
        keep
    });
    let mut allocated = 0;
    if config.enabled {
        for (&id, &(inputs, _, _)) in &wanted {
            if entries.pages.contains_key(&id)
                || allocated >= BUILDS_PER_FRAME
                || bytes + PAGE_BYTES > config.max_bytes
            {
                continue;
            }
            entries.pages.insert(
                id,
                Entry {
                    inputs,
                    image: images.add(control_image()),
                },
            );
            allocated += 1;
            bytes += PAGE_BYTES;
        }
    }
    let mut shared = stats.0.lock().unwrap();
    let mut changes = Vec::new();
    let mut active = 0;
    for (id, m) in materials.iter() {
        let candidate =
            entries
                .pages
                .get(&id)
                .zip(wanted.get(&id))
                .filter(|(e, (_, albedo, _))| {
                    config.enabled
                        && shared.ready.contains(&e.image.id())
                        && server.is_loaded_with_dependencies(albedo.id())
                });
        let (prepared, weights, albedo, period) = match candidate {
            Some((e, (_, albedo, period))) => (true, e.image.clone(), albedo.clone(), *period),
            None => (
                false,
                m.source_weights.clone(),
                m.source_base_color_array.clone(),
                0.0,
            ),
        };
        active += usize::from(prepared);
        if m.prepared != prepared
            || m.weights != weights
            || m.base_color_array != albedo
            || m.settings.surface_layers.w != period
        {
            changes.push((id, prepared, weights, albedo, period));
        }
    }
    for (id, prepared, weights, albedo, period) in changes {
        let mut m = materials.get_mut(id).unwrap();
        m.prepared = prepared;
        m.weights = weights;
        m.base_color_array = albedo;
        m.settings.surface_layers.w = period;
    }
    requests.0 = entries
        .pages
        .iter()
        .map(|(id, e)| Request {
            inputs: e.inputs,
            output: e.image.id(),
            albedo: wanted[id].1.id(),
        })
        .collect();
    shared.snapshot.enabled = config.enabled;
    shared.snapshot.pages = entries.pages.len();
    shared.snapshot.active = active;
    shared.snapshot.bytes = bytes;
}

#[derive(Clone, ShaderType)]
struct ControlUniform {
    minimum: Vec2,
    extent: Vec2,
    scales: Vec4,
    mip: UVec4,
}
#[derive(Resource)]
struct ControlPipeline {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
}
fn initialize(mut commands: Commands, server: Res<AssetServer>, cache: Res<PipelineCache>) {
    use binding_types::*;
    let layout = BindGroupLayoutDescriptor::new(
        "terrain control preparation",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<ControlUniform>(false),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_storage_2d(TextureFormat::Rgba8Unorm, StorageTextureAccess::WriteOnly),
            ),
        ),
    );
    let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("terrain control preparation".into()),
        layout: vec![layout.clone()],
        shader: server.load("shaders/terrain_prepare_control.wgsl"),
        entry_point: Some("prepare".into()),
        ..default()
    });
    commands.insert_resource(ControlPipeline { layout, pipeline });
}
#[derive(Resource, Default)]
struct Built(HashMap<AssetId<Image>, (TextureId, TextureId, TextureId, ComputePipelineId)>);

#[allow(clippy::too_many_arguments)]
fn build_controls(
    mut context: RenderContext,
    requests: Res<Requests>,
    stats: Res<TerrainPreparedStats>,
    pipeline: Res<ControlPipeline>,
    cache: Res<PipelineCache>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut built: ResMut<Built>,
) {
    let mut shared = stats.0.lock().unwrap();
    let requested: HashSet<_> = requests.0.iter().map(|r| r.output).collect();
    built.0.retain(|id, _| requested.contains(id));
    shared.ready.retain(|id| requested.contains(id));
    let Some(compute) = cache.get_compute_pipeline(pipeline.pipeline) else {
        shared.ready.clear();
        built.0.clear();
        return;
    };
    let mut count = 0;
    for request in &requests.0 {
        let albedo_ready = images.get(request.albedo).is_some();
        if !albedo_ready {
            shared.ready.remove(&request.output);
        }
        let (Some(weights), Some(macro_image), Some(output)) = (
            images.get(request.inputs.weights),
            images.get(request.inputs.macro_image),
            images.get(request.output),
        ) else {
            shared.ready.remove(&request.output);
            built.0.remove(&request.output);
            continue;
        };
        let key = (
            weights.texture.id(),
            macro_image.texture.id(),
            output.texture.id(),
            compute.id(),
        );
        if built.0.get(&request.output) == Some(&key) {
            if albedo_ready {
                shared.ready.insert(request.output);
            }
            continue;
        }
        shared.ready.remove(&request.output);
        if count >= BUILDS_PER_FRAME {
            continue;
        }
        for mip in 0..MIPS {
            let mut uniform = UniformBuffer::from(ControlUniform {
                minimum: request.inputs.minimum,
                extent: request.inputs.extent,
                scales: request.inputs.scales.extend(0.0),
                mip: UVec4::new(mip, 0, 0, 0),
            });
            uniform.write_buffer(&device, &queue);
            let view = output.texture.create_view(&TextureViewDescriptor {
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..default()
            });
            let bindings = device.create_bind_group(
                "terrain control preparation",
                &cache.get_bind_group_layout(&pipeline.layout),
                &BindGroupEntries::sequential((
                    uniform.binding().unwrap(),
                    &weights.texture_view,
                    &weights.sampler,
                    &macro_image.texture_view,
                    &macro_image.sampler,
                    &view,
                )),
            );
            let mut pass = context
                .command_encoder()
                .begin_compute_pass(&ComputePassDescriptor {
                    label: Some("terrain control preparation"),
                    ..default()
                });
            pass.set_pipeline(compute);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups((SIDE >> mip).div_ceil(8), (SIDE >> mip).div_ceil(8), 1);
        }
        // This graph work executes before camera draws; main-world activation is next frame.
        built.0.insert(request.output, key);
        if albedo_ready {
            shared.ready.insert(request.output);
        }
        shared.snapshot.builds += 1;
        count += 1;
    }
}
