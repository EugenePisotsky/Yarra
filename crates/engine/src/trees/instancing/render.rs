//! Drawing the gathered instances: one instanced draw per group through the tree material's
//! pipelines, in the main, prepass and shadow phases.
use super::{DRAWS, Form, Group, TreeInstanceDraw, TreeInstanceFrame, instance_layout};
use crate::trees::material::TreeWindMaterial;
use bevy::{
    core_pipeline::{
        core_3d::{AlphaMask3d, Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey},
        prepass::{
            AlphaMask3dPrepass, Opaque3dPrepass, OpaqueNoLightmap3dBatchSetKey,
            OpaqueNoLightmap3dBinKey,
        },
    },
    ecs::{
        query::ROQueryItem,
        system::{SystemParamItem, lifetimeless::SRes},
    },
    material::{
        AlphaMode, RenderPhaseType,
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
        erased_render_asset::ErasedRenderAssets,
        material_bind_groups::MaterialBindGroupAllocators,
        mesh::{
            MeshMetadataFallbackBuffer, RenderMesh, RenderMeshBufferInfo, allocator::MeshAllocator,
        },
        render_asset::RenderAssets,
        render_phase::{
            BinnedPhaseItem, BinnedRenderPhaseType, DrawFunctionId, DrawFunctions,
            InputUniformIndex, PhaseItem, RenderCommand, RenderCommandResult, SetItemPipeline,
            TrackedRenderPass, ViewBinnedRenderPhases,
        },
        render_resource::*,
        renderer::{RenderDevice, RenderQueue},
        sync_world::MainEntity,
        view::{ExtractedView, RetainedViewEntity},
    },
};
use std::{any::TypeId, collections::HashMap};

#[derive(Resource, Default)]
pub(super) struct TreeInstanceGpu {
    buffer: Option<Buffer>,
    capacity: u64,
    bind_group: Option<BindGroup>,
    groups: Vec<Group>,
    cascades: HashMap<RetainedViewEntity, Vec<Group>>,
}

impl TreeInstanceGpu {
    /// The groups drawn in a view: a shadow cascade's own, else the main view's.
    fn groups(&self, view: &RetainedViewEntity) -> &[Group] {
        self.cascades.get(view).unwrap_or(&self.groups)
    }
}

pub(super) fn prepare(
    frame: Res<TreeInstanceFrame>,
    mut gpu: ResMut<TreeInstanceGpu>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
) {
    gpu.groups = frame.groups.to_vec();
    gpu.cascades = (*frame.cascades).clone();
    if frame.instances.is_empty() {
        return;
    }
    let bytes: &[u8] = bytemuck::cast_slice(&frame.instances);
    if gpu.buffer.is_none() || gpu.capacity < bytes.len() as u64 {
        let capacity = (bytes.len() as u64).next_power_of_two();
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("tree instances"),
            size: capacity,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.bind_group = Some(device.create_bind_group(
            "tree instances",
            &cache.get_bind_group_layout(&instance_layout()),
            &BindGroupEntries::single(buffer.as_entire_binding()),
        ));
        gpu.buffer = Some(buffer);
        gpu.capacity = capacity;
    }
    if let Some(buffer) = &gpu.buffer {
        queue.write_buffer(buffer, 0, bytes);
    }
}

/// The draw entities in the render world, by group index.
fn draw_entities(world: &mut World) -> Vec<Option<(Entity, MainEntity)>> {
    let mut draws = world.query::<(Entity, &MainEntity, &TreeInstanceDraw)>();
    let mut entities = vec![None; DRAWS];
    for (entity, main, draw) in draws.iter(world) {
        if draw.0 < DRAWS {
            entities[draw.0] = Some((entity, *main));
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

/// Specializes each group's instanced pipelines for every main view (and its prepass, if it
/// has one), as Bevy specializes the tree material for its entities but with
/// `TreeWindKey::instanced`, and queues one item per group into each phase.
pub(super) fn queue(
    world: &mut World,
    mut pipelines: Local<HashMap<PipelineFor, CachedRenderPipelineId>>,
) {
    let groups = world.resource::<TreeInstanceFrame>().groups.clone();
    let mut views = world.query::<(&ExtractedView, &Msaa)>();
    let views: Vec<(RetainedViewEntity, Msaa)> = views
        .iter(world)
        .map(|(view, msaa)| (view.retained_view_entity, *msaa))
        .collect();
    let draw_entities = draw_entities(world);
    let functions = Functions {
        opaque: function::<Opaque3d, DrawTreeInstances>(world),
        mask: function::<AlphaMask3d, DrawTreeInstances>(world),
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
        // Last frame's items go; this frame's groups come back below.
        for (_, main) in draw_entities.iter().flatten() {
            remove::<Opaque3d>(world, &view, *main);
            remove::<AlphaMask3d>(world, &view, *main);
            remove::<Opaque3dPrepass>(world, &view, *main);
            remove::<AlphaMask3dPrepass>(world, &view, *main);
        }
        for (index, group) in groups.iter().enumerate() {
            let Some(item) = draw_entities[index] else {
                continue;
            };
            for (pass, key) in [(Pass::Main, Some(view_key)), (Pass::Prepass, prepass_key)] {
                let Some(key) = key else {
                    continue;
                };
                let Some((pipeline, phase)) =
                    specialize(world, &mut pipelines, view, pass, key, msaa, group)
                else {
                    continue;
                };
                let mesh = group.mesh.untyped();
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

/// Queues a group into a phase binned like the alpha-masked and prepass phases.
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
pub(super) struct PipelineFor {
    view: RetainedViewEntity,
    pass: Pass,
    view_key: u64,
    mesh: AssetId<Mesh>,
    material: AssetId<TreeWindMaterial>,
}

/// The tree material's key for an instanced pipeline.
fn instanced_key(
    properties: &bevy::material::MaterialProperties,
    mesh_key: MeshPipelineKey,
) -> ErasedMaterialPipelineKey {
    let mut data: <TreeWindMaterial as AsBindGroup>::Data = properties.material_key.to_key();
    data.extension.instanced = true;
    ErasedMaterialPipelineKey {
        type_id: TypeId::of::<TreeWindMaterial>(),
        mesh_key: ErasedMeshPipelineKey::new(mesh_key),
        material_key: ErasedMaterialKey::new(data),
    }
}

fn specialize(
    world: &mut World,
    pipelines: &mut HashMap<PipelineFor, CachedRenderPipelineId>,
    view: RetainedViewEntity,
    pass: Pass,
    view_key: MeshPipelineKey,
    msaa: Msaa,
    group: &Group,
) -> Option<(CachedRenderPipelineId, RenderPhaseType)> {
    let material = world
        .resource::<ErasedRenderAssets<PreparedMaterial>>()
        .get(group.material.untyped())?;
    let properties = material.properties.clone();
    let phase = properties.render_phase_type;
    let at = PipelineFor {
        view,
        pass,
        view_key: view_key.bits(),
        mesh: group.mesh,
        material: group.material,
    };
    if let Some(&pipeline) = pipelines.get(&at) {
        return Some((pipeline, phase));
    }
    let mesh = world
        .resource::<RenderAssets<RenderMesh>>()
        .get(group.mesh)?;
    let layout = mesh.layout.clone();
    // Timed fades carry a crossfade range for exactly this: Bevy's main and prepass pipelines
    // dither them by their tag without the shadows' overlap.
    let mut mesh_key = view_key
        | MeshPipelineKey::from_bits_retain(mesh.key_bits.bits())
        | alpha_mode_pipeline_key(properties.alpha_mode, &msaa)
        | MeshPipelineKey::VISIBILITY_RANGE_DITHER;
    let specialized = match pass {
        Pass::Main => {
            mesh_key |= properties.mesh_pipeline_key_bits.downcast();
            let key = instanced_key(&properties, mesh_key);
            (properties.base_specialize?)(world, key, &layout, &properties)
        }
        Pass::Prepass => {
            if !matches!(phase, RenderPhaseType::Opaque | RenderPhaseType::AlphaMask) {
                return None;
            }
            // The tree material has its own prepass shaders (`specialize_shadow`).
            mesh_key |= MeshPipelineKey::PREPASS_READS_MATERIAL;
            let key = instanced_key(&properties, mesh_key);
            (properties.prepass_specialize?)(world, &key, &layout, &properties)
        }
    };
    match specialized {
        Ok(pipeline) => {
            pipelines.insert(at, pipeline);
            Some((pipeline, phase))
        }
        Err(error) => {
            warn_once!("tree instancing ({pass:?}): {error}");
            None
        }
    }
}

/// Queues every shadow cascade's groups into its shadow phase, through the tree material's
/// shadow pipeline (as Bevy's `specialize_shadows` keys it for a directional light) specialized
/// for the instance buffer.
pub(super) fn queue_shadows(
    world: &mut World,
    mut pipelines: Local<HashMap<Form, CachedRenderPipelineId>>,
) {
    let cascades = world.resource::<TreeInstanceFrame>().cascades.clone();
    let mut views = world.query::<(&ExtractedView, &LightEntity)>();
    let views: Vec<RetainedViewEntity> = views
        .iter(world)
        .filter(|(_, light)| matches!(light, LightEntity::Directional { .. }))
        .map(|(view, _)| view.retained_view_entity)
        .collect();
    let draw_entities = draw_entities(world);
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
        let Some(groups) = cascades.get(&view) else {
            continue;
        };
        for (index, group) in groups.iter().enumerate() {
            let Some((entity, main)) = draw_entities[index] else {
                continue;
            };
            let Some(pipeline) = specialize_shadow(world, &mut pipelines, group) else {
                continue;
            };
            if let Some(phase) = world
                .resource_mut::<ViewBinnedRenderPhases<Shadow>>()
                .get_mut(&view)
            {
                phase.add(
                    shadow_batch_key(pipeline, draw_function),
                    ShadowBinKey {
                        asset_id: group.mesh.untyped(),
                    },
                    (entity, main),
                    InputUniformIndex::default(),
                    BinnedRenderPhaseType::NonMesh,
                );
            }
        }
    }
}

fn shadow_batch_key(
    pipeline: CachedRenderPipelineId,
    function: DrawFunctionId,
) -> ShadowBatchSetKey {
    ShadowBatchSetKey {
        pipeline,
        draw_function: function,
        material_bind_group_index: None,
        slabs: default(),
    }
}

fn specialize_shadow(
    world: &mut World,
    pipelines: &mut HashMap<Form, CachedRenderPipelineId>,
    group: &Group,
) -> Option<CachedRenderPipelineId> {
    if let Some(&pipeline) = pipelines.get(&(group.mesh, group.material)) {
        return Some(pipeline);
    }
    let material = world
        .resource::<ErasedRenderAssets<PreparedMaterial>>()
        .get(group.material.untyped())?;
    let properties = material.properties.clone();
    let mesh = world
        .resource::<RenderAssets<RenderMesh>>()
        .get(group.mesh)?;
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
    let key = instanced_key(&properties, mesh_key);
    let prepass_specialize = properties.prepass_specialize?;
    match prepass_specialize(world, &key, &layout, &properties) {
        Ok(pipeline) => {
            pipelines.insert((group.mesh, group.material), pipeline);
            Some(pipeline)
        }
        Err(error) => {
            warn_once!("tree instancing shadows: {error}");
            None
        }
    }
}

pub(super) type DrawTreeInstances = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    DrawTreeGroup,
);

/// Shadow cascades and the depth and motion prepass.
pub(super) type DrawTreeDepth = (
    SetItemPipeline,
    SetPrepassViewBindGroup<0>,
    SetPrepassViewEmptyBindGroup<1>,
    DrawTreeGroup,
);

pub(super) struct DrawTreeGroup;

impl<P: PhaseItem> RenderCommand<P> for DrawTreeGroup {
    type Param = (
        SRes<TreeInstanceGpu>,
        SRes<ErasedRenderAssets<PreparedMaterial>>,
        SRes<MaterialBindGroupAllocators>,
        SRes<RenderAssets<RenderMesh>>,
        SRes<MeshAllocator>,
        SRes<MeshBindGroups>,
        SRes<MeshMetadataFallbackBuffer>,
    );
    type ViewQuery = &'static ExtractedView;
    type ItemQuery = &'static TreeInstanceDraw;

    fn render<'w>(
        _item: &P,
        view: ROQueryItem<'w, '_, Self::ViewQuery>,
        draw: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        (gpu, materials, allocators, meshes, mesh_allocator, mesh_bind_groups, metadata_fallback): SystemParamItem<
            'w,
            '_,
            Self::Param,
        >,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let gpu = gpu.into_inner();
        let (Some(draw), Some(instances)) = (draw, gpu.bind_group.as_ref()) else {
            return RenderCommandResult::Skip;
        };
        let Some(group) = gpu.groups(&view.retained_view_entity).get(draw.0) else {
            return RenderCommandResult::Skip;
        };
        let materials = materials.into_inner();
        let Some(material) = materials.get(group.material.untyped()) else {
            return RenderCommandResult::Skip;
        };
        let Some(material_group) = allocators
            .into_inner()
            .get(&TypeId::of::<TreeWindMaterial>())
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
            .mesh_slabs(&group.mesh)
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
        let (Some(mesh), Some(vertices)) = (
            meshes.into_inner().get(group.mesh),
            mesh_allocator.mesh_vertex_slice(&group.mesh),
        ) else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(2, mesh_group, &[]);
        pass.set_bind_group(MATERIAL_BIND_GROUP_INDEX, material_group, &[]);
        pass.set_bind_group(4, instances, &[]);
        pass.set_vertex_buffer(0, vertices.buffer.slice(..));
        let instances = group.first..group.first + group.count;
        match &mesh.buffer_info {
            RenderMeshBufferInfo::Indexed {
                index_format,
                count,
            } => {
                let Some(indices) = mesh_allocator.mesh_index_slice(&group.mesh) else {
                    return RenderCommandResult::Skip;
                };
                pass.set_index_buffer(indices.buffer.slice(..), *index_format);
                pass.draw_indexed(
                    indices.range.start..indices.range.start + count,
                    vertices.range.start as i32,
                    instances,
                );
            }
            RenderMeshBufferInfo::NonIndexed => {
                pass.draw(
                    vertices.range.start..vertices.range.start + mesh.vertex_count,
                    instances,
                );
            }
        }
        RenderCommandResult::Success
    }
}
