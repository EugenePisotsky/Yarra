use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    mem::size_of,
    sync::atomic::{AtomicU64, Ordering},
};

use bevy::{
    asset::AssetId,
    core_pipeline::{
        core_3d::{CORE_3D_DEPTH_FORMAT, Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey},
        prepass::{AlphaMask3dPrepass, OpaqueNoLightmap3dBatchSetKey, OpaqueNoLightmap3dBinKey},
        schedule::{Core3d, Core3dSystems, camera_driver},
    },
    ecs::{
        query::ROQueryItem,
        system::{
            SystemParamItem,
            lifetimeless::{SRes, SResMut},
        },
    },
    mesh::Mesh,
    pbr::{
        MeshPipelineSystems, MeshPipelineViewLayoutKey, MeshPipelineViewLayouts, MeshViewBindGroup,
        SetMeshViewBindGroup, ViewKeyCache,
    },
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_component::ExtractComponentPlugin,
        mesh::allocator::MeshSlabs,
        render_asset::{PrepareAssetError, RenderAsset, RenderAssetPlugin, RenderAssets},
        render_phase::{
            AddRenderCommand, BinnedRenderPhaseType, DrawFunctions, InputUniformIndex, PhaseItem,
            RenderCommand, RenderCommandResult, SetItemPipeline, TrackedRenderPass,
            ViewBinnedRenderPhases,
        },
        render_resource::{
            AddressMode, BindGroup, BindGroupEntries, BindGroupLayoutDescriptor,
            BindGroupLayoutEntries, Buffer, BufferDescriptor, BufferInitDescriptor, BufferUsages,
            CachedComputePipelineId, CachedRenderPipelineId, Canonical, ColorTargetState,
            ColorWrites, CompareFunction, ComputePassDescriptor, ComputePipelineDescriptor,
            DepthStencilState, Extent3d, FilterMode, FragmentState, FrontFace, LoadOp,
            MipmapFilterMode, Operations, Origin3d, PipelineCache, PolygonMode, PrimitiveState,
            PrimitiveTopology, RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline,
            RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
            Specializer, SpecializerKey, StoreOp, TexelCopyBufferLayout, TexelCopyTextureInfo,
            Texture, TextureAspect, TextureDescriptor, TextureDimension, TextureFormat,
            TextureSampleType, TextureUsages, TextureView, TextureViewDescriptor,
            TextureViewDimension, Variants, VertexState,
            binding_types::{
                sampler, storage_buffer_read_only_sized, storage_buffer_sized, texture_2d_array,
                uniform_buffer_sized,
            },
        },
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue, ViewQuery},
        sync_world::MainEntity,
        view::ExtractedView,
    },
};
use bytemuck::{Pod, Zeroable};
use world::{
    GROUND_COVER_ARTWORK_RESOLUTION, GroundCoverCardArtwork, GroundCoverSpeciesId,
    MAX_GROUND_COVER_ARTWORK_ATLAS_LAYERS,
};

use crate::{
    GroundCoverDebug, GroundCoverInteraction, GroundCoverPage3d, GroundCoverPageAsset,
    GroundCoverView, GroundCoverWind, MAX_GROUND_COVER_INTERACTION_STAMPS,
};

const COMPUTE_SHADER_PATH: &str = "shaders/ground_cover_cull.wgsl";
const RENDER_SHADER_PATH: &str = "shaders/ground_cover.wgsl";
const SHADOW_VOLUME_SHADER_PATH: &str = "shaders/ground_cover_shadow_volume.wgsl";
const MAX_VISIBLE_INSTANCES: u32 = 131_072;
const CULL_WORKGROUP_SIZE: u32 = 64;
const SHADOW_VOLUME_RESOLUTION: u32 = 192;
const SHADOW_VOLUME_SLICE_COUNT: u32 = 4;
const SHADOW_VOLUME_WORLD_EXTENT: f32 = 32.0;
const SHADOW_VOLUME_BASE_HEIGHT: f32 = 0.0;
const SHADOW_VOLUME_TOP_HEIGHT: f32 = 1.2;
const SHADOW_VOLUME_FILTER_RADIUS: f32 = 0.22;
const SHADOW_VOLUME_FORMAT: TextureFormat = TextureFormat::R8Unorm;
static NEXT_PAGE_UPLOAD: AtomicU64 = AtomicU64::new(1);

pub(crate) struct GroundCoverRenderPlugin;

impl Plugin for GroundCoverRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ExtractComponentPlugin::<GroundCoverPage3d>::default(),
            ExtractComponentPlugin::<GroundCoverView>::default(),
            RenderAssetPlugin::<GpuGroundCoverPage>::default(),
        ));

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .add_render_command::<Opaque3d, DrawGroundCoverCards>()
            .add_render_command::<Opaque3d, DrawGroundCoverBlades>()
            .add_render_command::<AlphaMask3dPrepass, DrawGroundCoverCardsPrepass>()
            .add_render_command::<AlphaMask3dPrepass, DrawGroundCoverBladesPrepass>()
            .add_systems(
                RenderStartup,
                init_ground_cover_resources.after(MeshPipelineSystems),
            )
            .add_systems(
                Render,
                (
                    prepare_ground_cover.in_set(RenderSystems::PrepareBindGroups),
                    queue_ground_cover.in_set(RenderSystems::Queue),
                ),
            )
            .add_systems(
                Core3d,
                render_ground_shadow_volume
                    .after(Core3dSystems::Prepass)
                    .before(Core3dSystems::MainPass),
            )
            .add_systems(RenderGraph, run_ground_cover_culling.before(camera_driver));
    }
}

fn init_ground_cover_resources(world: &mut World) {
    world.init_resource::<GroundCoverPipelines>();
    world.init_resource::<GroundCoverArtworkAtlas>();
    world.init_resource::<GroundCoverBuffers>();
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct ClusterGpu {
    center_density: [f32; 4],
    half_extents_area: [f32; 4],
    coverage_half_extents: [f32; 4],
    metadata: [u32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct SpeciesGpu {
    bottom_min_height: [f32; 4],
    top_max_height: [f32; 4],
    card: [f32; 4],
    // x: first texture-array layer, y: variant count
    artwork: [u32; 4],
}

pub(crate) struct GpuGroundCoverPage {
    clusters: Buffer,
    species: Buffer,
    cluster_count: u32,
    upload_serial: u64,
}

impl RenderAsset for GpuGroundCoverPage {
    type SourceAsset = GroundCoverPageAsset;
    type Param = (
        SRes<RenderDevice>,
        SRes<RenderQueue>,
        SResMut<GroundCoverArtworkAtlas>,
    );

    fn byte_len(source: &Self::SourceAsset) -> Option<usize> {
        Some(
            source.page.clusters.len() * size_of::<ClusterGpu>()
                + source.species.len() * size_of::<SpeciesGpu>(),
        )
    }

    fn prepare_asset(
        source: Self::SourceAsset,
        _asset_id: AssetId<Self::SourceAsset>,
        parameters: &mut SystemParamItem<Self::Param>,
        _previous_asset: Option<&Self>,
    ) -> Result<Self, PrepareAssetError<Self::SourceAsset>> {
        let (render_device, render_queue, atlas) = parameters;
        let render_device: &RenderDevice = render_device;
        let allocations = source
            .species
            .iter()
            .map(|species| {
                atlas
                    .register(render_queue, species.id, &species.artwork)
                    .map(|allocation| (species.id, allocation))
            })
            .collect::<Option<HashMap<_, _>>>();
        let Some(allocations) = allocations else {
            error!(
                "ground-cover artwork atlas exhausted its {} layers",
                MAX_GROUND_COVER_ARTWORK_ATLAS_LAYERS
            );
            return Err(PrepareAssetError::RetryNextUpdate(source));
        };
        let species_indices: HashMap<GroundCoverSpeciesId, u32> = source
            .species
            .iter()
            .enumerate()
            .map(|(index, species)| (species.id, index as u32))
            .collect();
        let species: Vec<_> = source
            .species
            .iter()
            .map(|species| SpeciesGpu {
                bottom_min_height: [
                    species.bottom_color[0],
                    species.bottom_color[1],
                    species.bottom_color[2],
                    species.minimum_card_height,
                ],
                top_max_height: [
                    species.top_color[0],
                    species.top_color[1],
                    species.top_color[2],
                    species.maximum_card_height,
                ],
                card: [
                    species.minimum_card_width,
                    species.maximum_card_width,
                    species.flattened_card_probability,
                    species.maximum_wind_displacement,
                ],
                artwork: [
                    allocations[&species.id].first_layer,
                    allocations[&species.id].variant_count,
                    0,
                    0,
                ],
            })
            .collect();
        let cell_origin = [
            (f64::from(source.key.cell.x) - f64::from(source.origin_cell.x))
                * f64::from(source.cell_size),
            (f64::from(source.key.cell.z) - f64::from(source.origin_cell.z))
                * f64::from(source.cell_size),
        ];
        let clusters: Vec<_> = source
            .page
            .clusters
            .iter()
            .map(|cluster| {
                let species_index = species_indices[&cluster.species];
                ClusterGpu {
                    center_density: [
                        cell_origin[0] as f32 + cluster.local_center[0],
                        cluster.local_center[1],
                        cell_origin[1] as f32 + cluster.local_center[2],
                        cluster.density_per_square_meter,
                    ],
                    half_extents_area: [
                        cluster.half_extents[0],
                        cluster.half_extents[1],
                        cluster.half_extents[2],
                        cluster.coverage_half_extents[0] * cluster.coverage_half_extents[1] * 4.0,
                    ],
                    coverage_half_extents: [
                        cluster.coverage_half_extents[0],
                        cluster.coverage_half_extents[1],
                        0.0,
                        0.0,
                    ],
                    metadata: [species_index, cluster.seed, 0, 0],
                }
            })
            .collect();

        Ok(Self {
            clusters: create_storage_buffer(render_device, "ground-cover clusters", &clusters),
            species: create_storage_buffer(render_device, "ground-cover species", &species),
            cluster_count: clusters.len() as u32,
            upload_serial: NEXT_PAGE_UPLOAD.fetch_add(1, Ordering::Relaxed),
        })
    }
}

fn create_storage_buffer<T: Pod>(
    render_device: &RenderDevice,
    label: &'static str,
    values: &[T],
) -> Buffer {
    let zero = [0_u8; 16];
    render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some(label),
        contents: if values.is_empty() {
            &zero
        } else {
            bytemuck::cast_slice(values)
        },
        usage: BufferUsages::STORAGE,
    })
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct CameraGpu {
    clip_from_world: [f32; 16],
    camera_position: [f32; 4],
    view_direction: [f32; 4],
    viewport: [f32; 4],
    limits: [f32; 4],
    wind: [f32; 4],
    wind_direction: [f32; 4],
    debug: [u32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct DrawConfigGpu {
    geometry: [u32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct GroundShadowVolumeGpu {
    // xy: minimum world x/z, z: square extent, w: first slice world height
    origin_extent: [f32; 4],
    // x: slice spacing, y: inverse spacing, z: last slice index, w: edge blend width in UV
    height: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct GroundShadowSliceGpu {
    // xy: minimum world x/z, z: square extent, w: this slice's world height
    origin_extent: [f32; 4],
    // x: world-space filter radius
    filter: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct VisibleInstanceGpu {
    position_yaw: [f32; 4],
    bottom_height: [f32; 4],
    top_half_width: [f32; 4],
    motion: [f32; 4],
    interaction: [f32; 4],
    artwork: [u32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct InteractionGpu {
    metadata: [u32; 4],
    centers: [[f32; 4]; MAX_GROUND_COVER_INTERACTION_STAMPS],
    ends: [[f32; 4]; MAX_GROUND_COVER_INTERACTION_STAMPS],
}

struct PageComputeBinding {
    bind_group: BindGroup,
    cluster_count: u32,
    upload_serial: u64,
}

#[derive(Clone, Copy)]
struct ArtworkAllocation {
    first_layer: u32,
    variant_count: u32,
    fingerprint: u64,
}

#[derive(Resource)]
pub(crate) struct GroundCoverArtworkAtlas {
    texture: Texture,
    view: TextureView,
    sampler: Sampler,
    allocations: HashMap<GroundCoverSpeciesId, ArtworkAllocation>,
    next_layer: u32,
}

impl FromWorld for GroundCoverArtworkAtlas {
    fn from_world(world: &mut World) -> Self {
        let render_device = world.resource::<RenderDevice>();
        let texture = render_device.create_texture(&TextureDescriptor {
            label: Some("ground-cover artwork atlas"),
            size: Extent3d {
                width: u32::from(GROUND_COVER_ARTWORK_RESOLUTION),
                height: u32::from(GROUND_COVER_ARTWORK_RESOLUTION),
                depth_or_array_layers: MAX_GROUND_COVER_ARTWORK_ATLAS_LAYERS,
            },
            mip_level_count: u32::from(GROUND_COVER_ARTWORK_RESOLUTION).ilog2() + 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::R8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor {
            dimension: Some(TextureViewDimension::D2Array),
            ..default()
        });
        let sampler = render_device.create_sampler(&SamplerDescriptor {
            label: Some("ground-cover artwork sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            ..default()
        });
        Self {
            texture,
            view,
            sampler,
            allocations: HashMap::new(),
            next_layer: 0,
        }
    }
}

impl GroundCoverArtworkAtlas {
    fn register(
        &mut self,
        render_queue: &RenderQueue,
        species: GroundCoverSpeciesId,
        artwork: &GroundCoverCardArtwork,
    ) -> Option<ArtworkAllocation> {
        if !artwork.is_valid() {
            return None;
        }
        let mut hasher = DefaultHasher::new();
        artwork.resolution.hash(&mut hasher);
        artwork.variant_count.hash(&mut hasher);
        artwork.mip_level_count.hash(&mut hasher);
        artwork.coverage_mips.hash(&mut hasher);
        let fingerprint = hasher.finish();
        if let Some(existing) = self.allocations.get(&species).copied()
            && existing.fingerprint == fingerprint
        {
            return Some(existing);
        }

        let variant_count = u32::from(artwork.variant_count);
        let allocation = if let Some(existing) = self.allocations.get(&species).copied()
            && existing.variant_count >= variant_count
        {
            ArtworkAllocation {
                variant_count,
                fingerprint,
                ..existing
            }
        } else {
            let next_layer = self.next_layer.checked_add(variant_count)?;
            if next_layer > MAX_GROUND_COVER_ARTWORK_ATLAS_LAYERS {
                return None;
            }
            let allocation = ArtworkAllocation {
                first_layer: self.next_layer,
                variant_count,
                fingerprint,
            };
            self.next_layer = next_layer;
            allocation
        };

        let per_variant = artwork.coverage_mips.len() / usize::from(artwork.variant_count);
        for variant in 0..artwork.variant_count {
            let mut offset = usize::from(variant) * per_variant;
            let mut side = u32::from(artwork.resolution);
            for mip_level in 0..u32::from(artwork.mip_level_count) {
                let byte_count = (side * side) as usize;
                render_queue.write_texture(
                    TexelCopyTextureInfo {
                        texture: &self.texture,
                        mip_level,
                        origin: Origin3d {
                            x: 0,
                            y: 0,
                            z: allocation.first_layer + u32::from(variant),
                        },
                        aspect: TextureAspect::All,
                    },
                    &artwork.coverage_mips[offset..offset + byte_count],
                    TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(side),
                        rows_per_image: Some(side),
                    },
                    Extent3d {
                        width: side,
                        height: side,
                        depth_or_array_layers: 1,
                    },
                );
                offset += byte_count;
                side = (side / 2).max(1);
            }
        }
        self.allocations.insert(species, allocation);
        Some(allocation)
    }
}

#[derive(Resource)]
struct GroundCoverBuffers {
    near_visible: Buffer,
    mid_visible: Buffer,
    far_visible: Buffer,
    near_args: Buffer,
    mid_args: Buffer,
    far_args: Buffer,
    ribbon_candidates: Buffer,
    camera: Buffer,
    interaction: Buffer,
    shadow_volume_uniform: Buffer,
    _shadow_volume_texture: Texture,
    _shadow_volume_view: TextureView,
    _shadow_volume_sampler: Sampler,
    shadow_slice_buffers: Vec<Buffer>,
    shadow_slice_views: Vec<TextureView>,
    shadow_slice_bind_groups: Vec<BindGroup>,
    _near_config: Buffer,
    _mid_config: Buffer,
    _far_config: Buffer,
    near_draw_bind_group: BindGroup,
    mid_draw_bind_group: BindGroup,
    far_draw_bind_group: BindGroup,
    finalize_bind_group: BindGroup,
    page_bind_groups: HashMap<AssetId<GroundCoverPageAsset>, PageComputeBinding>,
}

impl FromWorld for GroundCoverBuffers {
    fn from_world(world: &mut World) -> Self {
        let pipelines = world.resource::<GroundCoverPipelines>();
        let pipeline_cache = world.resource::<PipelineCache>();
        let draw_layout = pipeline_cache.get_bind_group_layout(&pipelines.draw_layout);
        let finalize_layout = pipeline_cache.get_bind_group_layout(&pipelines.finalize_layout);
        let shadow_slice_layout =
            pipeline_cache.get_bind_group_layout(&pipelines.shadow_slice_layout);
        let render_device = world.resource::<RenderDevice>();
        let (artwork_view, artwork_sampler) = {
            let atlas = world.resource::<GroundCoverArtworkAtlas>();
            (atlas.view.clone(), atlas.sampler.clone())
        };
        let visible_size =
            u64::from(MAX_VISIBLE_INSTANCES) * size_of::<VisibleInstanceGpu>() as u64;
        let near_visible = render_device.create_buffer(&BufferDescriptor {
            label: Some("ground-cover near visible instances"),
            size: visible_size,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let mid_visible = render_device.create_buffer(&BufferDescriptor {
            label: Some("ground-cover mid visible instances"),
            size: visible_size,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let far_visible = render_device.create_buffer(&BufferDescriptor {
            label: Some("ground-cover far visible instances"),
            size: visible_size,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let args_usage = BufferUsages::STORAGE | BufferUsages::INDIRECT | BufferUsages::COPY_DST;
        let near_args = render_device.create_buffer(&BufferDescriptor {
            label: Some("ground-cover near indirect args"),
            size: 16,
            usage: args_usage,
            mapped_at_creation: false,
        });
        let mid_args = render_device.create_buffer(&BufferDescriptor {
            label: Some("ground-cover mid indirect args"),
            size: 16,
            usage: args_usage,
            mapped_at_creation: false,
        });
        let far_args = render_device.create_buffer(&BufferDescriptor {
            label: Some("ground-cover far indirect args"),
            size: 16,
            usage: args_usage,
            mapped_at_creation: false,
        });
        let ribbon_candidates = render_device.create_buffer(&BufferDescriptor {
            label: Some("ground-cover ribbon candidate counts"),
            size: 16,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera = render_device.create_buffer(&BufferDescriptor {
            label: Some("ground-cover camera"),
            size: size_of::<CameraGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let interaction = render_device.create_buffer(&BufferDescriptor {
            label: Some("ground-cover interaction"),
            size: size_of::<InteractionGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_volume_uniform = render_device.create_buffer(&BufferDescriptor {
            label: Some("ground-cover shadow volume uniform"),
            size: size_of::<GroundShadowVolumeGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_volume_texture = render_device.create_texture(&TextureDescriptor {
            label: Some("ground-cover shadow volume"),
            size: Extent3d {
                width: SHADOW_VOLUME_RESOLUTION,
                height: SHADOW_VOLUME_RESOLUTION,
                depth_or_array_layers: SHADOW_VOLUME_SLICE_COUNT,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: SHADOW_VOLUME_FORMAT,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_volume_view = shadow_volume_texture.create_view(&TextureViewDescriptor {
            label: Some("ground-cover shadow volume array view"),
            dimension: Some(TextureViewDimension::D2Array),
            base_array_layer: 0,
            array_layer_count: Some(SHADOW_VOLUME_SLICE_COUNT),
            ..default()
        });
        let shadow_slice_views = (0..SHADOW_VOLUME_SLICE_COUNT)
            .map(|slice| {
                shadow_volume_texture.create_view(&TextureViewDescriptor {
                    label: Some("ground-cover shadow volume slice view"),
                    dimension: Some(TextureViewDimension::D2),
                    base_array_layer: slice,
                    array_layer_count: Some(1),
                    ..default()
                })
            })
            .collect::<Vec<_>>();
        let shadow_volume_sampler = render_device.create_sampler(&SamplerDescriptor {
            label: Some("ground-cover shadow volume sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Nearest,
            ..default()
        });
        let shadow_slice_buffers = (0..SHADOW_VOLUME_SLICE_COUNT)
            .map(|_slice| {
                render_device.create_buffer(&BufferDescriptor {
                    label: Some("ground-cover shadow volume slice uniform"),
                    size: size_of::<GroundShadowSliceGpu>() as u64,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect::<Vec<_>>();
        let shadow_slice_bind_groups = shadow_slice_buffers
            .iter()
            .map(|buffer| {
                render_device.create_bind_group(
                    Some("ground-cover shadow volume slice bind group"),
                    &shadow_slice_layout,
                    &BindGroupEntries::single(buffer.as_entire_binding()),
                )
            })
            .collect::<Vec<_>>();
        let near_config = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("ground-cover near draw config"),
            contents: bytemuck::bytes_of(&DrawConfigGpu {
                geometry: [3, 2, 0, 0],
            }),
            usage: BufferUsages::UNIFORM,
        });
        let mid_config = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("ground-cover mid draw config"),
            contents: bytemuck::bytes_of(&DrawConfigGpu {
                geometry: [2, 2, 1, 0],
            }),
            usage: BufferUsages::UNIFORM,
        });
        let far_config = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("ground-cover far draw config"),
            contents: bytemuck::bytes_of(&DrawConfigGpu {
                geometry: [1, 1, 2, 0],
            }),
            usage: BufferUsages::UNIFORM,
        });

        let near_draw_bind_group = render_device.create_bind_group(
            Some("ground-cover near draw bind group"),
            &draw_layout,
            &BindGroupEntries::sequential((
                near_visible.as_entire_binding(),
                camera.as_entire_binding(),
                near_config.as_entire_binding(),
                &artwork_view,
                &artwork_sampler,
                &shadow_volume_view,
                &shadow_volume_sampler,
                shadow_volume_uniform.as_entire_binding(),
            )),
        );
        let mid_draw_bind_group = render_device.create_bind_group(
            Some("ground-cover mid draw bind group"),
            &draw_layout,
            &BindGroupEntries::sequential((
                mid_visible.as_entire_binding(),
                camera.as_entire_binding(),
                mid_config.as_entire_binding(),
                &artwork_view,
                &artwork_sampler,
                &shadow_volume_view,
                &shadow_volume_sampler,
                shadow_volume_uniform.as_entire_binding(),
            )),
        );
        let far_draw_bind_group = render_device.create_bind_group(
            Some("ground-cover far draw bind group"),
            &draw_layout,
            &BindGroupEntries::sequential((
                far_visible.as_entire_binding(),
                camera.as_entire_binding(),
                far_config.as_entire_binding(),
                &artwork_view,
                &artwork_sampler,
                &shadow_volume_view,
                &shadow_volume_sampler,
                shadow_volume_uniform.as_entire_binding(),
            )),
        );
        let finalize_bind_group = render_device.create_bind_group(
            Some("ground-cover finalize bind group"),
            &finalize_layout,
            &BindGroupEntries::sequential((
                near_args.as_entire_binding(),
                mid_args.as_entire_binding(),
                far_args.as_entire_binding(),
                camera.as_entire_binding(),
            )),
        );

        Self {
            near_visible,
            mid_visible,
            far_visible,
            near_args,
            mid_args,
            far_args,
            ribbon_candidates,
            camera,
            interaction,
            shadow_volume_uniform,
            _shadow_volume_texture: shadow_volume_texture,
            _shadow_volume_view: shadow_volume_view,
            _shadow_volume_sampler: shadow_volume_sampler,
            shadow_slice_buffers,
            shadow_slice_views,
            shadow_slice_bind_groups,
            _near_config: near_config,
            _mid_config: mid_config,
            _far_config: far_config,
            near_draw_bind_group,
            mid_draw_bind_group,
            far_draw_bind_group,
            finalize_bind_group,
            page_bind_groups: HashMap::new(),
        }
    }
}

#[derive(Resource)]
struct GroundCoverPipelines {
    cull_layout: BindGroupLayoutDescriptor,
    finalize_layout: BindGroupLayoutDescriptor,
    draw_layout: BindGroupLayoutDescriptor,
    shadow_slice_layout: BindGroupLayoutDescriptor,
    cull_pipeline: CachedComputePipelineId,
    count_ribbon_candidates_pipeline: CachedComputePipelineId,
    finalize_pipeline: CachedComputePipelineId,
    draw_variants: Variants<RenderPipeline, GroundCoverPipelineSpecializer>,
    prepass_variants: Variants<RenderPipeline, GroundCoverPipelineSpecializer>,
    blade_draw_variants: Variants<RenderPipeline, GroundCoverPipelineSpecializer>,
    blade_prepass_variants: Variants<RenderPipeline, GroundCoverPipelineSpecializer>,
    shadow_volume_variants: Variants<RenderPipeline, GroundShadowVolumePipelineSpecializer>,
    shadow_volume_pipelines: HashMap<Entity, CachedRenderPipelineId>,
}

impl FromWorld for GroundCoverPipelines {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        let pipeline_cache = world.resource::<PipelineCache>();
        let view_layouts = world.resource::<MeshPipelineViewLayouts>().clone();
        let compute_shader = asset_server.load(COMPUTE_SHADER_PATH);
        let render_shader = asset_server.load(RENDER_SHADER_PATH);
        let shadow_volume_shader = asset_server.load(SHADOW_VOLUME_SHADER_PATH);

        let cull_layout = BindGroupLayoutDescriptor::new(
            "ground-cover cull",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                ),
            ),
        );
        let finalize_layout = BindGroupLayoutDescriptor::new(
            "ground-cover finalize",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                ),
            ),
        );
        let draw_layout = BindGroupLayoutDescriptor::new(
            "ground-cover draw",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    storage_buffer_read_only_sized(false, None),
                    uniform_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    texture_2d_array(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    texture_2d_array(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    uniform_buffer_sized(false, None),
                ),
            ),
        );
        let shadow_slice_layout = BindGroupLayoutDescriptor::new(
            "ground-cover shadow volume generation",
            &BindGroupLayoutEntries::single(
                ShaderStages::FRAGMENT,
                uniform_buffer_sized(false, None),
            ),
        );
        let cull_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("ground-cover cull pipeline".into()),
            layout: vec![cull_layout.clone()],
            shader: compute_shader.clone(),
            entry_point: Some(Cow::Borrowed("cull")),
            ..default()
        });
        let count_ribbon_candidates_pipeline =
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some("ground-cover ribbon candidate count pipeline".into()),
                layout: vec![cull_layout.clone()],
                shader: compute_shader.clone(),
                entry_point: Some(Cow::Borrowed("count_ribbon_candidates")),
                ..default()
            });
        let finalize_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("ground-cover finalize pipeline".into()),
            layout: vec![finalize_layout.clone()],
            shader: compute_shader,
            entry_point: Some(Cow::Borrowed("finalize")),
            ..default()
        });
        let draw_descriptor = RenderPipelineDescriptor {
            label: Some("ground-cover draw pipeline".into()),
            // The exact mesh-view layout is installed by the per-view specializer.
            layout: Vec::new(),
            vertex: VertexState {
                shader: render_shader.clone(),
                entry_point: Some(Cow::Borrowed("vertex")),
                shader_defs: vec!["SHADOW_FILTER_METHOD_HARDWARE_2X2".into()],
                buffers: Vec::new(),
                ..default()
            },
            fragment: Some(FragmentState {
                shader: render_shader.clone(),
                entry_point: Some(Cow::Borrowed("fragment")),
                shader_defs: vec!["SHADOW_FILTER_METHOD_HARDWARE_2X2".into()],
                targets: vec![Some(ColorTargetState {
                    format: TextureFormat::Rgba16Float,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: PolygonMode::Fill,
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                // The alpha-tested prepass owns depth. Exact equality ensures only the
                // frontmost surviving blades execute the shadowed color pass.
                depth_write_enabled: Some(false),
                depth_compare: Some(CompareFunction::Equal),
                stencil: default(),
                bias: default(),
            }),
            ..default()
        };
        let prepass_descriptor = RenderPipelineDescriptor {
            label: Some("ground-cover alpha depth prepass pipeline".into()),
            layout: Vec::new(),
            vertex: VertexState {
                shader: render_shader.clone(),
                entry_point: Some(Cow::Borrowed("vertex")),
                buffers: Vec::new(),
                ..default()
            },
            fragment: Some(FragmentState {
                shader: render_shader,
                entry_point: Some(Cow::Borrowed("prepass_fragment")),
                targets: Vec::new(),
                ..default()
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: PolygonMode::Fill,
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: default(),
                bias: default(),
            }),
            ..default()
        };
        let mut blade_draw_descriptor = draw_descriptor.clone();
        blade_draw_descriptor.label = Some("ground-cover ribbon draw pipeline".into());
        blade_draw_descriptor.primitive.topology = PrimitiveTopology::TriangleStrip;
        let mut blade_prepass_descriptor = prepass_descriptor.clone();
        blade_prepass_descriptor.label = Some("ground-cover ribbon depth prepass pipeline".into());
        blade_prepass_descriptor.primitive.topology = PrimitiveTopology::TriangleStrip;
        let shadow_volume_descriptor = RenderPipelineDescriptor {
            label: Some("ground-cover shadow volume pipeline".into()),
            layout: Vec::new(),
            vertex: VertexState {
                shader: shadow_volume_shader.clone(),
                entry_point: Some(Cow::Borrowed("vertex")),
                shader_defs: vec!["SHADOW_FILTER_METHOD_HARDWARE_2X2".into()],
                buffers: Vec::new(),
                ..default()
            },
            fragment: Some(FragmentState {
                shader: shadow_volume_shader,
                entry_point: Some(Cow::Borrowed("fragment")),
                shader_defs: vec!["SHADOW_FILTER_METHOD_HARDWARE_2X2".into()],
                targets: vec![Some(ColorTargetState {
                    format: SHADOW_VOLUME_FORMAT,
                    blend: None,
                    write_mask: ColorWrites::RED,
                })],
                ..default()
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: PolygonMode::Fill,
                ..default()
            },
            depth_stencil: None,
            ..default()
        };

        Self {
            cull_layout,
            finalize_layout,
            draw_layout: draw_layout.clone(),
            shadow_slice_layout: shadow_slice_layout.clone(),
            cull_pipeline,
            count_ribbon_candidates_pipeline,
            finalize_pipeline,
            draw_variants: Variants::new(
                GroundCoverPipelineSpecializer {
                    view_layouts: view_layouts.clone(),
                    draw_layout: draw_layout.clone(),
                    color_pass: true,
                },
                draw_descriptor,
            ),
            prepass_variants: Variants::new(
                GroundCoverPipelineSpecializer {
                    view_layouts: view_layouts.clone(),
                    draw_layout: draw_layout.clone(),
                    color_pass: false,
                },
                prepass_descriptor,
            ),
            blade_draw_variants: Variants::new(
                GroundCoverPipelineSpecializer {
                    view_layouts: view_layouts.clone(),
                    draw_layout: draw_layout.clone(),
                    color_pass: true,
                },
                blade_draw_descriptor,
            ),
            blade_prepass_variants: Variants::new(
                GroundCoverPipelineSpecializer {
                    view_layouts: view_layouts.clone(),
                    draw_layout,
                    color_pass: false,
                },
                blade_prepass_descriptor,
            ),
            shadow_volume_variants: Variants::new(
                GroundShadowVolumePipelineSpecializer {
                    view_layouts,
                    shadow_slice_layout,
                },
                shadow_volume_descriptor,
            ),
            shadow_volume_pipelines: HashMap::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, SpecializerKey)]
struct GroundShadowVolumePipelineKey {
    view_layout_bits: u32,
}

struct GroundShadowVolumePipelineSpecializer {
    view_layouts: MeshPipelineViewLayouts,
    shadow_slice_layout: BindGroupLayoutDescriptor,
}

impl Specializer<RenderPipeline> for GroundShadowVolumePipelineSpecializer {
    type Key = GroundShadowVolumePipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        descriptor: &mut RenderPipelineDescriptor,
    ) -> Result<Canonical<Self::Key>, BevyError> {
        let view_layout =
            self.view_layouts
                .get_view_layout(MeshPipelineViewLayoutKey::from_bits_retain(
                    key.view_layout_bits,
                ));
        descriptor.layout = vec![view_layout.main_layout, self.shadow_slice_layout.clone()];
        Ok(key)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, SpecializerKey)]
struct GroundCoverPipelineKey {
    msaa: Msaa,
    target_format: TextureFormat,
    view_layout_bits: u32,
}

struct GroundCoverPipelineSpecializer {
    view_layouts: MeshPipelineViewLayouts,
    draw_layout: BindGroupLayoutDescriptor,
    color_pass: bool,
}

impl Specializer<RenderPipeline> for GroundCoverPipelineSpecializer {
    type Key = GroundCoverPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        descriptor: &mut RenderPipelineDescriptor,
    ) -> Result<Canonical<Self::Key>, BevyError> {
        descriptor.multisample.count = key.msaa.samples();
        descriptor.multisample.alpha_to_coverage_enabled = key.msaa.samples() > 1;
        let view_layout =
            self.view_layouts
                .get_view_layout(MeshPipelineViewLayoutKey::from_bits_retain(
                    key.view_layout_bits,
                ));
        descriptor.layout = vec![view_layout.main_layout, self.draw_layout.clone()];
        if self.color_pass {
            descriptor.fragment.as_mut().unwrap().targets[0]
                .as_mut()
                .unwrap()
                .format = key.target_format;
        }
        Ok(key)
    }
}

#[allow(clippy::too_many_arguments)] // Bevy render-world system parameters are independently borrowed resources.
fn prepare_ground_cover(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<GroundCoverPipelines>,
    pages: Res<RenderAssets<GpuGroundCoverPage>>,
    active_pages: Query<&GroundCoverPage3d>,
    views: Query<(&ExtractedView, &GroundCoverView)>,
    wind: Res<GroundCoverWind>,
    debug: Res<GroundCoverDebug>,
    interaction: Res<GroundCoverInteraction>,
    mut buffers: ResMut<GroundCoverBuffers>,
) {
    let active_page_ids = active_pages
        .iter()
        .map(|page| page.id())
        .collect::<HashSet<_>>();
    buffers
        .page_bind_groups
        .retain(|id, _| active_page_ids.contains(id) && pages.get(*id).is_some());

    let cull_layout = pipeline_cache.get_bind_group_layout(&pipelines.cull_layout);
    for id in active_page_ids {
        let Some(page) = pages.get(id) else {
            continue;
        };
        if buffers
            .page_bind_groups
            .get(&id)
            .is_some_and(|binding| binding.upload_serial == page.upload_serial)
        {
            continue;
        }
        let bind_group = render_device.create_bind_group(
            Some("ground-cover page cull bind group"),
            &cull_layout,
            &BindGroupEntries::sequential((
                page.clusters.as_entire_binding(),
                page.species.as_entire_binding(),
                buffers.near_visible.as_entire_binding(),
                buffers.mid_visible.as_entire_binding(),
                buffers.far_visible.as_entire_binding(),
                buffers.near_args.as_entire_binding(),
                buffers.mid_args.as_entire_binding(),
                buffers.far_args.as_entire_binding(),
                buffers.camera.as_entire_binding(),
                buffers.interaction.as_entire_binding(),
                buffers.ribbon_candidates.as_entire_binding(),
            )),
        );
        buffers.page_bind_groups.insert(
            id,
            PageComputeBinding {
                bind_group,
                cluster_count: page.cluster_count,
                upload_serial: page.upload_serial,
            },
        );
    }

    let Some((view, ground_cover_view)) = views.iter().next() else {
        return;
    };
    let clip_from_world = view
        .clip_from_world
        .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
    let view_direction = (view.world_from_view.rotation() * Vec3::NEG_Z).normalize_or_zero();
    let top_down_card_blend = ground_cover_view.normalized_zoom.clamp(0.0, 1.0);
    let wind_direction = wind.direction.try_normalize().unwrap_or(Vec2::X);
    let uniform = CameraGpu {
        clip_from_world: clip_from_world.to_cols_array(),
        camera_position: view.world_from_view.translation().extend(1.0).to_array(),
        view_direction: view_direction.extend(top_down_card_blend).to_array(),
        viewport: [view.viewport.z as f32, view.viewport.w as f32, 0.0, 0.0],
        // Near detail, mid detail, output capacity, and geometry cutoff in pixels. Far cards are
        // cheap enough to remain below one pixel; the lower cutoff prevents visible horizon gaps.
        limits: [18.0, 6.0, MAX_VISIBLE_INSTANCES as f32, 0.75],
        wind: [
            wind.elapsed_seconds,
            wind.base_strength,
            wind.gust_strength,
            wind.spatial_scale,
        ],
        wind_direction: [wind_direction.x, wind_direction.y, wind.speed, 0.0],
        debug: [
            debug.mode.gpu_value(),
            u32::from(debug.procedural_blades),
            0,
            0,
        ],
    };
    render_queue.write_buffer(&buffers.camera, 0, bytemuck::bytes_of(&uniform));

    // The near shadow field remains world-aligned and only moves in complete texels. This
    // prevents sub-texel camera motion from changing the filtered field under stationary grass.
    let shadow_texel_size = SHADOW_VOLUME_WORLD_EXTENT / SHADOW_VOLUME_RESOLUTION as f32;
    let camera_position = view.world_from_view.translation();
    let shadow_origin = Vec2::new(
        camera_position.x - SHADOW_VOLUME_WORLD_EXTENT * 0.5,
        camera_position.z - SHADOW_VOLUME_WORLD_EXTENT * 0.5,
    );
    let shadow_origin = (shadow_origin / shadow_texel_size).floor() * shadow_texel_size;
    let slice_spacing = (SHADOW_VOLUME_TOP_HEIGHT - SHADOW_VOLUME_BASE_HEIGHT)
        / (SHADOW_VOLUME_SLICE_COUNT - 1) as f32;
    let shadow_volume = GroundShadowVolumeGpu {
        origin_extent: [
            shadow_origin.x,
            shadow_origin.y,
            SHADOW_VOLUME_WORLD_EXTENT,
            SHADOW_VOLUME_BASE_HEIGHT,
        ],
        height: [
            slice_spacing,
            slice_spacing.recip(),
            (SHADOW_VOLUME_SLICE_COUNT - 1) as f32,
            0.045,
        ],
    };
    render_queue.write_buffer(
        &buffers.shadow_volume_uniform,
        0,
        bytemuck::bytes_of(&shadow_volume),
    );
    for (slice, buffer) in buffers.shadow_slice_buffers.iter().enumerate() {
        let slice_uniform = GroundShadowSliceGpu {
            origin_extent: [
                shadow_origin.x,
                shadow_origin.y,
                SHADOW_VOLUME_WORLD_EXTENT,
                SHADOW_VOLUME_BASE_HEIGHT + slice as f32 * slice_spacing,
            ],
            filter: [SHADOW_VOLUME_FILTER_RADIUS, 0.0, 0.0, 0.0],
        };
        render_queue.write_buffer(buffer, 0, bytemuck::bytes_of(&slice_uniform));
    }

    let mut interaction_uniform = InteractionGpu::zeroed();
    let stamp_count = interaction
        .stamps
        .len()
        .min(MAX_GROUND_COVER_INTERACTION_STAMPS);
    interaction_uniform.metadata[0] = stamp_count as u32;
    for (index, stamp) in interaction.stamps.iter().take(stamp_count).enumerate() {
        interaction_uniform.centers[index] = [
            stamp.start.x,
            stamp.start.y,
            stamp.start_recovery,
            stamp.radius,
        ];
        interaction_uniform.ends[index] = [
            stamp.end.x,
            stamp.end.y,
            stamp.end_recovery,
            stamp.maximum_displacement,
        ];
    }
    render_queue.write_buffer(
        &buffers.interaction,
        0,
        bytemuck::bytes_of(&interaction_uniform),
    );
}

fn run_ground_cover_culling(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<GroundCoverPipelines>,
    buffers: Res<GroundCoverBuffers>,
) {
    let encoder = render_context.command_encoder();
    encoder.clear_buffer(&buffers.near_args, 0, None);
    encoder.clear_buffer(&buffers.mid_args, 0, None);
    encoder.clear_buffer(&buffers.far_args, 0, None);
    encoder.clear_buffer(&buffers.ribbon_candidates, 0, None);

    let (Some(count_pipeline), Some(cull_pipeline), Some(finalize_pipeline)) = (
        pipeline_cache.get_compute_pipeline(pipelines.count_ribbon_candidates_pipeline),
        pipeline_cache.get_compute_pipeline(pipelines.cull_pipeline),
        pipeline_cache.get_compute_pipeline(pipelines.finalize_pipeline),
    ) else {
        return;
    };

    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("ground-cover GPU culling"),
        ..default()
    });
    pass.set_pipeline(count_pipeline);
    for page in buffers.page_bind_groups.values() {
        if page.cluster_count == 0 {
            continue;
        }
        pass.set_bind_group(0, &page.bind_group, &[]);
        pass.dispatch_workgroups(page.cluster_count.div_ceil(CULL_WORKGROUP_SIZE), 1, 1);
    }
    pass.set_pipeline(cull_pipeline);
    for page in buffers.page_bind_groups.values() {
        if page.cluster_count == 0 {
            continue;
        }
        pass.set_bind_group(0, &page.bind_group, &[]);
        pass.dispatch_workgroups(page.cluster_count.div_ceil(CULL_WORKGROUP_SIZE), 1, 1);
    }
    pass.set_pipeline(finalize_pipeline);
    pass.set_bind_group(0, &buffers.finalize_bind_group, &[]);
    pass.dispatch_workgroups(1, 1, 1);
}

fn render_ground_shadow_volume(
    view: ViewQuery<(&GroundCoverView, &MeshViewBindGroup)>,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<GroundCoverPipelines>,
    buffers: Res<GroundCoverBuffers>,
    mut render_context: RenderContext,
) {
    let view_entity = view.entity();
    let (_, mesh_view_bind_group) = view.into_inner();
    let Some(pipeline_id) = pipelines.shadow_volume_pipelines.get(&view_entity) else {
        return;
    };
    let Some(pipeline) = pipeline_cache.get_render_pipeline(*pipeline_id) else {
        return;
    };

    for (slice_view, slice_bind_group) in buffers
        .shadow_slice_views
        .iter()
        .zip(&buffers.shadow_slice_bind_groups)
    {
        let color_attachments = [Some(RenderPassColorAttachment {
            view: slice_view,
            depth_slice: None,
            resolve_target: None,
            ops: Operations {
                load: LoadOp::Clear(Color::WHITE.to_linear().into()),
                store: StoreOp::Store,
            },
        })];
        let pass_descriptor = RenderPassDescriptor {
            label: Some("ground-cover shadow volume slice"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        };
        let mut pass = render_context
            .command_encoder()
            .begin_render_pass(&pass_descriptor);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(
            0,
            &mesh_view_bind_group.main,
            &mesh_view_bind_group.main_offsets,
        );
        pass.set_bind_group(1, slice_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn queue_ground_cover(
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<GroundCoverPipelines>,
    debug: Res<GroundCoverDebug>,
    pages: Res<RenderAssets<GpuGroundCoverPage>>,
    mut opaque_phases: ResMut<ViewBinnedRenderPhases<Opaque3d>>,
    mut alpha_mask_prepass_phases: ResMut<ViewBinnedRenderPhases<AlphaMask3dPrepass>>,
    draw_functions: Res<DrawFunctions<Opaque3d>>,
    prepass_draw_functions: Res<DrawFunctions<AlphaMask3dPrepass>>,
    view_key_cache: Res<ViewKeyCache>,
    views: Query<(Entity, &MainEntity, &ExtractedView, &Msaa), With<GroundCoverView>>,
) {
    let card_draw_function = draw_functions.read().id::<DrawGroundCoverCards>();
    let blade_draw_function = draw_functions.read().id::<DrawGroundCoverBlades>();
    let card_prepass_draw_function = prepass_draw_functions
        .read()
        .id::<DrawGroundCoverCardsPrepass>();
    let blade_prepass_draw_function = prepass_draw_functions
        .read()
        .id::<DrawGroundCoverBladesPrepass>();
    pipelines.shadow_volume_pipelines.clear();
    for (entity, main_entity, view, msaa) in &views {
        let (Some(phase), Some(prepass_phase), Some(mesh_view_key)) = (
            opaque_phases.get_mut(&view.retained_view_entity),
            alpha_mask_prepass_phases.get_mut(&view.retained_view_entity),
            view_key_cache.get(&view.retained_view_entity),
        ) else {
            continue;
        };
        phase.remove(*main_entity);
        prepass_phase.remove(*main_entity);
        if pages.iter().next().is_none() {
            continue;
        }
        let key = GroundCoverPipelineKey {
            msaa: *msaa,
            target_format: view.target_format,
            view_layout_bits: MeshPipelineViewLayoutKey::from(*mesh_view_key).bits(),
        };
        let shadow_volume_key = GroundShadowVolumePipelineKey {
            view_layout_bits: key.view_layout_bits,
        };
        let (
            Ok(card_pipeline),
            Ok(card_prepass_pipeline),
            Ok(blade_pipeline),
            Ok(blade_prepass_pipeline),
            Ok(shadow_volume_pipeline),
        ) = (
            pipelines.draw_variants.specialize(&pipeline_cache, key),
            pipelines.prepass_variants.specialize(&pipeline_cache, key),
            pipelines
                .blade_draw_variants
                .specialize(&pipeline_cache, key),
            pipelines
                .blade_prepass_variants
                .specialize(&pipeline_cache, key),
            pipelines
                .shadow_volume_variants
                .specialize(&pipeline_cache, shadow_volume_key),
        )
        else {
            continue;
        };
        pipelines
            .shadow_volume_pipelines
            .insert(entity, shadow_volume_pipeline);
        prepass_phase.add(
            OpaqueNoLightmap3dBatchSetKey {
                draw_function: card_prepass_draw_function,
                pipeline: card_prepass_pipeline,
                material_bind_group_index: None,
                slabs: MeshSlabs::default(),
            },
            OpaqueNoLightmap3dBinKey {
                asset_id: AssetId::<Mesh>::invalid().untyped(),
            },
            (entity, *main_entity),
            InputUniformIndex::default(),
            BinnedRenderPhaseType::NonMesh,
        );
        phase.add(
            Opaque3dBatchSetKey {
                draw_function: card_draw_function,
                pipeline: card_pipeline,
                material_bind_group_index: None,
                lightmap_slab: None,
                slabs: MeshSlabs::default(),
            },
            Opaque3dBinKey {
                asset_id: AssetId::<Mesh>::invalid().untyped(),
            },
            (entity, *main_entity),
            InputUniformIndex::default(),
            BinnedRenderPhaseType::NonMesh,
        );
        if debug.procedural_blades {
            prepass_phase.add(
                OpaqueNoLightmap3dBatchSetKey {
                    draw_function: blade_prepass_draw_function,
                    pipeline: blade_prepass_pipeline,
                    material_bind_group_index: None,
                    slabs: MeshSlabs::default(),
                },
                OpaqueNoLightmap3dBinKey {
                    asset_id: AssetId::<Mesh>::invalid().untyped(),
                },
                (entity, *main_entity),
                InputUniformIndex::default(),
                BinnedRenderPhaseType::NonMesh,
            );
            phase.add(
                Opaque3dBatchSetKey {
                    draw_function: blade_draw_function,
                    pipeline: blade_pipeline,
                    material_bind_group_index: None,
                    lightmap_slab: None,
                    slabs: MeshSlabs::default(),
                },
                Opaque3dBinKey {
                    asset_id: AssetId::<Mesh>::invalid().untyped(),
                },
                (entity, *main_entity),
                InputUniformIndex::default(),
                BinnedRenderPhaseType::NonMesh,
            );
        }
    }
}

type DrawGroundCoverCards = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    DrawGroundCoverCardsIndirect,
);
type DrawGroundCoverCardsPrepass = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    DrawGroundCoverCardsIndirect,
);
type DrawGroundCoverBlades = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    DrawGroundCoverBladesIndirect,
);
type DrawGroundCoverBladesPrepass = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    DrawGroundCoverBladesIndirect,
);

struct DrawGroundCoverCardsIndirect;

impl<P: PhaseItem> RenderCommand<P> for DrawGroundCoverCardsIndirect {
    type Param = (SRes<GroundCoverBuffers>, SRes<GroundCoverDebug>);
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        _view: ROQueryItem<'w, '_, Self::ViewQuery>,
        _entity: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        parameters: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let (buffers, debug) = parameters;
        let buffers = buffers.into_inner();
        if !debug.procedural_blades {
            pass.set_bind_group(1, &buffers.near_draw_bind_group, &[]);
            pass.draw_indirect(&buffers.near_args, 0);
            pass.set_bind_group(1, &buffers.mid_draw_bind_group, &[]);
            pass.draw_indirect(&buffers.mid_args, 0);
        }
        pass.set_bind_group(1, &buffers.far_draw_bind_group, &[]);
        pass.draw_indirect(&buffers.far_args, 0);
        RenderCommandResult::Success
    }
}

struct DrawGroundCoverBladesIndirect;

impl<P: PhaseItem> RenderCommand<P> for DrawGroundCoverBladesIndirect {
    type Param = SRes<GroundCoverBuffers>;
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        _view: ROQueryItem<'w, '_, Self::ViewQuery>,
        _entity: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        buffers: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let buffers = buffers.into_inner();
        pass.set_bind_group(1, &buffers.near_draw_bind_group, &[]);
        pass.draw_indirect(&buffers.near_args, 0);
        pass.set_bind_group(1, &buffers.mid_draw_bind_group, &[]);
        pass.draw_indirect(&buffers.mid_args, 0);
        RenderCommandResult::Success
    }
}

#[cfg(test)]
mod tests {
    fn validate_shader(label: &str, source: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{label} should parse as WGSL: {error:?}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("shader should pass Naga validation");
    }

    #[test]
    fn ground_cover_shaders_are_valid_wgsl() {
        validate_shader(
            "ground-cover cull shader",
            include_str!("../../../assets/shaders/ground_cover_cull.wgsl"),
        );

        // Naga alone does not understand Bevy/naga-oil `#import` directives. Validate the complete
        // ground-cover shader with only the imported shadow adapter replaced by an identity stub;
        // Bevy resolves and validates that adapter against its mesh-view libraries at runtime.
        let render_shader = include_str!("../../../assets/shaders/ground_cover.wgsl");
        let declarations = render_shader
            .find("struct VisibleInstance")
            .expect("ground-cover declarations should follow imports");
        let shadow_adapter = render_shader
            .find("fn exact_directional_shadow_visibility")
            .expect("ground cover should receive directional shadows");
        let fragment_entries = render_shader
            .find("// This fragment entry point")
            .expect("ground cover should document its depth prepass");
        let sanitized = format!(
            "{}fn directional_shadow_visibility(_input: VertexOutput) -> f32 {{ return 1.0; }}\nfn blade_surface_response(_input: VertexOutput, coverage: f32) -> f32 {{ return coverage; }}\n\n{}",
            &render_shader[declarations..shadow_adapter],
            &render_shader[fragment_entries..],
        );
        validate_shader("ground-cover render shader", &sanitized);

        // The volume generator also imports Bevy's CSM adapter. Its local uniform and
        // fullscreen-triangle interface remain independently valid WGSL.
        let volume_shader = include_str!("../../../assets/shaders/ground_cover_shadow_volume.wgsl");
        let volume_declarations = volume_shader
            .find("struct GroundShadowSlice")
            .expect("shadow-volume declarations should follow imports");
        let volume_shadow_adapter = volume_shader
            .find("fn exact_visibility")
            .expect("shadow volume should sample the directional CSM");
        let sanitized_volume = format!(
            "{}@fragment\nfn fragment(_input: VertexOutput) -> @location(0) vec4<f32> {{ return vec4<f32>(1.0); }}",
            &volume_shader[volume_declarations..volume_shadow_adapter],
        );
        validate_shader("ground-cover shadow-volume shader", &sanitized_volume);
    }

    #[test]
    fn depth_prepass_owns_cutout_before_the_shadowed_color_pass() {
        let render_shader = include_str!("../../../assets/shaders/ground_cover.wgsl");
        assert!(render_shader.contains("fn prepass_fragment(input: VertexOutput)"));
        assert!(render_shader.contains("fn fragment(input: VertexOutput)"));
        assert_eq!(
            render_shader
                .matches("visible_card_coverage(input)")
                .count(),
            2,
            "the prepass and color pass must share the exact alpha test",
        );
        assert!(render_shader.contains("textureSampleLevel(\n        ground_shadow_volume"));
        assert!(
            include_str!("../../../assets/shaders/ground_cover_shadow_volume.wgsl")
                .contains("shadows::fetch_directional_shadow("),
            "the filtered volume must retain exact animated caster silhouettes",
        );
    }

    #[test]
    fn ribbon_backend_uses_independent_spatial_blades() {
        let cull_shader = include_str!("../../../assets/shaders/ground_cover_cull.wgsl");
        let render_shader = include_str!("../../../assets/shaders/ground_cover.wgsl");
        assert!(cull_shader.contains("fn sample_clump_field(root: vec2<f32>)"));
        assert!(cull_shader.contains("let blade_visible = VisibleInstance("));
        assert!(cull_shader.contains("let blade_count = budgeted_blade_count(near_selected)"));
        assert!(cull_shader.contains("fn count_ribbon_candidates("));
        assert!(cull_shader.contains("let clump = sample_clump_field(root_xz)"));
        assert!(cull_shader.contains("let root_projection_scale = bottom_clip_w / root_clip_w"));
        assert!(cull_shader.contains("let coverage_rank = fract("));
        assert!(cull_shader.contains("field.xy * inverseSqrt"));
        assert!(render_shader.contains("let section_count = select(7u, 3u, low_detail)"));
        assert!(cull_shader.contains("15u,\n        final_camera.debug.y != 0u"));
        assert!(cull_shader.contains("7u,\n        final_camera.debug.y != 0u"));
        assert!(render_shader.contains("fn cubic_bezier_derivative("));
        assert!(render_shader.contains("let root = instance.position_yaw.xyz"));
        assert!(render_shader.contains("let resting_tip = rest_direction * resting_reach"));
        assert!(cull_shader.contains("fn ribbon_wind_displacement("));
        assert!(cull_shader.contains("let blade_wind = ribbon_wind_displacement("));
        assert!(render_shader.contains("let wind_displacement = instance.interaction.zw"));
        assert!(render_shader.contains("let vertical_wind_displacement = instance.motion.x"));
        assert!(
            !render_shader.contains("let group_wave = sin("),
            "ribbon wind phases must be evaluated once per blade during GPU expansion",
        );
        assert!(
            !render_shader.contains("let blade_index = vertex_index /"),
            "one visible ribbon instance must not expand back into a hidden blade clump",
        );
        let renderer = include_str!("renderer.rs");
        assert!(renderer.contains(
            "blade_draw_descriptor.primitive.topology = PrimitiveTopology::TriangleStrip"
        ));
        assert!(renderer.contains(
            "blade_prepass_descriptor.primitive.topology = PrimitiveTopology::TriangleStrip"
        ));
    }

    #[test]
    fn procedural_clump_texture_has_complete_coverage_preserving_mips() {
        let artwork =
            world::generate_ground_cover_card_artwork(world::GroundCoverBladeRecipe::built_in_v1());
        assert_eq!(artwork.mip_level_count, 9);
        assert!(artwork.is_valid());
        let expected_bytes = artwork.coverage_mips.len() / usize::from(artwork.variant_count);
        let base_level_bytes = usize::from(artwork.resolution).pow(2);
        for layer in 0..usize::from(artwork.variant_count) {
            let layer_start = layer * expected_bytes;
            let covered_texels = artwork.coverage_mips[layer_start..layer_start + base_level_bytes]
                .iter()
                .filter(|alpha| **alpha >= 82)
                .count();
            let coverage = covered_texels as f32 / base_level_bytes as f32;
            assert!(
                (0.08..0.65).contains(&coverage),
                "unexpected clump atlas coverage {coverage:.3} for layer {layer}"
            );
        }
        assert_ne!(
            *artwork.coverage_mips.last().unwrap(),
            0,
            "the final mip lost all coverage"
        );
    }
}
