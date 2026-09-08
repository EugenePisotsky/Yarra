//! Small, exact-resolution lookup tables for the static anti-tiling lattice.
//!
//! These store transforms, not baked colour: texture detail and derivatives are unchanged.
//! Both paths share the GPU hash code, avoiding CPU/GPU floating-point hash disagreement.
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use bevy::{
    asset::{AssetId, RenderAssetUsages},
    core_pipeline::schedule::camera_driver,
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::*,
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        storage::{GpuShaderBuffer, ShaderBuffer},
    },
};

use super::{TerrainMaterial, TerrainMaterialUniform};

const CACHE_SHADER: &str = "shaders/terrain_stochastic_cache.wgsl";
const MAX_SIDE: u32 = 128;
const BUILDS_PER_FRAME: usize = 8;

#[derive(Resource, Clone, Debug)]
pub struct TerrainCacheSettings {
    pub enabled: bool,
    /// GPU buffer payload budget; driver allocation/texture metadata is additional.
    pub max_bytes: u64,
}

impl Default for TerrainCacheSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TerrainCacheSnapshot {
    pub enabled: bool,
    pub tables: usize,
    pub ready: usize,
    pub bytes: u64,
    pub builds: u64,
    pub reused_frames: u64,
}

#[derive(Default)]
struct SharedState {
    ready: HashSet<AssetId<ShaderBuffer>>,
    snapshot: TerrainCacheSnapshot,
}

#[derive(Resource, Clone, Default)]
pub struct TerrainCacheStats(Arc<Mutex<SharedState>>);

impl TerrainCacheStats {
    pub fn snapshot(&self) -> TerrainCacheSnapshot {
        self.0.lock().unwrap().snapshot
    }
}

pub(super) fn install(app: &mut App) {
    let stats = TerrainCacheStats::default();
    app.init_resource::<TerrainCacheSettings>()
        .init_resource::<CacheEntries>()
        .init_resource::<CacheRequests>()
        .insert_resource(stats.clone())
        .add_plugins(ExtractResourcePlugin::<CacheRequests>::default())
        .add_systems(
            PostUpdate,
            maintain.after(super::TerrainMaterialPreparation),
        );
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .insert_resource(stats)
            .init_resource::<BuiltTables>()
            .add_systems(RenderStartup, initialize)
            .add_systems(RenderGraph, build_tables.before(camera_driver));
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    origins: Vec4,
    layers: Vec4,
    size: UVec2,
}

impl Layout {
    fn for_material(settings: &TerrainMaterialUniform) -> Option<Self> {
        if settings.macro_settings.z < 0.5 && settings.macro_settings.w < 0.5 {
            return None;
        }
        let mut origins = [Vec2::ZERO; 2];
        let mut size = UVec2::ZERO;
        for (slot, tile) in [settings.tile_sizes.x, settings.tile_sizes.y]
            .into_iter()
            .enumerate()
        {
            if !tile.is_finite()
                || tile <= 0.0
                || !settings.chunk_extent.is_finite()
                || settings.chunk_extent.min_element() <= 0.0
                || !settings.chunk_minimum.is_finite()
            {
                return None;
            }
            let lattice = |world: Vec2| {
                let uv = world / tile.max(0.001);
                Vec2::new(uv.x + uv.y * 0.577_350_26, uv.y * 1.154_700_5)
            };
            // Positive lattice coefficients make opposite page corners its extrema.
            // One extra node on each side protects page-edge rasterization/roundoff.
            let minimum = lattice(settings.chunk_minimum).floor() - Vec2::ONE;
            let maximum =
                lattice(settings.chunk_minimum + settings.chunk_extent).floor() + Vec2::splat(2.0);
            let extent = maximum - minimum + Vec2::ONE;
            if !extent.is_finite()
                || extent.min_element() < 1.0
                || extent.max_element() > MAX_SIDE as f32
                || minimum.abs().max_element() > 8_000_000.0
            {
                return None;
            }
            origins[slot] = minimum;
            size = size.max(extent.as_uvec2());
        }
        Some(Self {
            origins: Vec4::new(origins[0].x, origins[0].y, origins[1].x, origins[1].y),
            layers: settings.surface_layers,
            size,
        })
    }

    fn bytes(self) -> u64 {
        u64::from(self.size.x) * u64::from(self.size.y) * 2 * 16
    }
}

struct Entry {
    layout: Layout,
    buffer: Handle<ShaderBuffer>,
}
#[derive(Resource, Default)]
struct CacheEntries(
    HashMap<AssetId<TerrainMaterial>, Entry>,
    Option<Handle<ShaderBuffer>>,
);

#[derive(Clone)]
struct Request {
    buffer: AssetId<ShaderBuffer>,
    layout: Layout,
}
#[derive(Resource, Clone, Default, ExtractResource)]
struct CacheRequests(Vec<Request>);

fn maintain(
    config: Res<TerrainCacheSettings>,
    stats: Res<TerrainCacheStats>,
    mut entries: ResMut<CacheEntries>,
    mut requests: ResMut<CacheRequests>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    let fallback = entries
        .1
        .get_or_insert_with(|| {
            buffers.add(ShaderBuffer::with_size(16, RenderAssetUsages::RENDER_WORLD))
        })
        .clone();
    let wanted: HashMap<_, _> = materials
        .iter()
        .filter_map(|(id, m)| {
            (config.enabled && !m.prepared_albedo)
                .then(|| Layout::for_material(&m.settings))
                .flatten()
                .map(|layout| (id, layout))
        })
        .collect();
    let mut bytes = 0;
    entries.0.retain(|id, entry| {
        let keep = wanted.get(id) == Some(&entry.layout)
            && bytes + entry.layout.bytes() <= config.max_bytes;
        if keep {
            bytes += entry.layout.bytes();
        } else {
            buffers.remove(entry.buffer.id());
        }
        keep
    });
    let mut allocated = 0;
    for (&id, &layout) in &wanted {
        if entries.0.contains_key(&id)
            || allocated >= BUILDS_PER_FRAME
            || bytes + layout.bytes() > config.max_bytes
        {
            continue;
        }
        let mut buffer =
            ShaderBuffer::with_size(layout.bytes() as usize, RenderAssetUsages::RENDER_WORLD);
        buffer.buffer_description.label = Some("terrain stochastic transforms");
        buffer.buffer_description.usage = BufferUsages::STORAGE;
        entries.0.insert(
            id,
            Entry {
                layout,
                buffer: buffers.add(buffer),
            },
        );
        bytes += layout.bytes();
        allocated += 1;
    }
    let mut shared = stats.0.lock().unwrap();
    // Do not mark every material modified every frame: that would rebuild its GPU bindings.
    let changes: Vec<_> = materials
        .iter()
        .filter_map(|(id, material)| {
            let entry = entries.0.get(&id);
            let ready = entry.is_some_and(|entry| shared.ready.contains(&entry.buffer.id()));
            let handle = entry.map_or_else(|| fallback.clone(), |entry| entry.buffer.clone());
            let origins = entry.map_or(Vec4::ZERO, |entry| entry.layout.origins);
            let size = entry.map_or(UVec4::ZERO, |entry| {
                UVec4::new(entry.layout.size.x, entry.layout.size.y, 0, 0)
            });
            (material.stochastic_cached != ready
                || material.stochastic_cache != handle
                || material.settings.cache_origins != origins
                || material.settings.cache_size != size)
                .then_some((id, ready, handle, origins, size))
        })
        .collect();
    for (id, ready, handle, origins, size) in changes {
        let mut material = materials.get_mut(id).unwrap();
        material.stochastic_cached = ready;
        material.stochastic_cache = handle;
        material.settings.cache_origins = origins;
        material.settings.cache_size = size;
    }
    requests.0 = entries
        .0
        .values()
        .map(|entry| Request {
            buffer: entry.buffer.id(),
            layout: entry.layout,
        })
        .collect();
    shared.snapshot.enabled = config.enabled;
    shared.snapshot.tables = entries.0.len();
    shared.snapshot.ready = entries
        .0
        .values()
        .filter(|e| shared.ready.contains(&e.buffer.id()))
        .count();
    shared.snapshot.bytes = bytes;
}

#[derive(Resource)]
struct CachePipeline {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
}

fn initialize(mut commands: Commands, server: Res<AssetServer>, cache: Res<PipelineCache>) {
    let layout = BindGroupLayoutDescriptor::new(
        "terrain stochastic cache",
        &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: Some(CacheUniform::min_size()),
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    );
    let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("terrain stochastic cache build".into()),
        layout: vec![layout.clone()],
        shader: server.load(CACHE_SHADER),
        entry_point: Some("build_cache".into()),
        ..default()
    });
    commands.insert_resource(CachePipeline { layout, pipeline });
}

#[derive(Clone, ShaderType)]
struct CacheUniform {
    origins: Vec4,
    layers: Vec4,
    size: UVec4,
}
#[derive(Resource, Default)]
struct BuiltTables(HashMap<AssetId<ShaderBuffer>, (BufferId, ComputePipelineId)>);

#[allow(clippy::too_many_arguments)]
fn build_tables(
    mut context: RenderContext,
    requests: Res<CacheRequests>,
    stats: Res<TerrainCacheStats>,
    pipeline: Res<CachePipeline>,
    cache: Res<PipelineCache>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut built: ResMut<BuiltTables>,
) {
    let mut shared = stats.0.lock().unwrap();
    let requested: HashSet<_> = requests.0.iter().map(|r| r.buffer).collect();
    built.0.retain(|id, _| requested.contains(id));
    shared.ready.retain(|id| requested.contains(id));
    let Some(compute) = cache.get_compute_pipeline(pipeline.pipeline) else {
        shared.ready.clear();
        built.0.clear();
        return;
    };
    let mut jobs = Vec::new();
    for request in &requests.0 {
        let Some(buffer) = buffers.get(request.buffer) else {
            shared.ready.remove(&request.buffer);
            built.0.remove(&request.buffer);
            continue;
        };
        let key = (buffer.buffer.id(), compute.id());
        if built.0.get(&request.buffer) == Some(&key) {
            continue;
        }
        shared.ready.remove(&request.buffer);
        if jobs.len() >= BUILDS_PER_FRAME {
            continue;
        }
        let mut uniform = UniformBuffer::from(CacheUniform {
            origins: request.layout.origins,
            layers: request.layout.layers,
            size: UVec4::new(request.layout.size.x, request.layout.size.y, 0, 0),
        });
        uniform.write_buffer(&device, &queue);
        let bindings = device.create_bind_group(
            "terrain stochastic cache build",
            &cache.get_bind_group_layout(&pipeline.layout),
            &BindGroupEntries::sequential((
                uniform.binding().unwrap(),
                buffer.buffer.as_entire_binding(),
            )),
        );
        jobs.push((request, key, bindings, uniform));
    }
    if jobs.is_empty() {
        if !shared.ready.is_empty() {
            shared.snapshot.reused_frames += 1;
        }
        return;
    }
    let mut pass = context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("terrain stochastic cache build"),
            ..default()
        });
    pass.set_pipeline(compute);
    for (request, key, bindings, _uniform) in &jobs {
        pass.set_bind_group(0, bindings, &[]);
        pass.dispatch_workgroups(
            request.layout.size.x.div_ceil(8),
            request.layout.size.y.div_ceil(8),
            2,
        );
        built.0.insert(request.buffer, *key);
        shared.ready.insert(request.buffer);
        shared.snapshot.builds += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(minimum: Vec2) -> TerrainMaterialUniform {
        TerrainMaterialUniform {
            chunk_minimum: minimum,
            chunk_extent: Vec2::splat(32.0),
            surface_layers: Vec4::new(0.0, 1.0, 2.0, 0.0),
            tile_sizes: Vec4::new(3.2, 1.6, 0.0, 0.0),
            normal_settings: Vec4::ONE,
            roughness_ranges: Vec4::ONE,
            macro_scales: Vec4::ONE,
            macro_settings: Vec4::ONE,
            cache_origins: Vec4::ZERO,
            cache_size: UVec4::ZERO,
        }
    }

    #[test]
    fn covers_both_lattices_at_page_edges_and_negative_coordinates() {
        for minimum in [
            Vec2::ZERO,
            Vec2::new(-32.0, -64.0),
            Vec2::new(3200.0, -6400.0),
        ] {
            let s = settings(minimum);
            let layout = Layout::for_material(&s).unwrap();
            assert!(layout.bytes() < 40_000);
            for (slot, tile) in [3.2, 1.6].into_iter().enumerate() {
                let origin = if slot == 0 {
                    layout.origins.xy()
                } else {
                    layout.origins.zw()
                };
                for x in 0..=128 {
                    for y in 0..=128 {
                        let uv = (minimum + Vec2::new(x as f32, y as f32) * 0.25) / tile;
                        let cell =
                            Vec2::new(uv.x + uv.y * 0.577_350_26, uv.y * 1.154_700_5).floor();
                        // Every triangle uses three of these four corners.
                        for corner in [Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ONE] {
                            let index = cell + corner - origin;
                            assert!(
                                index.min_element() >= 0.0
                                    && index.cmplt(layout.size.as_vec2()).all()
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn unreasonable_density_falls_back_and_only_transform_inputs_invalidate() {
        let mut s = settings(Vec2::ZERO);
        let first = Layout::for_material(&s).unwrap();
        s.macro_settings.y = 0.0;
        s.normal_settings = Vec4::ZERO;
        assert_eq!(Layout::for_material(&s), Some(first));
        s.tile_sizes.x *= 0.25;
        assert_ne!(Layout::for_material(&s), Some(first));
        s.tile_sizes.x = 0.00001;
        assert!(Layout::for_material(&s).is_none());
        s.tile_sizes.x = f32::NAN;
        assert!(Layout::for_material(&s).is_none());
    }

    #[test]
    fn respects_budget_build_limit_and_reclaims_unloaded_materials() {
        let mut app = App::new();
        app.init_resource::<Assets<ShaderBuffer>>()
            .init_resource::<Assets<TerrainMaterial>>()
            .init_resource::<TerrainCacheSettings>()
            .init_resource::<TerrainCacheStats>()
            .init_resource::<CacheEntries>()
            .init_resource::<CacheRequests>()
            .add_systems(Update, maintain);
        let mut handles = Vec::new();
        for _ in 0..20 {
            handles.push(
                app.world_mut()
                    .resource_mut::<Assets<TerrainMaterial>>()
                    .add(TerrainMaterial {
                        shading_mode: super::super::TerrainShadingMode::Production,
                        stochastic_cached: false,
                        prepared: false,
                        prepared_albedo: false,
                        source_weights: Handle::default(),
                        source_base_color_array: Handle::default(),
                        settings: settings(Vec2::ZERO),
                        weights: Handle::default(),
                        base_color_array: Handle::default(),
                        normal_material_array: Handle::default(),
                        macro_variation: Handle::default(),
                        stochastic_cache: Handle::default(),
                    }),
            );
        }
        let page_bytes = Layout::for_material(&settings(Vec2::ZERO)).unwrap().bytes();
        app.world_mut()
            .resource_mut::<TerrainCacheSettings>()
            .max_bytes = page_bytes * 10;
        app.update();
        assert_eq!(
            app.world().resource::<CacheEntries>().0.len(),
            BUILDS_PER_FRAME
        );
        app.update();
        assert_eq!(
            app.world().resource::<TerrainCacheStats>().snapshot().bytes,
            page_bytes * 10
        );
        let id = *app
            .world()
            .resource::<CacheEntries>()
            .0
            .keys()
            .next()
            .unwrap();
        let old_buffer = app.world().resource::<CacheEntries>().0[&id].buffer.id();
        app.world_mut()
            .resource_mut::<Assets<TerrainMaterial>>()
            .remove(id);
        app.update();
        assert!(!app.world().resource::<CacheEntries>().0.contains_key(&id));
        assert!(
            app.world()
                .resource::<Assets<ShaderBuffer>>()
                .get(old_buffer)
                .is_none()
        );
        app.world_mut()
            .resource_mut::<TerrainCacheSettings>()
            .max_bytes = 0;
        app.update();
        assert_eq!(
            app.world().resource::<TerrainCacheStats>().snapshot().bytes,
            0
        );
        assert!(
            app.world()
                .resource::<Assets<TerrainMaterial>>()
                .iter()
                .all(|(_, m)| !m.stochastic_cached)
        );
    }

    #[test]
    fn a_layer_edit_cannot_keep_using_the_old_ready_table() {
        let mut app = App::new();
        app.init_resource::<Assets<ShaderBuffer>>()
            .init_resource::<Assets<TerrainMaterial>>()
            .init_resource::<TerrainCacheSettings>()
            .init_resource::<TerrainCacheStats>()
            .init_resource::<CacheEntries>()
            .init_resource::<CacheRequests>()
            .add_systems(Update, maintain);
        let material = app
            .world_mut()
            .resource_mut::<Assets<TerrainMaterial>>()
            .add(TerrainMaterial {
                shading_mode: super::super::TerrainShadingMode::Production,
                stochastic_cached: false,
                prepared: false,
                prepared_albedo: false,
                source_weights: Handle::default(),
                source_base_color_array: Handle::default(),
                settings: settings(Vec2::ZERO),
                weights: Handle::default(),
                base_color_array: Handle::default(),
                normal_material_array: Handle::default(),
                macro_variation: Handle::default(),
                stochastic_cache: Handle::default(),
            });
        app.update();
        let old = app.world().resource::<CacheEntries>().0[&material.id()]
            .buffer
            .id();
        app.world()
            .resource::<TerrainCacheStats>()
            .0
            .lock()
            .unwrap()
            .ready
            .insert(old);
        app.update();
        assert!(
            app.world()
                .resource::<Assets<TerrainMaterial>>()
                .get(&material)
                .unwrap()
                .stochastic_cached
        );
        app.world_mut()
            .resource_mut::<Assets<TerrainMaterial>>()
            .get_mut(&material)
            .unwrap()
            .settings
            .surface_layers
            .x = 7.0;
        app.update();
        assert_ne!(
            app.world().resource::<CacheEntries>().0[&material.id()]
                .buffer
                .id(),
            old
        );
        assert!(
            !app.world()
                .resource::<Assets<TerrainMaterial>>()
                .get(&material)
                .unwrap()
                .stochastic_cached
        );
        assert!(
            app.world()
                .resource::<Assets<ShaderBuffer>>()
                .get(old)
                .is_none()
        );
    }
}
