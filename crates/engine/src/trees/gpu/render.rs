//! The render world's side: the pool's buffers and uploads, the compute pass that fills the
//! buckets (`shaders/tree/lod.wesl`), and one indirect draw per draw entry in the main,
//! prepass and shadow phases, through the tree and impostor materials' own pipelines.
use super::super::impostor::ImpostorMaterial;
use super::super::material::TreeWindMaterial;
use super::frame::TreeGpuFrame;
use super::layout::{DrawEntry, Drawn, TreeDrawLayout};
use super::pool::PoolData;
use super::{
    DRAWS, GpuBucket, GpuCamera, GpuForm, GpuInstance, MAX_SHADOW_VIEWS, VIEWS, instance_layout,
};
use bevy::{
    core_pipeline::{
        core_3d::{AlphaMask3d, Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey},
        prepass::{
            AlphaMask3dPrepass, Opaque3dPrepass, OpaqueNoLightmap3dBatchSetKey,
            OpaqueNoLightmap3dBinKey,
        },
        schedule::camera_driver,
    },
    ecs::{
        query::ROQueryItem,
        system::{SystemParamItem, lifetimeless::SRes},
    },
    material::{
        AlphaMode, MaterialProperties, RenderPhaseType,
        key::{ErasedMaterialKey, ErasedMaterialPipelineKey, ErasedMeshPipelineKey},
    },
    mesh::Mesh,
    pbr::{
        LightEntity, MATERIAL_BIND_GROUP_INDEX, MeshBindGroups, MeshMorphBindGroupKey,
        MeshPipelineKey, PreparedMaterial, SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup,
        SetPrepassViewBindGroup, SetPrepassViewEmptyBindGroup, Shadow, ShadowBatchSetKey,
        ShadowBinKey, ViewKeyCache, ViewKeyPrepassCache, alpha_mode_pipeline_key,
    },
    prelude::*,
    render::{
        Render, RenderSystems,
        diagnostic::RecordDiagnostics,
        erased_render_asset::ErasedRenderAssets,
        extract_component::ExtractComponent,
        material_bind_groups::MaterialBindGroupAllocators,
        mesh::{
            MeshMetadataFallbackBuffer, RenderMesh, RenderMeshBufferInfo, allocator::MeshAllocator,
        },
        render_asset::RenderAssets,
        render_phase::{
            AddRenderCommand, BinnedPhaseItem, BinnedRenderPhaseType, DrawFunctionId,
            DrawFunctions, InputUniformIndex, PhaseItem, RenderCommand, RenderCommandResult,
            SetItemPipeline, TrackedRenderPass, ViewBinnedRenderPhases,
        },
        render_resource::{
            binding_types::{
                storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer_sized,
            },
            *,
        },
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        sync_world::MainEntity,
        view::{ExtractedView, RetainedViewEntity},
    },
};
use std::{
    any::TypeId,
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, Mutex},
};

const SHADER: &str = "shaders/tree/lod.wesl";
const WORKGROUP: u32 = 64;
/// Bytes of one indexed indirect draw's arguments.
const ARGS_BYTES: u64 = 20;

/// The entity a draw entry's phase items are drawn for; its index is the entry's.
#[derive(Component, ExtractComponent, Clone, Copy, Debug)]
#[extract_app(bevy::render::RenderApp)]
pub(super) struct TreeDraw(usize);

pub(super) fn spawn_draws(mut commands: Commands) {
    commands.spawn_batch((0..DRAWS).map(|i| (TreeDraw(i), Name::new("Tree draw"))));
}

#[derive(Resource)]
struct SharedPool(Arc<Mutex<PoolData>>);

pub(super) fn install(render_app: &mut SubApp, shared: Arc<Mutex<PoolData>>) {
    render_app
        .insert_resource(SharedPool(shared))
        .add_render_command::<Opaque3d, DrawTrees>()
        .add_render_command::<AlphaMask3d, DrawTrees>()
        .add_render_command::<Opaque3dPrepass, DrawTreeDepth>()
        .add_render_command::<AlphaMask3dPrepass, DrawTreeDepth>()
        .add_render_command::<Shadow, DrawTreeDepth>()
        .add_systems(
            Render,
            (
                (queue, queue_shadows).in_set(RenderSystems::Queue),
                prepare.in_set(RenderSystems::PrepareBindGroups),
            ),
        )
        .add_systems(RenderGraph, cull.before(camera_driver));
}

pub(super) fn initialize(world: &mut World) {
    world.init_resource::<TreeGpuPipelines>();
    world.init_resource::<TreeGpu>();
}

#[derive(Resource)]
struct TreeGpuPipelines {
    compute_layout: BindGroupLayoutDescriptor,
    lod: CachedComputePipelineId,
    finalize: CachedComputePipelineId,
}

impl FromWorld for TreeGpuPipelines {
    fn from_world(world: &mut World) -> Self {
        let shader = world.resource::<AssetServer>().load(SHADER);
        let compute_layout = BindGroupLayoutDescriptor::new(
            "tree LOD",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                ),
            ),
        );
        let cache = world.resource::<PipelineCache>();
        let pipeline = |label: &'static str, entry: &'static str| {
            cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some(label.into()),
                layout: vec![compute_layout.clone()],
                shader: shader.clone(),
                entry_point: Some(Cow::Borrowed(entry)),
                ..default()
            })
        };
        Self {
            lod: pipeline("tree LOD", "lod"),
            finalize: pipeline("tree LOD draws", "finalize"),
            compute_layout,
        }
    }
}

/// A storage buffer that grows by doubling.
#[derive(Default)]
struct Growing {
    buffer: Option<Buffer>,
    bytes: u64,
}

impl Growing {
    /// Makes room for `bytes`; true when the buffer is new (its contents lost).
    fn reserve(
        &mut self,
        device: &RenderDevice,
        label: &'static str,
        usage: BufferUsages,
        bytes: u64,
    ) -> bool {
        let bytes = bytes.max(256);
        if self.buffer.is_some() && self.bytes >= bytes {
            return false;
        }
        let size = bytes.next_power_of_two();
        self.buffer = Some(device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.bytes = size;
        true
    }

    fn get(&self) -> &Buffer {
        self.buffer.as_ref().expect("reserved before use")
    }
}

#[derive(Resource, Default)]
pub(super) struct TreeGpu {
    instances: Growing,
    states: Growing,
    transforms: Growing,
    origins: Growing,
    forms: Growing,
    buckets: Growing,
    counters: Growing,
    records: Growing,
    args: Growing,
    camera: Growing,
    /// The layout the buffers hold, and the draws' fixed arguments written for it.
    layout: Option<Arc<super::layout::Layout>>,
    args_written: Vec<[u32; 5]>,
    compute_group: Option<BindGroup>,
    draw_group: Option<BindGroup>,
    /// Instances the compute pass visits, and draw entries per view.
    instances_len: u32,
    entries_len: u32,
    /// Shadow views by their retained entity, numbered as the compute pass numbers them.
    shadow_views: HashMap<RetainedViewEntity, usize>,
    /// The compute pass ran with this frame's layout: draws may read the arguments.
    active: bool,
    /// Bucket counts copied back now and then for the `TREE_GPU_COUNTS` debug line.
    readback: Readback,
}

/// Frames between bucket count readbacks.
const READBACK_FRAMES: u32 = 600;

#[derive(Default)]
struct Readback {
    buffer: Option<Buffer>,
    countdown: u32,
    /// Copied this frame, mapping, or mapped (read in `prepare`).
    copied: bool,
    mapped: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl TreeGpu {
    /// The view a phase draws for: a shadow view's own number, else the main view's.
    fn view_index(&self, view: &RetainedViewEntity) -> usize {
        self.shadow_views.get(view).map_or(0, |i| 1 + i)
    }
}

/// The fixed arguments of one entry's indexed draw (index count, first index, base vertex),
/// once its mesh is allocated.
fn mesh_args(
    mesh: AssetId<Mesh>,
    meshes: &RenderAssets<RenderMesh>,
    allocator: &MeshAllocator,
) -> Option<(u32, u32, i32)> {
    let render = meshes.get(mesh)?;
    let RenderMeshBufferInfo::Indexed { count, .. } = render.buffer_info else {
        return None;
    };
    let indices = allocator.mesh_index_slice(&mesh)?;
    let vertices = allocator.mesh_vertex_slice(&mesh)?;
    Some((count, indices.range.start, vertices.range.start as i32))
}

fn entry_mesh(entry: &DrawEntry) -> AssetId<Mesh> {
    match entry.drawn {
        Drawn::Mesh { mesh, .. } | Drawn::Impostor { mesh, .. } => mesh,
    }
}

/// Logs, per view, the instances the last readback's draws held: mesh primitives (as many as
/// entity draws would be), impostors, and each shadow view's mesh primitives.
fn log_counts(gpu: &mut TreeGpu, layout: &super::layout::Layout) {
    let Some(buffer) = gpu.readback.buffer.as_ref() else {
        return;
    };
    if let Some(mapped) = &gpu.readback.mapped {
        if !mapped.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let counts: Vec<u32> = {
            let view = buffer.slice(..).get_mapped_range().expect("mapped");
            bytemuck::cast_slice(&view).to_vec()
        };
        buffer.unmap();
        gpu.readback.mapped = None;
        let buckets = layout.buckets.len();
        let per_view = |view: usize, impostors: bool| -> u32 {
            layout
                .entries
                .iter()
                .zip(&layout.buckets)
                .enumerate()
                .filter(|(_, (e, _))| matches!(e.drawn, Drawn::Impostor { .. }) == impostors)
                .map(|(i, (_, bucket))| {
                    counts
                        .get(view * buckets + i)
                        .copied()
                        .unwrap_or(0)
                        .min(bucket.capacity)
                })
                .sum()
        };
        let cascades: Vec<u32> = (0..gpu.shadow_views.len())
            .map(|v| per_view(1 + v, false))
            .collect();
        debug!(
            "TREE_GPU_COUNTS meshes={} impostors={} cascades={cascades:?}",
            per_view(0, false),
            per_view(0, true),
        );
    } else if gpu.readback.copied {
        gpu.readback.copied = false;
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = done.clone();
        buffer.slice(..).map_async(MapMode::Read, move |result| {
            flag.store(result.is_ok(), std::sync::atomic::Ordering::Release);
        });
        gpu.readback.mapped = Some(done);
    }
}

#[allow(clippy::too_many_arguments)] // The pool, layout, frame and GPU resources it writes.
fn prepare(
    frame: Res<TreeGpuFrame>,
    layout: Res<TreeDrawLayout>,
    shared: Res<SharedPool>,
    mut gpu: ResMut<TreeGpu>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    pipelines: Res<TreeGpuPipelines>,
    cache: Res<PipelineCache>,
    (meshes, allocator): (Res<RenderAssets<RenderMesh>>, Res<MeshAllocator>),
) {
    let gpu = gpu.as_mut();
    let mut regroup = false;

    // The pool: a new buffer takes everything, and every fade restarts.
    {
        let mut data = shared.0.lock().unwrap();
        let bytes = data.instances.len() as u64 * std::mem::size_of::<GpuInstance>() as u64;
        let grown = gpu
            .instances
            .reserve(&device, "tree pool", BufferUsages::STORAGE, bytes);
        let states_grown = gpu.states.reserve(
            &device,
            "tree fade states",
            BufferUsages::STORAGE,
            data.instances.len() as u64 * 4,
        );
        regroup |= gpu.transforms.reserve(
            &device,
            "tree transforms",
            BufferUsages::STORAGE,
            data.instances.len() as u64 * 48,
        );
        let origin_bytes = data.origins.len() as u64 * 16;
        let origins_grown =
            gpu.origins
                .reserve(&device, "tree origins", BufferUsages::STORAGE, origin_bytes);
        regroup |= grown || states_grown || origins_grown;
        let uploads = data.take_uploads(grown || states_grown || origins_grown);
        drop(data);
        for (start, instances) in &uploads.instances {
            queue.write_buffer(
                gpu.instances.get(),
                u64::from(*start) * std::mem::size_of::<GpuInstance>() as u64,
                bytemuck::cast_slice(instances),
            );
        }
        for range in &uploads.restart {
            let zeros = vec![0u32; range.len()];
            queue.write_buffer(
                gpu.states.get(),
                u64::from(range.start) * 4,
                bytemuck::cast_slice(&zeros),
            );
        }
        if let Some(origins) = &uploads.origins {
            queue.write_buffer(gpu.origins.get(), 0, bytemuck::cast_slice(origins));
        }
        gpu.instances_len = uploads.len;
    }

    // The layout: forms, entries and room for every view's records and counters.
    let next = layout.layout.clone();
    if gpu.layout.as_ref().is_none_or(|l| !Arc::ptr_eq(l, &next)) {
        let forms: Vec<GpuForm> = next.forms.clone();
        regroup |= gpu.forms.reserve(
            &device,
            "tree forms",
            BufferUsages::STORAGE,
            std::mem::size_of_val(forms.as_slice()) as u64,
        );
        if !forms.is_empty() {
            queue.write_buffer(gpu.forms.get(), 0, bytemuck::cast_slice(&forms));
        }
        let buckets: &[GpuBucket] = &next.buckets;
        regroup |= gpu.buckets.reserve(
            &device,
            "tree buckets",
            BufferUsages::STORAGE,
            std::mem::size_of_val(buckets) as u64,
        );
        if !buckets.is_empty() {
            queue.write_buffer(gpu.buckets.get(), 0, bytemuck::cast_slice(buckets));
        }
        let views = VIEWS as u64;
        regroup |= gpu.counters.reserve(
            &device,
            "tree bucket counters",
            BufferUsages::STORAGE | BufferUsages::COPY_SRC,
            views * next.buckets.len() as u64 * 4,
        );
        regroup |= gpu.records.reserve(
            &device,
            "tree records",
            BufferUsages::STORAGE,
            views * u64::from(next.records_per_view) * 4,
        );
        regroup |= gpu.args.reserve(
            &device,
            "tree draws",
            BufferUsages::STORAGE | BufferUsages::INDIRECT,
            views * next.entries.len() as u64 * ARGS_BYTES,
        );
        gpu.entries_len = next.entries.len() as u32;
        gpu.layout = Some(next.clone());
        gpu.args_written.clear();
        if gpu.readback.mapped.is_none() {
            gpu.readback.buffer = Some(device.create_buffer(&BufferDescriptor {
                label: Some("tree bucket counts readback"),
                size: gpu.counters.bytes,
                usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            gpu.readback.copied = false;
        }
    }
    if bevy::log::tracing::enabled!(bevy::log::Level::DEBUG) {
        log_counts(gpu, &next);
    }

    // The draws' fixed arguments, rewritten whenever a mesh moves or arrives.
    let mut args = Vec::with_capacity(VIEWS * next.entries.len());
    for view in 0..VIEWS as u32 {
        for (entry, bucket) in next.entries.iter().zip(&next.buckets) {
            let (count, first, base) =
                mesh_args(entry_mesh(entry), &meshes, &allocator).unwrap_or((0, 0, 0));
            args.push([
                count,
                0,
                first,
                base as u32,
                view * next.records_per_view + bucket.records,
            ]);
        }
    }
    if args != gpu.args_written && !args.is_empty() {
        queue.write_buffer(gpu.args.get(), 0, bytemuck::cast_slice(&args));
        gpu.args_written = args;
    }

    // This frame's view.
    gpu.camera.reserve(
        &device,
        "tree LOD camera",
        BufferUsages::UNIFORM,
        std::mem::size_of::<GpuCamera>() as u64,
    );
    let mut camera = GpuCamera {
        eye_dt: [0.0; 4],
        planes: [[0.0; 4]; 6],
        shadow_planes: [[0.0; 4]; 6 * MAX_SHADOW_VIEWS],
        counts: [0, 0, frame.shadow_lod, next.records_per_view],
        sizes: [next.buckets.len() as u32, 0, u32::from(frame.hidden), 0],
    };
    gpu.shadow_views.clear();
    if let Some(view) = &frame.view {
        camera.eye_dt = view.eye.extend(frame.dt).to_array();
        camera.planes = view.planes.map(|p| p.to_array());
        for (i, (retained, planes)) in view.cascades.iter().enumerate() {
            for (p, plane) in planes.iter().enumerate() {
                camera.shadow_planes[i * 6 + p] = plane.to_array();
            }
            gpu.shadow_views.insert(*retained, i);
        }
        camera.counts[0] = gpu.instances_len;
        camera.counts[1] = view.cascades.len() as u32;
    }
    queue.write_buffer(gpu.camera.get(), 0, bytemuck::bytes_of(&camera));

    if regroup || gpu.compute_group.is_none() {
        let layout = cache.get_bind_group_layout(&pipelines.compute_layout);
        gpu.compute_group = Some(device.create_bind_group(
            "tree LOD",
            &layout,
            &BindGroupEntries::sequential((
                gpu.camera.get().as_entire_binding(),
                gpu.instances.get().as_entire_binding(),
                gpu.origins.get().as_entire_binding(),
                gpu.forms.get().as_entire_binding(),
                gpu.states.get().as_entire_binding(),
                gpu.counters.get().as_entire_binding(),
                gpu.records.get().as_entire_binding(),
                gpu.buckets.get().as_entire_binding(),
                gpu.args.get().as_entire_binding(),
                gpu.transforms.get().as_entire_binding(),
            )),
        ));
        gpu.draw_group = Some(device.create_bind_group(
            "tree instances",
            &cache.get_bind_group_layout(&instance_layout()),
            &BindGroupEntries::sequential((
                gpu.records.get().as_entire_binding(),
                gpu.instances.get().as_entire_binding(),
                gpu.origins.get().as_entire_binding(),
                gpu.transforms.get().as_entire_binding(),
            )),
        ));
    }
    gpu.active = frame.view.is_some()
        && gpu.entries_len > 0
        && cache.get_compute_pipeline(pipelines.lod).is_some()
        && cache.get_compute_pipeline(pipelines.finalize).is_some();
}

/// Fills every view's buckets and the draws' instance counts.
fn cull(
    mut render_context: RenderContext,
    mut gpu: ResMut<TreeGpu>,
    pipelines: Res<TreeGpuPipelines>,
    cache: Res<PipelineCache>,
) {
    if !gpu.active {
        return;
    }
    let (Some(group), Some(lod), Some(finalize)) = (
        gpu.compute_group.as_ref(),
        cache.get_compute_pipeline(pipelines.lod),
        cache.get_compute_pipeline(pipelines.finalize),
    ) else {
        return;
    };
    render_context
        .command_encoder()
        .clear_buffer(gpu.counters.get(), 0, None);
    let diagnostics = render_context.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let mut pass = render_context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("tree LOD"),
            ..default()
        });
    let span = diagnostics.pass_span(&mut pass, "tree_lod");
    pass.set_bind_group(0, group, &[]);
    pass.set_pipeline(lod);
    pass.dispatch_workgroups(gpu.instances_len.div_ceil(WORKGROUP).max(1), 1, 1);
    pass.set_pipeline(finalize);
    pass.dispatch_workgroups((gpu.entries_len * VIEWS as u32).div_ceil(WORKGROUP), 1, 1);
    span.end(&mut pass);
    drop(pass);
    // Now and then, the counts go back for the debug line (`log_counts`).
    let gpu = gpu.as_mut();
    gpu.readback.countdown = gpu.readback.countdown.saturating_sub(1);
    if gpu.readback.countdown == 0
        && bevy::log::tracing::enabled!(bevy::log::Level::DEBUG)
        && gpu.readback.mapped.is_none()
        && !gpu.readback.copied
        && let Some(readback) = &gpu.readback.buffer
        && readback.size() == gpu.counters.bytes
    {
        gpu.readback.countdown = READBACK_FRAMES;
        gpu.readback.copied = true;
        render_context.command_encoder().copy_buffer_to_buffer(
            gpu.counters.get(),
            0,
            readback,
            0,
            gpu.counters.bytes,
        );
    }
}

/// The draw entities in the render world, by entry index, as far as `count` entries.
fn draw_entities(world: &mut World, count: usize) -> Vec<Option<(Entity, MainEntity)>> {
    let mut draws = world.query::<(Entity, &MainEntity, &TreeDraw)>();
    let mut entities = vec![None; count.min(DRAWS)];
    for (entity, main, draw) in draws.iter(world) {
        if let Some(slot) = entities.get_mut(draw.0) {
            *slot = Some((entity, *main));
        }
    }
    entities
}

/// Which of a main view's passes a pipeline draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Pass {
    Main,
    /// The depth and motion prepass, which MetalFX Temporal reads.
    Prepass,
}

struct Functions {
    opaque: DrawFunctionId,
    mask: DrawFunctionId,
    opaque_prepass: DrawFunctionId,
    mask_prepass: DrawFunctionId,
}

/// Specializes each entry's instanced pipelines for every main view (and its prepass, if it
/// has one) and queues one item per entry into each phase. Impostors draw in the main pass
/// only.
fn queue(
    world: &mut World,
    mut pipelines: Local<HashMap<PipelineFor, CachedRenderPipelineId>>,
    mut queued: Local<usize>,
) {
    let layout = world.resource::<TreeDrawLayout>().layout.clone();
    let mut views = world.query::<(&ExtractedView, &Msaa)>();
    let views: Vec<(RetainedViewEntity, Msaa)> = views
        .iter(world)
        .map(|(view, msaa)| (view.retained_view_entity, *msaa))
        .collect();
    // Items of every entry queued last frame go, whatever this frame's layout holds.
    let draw_entities = draw_entities(world, layout.entries.len().max(*queued));
    *queued = layout.entries.len();
    let frame = world.resource::<TreeGpuFrame>();
    let drawn = frame
        .view
        .as_ref()
        .filter(|_| !frame.hidden)
        .map(|view| view.drawn.clone())
        .unwrap_or_default();
    let functions = Functions {
        opaque: function::<Opaque3d, DrawTrees>(world),
        mask: function::<AlphaMask3d, DrawTrees>(world),
        opaque_prepass: function::<Opaque3dPrepass, DrawTreeDepth>(world),
        mask_prepass: function::<AlphaMask3dPrepass, DrawTreeDepth>(world),
    };
    for (view, msaa) in views {
        let Some(view_key) = world.resource::<ViewKeyCache>().get(&view).copied() else {
            continue;
        };
        if !world
            .resource::<ViewBinnedRenderPhases<Opaque3d>>()
            .contains_key(&view)
        {
            continue;
        }
        let prepass_key = world
            .resource::<ViewKeyPrepassCache>()
            .get(&view)
            .copied()
            .filter(|_| {
                world
                    .resource::<ViewBinnedRenderPhases<Opaque3dPrepass>>()
                    .contains_key(&view)
            });
        // Last frame's items go; this frame's entries come back below.
        for (_, main) in draw_entities.iter().flatten() {
            remove::<Opaque3d>(world, &view, *main);
            remove::<AlphaMask3d>(world, &view, *main);
            remove::<Opaque3dPrepass>(world, &view, *main);
            remove::<AlphaMask3dPrepass>(world, &view, *main);
        }
        for (index, entry) in layout.entries.iter().enumerate().take(DRAWS) {
            let Some(item) = draw_entities[index] else {
                continue;
            };
            if !has(&drawn, entry) {
                continue;
            }
            for (pass, key) in [(Pass::Main, Some(view_key)), (Pass::Prepass, prepass_key)] {
                let Some(key) = key else {
                    continue;
                };
                if pass == Pass::Prepass && matches!(entry.drawn, Drawn::Impostor { .. }) {
                    continue;
                }
                let Some((pipeline, phase)) =
                    specialize(world, &mut pipelines, view, pass, key, msaa, entry)
                else {
                    continue;
                };
                let mesh = entry_mesh(entry).untyped();
                match (pass, phase) {
                    (Pass::Main, RenderPhaseType::Opaque) => {
                        if let Some(phase) = world
                            .resource_mut::<ViewBinnedRenderPhases<Opaque3d>>()
                            .get_mut(&view)
                        {
                            phase.add(
                                Opaque3dBatchSetKey {
                                    draw_function: functions.opaque,
                                    pipeline,
                                    material_bind_group_index: None,
                                    lightmap_slab: None,
                                    slabs: default(),
                                },
                                Opaque3dBinKey { asset_id: mesh },
                                item,
                                InputUniformIndex::default(),
                                BinnedRenderPhaseType::NonMesh,
                            );
                        }
                    }
                    (Pass::Main, RenderPhaseType::AlphaMask) => {
                        add::<AlphaMask3d>(world, &view, functions.mask, pipeline, mesh, item);
                    }
                    (Pass::Prepass, RenderPhaseType::Opaque) => {
                        add::<Opaque3dPrepass>(
                            world,
                            &view,
                            functions.opaque_prepass,
                            pipeline,
                            mesh,
                            item,
                        );
                    }
                    (Pass::Prepass, RenderPhaseType::AlphaMask) => {
                        add::<AlphaMask3dPrepass>(
                            world,
                            &view,
                            functions.mask_prepass,
                            pipeline,
                            mesh,
                            item,
                        );
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Whether some batch could draw an entry's representation, by the frame's bits per form.
fn has(representations: &[u8], entry: &DrawEntry) -> bool {
    representations
        .get(entry.form as usize)
        .is_some_and(|bits| bits & (1 << entry.representation) != 0)
}

/// The id of draw function `C` in phase `P`.
fn function<P: PhaseItem, C: 'static>(world: &World) -> DrawFunctionId {
    world.resource::<DrawFunctions<P>>().read().id::<C>()
}

fn remove<P: BinnedPhaseItem>(world: &mut World, view: &RetainedViewEntity, entity: MainEntity) {
    if let Some(phase) = world
        .resource_mut::<ViewBinnedRenderPhases<P>>()
        .get_mut(view)
    {
        phase.remove(entity);
    }
}

/// Queues an entry into a phase binned like the alpha-masked and prepass phases.
fn add<P>(
    world: &mut World,
    view: &RetainedViewEntity,
    draw_function: DrawFunctionId,
    pipeline: CachedRenderPipelineId,
    mesh: bevy::asset::UntypedAssetId,
    item: (Entity, MainEntity),
) where
    P: BinnedPhaseItem<
            BatchSetKey = OpaqueNoLightmap3dBatchSetKey,
            BinKey = OpaqueNoLightmap3dBinKey,
        >,
{
    if let Some(phase) = world
        .resource_mut::<ViewBinnedRenderPhases<P>>()
        .get_mut(view)
    {
        phase.add(
            OpaqueNoLightmap3dBatchSetKey {
                draw_function,
                pipeline,
                material_bind_group_index: None,
                slabs: default(),
            },
            OpaqueNoLightmap3dBinKey { asset_id: mesh },
            item,
            InputUniformIndex::default(),
            BinnedRenderPhaseType::NonMesh,
        );
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct PipelineFor {
    view: RetainedViewEntity,
    pass: Pass,
    view_key: u64,
    mesh: AssetId<Mesh>,
    material: bevy::asset::UntypedAssetId,
}

/// A material's key for its instanced pipeline.
fn instanced_key(
    drawn: &Drawn,
    properties: &MaterialProperties,
    mesh_key: MeshPipelineKey,
) -> ErasedMaterialPipelineKey {
    match drawn {
        Drawn::Mesh { .. } => {
            let mut data: <TreeWindMaterial as AsBindGroup>::Data =
                properties.material_key.to_key();
            data.extension.instanced = true;
            ErasedMaterialPipelineKey {
                type_id: TypeId::of::<TreeWindMaterial>(),
                mesh_key: ErasedMeshPipelineKey::new(mesh_key),
                material_key: ErasedMaterialKey::new(data),
            }
        }
        Drawn::Impostor { .. } => {
            let mut data: <ImpostorMaterial as AsBindGroup>::Data =
                properties.material_key.to_key();
            data.extension.instanced = true;
            ErasedMaterialPipelineKey {
                type_id: TypeId::of::<ImpostorMaterial>(),
                mesh_key: ErasedMeshPipelineKey::new(mesh_key),
                material_key: ErasedMaterialKey::new(data),
            }
        }
    }
}

fn entry_material(entry: &DrawEntry) -> bevy::asset::UntypedAssetId {
    match entry.drawn {
        Drawn::Mesh { material, .. } => material.untyped(),
        Drawn::Impostor { material, .. } => material.untyped(),
    }
}

fn specialize(
    world: &mut World,
    pipelines: &mut HashMap<PipelineFor, CachedRenderPipelineId>,
    view: RetainedViewEntity,
    pass: Pass,
    view_key: MeshPipelineKey,
    msaa: Msaa,
    entry: &DrawEntry,
) -> Option<(CachedRenderPipelineId, RenderPhaseType)> {
    let material = world
        .resource::<ErasedRenderAssets<PreparedMaterial>>()
        .get(entry_material(entry))?;
    let properties = material.properties.clone();
    let phase = properties.render_phase_type;
    let at = PipelineFor {
        view,
        pass,
        view_key: view_key.bits(),
        mesh: entry_mesh(entry),
        material: entry_material(entry),
    };
    if let Some(&pipeline) = pipelines.get(&at) {
        return Some((pipeline, phase));
    }
    let mesh = world
        .resource::<RenderAssets<RenderMesh>>()
        .get(entry_mesh(entry))?;
    let layout = mesh.layout.clone();
    let mut mesh_key = view_key
        | MeshPipelineKey::from_bits_retain(mesh.key_bits.bits())
        | alpha_mode_pipeline_key(properties.alpha_mode, &msaa);
    // Tree meshes fade by their record's dither level: Bevy's main and prepass pipelines
    // dither them as visibility ranges do, without the shadows' overlap. Impostors dither in
    // their own shader.
    if matches!(entry.drawn, Drawn::Mesh { .. }) {
        mesh_key |= MeshPipelineKey::VISIBILITY_RANGE_DITHER;
    }
    let specialized = match pass {
        Pass::Main => {
            mesh_key |= properties.mesh_pipeline_key_bits.downcast();
            let key = instanced_key(&entry.drawn, &properties, mesh_key);
            (properties.base_specialize?)(world, key, &layout, &properties)
        }
        Pass::Prepass => {
            if !matches!(phase, RenderPhaseType::Opaque | RenderPhaseType::AlphaMask) {
                return None;
            }
            // The tree material has its own prepass shaders (`specialize_shadow`).
            mesh_key |= MeshPipelineKey::PREPASS_READS_MATERIAL;
            let key = instanced_key(&entry.drawn, &properties, mesh_key);
            (properties.prepass_specialize?)(world, &key, &layout, &properties)
        }
    };
    match specialized {
        Ok(pipeline) => {
            pipelines.insert(at, pipeline);
            Some((pipeline, phase))
        }
        Err(error) => {
            warn_once!("GPU trees ({pass:?}): {error}");
            None
        }
    }
}

/// Queues every mesh entry into each shadow view's phase, through the tree material's
/// shadow pipeline (as Bevy's `specialize_shadows` keys it for a directional light).
fn queue_shadows(
    world: &mut World,
    mut pipelines: Local<
        HashMap<(AssetId<Mesh>, bevy::asset::UntypedAssetId), CachedRenderPipelineId>,
    >,
    mut queued: Local<usize>,
) {
    let layout = world.resource::<TreeDrawLayout>().layout.clone();
    let frame = world.resource::<TreeGpuFrame>();
    let (shadow_views, cast): (Vec<RetainedViewEntity>, Arc<Vec<Vec<u8>>>) = frame
        .view
        .as_ref()
        .filter(|_| !frame.hidden)
        .map(|view| {
            (
                view.cascades
                    .iter()
                    .map(|(retained, _)| *retained)
                    .collect(),
                view.cast.clone(),
            )
        })
        .unwrap_or_default();
    let mut views = world.query::<(&ExtractedView, &LightEntity)>();
    let views: Vec<RetainedViewEntity> = views
        .iter(world)
        .filter(|(_, light)| matches!(light, LightEntity::Directional { .. }))
        .map(|(view, _)| view.retained_view_entity)
        .collect();
    let draw_entities = draw_entities(world, layout.entries.len().max(*queued));
    *queued = layout.entries.len();
    let draw_function = function::<Shadow, DrawTreeDepth>(world);
    for view in views {
        let Some(phase) = world
            .resource_mut::<ViewBinnedRenderPhases<Shadow>>()
            .into_inner()
            .get_mut(&view)
        else {
            continue;
        };
        for (_, main) in draw_entities.iter().flatten() {
            phase.remove(*main);
        }
        let Some(shadow) = shadow_views.iter().position(|v| *v == view) else {
            continue;
        };
        for (index, entry) in layout.entries.iter().enumerate().take(DRAWS) {
            let Drawn::Mesh { mesh, material } = entry.drawn else {
                continue;
            };
            if !has(&cast[shadow], entry) {
                continue;
            }
            let Some((entity, main)) = draw_entities[index] else {
                continue;
            };
            let Some(pipeline) = specialize_shadow(world, &mut pipelines, mesh, material) else {
                continue;
            };
            if let Some(phase) = world
                .resource_mut::<ViewBinnedRenderPhases<Shadow>>()
                .get_mut(&view)
            {
                phase.add(
                    ShadowBatchSetKey {
                        pipeline,
                        draw_function,
                        material_bind_group_index: None,
                        slabs: default(),
                    },
                    ShadowBinKey {
                        asset_id: mesh.untyped(),
                    },
                    (entity, main),
                    InputUniformIndex::default(),
                    BinnedRenderPhaseType::NonMesh,
                );
            }
        }
    }
}

fn specialize_shadow(
    world: &mut World,
    pipelines: &mut HashMap<(AssetId<Mesh>, bevy::asset::UntypedAssetId), CachedRenderPipelineId>,
    mesh_id: AssetId<Mesh>,
    material_id: AssetId<TreeWindMaterial>,
) -> Option<CachedRenderPipelineId> {
    if let Some(&pipeline) = pipelines.get(&(mesh_id, material_id.untyped())) {
        return Some(pipeline);
    }
    let material = world
        .resource::<ErasedRenderAssets<PreparedMaterial>>()
        .get(material_id.untyped())?;
    let properties = material.properties.clone();
    let mesh = world.resource::<RenderAssets<RenderMesh>>().get(mesh_id)?;
    let layout = mesh.layout.clone();
    // A directional light's key (`check_views_lights_need_specialization`); the tree material
    // has its own prepass shaders, so its shadow pipeline always reads the material.
    let mut mesh_key = MeshPipelineKey::DEPTH_PREPASS
        | MeshPipelineKey::VIEW_PROJECTION_ORTHOGRAPHIC
        | MeshPipelineKey::UNCLIPPED_DEPTH_ORTHO
        | MeshPipelineKey::PREPASS_READS_MATERIAL
        | MeshPipelineKey::from_bits_retain(mesh.key_bits.bits());
    if !matches!(properties.alpha_mode, AlphaMode::Opaque) {
        mesh_key |= MeshPipelineKey::MAY_DISCARD;
    }
    let drawn = Drawn::Mesh {
        mesh: mesh_id,
        material: material_id,
    };
    let key = instanced_key(&drawn, &properties, mesh_key);
    let prepass_specialize = properties.prepass_specialize?;
    match prepass_specialize(world, &key, &layout, &properties) {
        Ok(pipeline) => {
            pipelines.insert((mesh_id, material_id.untyped()), pipeline);
            Some(pipeline)
        }
        Err(error) => {
            warn_once!("GPU trees shadows: {error}");
            None
        }
    }
}

type DrawTrees = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    DrawTreeEntry,
);

/// Shadow cascades and the depth and motion prepass.
type DrawTreeDepth = (
    SetItemPipeline,
    SetPrepassViewBindGroup<0>,
    SetPrepassViewEmptyBindGroup<1>,
    DrawTreeEntry,
);

struct DrawTreeEntry;

impl<P: PhaseItem> RenderCommand<P> for DrawTreeEntry {
    type Param = (
        SRes<TreeGpu>,
        SRes<TreeDrawLayout>,
        SRes<ErasedRenderAssets<PreparedMaterial>>,
        SRes<MaterialBindGroupAllocators>,
        SRes<MeshAllocator>,
        SRes<MeshBindGroups>,
        SRes<MeshMetadataFallbackBuffer>,
        SRes<RenderAssets<RenderMesh>>,
    );
    type ViewQuery = &'static ExtractedView;
    type ItemQuery = &'static TreeDraw;

    fn render<'w>(
        _item: &P,
        view: ROQueryItem<'w, '_, Self::ViewQuery>,
        draw: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        (
            gpu,
            layout,
            materials,
            allocators,
            mesh_allocator,
            mesh_bind_groups,
            metadata_fallback,
            meshes,
        ): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let gpu = gpu.into_inner();
        let (Some(draw), Some(instances), true) = (draw, gpu.draw_group.as_ref(), gpu.active)
        else {
            return RenderCommandResult::Skip;
        };
        let layout = &layout.into_inner().layout;
        // The arguments were written for the layout the buffers hold.
        if gpu.layout.as_ref().is_none_or(|l| !Arc::ptr_eq(l, layout)) {
            return RenderCommandResult::Skip;
        }
        let Some(entry) = layout.entries.get(draw.0) else {
            return RenderCommandResult::Skip;
        };
        let (mesh, material, material_type) = match entry.drawn {
            Drawn::Mesh { mesh, material } => {
                (mesh, material.untyped(), TypeId::of::<TreeWindMaterial>())
            }
            Drawn::Impostor { mesh, material } => {
                (mesh, material.untyped(), TypeId::of::<ImpostorMaterial>())
            }
        };
        let Some(material) = materials.into_inner().get(material) else {
            return RenderCommandResult::Skip;
        };
        let Some(material_group) = allocators
            .into_inner()
            .get(&material_type)
            .and_then(|allocator| allocator.get(material.binding.group))
            .and_then(|slab| slab.bind_group())
        else {
            return RenderCommandResult::Skip;
        };
        // The instanced shaders read nothing from the mesh uniforms, but their layout has them;
        // any phase's bind group fits when this phase has none of its own.
        let phase_groups = match mesh_bind_groups.into_inner() {
            MeshBindGroups::CpuPreprocessing(groups) => Some(groups),
            MeshBindGroups::GpuPreprocessing(groups) => groups
                .get(&TypeId::of::<P>())
                .or_else(|| groups.values().next()),
        };
        // Since Bevy 0.20 mesh bind groups are per metadata slab, as in `SetMeshBindGroup`.
        let mesh_allocator = mesh_allocator.into_inner();
        let metadata_slab = mesh_allocator
            .mesh_slabs(&mesh)
            .and_then(|slabs| slabs.metadata_slab_id)
            .unwrap_or(metadata_fallback.slab_id);
        let Some(mesh_group) = phase_groups.and_then(|groups| {
            groups.get(
                metadata_slab,
                None,
                false,
                MeshMorphBindGroupKey::NoMorphTargets,
                false,
            )
        }) else {
            return RenderCommandResult::Skip;
        };
        let (Some(vertices), Some(indices)) = (
            mesh_allocator.mesh_vertex_slice(&mesh),
            mesh_allocator.mesh_index_slice(&mesh),
        ) else {
            return RenderCommandResult::Skip;
        };
        // Its arguments name no indices until its mesh is allocated.
        let at = gpu.view_index(&view.retained_view_entity) * layout.entries.len() + draw.0;
        if gpu.args_written.get(at).is_none_or(|args| args[0] == 0) {
            return RenderCommandResult::Skip;
        }
        let Some(RenderMeshBufferInfo::Indexed { index_format, .. }) =
            meshes.into_inner().get(mesh).map(|m| &m.buffer_info)
        else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(2, mesh_group, &[]);
        pass.set_bind_group(MATERIAL_BIND_GROUP_INDEX, material_group, &[]);
        pass.set_bind_group(4, instances, &[]);
        pass.set_vertex_buffer(0, vertices.buffer.slice(..));
        pass.set_index_buffer(indices.buffer.slice(..), *index_format);
        pass.draw_indexed_indirect(gpu.args.get(), at as u64 * ARGS_BYTES);
        RenderCommandResult::Success
    }
}
