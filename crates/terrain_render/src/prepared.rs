//! Optional shared albedo bakes plus bounded, page-local weight/macro textures.
//! Shared albedo selection is independent of the page control cache, so streaming
//! and control-budget fallback do not replace the surface pattern.
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use super::{TerrainMaterial, TerrainShadingMode, load_repeat_image};
use bevy::{
    asset::{AssetId, AssetLoader, LoadContext, LoadState, RenderAssetUsages, io::Reader},
    core_pipeline::schedule::camera_driver,
    image::{
        CompressedImageFormatSupport, CompressedImageFormats, ImageFilterMode, ImageSampler,
        ImageSamplerDescriptor,
    },
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
    /// Prefer the optional native ASTC bake when the rendering device supports it.
    /// Disable to retain the manifest's primary image (the desktop universal bake).
    pub prefer_native_astc: bool,
    /// Page texture payload only; shared albedo arrays and driver overhead are additional.
    pub max_bytes: u64,
}
impl Default for TerrainPreparedSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            prefer_native_astc: true,
            max_bytes: 24 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct TerrainPreparedSnapshot {
    pub enabled: bool,
    pub pages: usize,
    pub active: usize,
    /// Pages using the shared albedo, including pages with uncached controls.
    pub albedo_active: usize,
    pub bytes: u64,
    pub builds: u64,
    /// Shared albedo payload observed on the GPU, separate from page control bytes.
    pub albedo_bytes: u64,
    pub albedo_images: usize,
    pub astc_8x8_images: usize,
}
#[derive(Default)]
struct Shared {
    ready: HashSet<(AssetId<Image>, AssetId<Image>)>,
    ready_albedos: HashSet<AssetId<Image>>,
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
    #[serde(default)]
    astc_image: Option<String>,
    period: f32,
}
impl ManifestEntry {
    fn preferred_image(&self, native_astc: bool) -> &str {
        if native_astc {
            self.astc_image.as_deref().unwrap_or(&self.image)
        } else {
            &self.image
        }
    }
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
                    || e.image.is_empty()
                    || e.astc_image.as_ref().is_some_and(String::is_empty)
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
            source_only: false,
            cloud_parameters: atmosphere::clouds::fallback_parameters(),
            cloud_shadows: None,
            rain_shelter: None,
            shading_mode: TerrainShadingMode::Production,
            stochastic_cached: false,
            prepared: false,
            prepared_albedo: false,
            source_weights: Handle::default(),
            source_base_color_array: Handle::default(),
            weights: Handle::default(),
            base_color_array: Handle::default(),
            normal_material_array: Handle::default(),
            macro_variation: Handle::default(),
            stochastic_cache: Handle::default(),
            canopy_bounds: Vec4::ZERO,
            canopy_shading: Default::default(),
            canopy_coverage: None,
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
struct AlbedoEntry {
    image: Handle<Image>,
    period: f32,
    preferred_path: String,
}
#[derive(Resource, Default)]
struct Entries {
    pages: HashMap<AssetId<TerrainMaterial>, Entry>,
    manifest: Option<Handle<PreparedManifest>>,
    albedos: HashMap<String, AlbedoEntry>,
}
#[derive(Clone)]
struct Request {
    inputs: Inputs,
    output: AssetId<Image>,
    albedo: AssetId<Image>,
}
#[derive(Resource, Default, Clone, ExtractResource)]
struct Requests {
    controls: Vec<Request>,
    albedos: HashSet<AssetId<Image>>,
}

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
    formats: Option<Res<CompressedImageFormatSupport>>,
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
    let sources: HashSet<_> = materials
        .iter()
        .filter(|(_, material)| eligible(material))
        .filter_map(|(_, material)| {
            material
                .source_base_color_array
                .path()
                .map(ToString::to_string)
        })
        .collect();
    // Shared bakes must leave residency too, rather than accumulating across biomes.
    entries.albedos.retain(|source, _| sources.contains(source));
    let native_astc = config.prefer_native_astc
        && formats.is_some_and(|formats| formats.0.contains(CompressedImageFormats::ASTC_LDR));
    if config.enabled
        && let Some(manifest) = manifest
    {
        for source in sources {
            let Some(entry) = manifest.entries.iter().find(|e| e.source == source) else {
                continue;
            };
            let preferred = entry.preferred_image(native_astc);
            let albedo = entries
                .albedos
                .entry(source)
                .or_insert_with(|| AlbedoEntry {
                    image: load_repeat_image(&server, preferred, true),
                    period: entry.period,
                    preferred_path: preferred.to_owned(),
                });
            if albedo.preferred_path != preferred {
                albedo.image = load_repeat_image(&server, preferred, true);
                albedo.preferred_path = preferred.to_owned();
            }
            // Load only one variant normally. A missing optional bake falls back
            // once to the manifest's original image without retrying every frame.
            if preferred != entry.image
                && matches!(server.load_state(albedo.image.id()), LoadState::Failed(_))
                && albedo
                    .image
                    .path()
                    .is_some_and(|path| path.to_string() == preferred)
            {
                warn!(
                    "prepared terrain ASTC unavailable; falling back to {}",
                    entry.image
                );
                albedo.image = load_repeat_image(&server, &entry.image, true);
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
            if !eligible(m) {
                continue;
            }
            let Some(path) = m.source_base_color_array.path() else {
                continue;
            };
            let Some(albedo) = entries.albedos.get(&path.to_string()) else {
                continue;
            };
            if let Some(inputs) = Inputs::from_material(m) {
                wanted.insert(id, (inputs, albedo.image.clone(), albedo.period));
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
            if materials.get(id).is_some_and(|m| m.source_only)
                || entries.pages.contains_key(&id)
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
    let mut albedo_active = 0;
    for (id, m) in materials.iter() {
        let albedo = wanted.get(&id).filter(|(_, albedo, _)| {
            config.enabled
                && shared.ready_albedos.contains(&albedo.id())
                && server.is_loaded_with_dependencies(albedo.id())
        });
        // Keep the same albedo while page controls load, rebuild, or exceed the budget.
        // The temporary shader evaluates only weights/macro from their original inputs.
        let control = entries.pages.get(&id).filter(|entry| {
            albedo.is_some_and(|(_, albedo, _)| {
                shared.ready.contains(&(entry.image.id(), albedo.id()))
            })
        });
        let prepared = control.is_some();
        let prepared_albedo = albedo.is_some();
        let weights = control.map_or_else(|| m.source_weights.clone(), |e| e.image.clone());
        let (albedo, period) = albedo.map_or_else(
            || (m.source_base_color_array.clone(), 0.0),
            |(_, albedo, period)| (albedo.clone(), *period),
        );
        active += usize::from(prepared);
        albedo_active += usize::from(prepared_albedo);
        if m.prepared != prepared
            || m.prepared_albedo != prepared_albedo
            || m.weights != weights
            || m.base_color_array != albedo
            || m.settings.surface_layers.w != period
        {
            changes.push((id, prepared, prepared_albedo, weights, albedo, period));
        }
    }
    for (id, prepared, prepared_albedo, weights, albedo, period) in changes {
        let mut m = materials.get_mut(id).unwrap();
        m.prepared = prepared;
        m.prepared_albedo = prepared_albedo;
        m.weights = weights;
        m.base_color_array = albedo;
        m.settings.surface_layers.w = period;
    }
    requests.controls = entries
        .pages
        .iter()
        .map(|(id, e)| Request {
            inputs: e.inputs,
            output: e.image.id(),
            albedo: wanted[id].1.id(),
        })
        .collect();
    requests.albedos = entries
        .albedos
        .values()
        .map(|albedo| albedo.image.id())
        .collect();
    shared.snapshot.enabled = config.enabled;
    shared.snapshot.pages = entries.pages.len();
    shared.snapshot.active = active;
    shared.snapshot.albedo_active = albedo_active;
    shared.snapshot.bytes = bytes;
}

fn eligible(material: &TerrainMaterial) -> bool {
    material.settings.macro_settings.z >= 0.5
        && (material.settings.surface_layers.z <= 1.5 || material.settings.macro_settings.w >= 0.5)
        && matches!(
            material.shading_mode,
            TerrainShadingMode::Production | TerrainShadingMode::SurfaceUnlit
        )
}

fn albedo_payload_bytes(descriptor: &TextureDescriptor<'_>) -> u64 {
    let (block_width, block_height) = descriptor.format.block_dimensions();
    let block_bytes = u64::from(descriptor.format.block_copy_size(None).unwrap_or(0));
    (0..descriptor.mip_level_count)
        .map(|mip| {
            u64::from((descriptor.size.width >> mip).max(1).div_ceil(block_width))
                * u64::from(
                    (descriptor.size.height >> mip)
                        .max(1)
                        .div_ceil(block_height),
                )
                * u64::from(descriptor.size.depth_or_array_layers)
                * block_bytes
        })
        .sum()
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
    let requested: HashMap<_, _> = requests
        .controls
        .iter()
        .map(|r| (r.output, r.albedo))
        .collect();
    built.0.retain(|id, _| requested.contains_key(id));
    shared
        .ready
        .retain(|(output, albedo)| requested.get(output) == Some(albedo));
    shared.ready_albedos = requests
        .albedos
        .iter()
        .copied()
        .filter(|id| images.get(*id).is_some())
        .collect();
    shared.snapshot.albedo_bytes = 0;
    shared.snapshot.albedo_images = 0;
    shared.snapshot.astc_8x8_images = 0;
    for image in requests.albedos.iter().filter_map(|id| images.get(*id)) {
        shared.snapshot.albedo_bytes += albedo_payload_bytes(&image.texture_descriptor);
        shared.snapshot.albedo_images += 1;
        shared.snapshot.astc_8x8_images += usize::from(matches!(
            image.texture_descriptor.format,
            TextureFormat::Astc {
                block: AstcBlock::B8x8,
                ..
            }
        ));
    }
    let Some(compute) = cache.get_compute_pipeline(pipeline.pipeline) else {
        shared.ready.clear();
        built.0.clear();
        return;
    };
    let mut count = 0;
    for request in &requests.controls {
        let ready_key = (request.output, request.albedo);
        let albedo_ready = images.get(request.albedo).is_some();
        if !albedo_ready {
            shared.ready.remove(&ready_key);
        }
        let (Some(weights), Some(macro_image), Some(output)) = (
            images.get(request.inputs.weights),
            images.get(request.inputs.macro_image),
            images.get(request.output),
        ) else {
            shared.ready.remove(&ready_key);
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
                shared.ready.insert(ready_key);
            }
            continue;
        }
        shared.ready.remove(&ready_key);
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
            shared.ready.insert(ready_key);
        }
        shared.snapshot.builds += 1;
        count += 1;
    }
}
