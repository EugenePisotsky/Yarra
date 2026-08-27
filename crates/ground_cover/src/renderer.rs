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
        schedule::camera_driver,
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
        MeshPipelineSystems, MeshPipelineViewLayoutKey, MeshPipelineViewLayouts,
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
            CachedComputePipelineId, Canonical, ColorTargetState, ColorWrites, CompareFunction,
            ComputePassDescriptor, ComputePipelineDescriptor, DepthStencilState, Extent3d,
            FilterMode, FragmentState, FrontFace, MipmapFilterMode, Origin3d, PipelineCache,
            PolygonMode, PrimitiveState, PrimitiveTopology, RenderPipeline,
            RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
            Specializer, SpecializerKey, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture,
            TextureAspect, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType,
            TextureUsages, TextureView, TextureViewDescriptor, TextureViewDimension, Variants,
            VertexState,
            binding_types::{
                sampler, storage_buffer_read_only_sized, storage_buffer_sized, texture_2d_array,
                uniform_buffer_sized,
            },
        },
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
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
const MAX_VISIBLE_INSTANCES: u32 = 131_072;
const CULL_WORKGROUP_SIZE: u32 = 64;
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
            .add_render_command::<Opaque3d, DrawGroundCover>()
            .add_render_command::<AlphaMask3dPrepass, DrawGroundCoverPrepass>()
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
    camera: Buffer,
    interaction: Buffer,
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
            camera,
            interaction,
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
    cull_pipeline: CachedComputePipelineId,
    finalize_pipeline: CachedComputePipelineId,
    draw_variants: Variants<RenderPipeline, GroundCoverPipelineSpecializer>,
    prepass_variants: Variants<RenderPipeline, GroundCoverPipelineSpecializer>,
}

impl FromWorld for GroundCoverPipelines {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        let pipeline_cache = world.resource::<PipelineCache>();
        let view_layouts = world.resource::<MeshPipelineViewLayouts>().clone();
        let compute_shader = asset_server.load(COMPUTE_SHADER_PATH);
        let render_shader = asset_server.load(RENDER_SHADER_PATH);

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
                ),
            ),
        );
        let cull_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("ground-cover cull pipeline".into()),
            layout: vec![cull_layout.clone()],
            shader: compute_shader.clone(),
            entry_point: Some(Cow::Borrowed("cull")),
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

        Self {
            cull_layout,
            finalize_layout,
            draw_layout: draw_layout.clone(),
            cull_pipeline,
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
                    view_layouts,
                    draw_layout,
                    color_pass: false,
                },
                prepass_descriptor,
            ),
        }
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
        // Near detail, mid detail, output capacity, and geometry cutoff in pixels.
        limits: [18.0, 6.0, MAX_VISIBLE_INSTANCES as f32, 2.0],
        wind: [
            wind.elapsed_seconds,
            wind.base_strength,
            wind.gust_strength,
            wind.spatial_scale,
        ],
        wind_direction: [wind_direction.x, wind_direction.y, wind.speed, 0.0],
        debug: [debug.mode.gpu_value(), 0, 0, 0],
    };
    render_queue.write_buffer(&buffers.camera, 0, bytemuck::bytes_of(&uniform));

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

    let (Some(cull_pipeline), Some(finalize_pipeline)) = (
        pipeline_cache.get_compute_pipeline(pipelines.cull_pipeline),
        pipeline_cache.get_compute_pipeline(pipelines.finalize_pipeline),
    ) else {
        return;
    };

    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("ground-cover GPU culling"),
        ..default()
    });
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

fn queue_ground_cover(
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<GroundCoverPipelines>,
    pages: Res<RenderAssets<GpuGroundCoverPage>>,
    mut opaque_phases: ResMut<ViewBinnedRenderPhases<Opaque3d>>,
    mut alpha_mask_prepass_phases: ResMut<ViewBinnedRenderPhases<AlphaMask3dPrepass>>,
    draw_functions: Res<DrawFunctions<Opaque3d>>,
    prepass_draw_functions: Res<DrawFunctions<AlphaMask3dPrepass>>,
    view_key_cache: Res<ViewKeyCache>,
    views: Query<(Entity, &MainEntity, &ExtractedView, &Msaa), With<GroundCoverView>>,
) {
    let draw_function = draw_functions.read().id::<DrawGroundCover>();
    let prepass_draw_function = prepass_draw_functions.read().id::<DrawGroundCoverPrepass>();
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
        let (Ok(pipeline), Ok(prepass_pipeline)) = (
            pipelines.draw_variants.specialize(&pipeline_cache, key),
            pipelines.prepass_variants.specialize(&pipeline_cache, key),
        ) else {
            continue;
        };
        prepass_phase.add(
            OpaqueNoLightmap3dBatchSetKey {
                draw_function: prepass_draw_function,
                pipeline: prepass_pipeline,
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
                draw_function,
                pipeline,
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

type DrawGroundCover = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    DrawGroundCoverIndirect,
);
type DrawGroundCoverPrepass = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    DrawGroundCoverIndirect,
);

struct DrawGroundCoverIndirect;

impl<P: PhaseItem> RenderCommand<P> for DrawGroundCoverIndirect {
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
        pass.set_bind_group(1, &buffers.far_draw_bind_group, &[]);
        pass.draw_indirect(&buffers.far_args, 0);
        RenderCommandResult::Success
    }
}

#[cfg(test)]
mod tests {
    fn validate_shader(source: &str) {
        let module = naga::front::wgsl::parse_str(source).expect("shader should parse as WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("shader should pass Naga validation");
    }

    #[test]
    fn ground_cover_shaders_are_valid_wgsl() {
        validate_shader(include_str!(
            "../../../assets/shaders/ground_cover_cull.wgsl"
        ));

        // Naga alone does not understand Bevy/naga-oil `#import` directives. Validate the complete
        // ground-cover shader with only the imported shadow adapter replaced by an identity stub;
        // Bevy resolves and validates that adapter against its mesh-view libraries at runtime.
        let render_shader = include_str!("../../../assets/shaders/ground_cover.wgsl");
        let declarations = render_shader
            .find("struct VisibleInstance")
            .expect("ground-cover declarations should follow imports");
        let shadow_adapter = render_shader
            .find("fn directional_shadow_visibility")
            .expect("ground cover should receive directional shadows");
        let fragment_entries = render_shader
            .find("// This fragment entry point")
            .expect("ground cover should document its depth prepass");
        let sanitized = format!(
            "{}fn directional_shadow_visibility(_input: VertexOutput) -> f32 {{ return 1.0; }}\n\n{}",
            &render_shader[declarations..shadow_adapter],
            &render_shader[fragment_entries..],
        );
        validate_shader(&sanitized);
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
        assert!(render_shader.contains("shadows::fetch_directional_shadow("));
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
