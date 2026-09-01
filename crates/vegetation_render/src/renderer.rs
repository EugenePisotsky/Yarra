use std::{borrow::Cow, collections::HashMap, mem::size_of};

use bevy::{
    asset::AssetId,
    core_pipeline::{
        core_3d::{CORE_3D_DEPTH_FORMAT, Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey},
        schedule::camera_driver,
    },
    ecs::{
        query::ROQueryItem,
        system::{SystemParamItem, lifetimeless::SRes},
    },
    mesh::Mesh,
    pbr::{MeshPipelineViewLayoutKey, MeshPipelineViewLayouts, SetMeshViewBindGroup, ViewKeyCache},
    prelude::*,
    render::{
        ExtractSchedule, Render, RenderSystems,
        diagnostic::{DiagnosticsRecorder, RecordDiagnostics},
        render_phase::{
            AddRenderCommand, BinnedRenderPhaseType, DrawFunctions, InputUniformIndex, PhaseItem,
            RenderCommand, RenderCommandResult, SetItemPipeline, TrackedRenderPass,
            ViewBinnedRenderPhases,
        },
        render_resource::{
            BindGroup, BindGroupEntries, BindGroupLayout, BindGroupLayoutDescriptor,
            BindGroupLayoutEntries, Buffer, BufferDescriptor, BufferInitDescriptor, BufferUsages,
            CachedComputePipelineId, Canonical, ColorTargetState, ColorWrites, CompareFunction,
            ComputePassDescriptor, ComputePipelineDescriptor, DepthStencilState, FragmentState,
            FrontFace, IndexFormat, MapMode, PipelineCache, PolygonMode, PrimitiveState,
            PrimitiveTopology, RenderPipeline, RenderPipelineDescriptor, ShaderStages, Specializer,
            SpecializerKey, TextureFormat, Variants, VertexState,
            binding_types::{
                storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer_sized,
            },
        },
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        sync_world::MainEntity,
        view::ExtractedView,
    },
};
use bytemuck::{Pod, Zeroable};
use vegetation::{
    GrowthPattern, RepresentationKind, TopologyFamily, TopologyProfile, VegetationGroupingProfile,
    candidate_density_retention, candidate_domain, decode_octahedral_normal,
};

use crate::{
    VegetationDebugDraw, VegetationDebugScene, VegetationDebugSettings, VegetationDebugView,
    VegetationDiagnostics, VegetationLighting, VegetationProfileMode, VegetationSun,
};

const COMPUTE_SHADER_PATH: &str = "shaders/vegetation_debug_compute.wgsl";
const SCHEDULE_SHADER_PATH: &str = "shaders/vegetation_schedule_compute.wgsl";
const DRAW_SHADER_PATH: &str = "shaders/vegetation_debug_draw.wgsl";
// Hard device-profile budgets, not density targets. A nonzero capacity-drop counter is a rejected
// configuration: normal population LOD must fit before the emergency guard is reached.
const SINGLE_HIGH_CAPACITY: u32 = 16_384;
const SINGLE_LOW_CAPACITY: u32 = 32_768;
const SPLIT_HIGH_CAPACITY: u32 = 32_768;
const SPLIT_LOW_CAPACITY: u32 = 262_144;
const PROCEDURAL_INSTANCE_CAPACITY: u32 =
    SINGLE_HIGH_CAPACITY + SINGLE_LOW_CAPACITY + SPLIT_HIGH_CAPACITY + SPLIT_LOW_CAPACITY;
// Keep the expensive high-topology population below the arena's hard guard even for a top-down
// view of a fully covered field. The remaining headroom absorbs stochastic placement variance,
// clump attraction, page boundaries, and the smooth high/low transition annulus.
const HIGH_DETAIL_BUDGET_UTILIZATION: f32 = 0.5;
const MAX_HIGH_DETAIL_RADIUS: f32 = 96.0;
const MAX_DIAGNOSTIC_INSTANCES: u32 = 65_536;
const TOPOLOGY_BIN_COUNT: u32 = 4;
const WORKGROUP_SIZE: u32 = 64;
const GPU_TELEMETRY_WORD_COUNT: u64 = 16;
const GPU_TELEMETRY_SIZE: u64 = GPU_TELEMETRY_WORD_COUNT * size_of::<u32>() as u64;
const DRAW_INDEXED_ARGS_WORD_COUNT: u64 = 5;
const DRAW_ARGS_SIZE: u64 =
    TOPOLOGY_BIN_COUNT as u64 * DRAW_INDEXED_ARGS_WORD_COUNT * size_of::<u32>() as u64;
const TELEMETRY_READBACK_SIZE: u64 = GPU_TELEMETRY_SIZE + DRAW_ARGS_SIZE;
const TELEMETRY_CAPTURE_INTERVAL_FRAMES: u32 = 30;
const MAX_RENDER_SECTIONS: u8 = 8;
const MAX_LOW_RENDER_SECTIONS: u8 = 3;
const DIAGNOSTIC_INDEX_COUNT: u32 = 6;
const SINGLE_HIGH_INDEX_COUNT: u32 = 48;
const SINGLE_LOW_INDEX_COUNT: u32 = 18;
const SPLIT_HIGH_INDEX_COUNT: u32 = 42;
const SPLIT_LOW_INDEX_COUNT: u32 = 6;
const DIAGNOSTIC_FIRST_INDEX: u32 = 0;
const SINGLE_HIGH_FIRST_INDEX: u32 = DIAGNOSTIC_FIRST_INDEX + DIAGNOSTIC_INDEX_COUNT;
const SINGLE_LOW_FIRST_INDEX: u32 = SINGLE_HIGH_FIRST_INDEX + SINGLE_HIGH_INDEX_COUNT;
const SPLIT_HIGH_FIRST_INDEX: u32 = SINGLE_LOW_FIRST_INDEX + SINGLE_LOW_INDEX_COUNT;
const SPLIT_LOW_FIRST_INDEX: u32 = SPLIT_HIGH_FIRST_INDEX + SPLIT_HIGH_INDEX_COUNT;

fn topology_vertex(blade: u16, row: u16, right: bool) -> u16 {
    debug_assert!(blade < 2 && row < 16);
    (blade << 5) | (row << 1) | u16::from(right)
}

fn append_strip_indices(indices: &mut Vec<u16>, blade: u16, section_count: u16) {
    for section in 0..section_count {
        let left = topology_vertex(blade, section, false);
        let right = topology_vertex(blade, section, true);
        let next_left = topology_vertex(blade, section + 1, false);
        let next_right = topology_vertex(blade, section + 1, true);
        indices.extend_from_slice(&[left, right, next_right, left, next_right, next_left]);
    }
}

fn build_topology_indices() -> Vec<u16> {
    let mut indices = vec![0, 1, 2, 0, 2, 3];
    append_strip_indices(&mut indices, 0, u16::from(MAX_RENDER_SECTIONS));
    append_strip_indices(&mut indices, 0, u16::from(MAX_LOW_RENDER_SECTIONS));

    // Two short blades divide the single-high vertex budget rather than duplicating it.
    append_strip_indices(&mut indices, 0, 4);
    append_strip_indices(&mut indices, 1, 3);

    // At low LOD each short blade is one tapered triangle. Its six unique inputs fit below the
    // single-low topology's eight-input budget.
    for blade in 0..2 {
        indices.extend_from_slice(&[
            topology_vertex(blade, 0, false),
            topology_vertex(blade, 0, true),
            topology_vertex(blade, 1, false),
        ]);
    }
    debug_assert_eq!(
        indices.len(),
        (SPLIT_LOW_FIRST_INDEX + SPLIT_LOW_INDEX_COUNT) as usize
    );
    indices
}

pub(crate) fn install(render_app: &mut SubApp) {
    render_app
        .add_render_command::<Opaque3d, DrawVegetationDebug>()
        .add_systems(ExtractSchedule, begin_telemetry_readback)
        .add_systems(
            Render,
            (
                prepare_telemetry_staging.in_set(RenderSystems::PrepareResourcesFlush),
                prepare.in_set(RenderSystems::PrepareBindGroups),
                queue.in_set(RenderSystems::Queue),
            ),
        )
        .add_systems(RenderGraph, generate.before(camera_driver));
}

pub(crate) fn initialize(world: &mut World) {
    world.init_resource::<VegetationPipelines>();
    world.init_resource::<VegetationBuffers>();
    world.init_resource::<VegetationTelemetryStaging>();
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct WorkItemGpu {
    // xy: page origin x/z, z: page size, w: minimum low-LOD population threshold
    page: [f32; 4],
    // xy: minimum world-lattice cell, zw: cell count
    domain: [i32; 4],
    // x: candidates/cell, y: candidate count, z: coverage offset, w: coverage resolution
    layout: [u32; 4],
    // x: choice offset, y: choice count, z: seed, w: competition group + 1 (zero is none)
    population: [u32; 4],
    // x: spacing, y: child radius, z: parent/uniform jitter, w: density retention
    growth: [f32; 4],
    // x: radial, y: tangential, z: random, w: field flow direction
    direction_weights: [f32; 4],
    // xy: world flow direction, z: requested density, w: unused
    flow_density: [f32; 4],
    // x: clump spacing, y: feature jitter, z: boundary softness, w: root attraction
    grouping: [f32; 4],
    // x: center retention, y: edge retention, z: falloff, w: group density variation
    group_density: [f32; 4],
    // x: shared group direction weight, y: per-root angular jitter
    orientation: [f32; 4],
    // x: first work item on page, y: work-item count on page, z: placement pattern,
    // w: grouping source (0 none, 1 parent, 2 Voronoi)
    peers: [u32; 4],
    // x: surface sample offset, y: surface resolution
    surface: [u32; 4],
    // x: maximum height, y: maximum horizontal reach, z: minimum high-LOD threshold,
    // w: maximum low-LOD density
    bounds: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct SpeciesChoiceGpu {
    // x: species index
    metadata: [u32; 4],
    // x: normalized cumulative threshold
    threshold: [f32; 4],
    // x: high density, y: low density, z: far density,
    // w: density-budgeted high-topology radius
    density: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct SpeciesGpu {
    root_color: [f32; 4],
    // xyz: tip color, w: maximum height
    tip_color_height: [f32; 4],
    // xy: height range, zw: half-width range
    bounds: [f32; 4],
    // x: high sections, y: low sections, z: blades/render unit, w: longitudinal power
    topology: [f32; 4],
    // xy: tilt range; zw: broad-leaf droop range
    shape: [f32; 4],
    // x: lateral curve/camber, y: pair spread,
    // z: tangent of maximum ribbon view-opening angle / broad crown radius,
    // w: maximum horizontal reach
    shape_secondary: [f32; 4],
    // xy: normalized-height root-handle forward/normal vector,
    // zw: normalized-height tip-handle forward/normal vector
    curve_variant_a: [f32; 4],
    curve_variant_b: [f32; 4],
    // x: clump color variation, y: roughness, z: transmission, w: normal rounding
    material: [f32; 4],
    // x: root AO, y: tip AO, z: high-LOD threshold,
    // w: density-budgeted high-topology radius
    shading: [f32; 4],
    // xyz: group coherence for height, complete silhouette, and lateral curve
    group_response: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct SurfaceSampleGpu {
    // x: height, y: validity
    height_validity: [f32; 4],
    normal: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct ProceduralInstanceGpu {
    // xyz: root, w: packed clump variant and nested LOD rank
    root_clump: [f32; 4],
    // x: packed rest direction, y: species index, z: packed surface normal xz,
    // w: low 24 bits seed + high 8 bits population-density target
    geometry: [u32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct DebugInstanceGpu {
    // xyz: root, w: rest direction x
    root_direction: [f32; 4],
    // x: rest direction z, y: clump variant, z: species index as f32, w: packed surface normal xz
    direction_species: [f32; 4],
    // xyz: parent position, w: candidate outcome code
    parent_status: [f32; 4],
    // x: local occupancy, y: stable rank, z: group distance, w: group influence
    diagnostics: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct CameraGpu {
    clip_from_world: [f32; 16],
    camera_position: [f32; 4],
    // x: vertical focal length in pixels, y: viewport width, z: viewport height
    projection: [f32; 4],
    // xyz: direction from the surface toward the strongest directional light, w: active
    sun_direction: [f32; 4],
    sun_radiance: [f32; 4],
    ambient_radiance: [f32; 4],
    // x: diffuse, y: specular, z: transmission, w: received-shadow strength
    lighting: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct DebugConfigGpu {
    // x: VegetationDebugMode; y: VegetationDensityMode; z: VegetationLightingMode; w: reserved.
    // Mirrors WGSL `vec4<u32>` exactly.
    values: [u32; 4],
}

#[derive(Resource)]
struct VegetationPipelines {
    schedule_layout: BindGroupLayoutDescriptor,
    compute_layout: BindGroupLayoutDescriptor,
    draw_layout: BindGroupLayoutDescriptor,
    schedule: CachedComputePipelineId,
    generate: CachedComputePipelineId,
    finalize: CachedComputePipelineId,
    draw_variants: Variants<RenderPipeline, VegetationPipelineSpecializer>,
}

impl FromWorld for VegetationPipelines {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        let compute_shader = asset_server.load(COMPUTE_SHADER_PATH);
        let schedule_shader = asset_server.load(SCHEDULE_SHADER_PATH);
        let draw_shader = asset_server.load(DRAW_SHADER_PATH);
        let pipeline_cache = world.resource::<PipelineCache>();
        let view_layouts = world.resource::<MeshPipelineViewLayouts>().clone();
        let schedule_layout = BindGroupLayoutDescriptor::new(
            "vegetation-v2 visible work scheduling",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                ),
            ),
        );
        let compute_layout = BindGroupLayoutDescriptor::new(
            "vegetation-v2 debug generation",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    uniform_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                ),
            ),
        );
        let draw_layout = BindGroupLayoutDescriptor::new(
            "vegetation-v2 debug draw",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    uniform_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                ),
            ),
        );
        let schedule = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("vegetation-v2 visible work scheduling".into()),
            layout: vec![schedule_layout.clone()],
            shader: schedule_shader,
            entry_point: Some(Cow::Borrowed("schedule")),
            ..default()
        });
        let generate = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("vegetation-v2 classify once and emit".into()),
            layout: vec![compute_layout.clone()],
            shader: compute_shader.clone(),
            entry_point: Some(Cow::Borrowed("generate")),
            ..default()
        });
        let finalize = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("vegetation-v2 debug finalize".into()),
            layout: vec![compute_layout.clone()],
            shader: compute_shader,
            entry_point: Some(Cow::Borrowed("finalize")),
            ..default()
        });
        let draw_descriptor = RenderPipelineDescriptor {
            label: Some("vegetation-v2 placement debug".into()),
            // The exact mesh-view layout is selected per camera by the pipeline specializer.
            layout: Vec::new(),
            vertex: VertexState {
                shader: draw_shader.clone(),
                entry_point: Some(Cow::Borrowed("vertex")),
                shader_defs: vec!["SHADOW_FILTER_METHOD_HARDWARE_2X2".into()],
                buffers: Vec::new(),
                ..default()
            },
            fragment: Some(FragmentState {
                shader: draw_shader,
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
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: default(),
                bias: default(),
            }),
            ..default()
        };
        Self {
            schedule_layout,
            compute_layout,
            draw_layout: draw_layout.clone(),
            schedule,
            generate,
            finalize,
            draw_variants: Variants::new(
                VegetationPipelineSpecializer {
                    view_layouts,
                    draw_layout,
                },
                draw_descriptor,
            ),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, SpecializerKey)]
struct VegetationPipelineKey {
    msaa: Msaa,
    target_format: TextureFormat,
    view_layout_bits: u32,
}

struct VegetationPipelineSpecializer {
    view_layouts: MeshPipelineViewLayouts,
    draw_layout: BindGroupLayoutDescriptor,
}

impl Specializer<RenderPipeline> for VegetationPipelineSpecializer {
    type Key = VegetationPipelineKey;

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
        descriptor.layout = vec![view_layout.main_layout, self.draw_layout.clone()];
        descriptor.multisample.count = key.msaa.samples();
        descriptor.fragment.as_mut().unwrap().targets[0]
            .as_mut()
            .unwrap()
            .format = key.target_format;
        Ok(key)
    }
}

#[derive(Resource)]
struct VegetationBuffers {
    work_items: Buffer,
    work_items_capacity: u64,
    visible_work_items: Buffer,
    visible_work_items_capacity: u64,
    candidate_dispatch_args: Buffer,
    choices: Buffer,
    choices_capacity: u64,
    coverage: Buffer,
    coverage_capacity: u64,
    surfaces: Buffer,
    surfaces_capacity: u64,
    species: Buffer,
    species_capacity: u64,
    procedural_instances: Buffer,
    diagnostic_instances: Buffer,
    topology_indices: Buffer,
    args: Buffer,
    gpu_telemetry: Buffer,
    camera: Buffer,
    debug_config: Buffer,
    schedule_bind_group: BindGroup,
    compute_bind_group: BindGroup,
    draw_bind_group: BindGroup,
    uploaded_revision: u64,
    work_item_count: u32,
    maximum_candidate_count: u32,
    active: bool,
}

impl FromWorld for VegetationBuffers {
    fn from_world(world: &mut World) -> Self {
        let pipelines = world.resource::<VegetationPipelines>();
        let pipeline_cache = world.resource::<PipelineCache>();
        let schedule_layout = pipeline_cache.get_bind_group_layout(&pipelines.schedule_layout);
        let compute_layout = pipeline_cache.get_bind_group_layout(&pipelines.compute_layout);
        let draw_layout = pipeline_cache.get_bind_group_layout(&pipelines.draw_layout);
        let render_device = world.resource::<RenderDevice>();
        let work_items = dummy_storage(render_device, "vegetation-v2 empty work items");
        let visible_work_items =
            dummy_storage(render_device, "vegetation-v2 empty visible work queue");
        let candidate_dispatch_args = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 candidate dispatch args"),
            size: 12,
            usage: BufferUsages::STORAGE | BufferUsages::INDIRECT | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let choices = dummy_storage(render_device, "vegetation-v2 empty choices");
        let coverage = dummy_storage(render_device, "vegetation-v2 empty coverage");
        let surfaces = dummy_storage(render_device, "vegetation-v2 empty surfaces");
        let species = dummy_storage(render_device, "vegetation-v2 empty species");
        let procedural_instances = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 compact topology-bin instances"),
            size: u64::from(PROCEDURAL_INSTANCE_CAPACITY)
                * size_of::<ProceduralInstanceGpu>() as u64,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let diagnostic_instances = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 placement diagnostic instances"),
            size: u64::from(MAX_DIAGNOSTIC_INSTANCES) * size_of::<DebugInstanceGpu>() as u64,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let topology_indices = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("vegetation-v2 fixed-budget topology indices"),
            contents: bytemuck::cast_slice(&build_topology_indices()),
            usage: BufferUsages::INDEX,
        });
        let args = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 topology-bin indirect args"),
            size: DRAW_ARGS_SIZE,
            usage: BufferUsages::STORAGE
                | BufferUsages::INDIRECT
                | BufferUsages::COPY_DST
                | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let gpu_telemetry = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 GPU telemetry"),
            size: GPU_TELEMETRY_SIZE,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let camera = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 debug camera"),
            size: size_of::<CameraGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let debug_config = render_device.create_buffer(&BufferDescriptor {
            label: Some("vegetation-v2 debug configuration"),
            size: size_of::<DebugConfigGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let schedule_bind_group = create_schedule_bind_group(
            render_device,
            &schedule_layout,
            &work_items,
            &visible_work_items,
            &candidate_dispatch_args,
            &camera,
            &gpu_telemetry,
            &debug_config,
        );
        let compute_bind_group = create_compute_bind_group(
            render_device,
            &compute_layout,
            [
                &work_items,
                &choices,
                &coverage,
                &surfaces,
                &procedural_instances,
                &diagnostic_instances,
                &args,
                &visible_work_items,
                &debug_config,
                &camera,
                &gpu_telemetry,
            ],
        );
        let draw_bind_group = create_draw_bind_group(
            render_device,
            &draw_layout,
            &procedural_instances,
            &diagnostic_instances,
            &species,
            &camera,
            &debug_config,
        );
        Self {
            work_items,
            work_items_capacity: 16,
            visible_work_items,
            visible_work_items_capacity: 16,
            candidate_dispatch_args,
            choices,
            choices_capacity: 16,
            coverage,
            coverage_capacity: 16,
            surfaces,
            surfaces_capacity: 16,
            species,
            species_capacity: 16,
            procedural_instances,
            diagnostic_instances,
            topology_indices,
            args,
            gpu_telemetry,
            camera,
            debug_config,
            schedule_bind_group,
            compute_bind_group,
            draw_bind_group,
            uploaded_revision: 0,
            work_item_count: 0,
            maximum_candidate_count: 0,
            active: false,
        }
    }
}

fn create_schedule_bind_group(
    render_device: &RenderDevice,
    layout: &BindGroupLayout,
    work_items: &Buffer,
    visible_work_items: &Buffer,
    candidate_dispatch_args: &Buffer,
    camera: &Buffer,
    gpu_telemetry: &Buffer,
    debug_config: &Buffer,
) -> BindGroup {
    render_device.create_bind_group(
        Some("vegetation-v2 visible work scheduling"),
        layout,
        &BindGroupEntries::sequential((
            work_items.as_entire_binding(),
            visible_work_items.as_entire_binding(),
            candidate_dispatch_args.as_entire_binding(),
            camera.as_entire_binding(),
            gpu_telemetry.as_entire_binding(),
            debug_config.as_entire_binding(),
        )),
    )
}

fn dummy_storage(render_device: &RenderDevice, label: &'static str) -> Buffer {
    render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some(label),
        contents: &[0; 16],
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
    })
}

fn create_compute_bind_group(
    render_device: &RenderDevice,
    layout: &BindGroupLayout,
    buffers: [&Buffer; 11],
) -> BindGroup {
    render_device.create_bind_group(
        Some("vegetation-v2 debug generation"),
        layout,
        &BindGroupEntries::sequential((
            buffers[0].as_entire_binding(),
            buffers[1].as_entire_binding(),
            buffers[2].as_entire_binding(),
            buffers[3].as_entire_binding(),
            buffers[4].as_entire_binding(),
            buffers[5].as_entire_binding(),
            buffers[6].as_entire_binding(),
            buffers[7].as_entire_binding(),
            buffers[8].as_entire_binding(),
            buffers[9].as_entire_binding(),
            buffers[10].as_entire_binding(),
        )),
    )
}

fn create_draw_bind_group(
    render_device: &RenderDevice,
    layout: &BindGroupLayout,
    procedural_instances: &Buffer,
    diagnostic_instances: &Buffer,
    species: &Buffer,
    camera: &Buffer,
    debug_config: &Buffer,
) -> BindGroup {
    render_device.create_bind_group(
        Some("vegetation-v2 debug draw"),
        layout,
        &BindGroupEntries::sequential((
            procedural_instances.as_entire_binding(),
            diagnostic_instances.as_entire_binding(),
            species.as_entire_binding(),
            camera.as_entire_binding(),
            debug_config.as_entire_binding(),
        )),
    )
}

#[derive(Resource)]
struct VegetationTelemetryStaging {
    buffer: Option<Buffer>,
    frames_until_capture: u32,
}

impl Default for VegetationTelemetryStaging {
    fn default() -> Self {
        Self {
            buffer: None,
            frames_until_capture: 0,
        }
    }
}

fn prepare_telemetry_staging(
    render_device: Res<RenderDevice>,
    mut staging: ResMut<VegetationTelemetryStaging>,
) {
    if staging.buffer.is_some() {
        return;
    }
    if staging.frames_until_capture > 0 {
        staging.frames_until_capture -= 1;
        return;
    }
    staging.buffer = Some(render_device.create_buffer(&BufferDescriptor {
        label: Some("vegetation-v2 telemetry readback"),
        size: TELEMETRY_READBACK_SIZE,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }));
    staging.frames_until_capture = TELEMETRY_CAPTURE_INTERVAL_FRAMES;
}

fn begin_telemetry_readback(
    mut staging: ResMut<VegetationTelemetryStaging>,
    diagnostics: Res<VegetationDiagnostics>,
) {
    let Some(buffer) = staging.buffer.take() else {
        return;
    };
    let map_buffer = buffer.clone();
    let diagnostics = diagnostics.clone();
    map_buffer
        .slice(..)
        .map_async(MapMode::Read, move |result| {
            if let Err(error) = result {
                warn!("vegetation-v2 telemetry readback failed: {error}");
                return;
            }

            {
                let mapped = buffer.slice(..).get_mapped_range();
                let words: &[u32] = bytemuck::cast_slice(&mapped);
                let required_words = (TELEMETRY_READBACK_SIZE / size_of::<u32>() as u64) as usize;
                if words.len() >= required_words {
                    let args_offset = GPU_TELEMETRY_WORD_COUNT as usize;
                    let emitted_instances = std::array::from_fn(|bin| {
                        words[args_offset + bin * DRAW_INDEXED_ARGS_WORD_COUNT as usize + 1]
                    });
                    diagnostics.update(|snapshot| {
                        snapshot.scheduled_work_items = words[0];
                        snapshot.dispatched_candidate_lanes = words[10]
                            .saturating_mul(WORKGROUP_SIZE)
                            .saturating_mul(words[0]);
                        snapshot.candidate_evaluations = words[1];
                        snapshot.eligible_instances.copy_from_slice(&words[2..6]);
                        snapshot
                            .capacity_dropped_instances
                            .copy_from_slice(&words[6..10]);
                        snapshot.emitted_instances = emitted_instances;
                        snapshot.submitted_indices = emitted_instances
                            .into_iter()
                            .zip([
                                SINGLE_HIGH_INDEX_COUNT,
                                SINGLE_LOW_INDEX_COUNT,
                                SPLIT_HIGH_INDEX_COUNT,
                                SPLIT_LOW_INDEX_COUNT,
                            ])
                            .map(|(instances, indices)| u64::from(instances) * u64::from(indices))
                            .sum();
                        snapshot.topology_vertex_inputs = emitted_instances
                            .into_iter()
                            .zip([18_u32, 8, 18, 6])
                            .map(|(instances, vertices)| u64::from(instances) * u64::from(vertices))
                            .sum();
                        snapshot.procedural_instance_capacity = PROCEDURAL_INSTANCE_CAPACITY;
                        snapshot.procedural_instance_bytes =
                            u64::from(PROCEDURAL_INSTANCE_CAPACITY)
                                * size_of::<ProceduralInstanceGpu>() as u64;
                        snapshot.gpu_samples = snapshot.gpu_samples.saturating_add(1);
                    });
                }
            }
            buffer.unmap();
        });
}

#[allow(clippy::too_many_arguments)] // Bevy render-world system parameters are independent resources.
fn prepare(
    scene: Option<Res<VegetationDebugScene>>,
    settings: Res<VegetationDebugSettings>,
    lighting: Res<VegetationLighting>,
    sun: Res<VegetationSun>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<VegetationPipelines>,
    diagnostics: Res<VegetationDiagnostics>,
    views: Query<&ExtractedView, With<VegetationDebugView>>,
    mut buffers: ResMut<VegetationBuffers>,
) {
    let Some(scene) = scene else {
        buffers.active = false;
        return;
    };
    if scene.revision() != buffers.uploaded_revision {
        let packed = pack_scene(scene.scene());
        let schedule_layout = pipeline_cache.get_bind_group_layout(&pipelines.schedule_layout);
        let compute_layout = pipeline_cache.get_bind_group_layout(&pipelines.compute_layout);
        let draw_layout = pipeline_cache.get_bind_group_layout(&pipelines.draw_layout);
        let mut replaced_buffers = 0_u64;
        let mut uploaded_bytes = 0_u64;

        let work_items = bytemuck::cast_slice(&packed.work_items);
        uploaded_bytes += work_items.len() as u64;
        if let Some((buffer, capacity)) = update_storage(
            &render_device,
            &render_queue,
            &buffers.work_items,
            buffers.work_items_capacity,
            "vegetation-v2 work items",
            work_items,
        ) {
            buffers.work_items = buffer;
            buffers.work_items_capacity = capacity;
            replaced_buffers += 1;
        }
        if let Some((buffer, capacity)) = grow_storage(
            &render_device,
            buffers.visible_work_items_capacity,
            "vegetation-v2 visible work queue",
            packed.work_items.len() as u64 * size_of::<u32>() as u64,
        ) {
            buffers.visible_work_items = buffer;
            buffers.visible_work_items_capacity = capacity;
            replaced_buffers += 1;
        }

        let choices = bytemuck::cast_slice(&packed.choices);
        uploaded_bytes += choices.len() as u64;
        if let Some((buffer, capacity)) = update_storage(
            &render_device,
            &render_queue,
            &buffers.choices,
            buffers.choices_capacity,
            "vegetation-v2 species choices",
            choices,
        ) {
            buffers.choices = buffer;
            buffers.choices_capacity = capacity;
            replaced_buffers += 1;
        }

        let coverage = bytemuck::cast_slice(&packed.coverage);
        uploaded_bytes += coverage.len() as u64;
        if let Some((buffer, capacity)) = update_storage(
            &render_device,
            &render_queue,
            &buffers.coverage,
            buffers.coverage_capacity,
            "vegetation-v2 coverage fields",
            coverage,
        ) {
            buffers.coverage = buffer;
            buffers.coverage_capacity = capacity;
            replaced_buffers += 1;
        }

        let surfaces = bytemuck::cast_slice(&packed.surfaces);
        uploaded_bytes += surfaces.len() as u64;
        if let Some((buffer, capacity)) = update_storage(
            &render_device,
            &render_queue,
            &buffers.surfaces,
            buffers.surfaces_capacity,
            "vegetation-v2 surface fields",
            surfaces,
        ) {
            buffers.surfaces = buffer;
            buffers.surfaces_capacity = capacity;
            replaced_buffers += 1;
        }

        let species = bytemuck::cast_slice(&packed.species);
        uploaded_bytes += species.len() as u64;
        if let Some((buffer, capacity)) = update_storage(
            &render_device,
            &render_queue,
            &buffers.species,
            buffers.species_capacity,
            "vegetation-v2 species",
            species,
        ) {
            buffers.species = buffer;
            buffers.species_capacity = capacity;
            replaced_buffers += 1;
        }

        if replaced_buffers > 0 {
            buffers.schedule_bind_group = create_schedule_bind_group(
                &render_device,
                &schedule_layout,
                &buffers.work_items,
                &buffers.visible_work_items,
                &buffers.candidate_dispatch_args,
                &buffers.camera,
                &buffers.gpu_telemetry,
                &buffers.debug_config,
            );
            buffers.compute_bind_group = create_compute_bind_group(
                &render_device,
                &compute_layout,
                [
                    &buffers.work_items,
                    &buffers.choices,
                    &buffers.coverage,
                    &buffers.surfaces,
                    &buffers.procedural_instances,
                    &buffers.diagnostic_instances,
                    &buffers.args,
                    &buffers.visible_work_items,
                    &buffers.debug_config,
                    &buffers.camera,
                    &buffers.gpu_telemetry,
                ],
            );
            buffers.draw_bind_group = create_draw_bind_group(
                &render_device,
                &draw_layout,
                &buffers.procedural_instances,
                &buffers.diagnostic_instances,
                &buffers.species,
                &buffers.camera,
                &buffers.debug_config,
            );
        }
        buffers.work_item_count = packed.work_items.len() as u32;
        buffers.maximum_candidate_count = packed.maximum_candidate_count;
        buffers.uploaded_revision = scene.revision();
        diagnostics.update(|snapshot| {
            snapshot.scene_revision = scene.revision();
            snapshot.source_repacks = snapshot.source_repacks.saturating_add(1);
            snapshot.source_buffer_reallocations = snapshot
                .source_buffer_reallocations
                .saturating_add(replaced_buffers);
            snapshot.source_uploaded_bytes = uploaded_bytes;
            snapshot.source_buffer_capacity_bytes = buffers.work_items_capacity
                + buffers.visible_work_items_capacity
                + buffers.choices_capacity
                + buffers.coverage_capacity
                + buffers.surfaces_capacity
                + buffers.species_capacity;
            snapshot.source_pages = scene.scene().pages.len() as u32;
            snapshot.source_work_items = buffers.work_item_count;
            snapshot.maximum_candidates_per_item = buffers.maximum_candidate_count;
        });
    }

    let Some(view) = views.iter().next() else {
        buffers.active = false;
        return;
    };
    let clip_from_world = view
        .clip_from_world
        .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
    render_queue.write_buffer(
        &buffers.camera,
        0,
        bytemuck::bytes_of(&CameraGpu {
            clip_from_world: clip_from_world.to_cols_array(),
            camera_position: view
                .world_from_view
                .translation()
                .extend(view.viewport.w.max(1) as f32)
                .to_array(),
            projection: [
                view.clip_from_view.y_axis.y.abs() * view.viewport.w.max(1) as f32 * 0.5,
                view.viewport.z.max(1) as f32,
                view.viewport.w.max(1) as f32,
                0.0,
            ],
            sun_direction: sun
                .direction_to_light
                .extend(if sun.active { 1.0 } else { 0.0 })
                .to_array(),
            sun_radiance: sun.radiance.extend(0.0).to_array(),
            ambient_radiance: sun.ambient_radiance.extend(0.0).to_array(),
            lighting: [
                lighting.diffuse_strength.max(0.0),
                lighting.specular_strength.max(0.0),
                lighting.transmission_strength.max(0.0),
                lighting.received_shadow_strength.clamp(0.0, 1.0),
            ],
        }),
    );
    render_queue.write_buffer(
        &buffers.debug_config,
        0,
        bytemuck::bytes_of(&DebugConfigGpu {
            values: [
                settings.mode as u32,
                settings.density_mode as u32,
                settings.lighting_mode as u32,
                0,
            ],
        }),
    );
    buffers.active = buffers.work_item_count > 0 && buffers.maximum_candidate_count > 0;
}

fn update_storage(
    render_device: &RenderDevice,
    render_queue: &RenderQueue,
    buffer: &Buffer,
    capacity: u64,
    label: &'static str,
    contents: &[u8],
) -> Option<(Buffer, u64)> {
    let required = (contents.len() as u64).max(16);
    if required <= capacity {
        if !contents.is_empty() {
            render_queue.write_buffer(buffer, 0, contents);
        }
        return None;
    }
    let new_capacity = storage_capacity_for(required);
    let buffer = render_device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: new_capacity,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    if !contents.is_empty() {
        render_queue.write_buffer(&buffer, 0, contents);
    }
    Some((buffer, new_capacity))
}

fn grow_storage(
    render_device: &RenderDevice,
    capacity: u64,
    label: &'static str,
    required: u64,
) -> Option<(Buffer, u64)> {
    let required = required.max(16);
    if required <= capacity {
        return None;
    }
    let new_capacity = storage_capacity_for(required);
    Some((
        render_device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: new_capacity,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        }),
        new_capacity,
    ))
}

fn storage_capacity_for(required: u64) -> u64 {
    required.max(16).next_power_of_two()
}

struct PackedScene {
    work_items: Vec<WorkItemGpu>,
    choices: Vec<SpeciesChoiceGpu>,
    coverage: Vec<f32>,
    surfaces: Vec<SurfaceSampleGpu>,
    species: Vec<SpeciesGpu>,
    maximum_candidate_count: u32,
}

fn high_detail_radii(scene: &vegetation::VegetationScene) -> [f32; 2] {
    let mut maximum_density = [0.0_f32; 2];
    for page in &scene.pages {
        let mut page_density = [0.0_f32; 2];
        for field in &page.fields {
            let population = scene
                .catalog
                .population(field.population)
                .expect("validated scene population");
            // A mixed population may choose either topology. Charge its full root density to each
            // possible high bin: this is deliberately conservative and keeps the hard arena guard
            // independent of species-choice noise.
            let mut topology_present = [false; 2];
            for choice in &population.species {
                let species = scene
                    .catalog
                    .species(choice.species)
                    .expect("validated species choice");
                topology_present[topology_bin(species.topology) as usize] = true;
            }
            for (density, present) in page_density.iter_mut().zip(topology_present) {
                if present {
                    *density += population.density_per_square_meter;
                }
            }
        }
        for (maximum, density) in maximum_density.iter_mut().zip(page_density) {
            *maximum = maximum.max(density);
        }
    }

    let radius_for = |capacity: u32, density: f32| {
        if density <= f32::EPSILON {
            return MAX_HIGH_DETAIL_RADIUS;
        }
        ((capacity as f32 * HIGH_DETAIL_BUDGET_UTILIZATION) / (std::f32::consts::PI * density))
            .sqrt()
            .min(MAX_HIGH_DETAIL_RADIUS)
    };
    [
        radius_for(SINGLE_HIGH_CAPACITY, maximum_density[0]),
        radius_for(SPLIT_HIGH_CAPACITY, maximum_density[1]),
    ]
}

fn effective_horizontal_reach(species: &vegetation::VegetationSpecies) -> f32 {
    let height = species.bounds.maximum_height;
    let width = species.bounds.maximum_half_width * 1.24;
    // A cubic Bezier stays inside the convex hull of its control points. Bound the same p1/p2/p3
    // construction used by the vertex shader in its orthonormal blade frame, then include the
    // root offset, grazing-angle width expansion, and maximum wind displacement. This prevents an
    // artist-entered reach that is too small from making whole pages disappear at view edges.
    let (p1_radius, p2_radius, root_offset) = match species.topology {
        TopologyProfile::Ribbon(profile) => {
            let maximum_root_handle = profile
                .curve_variant_a
                .root_handle_length
                .max(profile.curve_variant_b.root_handle_length);
            let maximum_tip_handle = profile
                .curve_variant_a
                .tip_handle_length
                .max(profile.curve_variant_b.tip_handle_length);
            // P2 is the tip endpoint minus its tangent handle plus lateral camber. The triangle
            // inequality is deliberately conservative for every independent tilt/curve sample.
            let p2_radius =
                height * (1.0 + maximum_tip_handle + profile.maximum_lateral_curve * 0.22);
            (
                height * maximum_root_handle,
                p2_radius,
                if profile.blades_per_render_unit > 1 {
                    species.bounds.maximum_half_width * 0.75
                } else {
                    0.0
                },
            )
        }
        TopologyProfile::BroadLeafCluster(profile) => {
            let maximum_tilt = 0.52 + profile.maximum_droop * 0.45;
            let maximum_bend = profile.maximum_droop;
            let p1_radius = height * (0.34_f32.powi(2) + (maximum_bend * 0.05).powi(2)).sqrt();
            let p2_normal = maximum_tilt.cos() * 0.68 + maximum_bend * 0.16;
            let p2_forward = maximum_tilt.sin() * 0.68 + maximum_bend * 0.22;
            let p2_side = profile.maximum_camber * 0.22;
            let p2_radius =
                height * (p2_normal.powi(2) + p2_forward.powi(2) + p2_side.powi(2)).sqrt();
            (p1_radius, p2_radius, profile.crown_radius * 0.35)
        }
    };
    species.bounds.maximum_horizontal_reach.max(
        height.max(p1_radius).max(p2_radius)
            + root_offset
            + width
            + species.wind.maximum_tip_displacement,
    )
}

fn pack_scene(scene: &vegetation::VegetationScene) -> PackedScene {
    let high_detail_radii = high_detail_radii(scene);
    let species_indices = scene
        .catalog
        .species
        .iter()
        .enumerate()
        .map(|(index, species)| (species.id, index as u32))
        .collect::<HashMap<_, _>>();
    let species = scene
        .catalog
        .species
        .iter()
        .map(|species| {
            pack_species(
                species,
                high_detail_radii[topology_bin(species.topology) as usize],
            )
        })
        .collect::<Vec<_>>();

    let mut work_items = Vec::new();
    let mut choices = Vec::new();
    let mut coverage = Vec::new();
    let mut surfaces = Vec::new();
    let mut maximum_candidate_count = 0;
    for page in &scene.pages {
        let page_work_start = work_items.len() as u32;
        let page_work_count = page.fields.len() as u32;
        let surface_offset = surfaces.len() as u32;
        let (minimum_surface_height, maximum_surface_height) = page.surface.heights.iter().fold(
            (f32::INFINITY, f32::NEG_INFINITY),
            |(minimum, maximum), height| (minimum.min(*height), maximum.max(*height)),
        );
        surfaces.extend(
            page.surface
                .heights
                .iter()
                .zip(&page.surface.normals_oct)
                .zip(&page.surface.validity)
                .map(|((height, normal), validity)| {
                    let normal = decode_octahedral_normal(*normal);
                    SurfaceSampleGpu {
                        height_validity: [*height, f32::from(*validity) / 255.0, 0.0, 0.0],
                        normal: [normal[0], normal[1], normal[2], 0.0],
                    }
                }),
        );
        for field in &page.fields {
            let population = scene
                .catalog
                .population(field.population)
                .expect("validated scene population");
            let domain = candidate_domain(page, population);
            maximum_candidate_count = maximum_candidate_count.max(domain.candidate_count());
            let coverage_offset = coverage.len() as u32;
            coverage.extend(field.coverage.iter().map(|value| f32::from(*value) / 255.0));
            let choice_offset = choices.len() as u32;
            let total_weight = population
                .species
                .iter()
                .map(|choice| choice.weight)
                .sum::<f32>();
            let mut cumulative_weight = 0.0;
            let mut maximum_height = 0.0_f32;
            let mut maximum_horizontal_reach = 0.0_f32;
            let mut minimum_high_threshold = f32::INFINITY;
            let mut minimum_low_threshold = f32::INFINITY;
            let mut maximum_low_density = 0.0_f32;
            for choice in &population.species {
                cumulative_weight += choice.weight;
                let species_definition = scene
                    .catalog
                    .species(choice.species)
                    .expect("validated species choice");
                let lod = procedural_lod_profile(species_definition);
                let horizontal_reach = effective_horizontal_reach(species_definition);
                maximum_height = maximum_height.max(species_definition.bounds.maximum_height);
                maximum_horizontal_reach = maximum_horizontal_reach.max(horizontal_reach);
                minimum_high_threshold = minimum_high_threshold.min(lod.high_minimum_pixels);
                minimum_low_threshold = minimum_low_threshold.min(lod.low_minimum_pixels);
                maximum_low_density = maximum_low_density.max(lod.low_density_fraction);
                choices.push(SpeciesChoiceGpu {
                    metadata: [
                        species_indices[&choice.species],
                        topology_bin(species_definition.topology),
                        horizontal_reach.to_bits(),
                        species_definition.bounds.maximum_height.to_bits(),
                    ],
                    threshold: [
                        cumulative_weight / total_weight,
                        lod.high_minimum_pixels,
                        lod.low_minimum_pixels,
                        lod.far_minimum_pixels,
                    ],
                    density: [
                        1.0,
                        lod.low_density_fraction,
                        lod.far_density_fraction,
                        high_detail_radii[topology_bin(species_definition.topology) as usize],
                    ],
                });
            }

            let (radius, jitter, pattern) = match population.growth {
                GrowthPattern::Uniform { jitter } => (0.0, jitter, 0),
                GrowthPattern::ParentChild {
                    radius,
                    parent_jitter,
                    ..
                } => (radius, parent_jitter, 1),
            };
            let (grouping, group_density, grouping_source) = match population.grouping {
                VegetationGroupingProfile::None => ([0.0; 4], [1.0, 1.0, 1.0, 0.0], 0),
                VegetationGroupingProfile::Parent => ([0.0; 4], [1.0, 1.0, 1.0, 0.0], 1),
                VegetationGroupingProfile::Voronoi(profile) => (
                    [
                        profile.spacing,
                        profile.feature_jitter,
                        profile.boundary_softness,
                        profile.root_attraction,
                    ],
                    [
                        profile.center_retention,
                        profile.edge_retention,
                        profile.retention_falloff,
                        profile.density_variation,
                    ],
                    2,
                ),
            };
            let orientation = population.orientation;
            work_items.push(WorkItemGpu {
                page: [
                    page.origin_xz[0],
                    page.origin_xz[1],
                    page.size,
                    minimum_low_threshold,
                ],
                domain: [
                    domain.cell_min[0],
                    domain.cell_min[1],
                    domain.cell_count[0] as i32,
                    domain.cell_count[1] as i32,
                ],
                layout: [
                    domain.candidates_per_cell,
                    domain.candidate_count(),
                    coverage_offset,
                    u32::from(field.resolution),
                ],
                population: [
                    choice_offset,
                    population.species.len() as u32,
                    population.seed,
                    population
                        .competition_group
                        .map_or(0, |group| u32::from(group) + 1),
                ],
                growth: [
                    domain.spacing,
                    radius,
                    jitter,
                    candidate_density_retention(population),
                ],
                direction_weights: [
                    orientation.radial_weight,
                    orientation.tangential_weight,
                    orientation.random_weight,
                    orientation.flow_weight,
                ],
                flow_density: [
                    field.flow_direction[0],
                    field.flow_direction[1],
                    population.density_per_square_meter,
                    0.0,
                ],
                grouping,
                group_density,
                orientation: [
                    orientation.shared_group_weight,
                    orientation.angular_jitter_radians,
                    0.0,
                    0.0,
                ],
                peers: [page_work_start, page_work_count, pattern, grouping_source],
                surface: [
                    surface_offset,
                    u32::from(page.surface.resolution),
                    minimum_surface_height.to_bits(),
                    maximum_surface_height.to_bits(),
                ],
                bounds: [
                    maximum_height,
                    maximum_horizontal_reach,
                    minimum_high_threshold,
                    maximum_low_density,
                ],
            });
        }
    }

    PackedScene {
        work_items,
        choices,
        coverage,
        surfaces,
        species,
        maximum_candidate_count,
    }
}

fn topology_code(family: TopologyFamily) -> f32 {
    match family {
        TopologyFamily::Ribbon => 0.0,
        TopologyFamily::RibbonTuft => 1.0,
        TopologyFamily::BroadLeafCluster => 2.0,
        TopologyFamily::StemAndHead => 3.0,
        TopologyFamily::CardImpostor => 4.0,
        TopologyFamily::AuthoredMesh => 5.0,
    }
}

fn topology_bin(topology: TopologyProfile) -> u32 {
    match topology {
        TopologyProfile::Ribbon(profile) if profile.blades_per_render_unit == 1 => 0,
        TopologyProfile::Ribbon(_) | TopologyProfile::BroadLeafCluster(_) => 1,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ProceduralLodProfile {
    high_minimum_pixels: f32,
    low_minimum_pixels: f32,
    far_minimum_pixels: f32,
    low_density_fraction: f32,
    far_density_fraction: f32,
}

fn procedural_lod_profile(species: &vegetation::VegetationSpecies) -> ProceduralLodProfile {
    let procedural_levels = species
        .representations
        .iter()
        .filter(|level| matches!(level.kind, RepresentationKind::Procedural(_)))
        .collect::<Vec<_>>();
    let high = procedural_levels
        .first()
        .copied()
        .expect("validated species has a procedural representation");
    let low = procedural_levels.get(1).copied().unwrap_or(high);
    let far = procedural_levels.last().copied().unwrap_or(low);
    ProceduralLodProfile {
        high_minimum_pixels: high.minimum_projected_size,
        low_minimum_pixels: low.minimum_projected_size,
        far_minimum_pixels: far.minimum_projected_size,
        low_density_fraction: low.density_fraction,
        far_density_fraction: far.density_fraction,
    }
}

fn pack_species(species: &vegetation::VegetationSpecies, high_detail_radius: f32) -> SpeciesGpu {
    let lod = procedural_lod_profile(species);
    let horizontal_reach = effective_horizontal_reach(species);
    let (topology, shape, shape_secondary, curve_variant_a, curve_variant_b) =
        match species.topology {
            TopologyProfile::Ribbon(profile) => (
                [
                    f32::from(profile.high_section_count.min(MAX_RENDER_SECTIONS)),
                    f32::from(profile.low_section_count.min(MAX_LOW_RENDER_SECTIONS)),
                    f32::from(profile.blades_per_render_unit.min(2)),
                    profile.longitudinal_power,
                ],
                [
                    profile.curve_variant_a.tip_tilt_radians,
                    profile.curve_variant_b.tip_tilt_radians,
                    0.0,
                    0.0,
                ],
                [
                    profile.maximum_lateral_curve,
                    profile.pair_spread_radians,
                    profile.maximum_view_opening_radians.tan(),
                    horizontal_reach,
                ],
                pack_ribbon_curve(profile.curve_variant_a),
                pack_ribbon_curve(profile.curve_variant_b),
            ),
            TopologyProfile::BroadLeafCluster(profile) => (
                [
                    f32::from(profile.high_section_count.min(MAX_RENDER_SECTIONS)),
                    f32::from(profile.low_section_count.min(MAX_LOW_RENDER_SECTIONS)),
                    2.0,
                    0.72,
                ],
                [
                    0.14 + profile.minimum_droop * 0.2,
                    0.52 + profile.maximum_droop * 0.45,
                    profile.minimum_droop,
                    profile.maximum_droop,
                ],
                [
                    profile.maximum_camber,
                    2.2,
                    profile.crown_radius,
                    horizontal_reach,
                ],
                [0.0; 4],
                [0.0; 4],
            ),
        };
    SpeciesGpu {
        root_color: [
            species.material.root_color[0],
            species.material.root_color[1],
            species.material.root_color[2],
            topology_code(species.topology.family()),
        ],
        tip_color_height: [
            species.material.tip_color[0],
            species.material.tip_color[1],
            species.material.tip_color[2],
            species.bounds.maximum_height,
        ],
        bounds: [
            species.bounds.minimum_height,
            species.bounds.maximum_height,
            species.bounds.minimum_half_width,
            species.bounds.maximum_half_width,
        ],
        topology,
        shape,
        shape_secondary,
        curve_variant_a,
        curve_variant_b,
        material: [
            species.material.clump_color_variation,
            species.material.perceptual_roughness,
            species.material.transmission,
            species.material.normal_rounding,
        ],
        shading: [
            species.material.root_ao,
            species.material.tip_ao,
            lod.high_minimum_pixels,
            high_detail_radius,
        ],
        group_response: [
            species.group_response.height_coherence,
            species.group_response.silhouette_coherence,
            species.group_response.lateral_curve_coherence,
            0.0,
        ],
    }
}

fn pack_ribbon_curve(profile: vegetation::RibbonCurveProfile) -> [f32; 4] {
    [
        profile.root_tangent_radians.sin() * profile.root_handle_length,
        profile.root_tangent_radians.cos() * profile.root_handle_length,
        profile.tip_tangent_radians.sin() * profile.tip_handle_length,
        profile.tip_tangent_radians.cos() * profile.tip_handle_length,
    ]
}

fn generate(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    pipelines: Res<VegetationPipelines>,
    buffers: Res<VegetationBuffers>,
    telemetry_staging: Res<VegetationTelemetryStaging>,
    settings: Res<VegetationDebugSettings>,
) {
    if settings.profile_mode == VegetationProfileMode::DrawFrozen {
        copy_telemetry_to_staging(&mut render_context, &buffers, &telemetry_staging);
        return;
    }
    render_context
        .command_encoder()
        .clear_buffer(&buffers.args, 0, None);
    render_context
        .command_encoder()
        .clear_buffer(&buffers.candidate_dispatch_args, 0, None);
    render_context
        .command_encoder()
        .clear_buffer(&buffers.gpu_telemetry, 0, None);
    if !buffers.active {
        copy_telemetry_to_staging(&mut render_context, &buffers, &telemetry_staging);
        return;
    }
    let (Some(schedule_pipeline), Some(generate_pipeline), Some(finalize_pipeline)) = (
        pipeline_cache.get_compute_pipeline(pipelines.schedule),
        pipeline_cache.get_compute_pipeline(pipelines.generate),
        pipeline_cache.get_compute_pipeline(pipelines.finalize),
    ) else {
        copy_telemetry_to_staging(&mut render_context, &buffers, &telemetry_staging);
        return;
    };
    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let mut schedule_pass =
        render_context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("vegetation-v2 visible work scheduling"),
                ..default()
            });
    let schedule_span = diagnostics.pass_span(&mut schedule_pass, "vegetation_v2_schedule");
    schedule_pass.set_bind_group(0, &buffers.schedule_bind_group, &[]);
    schedule_pass.set_pipeline(schedule_pipeline);
    schedule_pass.dispatch_workgroups(buffers.work_item_count.div_ceil(WORKGROUP_SIZE), 1, 1);
    schedule_span.end(&mut schedule_pass);
    drop(schedule_pass);
    if settings.profile_mode == VegetationProfileMode::ScheduleOnly {
        copy_telemetry_to_staging(&mut render_context, &buffers, &telemetry_staging);
        return;
    }

    let mut pass = render_context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("vegetation-v2 deterministic visible generation"),
            ..default()
        });
    let generate_span = diagnostics.pass_span(&mut pass, "vegetation_v2_generate");
    pass.set_bind_group(0, &buffers.compute_bind_group, &[]);
    pass.set_pipeline(generate_pipeline);
    pass.dispatch_workgroups_indirect(&buffers.candidate_dispatch_args, 0);
    generate_span.end(&mut pass);
    let finalize_span = diagnostics.pass_span(&mut pass, "vegetation_v2_finalize");
    pass.set_pipeline(finalize_pipeline);
    pass.dispatch_workgroups(1, 1, 1);
    finalize_span.end(&mut pass);
    drop(pass);
    copy_telemetry_to_staging(&mut render_context, &buffers, &telemetry_staging);
}

fn copy_telemetry_to_staging(
    render_context: &mut RenderContext,
    buffers: &VegetationBuffers,
    staging: &VegetationTelemetryStaging,
) {
    let Some(staging) = staging.buffer.as_ref() else {
        return;
    };
    render_context.command_encoder().copy_buffer_to_buffer(
        &buffers.gpu_telemetry,
        0,
        staging,
        0,
        GPU_TELEMETRY_SIZE,
    );
    render_context.command_encoder().copy_buffer_to_buffer(
        &buffers.args,
        0,
        staging,
        GPU_TELEMETRY_SIZE,
        DRAW_ARGS_SIZE,
    );
}

fn queue(
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<VegetationPipelines>,
    buffers: Res<VegetationBuffers>,
    settings: Res<VegetationDebugSettings>,
    mut opaque_phases: ResMut<ViewBinnedRenderPhases<Opaque3d>>,
    draw_functions: Res<DrawFunctions<Opaque3d>>,
    view_key_cache: Res<ViewKeyCache>,
    views: Query<(Entity, &ExtractedView, &Msaa), With<VegetationDebugView>>,
    draw_entity: Query<(Entity, &MainEntity), With<VegetationDebugDraw>>,
) {
    let Ok((draw_entity, draw_main_entity)) = draw_entity.single() else {
        return;
    };
    let draw_function = draw_functions.read().id::<DrawVegetationDebug>();
    for (_view_entity, view, msaa) in &views {
        let (Some(phase), Some(mesh_view_key)) = (
            opaque_phases.get_mut(&view.retained_view_entity),
            view_key_cache.get(&view.retained_view_entity),
        ) else {
            continue;
        };
        phase.remove(*draw_main_entity);
        if !buffers.active
            || matches!(
                settings.profile_mode,
                VegetationProfileMode::ComputeOnly | VegetationProfileMode::ScheduleOnly
            )
        {
            continue;
        }
        let Ok(pipeline) = pipelines.draw_variants.specialize(
            &pipeline_cache,
            VegetationPipelineKey {
                msaa: *msaa,
                target_format: view.target_format,
                view_layout_bits: MeshPipelineViewLayoutKey::from(*mesh_view_key).bits(),
            },
        ) else {
            continue;
        };
        phase.add(
            Opaque3dBatchSetKey {
                draw_function,
                pipeline,
                material_bind_group_index: None,
                lightmap_slab: None,
                slabs: default(),
            },
            Opaque3dBinKey {
                asset_id: AssetId::<Mesh>::invalid().untyped(),
            },
            (draw_entity, *draw_main_entity),
            InputUniformIndex::default(),
            BinnedRenderPhaseType::NonMesh,
        );
    }
}

type DrawVegetationDebug = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    DrawVegetationDebugIndirect,
);

struct DrawVegetationDebugIndirect;

impl<P: PhaseItem> RenderCommand<P> for DrawVegetationDebugIndirect {
    type Param = (SRes<VegetationBuffers>, SRes<DiagnosticsRecorder>);
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        _view: ROQueryItem<'w, '_, Self::ViewQuery>,
        _entity: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        (buffers, diagnostics): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let buffers = buffers.into_inner();
        let draw_span = diagnostics.pass_span(pass, "vegetation_v2_draw");
        pass.set_bind_group(1, &buffers.draw_bind_group, &[]);
        pass.set_index_buffer(buffers.topology_indices.slice(..), IndexFormat::Uint16);
        pass.draw_indexed_indirect(&buffers.args, 0);
        pass.draw_indexed_indirect(&buffers.args, 20);
        pass.draw_indexed_indirect(&buffers.args, 40);
        pass.draw_indexed_indirect(&buffers.args, 60);
        draw_span.end(pass);
        RenderCommandResult::Success
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn gpu_contracts_have_expected_alignment() {
        assert_eq!(size_of::<WorkItemGpu>(), 208);
        assert_eq!(size_of::<SpeciesChoiceGpu>(), 48);
        assert_eq!(size_of::<SpeciesGpu>(), 176);
        assert_eq!(size_of::<SurfaceSampleGpu>(), 32);
        assert_eq!(size_of::<ProceduralInstanceGpu>(), 32);
        assert_eq!(size_of::<DebugInstanceGpu>(), 64);
        assert_eq!(size_of::<CameraGpu>(), 160);
        assert_eq!(size_of::<DebugConfigGpu>(), 16);
        assert_eq!(GPU_TELEMETRY_SIZE, 64);
        assert_eq!(DRAW_ARGS_SIZE, 80);
        assert_eq!(TELEMETRY_READBACK_SIZE, 144);
    }

    #[test]
    fn reference_fixture_packs_all_fields() {
        let scene = vegetation::fixtures::reference_scene();
        let packed = pack_scene(&scene);
        let high_detail_radii = high_detail_radii(&scene);
        assert_eq!(packed.work_items.len(), 8);
        assert_eq!(packed.species.len(), 4);
        assert!(packed.choices.iter().any(|choice| choice.metadata[1] == 0));
        assert!(packed.choices.iter().any(|choice| choice.metadata[1] == 1));
        assert!(packed.choices.iter().all(|choice| choice.metadata[1] < 2));
        assert!(packed.maximum_candidate_count > 0);
        assert!(packed.coverage.len() > scene.pages.len());
        assert_eq!(packed.surfaces.len(), scene.pages.len() * 4);
        assert!(
            packed.choices.iter().all(|choice| {
                choice.density[3] == high_detail_radii[choice.metadata[1] as usize]
            })
        );
        let expected_single_radius = ((SINGLE_HIGH_CAPACITY as f32
            * HIGH_DETAIL_BUDGET_UTILIZATION)
            / (std::f32::consts::PI * 5.0))
            .sqrt();
        let expected_split_radius = ((SPLIT_HIGH_CAPACITY as f32 * HIGH_DETAIL_BUDGET_UTILIZATION)
            / (std::f32::consts::PI * 25.0))
            .sqrt();
        assert!((high_detail_radii[0] - expected_single_radius).abs() < 1e-4);
        assert!((high_detail_radii[1] - expected_split_radius).abs() < 1e-4);
        let short_species_index = scene
            .catalog
            .species
            .iter()
            .position(|species| species.key == "short_split_fill_ribbon")
            .unwrap();
        let short_species = &scene.catalog.species[short_species_index];
        let TopologyProfile::Ribbon(short_topology) = short_species.topology else {
            unreachable!();
        };
        assert_eq!(
            packed.species[short_species_index].curve_variant_a,
            pack_ribbon_curve(short_topology.curve_variant_a)
        );
        assert_eq!(
            packed.species[short_species_index].shape[..2],
            [
                short_topology.curve_variant_a.tip_tilt_radians,
                short_topology.curve_variant_b.tip_tilt_radians,
            ]
        );
        assert_eq!(
            packed.species[short_species_index].shape_secondary[2],
            short_topology.maximum_view_opening_radians.tan()
        );
        assert!(effective_horizontal_reach(short_species) >= short_species.bounds.maximum_height);
        assert!(
            effective_horizontal_reach(short_species)
                > short_species.bounds.maximum_horizontal_reach
        );
        assert!(scene.catalog.species.iter().all(|species| {
            let lod = procedural_lod_profile(species);
            lod.low_density_fraction <= 0.32 && lod.far_density_fraction <= 0.32
        }));
    }

    #[test]
    fn topology_indices_enforce_fixed_per_unit_vertex_budgets() {
        let indices = build_topology_indices();
        let ranges = [
            (DIAGNOSTIC_FIRST_INDEX, DIAGNOSTIC_INDEX_COUNT, 4_usize),
            (SINGLE_HIGH_FIRST_INDEX, SINGLE_HIGH_INDEX_COUNT, 18),
            (SINGLE_LOW_FIRST_INDEX, SINGLE_LOW_INDEX_COUNT, 8),
            (SPLIT_HIGH_FIRST_INDEX, SPLIT_HIGH_INDEX_COUNT, 18),
            (SPLIT_LOW_FIRST_INDEX, SPLIT_LOW_INDEX_COUNT, 6),
        ];
        for (first, count, expected_unique) in ranges {
            let range = first as usize..(first + count) as usize;
            assert_eq!(
                indices[range].iter().copied().collect::<HashSet<_>>().len(),
                expected_unique
            );
        }
        assert!(SPLIT_HIGH_INDEX_COUNT <= SINGLE_HIGH_INDEX_COUNT);
        assert!(SPLIT_LOW_INDEX_COUNT <= SINGLE_LOW_INDEX_COUNT);
        assert!(
            u64::from(PROCEDURAL_INSTANCE_CAPACITY) * size_of::<ProceduralInstanceGpu>() as u64
                <= 11 * 1024 * 1024
        );
    }

    #[test]
    fn shaders_parse_as_wgsl() {
        for source in [
            include_str!("../../../assets/shaders/vegetation_schedule_compute.wgsl"),
            include_str!("../../../assets/shaders/vegetation_debug_compute.wgsl"),
        ] {
            let module = naga::front::wgsl::parse_str(source).unwrap();
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap();
        }

        // Naga's standalone WGSL parser does not run Bevy's #import preprocessor. Validate the
        // complete draw shader with only the imported CSM adapter replaced by an identity stub.
        let draw = include_str!("../../../assets/shaders/vegetation_debug_draw.wgsl");
        let declarations = draw.find("struct ProceduralInstance").unwrap();
        let shadow_adapter = draw.find("fn directional_shadow_visibility").unwrap();
        let post_adapter = draw.find("fn radiance_tint").unwrap();
        let sanitized = format!(
            "{}fn directional_shadow_visibility(_input: VertexOutput) -> f32 {{ return 1.0; }}\n{}",
            &draw[declarations..shadow_adapter],
            &draw[post_adapter..],
        )
        .replace("pbr_lighting::D_GGX", "test_d_ggx")
        .replace(
            "pbr_lighting::V_SmithGGXCorrelated",
            "test_v_smith_ggx_correlated",
        )
        .replace("view_bindings::view.exposure", "1.0");
        let sanitized = format!(
            "fn test_d_ggx(_roughness: f32, _n_dot_h: f32) -> f32 {{ return 1.0; }}\n\
             fn test_v_smith_ggx_correlated(_roughness: f32, _n_dot_v: f32, _n_dot_l: f32) -> f32 {{ return 1.0; }}\n\
             {sanitized}"
        );
        let module = naga::front::wgsl::parse_str(&sanitized).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn production_generation_classifies_each_candidate_once() {
        let compute = include_str!("../../../assets/shaders/vegetation_debug_compute.wgsl");
        assert_eq!(compute.matches("evaluate_candidate(").count(), 2);
        assert!(compute.contains("fn generate("));
        assert!(!compute.contains("fn count("));
        assert!(!compute.contains("fn plan("));
        assert!(!compute.contains("fn emit("));
        assert!(!compute.contains("capacity_histogram"));
    }

    #[test]
    fn lod_uses_the_full_authored_blade_envelope() {
        let schedule = include_str!("../../../assets/shaders/vegetation_schedule_compute.wgsl");
        let compute = include_str!("../../../assets/shaders/vegetation_debug_compute.wgsl");
        let draw = include_str!("../../../assets/shaders/vegetation_debug_draw.wgsl");

        assert!(schedule.contains("fn maximum_projected_extent("));
        assert!(compute.contains("fn projected_blade_extent_pixels("));
        assert!(compute.contains("fn budgeted_projected_blade_extent_pixels("));
        assert!(draw.contains("fn projected_blade_extent_pixels("));
        assert!(draw.contains("fn budgeted_projected_blade_extent_pixels("));
        assert!(compute.contains("let maximum_reach = bitcast<f32>(choice.metadata.z);"));
        assert!(draw.contains("let reach = profile.shape_secondary.w;"));
        assert!(compute.contains("let high_radius = max(choice.density.w, 1e-3);"));
        assert!(draw.contains("let high_radius = max(profile.shading.w, 1e-3);"));
        assert!(schedule.contains("fn maximum_projected_population_spacing("));
        assert!(compute.contains("fn projected_population_spacing_pixels("));
        assert!(compute.contains("fn population_lod_density("));
        assert!(compute.contains("fn balanced_population_lod_density("));
        assert!(compute.contains("fn population_lod_retention_limit("));
        assert!(compute.contains("debug_config.values.y == DENSITY_MODE_BALANCED"));
        assert!(schedule.contains("debug_config.values.y != 0u"));
        assert!(draw.contains("let population_density = f32(instance.geometry.w >> 24u) / 255.0;"));
        assert!(draw.contains("let density_width = select("));
        assert!(draw.contains("let half_band = BALANCED_DENSITY_FADE_BAND * 0.5;"));
        assert!(!draw.contains("let density_scale = select("));
        assert!(compute.contains("let staggered_high_radius = high_radius * mix("));
        assert!(draw.contains("let staggered_high_radius = high_radius * mix("));
        assert!(draw.contains("let local_ribbon_side = normalize3_or("));
        assert!(draw.contains("let signed_alignment = dot("));
        assert!(draw.contains("let opening_tangent = min("));
        assert!(draw.contains("var rendered_ribbon_side = local_ribbon_side;"));
        assert!(draw.contains("rendered_ribbon_side = normalize3_or("));
        assert!(draw.contains("output.world_normal = physical_normal;"));
        assert!(draw.contains("output.ribbon_side_rounding = vec4<f32>("));
        assert!(!draw.contains("let view_opening_weight = smoothstep("));
        assert!(!draw.contains("cross(rendered_ribbon_side, curve_tangent)"));
        assert!(draw.contains("dot(input.world_normal, view_direction) >= 0.0"));
        assert!(!draw.contains("@builtin(front_facing)"));
        assert!(!draw.contains("fn apply_edge_on_fullness("));
        assert!(!draw.contains("let view_fullness = mix(1.0, 1.24"));
        assert!(!compute.contains("fn projected_height_pixels("));
        assert!(!schedule.contains("fn maximum_projected_height("));
    }

    #[test]
    fn production_draw_uses_exposure_aware_rounded_gloss_and_shadow_reception() {
        let draw = include_str!("../../../assets/shaders/vegetation_debug_draw.wgsl");
        assert!(draw.contains("shadows::fetch_directional_shadow("));
        assert!(draw.contains("camera.sun_direction.xyz"));
        assert!(draw.contains("let received_shadow = mix("));
        assert!(draw.contains("let shadow_floor = mix(0.16, 0.42, ambient_occlusion);"));
        assert!(draw.contains("lighting as pbr_lighting"));
        assert!(draw.contains("view_bindings::view.exposure"));
        assert!(draw.contains("fn stable_clump_normal("));
        assert!(draw.contains("fn analytic_rounded_normal("));
        assert!(draw.contains("fn ggx_foliage_specular("));
        assert!(draw.contains("let local_broad_specular = ggx_foliage_specular("));
        assert!(draw.contains("let local_sheen_specular = ggx_foliage_specular("));
        assert!(
            draw.contains(
                "let local_specular = local_broad_specular * 0.16 + local_sheen_specular;"
            )
        );
        assert!(draw.contains("let clump_specular = ggx_foliage_specular("));
        assert!(draw.contains("mix(local_specular, clump_specular * 0.34, distance_stability)"));
        assert!(draw.contains("let upper_ribbon = smoothstep("));
        assert!(draw.contains("let far_highlight_weight = mix("));
        assert!(draw.contains("debug_config.values.z == LIGHTING_MODE_LEGACY"));
    }

    #[test]
    fn resident_source_buffers_grow_geometrically() {
        assert_eq!(storage_capacity_for(0), 16);
        assert_eq!(storage_capacity_for(16), 16);
        assert_eq!(storage_capacity_for(17), 32);
        assert_eq!(storage_capacity_for(4_097), 8_192);
    }
}
