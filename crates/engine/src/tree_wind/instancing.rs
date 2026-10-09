//! Performance roadmap, step 1 (docs/REFACTORING.md): trees drawn in the main view from one
//! instance buffer instead of one render entity per mesh, opted into with `--tree-instancing`.
//! The tree entities stay, so LOD choice, fades and shadows still run through them, but they
//! leave the main view for render layer 1, which the sun and moon also cast from. Each frame the
//! ones visible in the main view are gathered into one buffer, grouped by mesh and material, and
//! each group is drawn with one instanced draw through the tree material's own pipeline,
//! specialized with `TREE_INSTANCED` so it reads transforms and fades from the buffer instead of
//! Bevy's mesh uniforms (`shaders/tree_wind.wgsl`, `shaders/clouds/material.wgsl`).
use super::material::TreeWindMaterial;
use bevy::{
    camera::{
        primitives::{Aabb, Frustum},
        visibility::{RenderLayers, VisibilityRange, VisibilitySystems},
    },
    core_pipeline::{
        core_3d::{AlphaMask3d, Opaque3d, Opaque3dBatchSetKey, Opaque3dBinKey},
        prepass::{OpaqueNoLightmap3dBatchSetKey, OpaqueNoLightmap3dBinKey},
    },
    ecs::{
        query::ROQueryItem,
        system::{SystemParamItem, lifetimeless::SRes},
    },
    material::{
        RenderPhaseType,
        key::{ErasedMaterialKey, ErasedMaterialPipelineKey, ErasedMeshPipelineKey},
    },
    mesh::{Mesh, MeshTag},
    pbr::{
        MATERIAL_BIND_GROUP_INDEX, MaterialBindGroupAllocators, MeshBindGroups,
        MeshMorphBindGroupKey, MeshPipelineKey, PreparedMaterial, SetMeshViewBindGroup,
        SetMeshViewBindingArrayBindGroup, ViewKeyCache, alpha_mode_pipeline_key,
    },
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        erased_render_asset::ErasedRenderAssets,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        mesh::{RenderMesh, RenderMeshBufferInfo, allocator::MeshAllocator},
        render_asset::RenderAssets,
        render_phase::{
            AddRenderCommand, BinnedRenderPhaseType, DrawFunctions, InputUniformIndex, PhaseItem,
            RenderCommand, RenderCommandResult, SetItemPipeline, TrackedRenderPass,
            ViewBinnedRenderPhases,
        },
        render_resource::{binding_types::storage_buffer_read_only_sized, *},
        renderer::{RenderDevice, RenderQueue},
        sync_world::MainEntity,
        view::{ExtractedView, RetainedViewEntity},
    },
};
use bytemuck::{Pod, Zeroable};
use std::{any::TypeId, collections::HashMap, sync::Arc};

/// Render layer the instanced trees' entities move to: out of the main view, still casting.
const TREE_LAYER: usize = 1;
/// Draw entities, one per group of instances drawn together (form, LOD and primitive).
const DRAWS: usize = 512;

/// Whether trees are drawn from the instance buffer (the prototype) or as entities.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeInstancing {
    pub enabled: bool,
}

/// One drawn tree mesh: the rows of its world-from-local transform and its fade tag
/// (`object_lod`'s `MeshTag`, 64 + dither level while it fades, else 0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(super) struct TreeInstance {
    rows: [[f32; 4]; 3],
    tag: u32,
    flags: u32,
    padding: [u32; 2],
}

#[derive(Clone, Debug)]
struct Group {
    mesh: AssetId<Mesh>,
    material: AssetId<TreeWindMaterial>,
    first: u32,
    count: u32,
}

/// This frame's instances in the main view, by group.
#[derive(Resource, Clone, Default, ExtractResource)]
struct TreeInstanceFrame {
    instances: Arc<Vec<TreeInstance>>,
    groups: Arc<Vec<Group>>,
}

/// The entity a group's phase item is drawn for; its index is the group's.
#[derive(Component, ExtractComponent, Clone, Copy, Debug)]
struct TreeInstanceDraw(usize);

/// A tree mesh entity moved out of the main view.
#[derive(Component)]
struct Instanced;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TreeInstancing>()
        .init_resource::<TreeInstanceFrame>()
        .add_systems(Startup, spawn_draws)
        .add_systems(
            PostUpdate,
            collect
                .after(VisibilitySystems::CheckVisibility)
                .after(bevy::transform::TransformSystems::Propagate),
        );
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }
    app.add_plugins((
        ExtractResourcePlugin::<TreeInstanceFrame>::default(),
        ExtractComponentPlugin::<TreeInstanceDraw>::default(),
    ));
    app.sub_app_mut(RenderApp)
        .init_resource::<TreeInstanceGpu>()
        .add_render_command::<Opaque3d, DrawTreeInstances>()
        .add_render_command::<AlphaMask3d, DrawTreeInstances>()
        .add_systems(
            Render,
            (
                queue.in_set(RenderSystems::Queue),
                prepare.in_set(RenderSystems::PrepareBindGroups),
            ),
        );
}

/// The instance buffer's bind group layout, group 4 of the instanced pipelines.
pub(super) fn instance_layout() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "tree instances",
        &BindGroupLayoutEntries::single(
            ShaderStages::VERTEX,
            storage_buffer_read_only_sized(false, None),
        ),
    )
}

fn spawn_draws(mut commands: Commands) {
    commands.spawn_batch((0..DRAWS).map(|i| (TreeInstanceDraw(i), Name::new("Tree instances"))));
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)] // Tree meshes, lights, camera.
fn collect(
    mut commands: Commands,
    settings: Res<TreeInstancing>,
    mut frame: ResMut<TreeInstanceFrame>,
    camera: Query<(&Frustum, &Camera), With<crate::WorldViewCamera>>,
    trees: Query<(
        Entity,
        &Mesh3d,
        &MeshMaterial3d<TreeWindMaterial>,
        &GlobalTransform,
        &InheritedVisibility,
        Option<&Aabb>,
        Option<&MeshTag>,
        Has<Instanced>,
        Option<&VisibilityRange>,
    )>,
    lights: Query<
        (Entity, Option<&RenderLayers>),
        Or<(With<atmosphere::WorldSun>, With<atmosphere::WorldMoon>)>,
    >,
    mut groups: Local<HashMap<(AssetId<Mesh>, AssetId<TreeWindMaterial>), Vec<TreeInstance>>>,
    mut log_in: Local<u32>,
) {
    if !settings.enabled {
        for (entity, .., instanced, _) in &trees {
            if instanced {
                commands
                    .entity(entity)
                    .remove::<(Instanced, RenderLayers)>();
            }
        }
        if !frame.groups.is_empty() {
            *frame = TreeInstanceFrame::default();
        }
        return;
    }
    let casting = RenderLayers::from_layers(&[0, TREE_LAYER]);
    for (light, layers) in &lights {
        if layers != Some(&casting) {
            commands.entity(light).insert(casting.clone());
        }
    }
    let frustum = camera
        .iter()
        .find_map(|(frustum, camera)| camera.is_active.then_some(frustum));
    for list in groups.values_mut() {
        list.clear();
    }
    for (entity, mesh, material, transform, visible, aabb, tag, instanced, range) in &trees {
        // Assets that crossfade by distance keep Bevy's per-view ranges, so they stay entities;
        // timed fades carry a constant range only for the dither.
        if range.is_some_and(|range| *range != crate::object_lod::TIMED_RANGE) {
            continue;
        }
        if !instanced {
            commands
                .entity(entity)
                .insert((Instanced, RenderLayers::layer(TREE_LAYER)));
        }
        let Some(frustum) = frustum else {
            continue;
        };
        let affine = transform.affine();
        if !visible.get()
            || aabb.is_some_and(|aabb| !frustum.intersects_obb(aabb, &affine, true, false))
        {
            continue;
        }
        let (m, t) = (affine.matrix3, affine.translation);
        groups
            .entry((mesh.id(), material.id()))
            .or_default()
            .push(TreeInstance {
                rows: [
                    [m.x_axis.x, m.y_axis.x, m.z_axis.x, t.x],
                    [m.x_axis.y, m.y_axis.y, m.z_axis.y, t.y],
                    [m.x_axis.z, m.y_axis.z, m.z_axis.z, t.z],
                ],
                tag: tag.map_or(0, |t| t.0),
                ..default()
            });
    }
    groups.retain(|_, list| !list.is_empty());
    let mut instances = Vec::with_capacity(groups.values().map(Vec::len).sum());
    let mut drawn = Vec::with_capacity(groups.len());
    for (&(mesh, material), list) in groups.iter().take(DRAWS) {
        drawn.push(Group {
            mesh,
            material,
            first: instances.len() as u32,
            count: list.len() as u32,
        });
        instances.extend_from_slice(list);
    }
    *log_in = log_in.saturating_sub(1);
    if *log_in == 0 {
        *log_in = 600;
        info!(
            "TREE_INSTANCING groups={} instances={} entities={}",
            drawn.len(),
            instances.len(),
            trees.iter().len()
        );
    }
    *frame = TreeInstanceFrame {
        instances: Arc::new(instances),
        groups: Arc::new(drawn),
    };
}

#[derive(Resource, Default)]
struct TreeInstanceGpu {
    buffer: Option<Buffer>,
    capacity: u64,
    bind_group: Option<BindGroup>,
    groups: Vec<Group>,
}

fn prepare(
    frame: Res<TreeInstanceFrame>,
    mut gpu: ResMut<TreeInstanceGpu>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
) {
    gpu.groups = frame.groups.to_vec();
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

/// Specializes each group's instanced pipeline for every main view, as Bevy specializes the
/// tree material for its entities but with `TreeWindKey::instanced`, and queues one item per
/// group into its phase.
fn queue(world: &mut World, mut pipelines: Local<HashMap<PipelineFor, CachedRenderPipelineId>>) {
    let groups = world.resource::<TreeInstanceFrame>().groups.clone();
    let mut views = world.query::<(&ExtractedView, &Msaa)>();
    let views: Vec<(RetainedViewEntity, Msaa)> = views
        .iter(world)
        .map(|(view, msaa)| (view.retained_view_entity, *msaa))
        .collect();
    let mut draws = world.query::<(Entity, &MainEntity, &TreeInstanceDraw)>();
    let mut draw_entities = vec![None; DRAWS];
    for (entity, main, draw) in draws.iter(world) {
        if draw.0 < DRAWS {
            draw_entities[draw.0] = Some((entity, *main));
        }
    }
    let opaque_function = world
        .resource::<DrawFunctions<Opaque3d>>()
        .read()
        .id::<DrawTreeInstances>();
    let mask_function = world
        .resource::<DrawFunctions<AlphaMask3d>>()
        .read()
        .id::<DrawTreeInstances>();
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
        // Last frame's items go; this frame's groups come back below.
        for (_, main) in draw_entities.iter().flatten() {
            if let Some(phase) = world
                .resource_mut::<ViewBinnedRenderPhases<Opaque3d>>()
                .get_mut(&view)
            {
                phase.remove(*main);
            }
            if let Some(phase) = world
                .resource_mut::<ViewBinnedRenderPhases<AlphaMask3d>>()
                .get_mut(&view)
            {
                phase.remove(*main);
            }
        }
        for (index, group) in groups.iter().enumerate() {
            let Some((entity, main)) = draw_entities[index] else {
                continue;
            };
            let Some((pipeline, phase)) =
                specialize(world, &mut pipelines, view, view_key, msaa, group)
            else {
                continue;
            };
            match phase {
                RenderPhaseType::Opaque => {
                    if let Some(phase) = world
                        .resource_mut::<ViewBinnedRenderPhases<Opaque3d>>()
                        .get_mut(&view)
                    {
                        phase.add(
                            Opaque3dBatchSetKey {
                                draw_function: opaque_function,
                                pipeline,
                                material_bind_group_index: None,
                                lightmap_slab: None,
                                slabs: default(),
                            },
                            Opaque3dBinKey {
                                asset_id: group.mesh.untyped(),
                            },
                            (entity, main),
                            InputUniformIndex::default(),
                            BinnedRenderPhaseType::NonMesh,
                        );
                    }
                }
                RenderPhaseType::AlphaMask => {
                    if let Some(phase) = world
                        .resource_mut::<ViewBinnedRenderPhases<AlphaMask3d>>()
                        .get_mut(&view)
                    {
                        phase.add(
                            OpaqueNoLightmap3dBatchSetKey {
                                draw_function: mask_function,
                                pipeline,
                                material_bind_group_index: None,
                                slabs: default(),
                            },
                            OpaqueNoLightmap3dBinKey {
                                asset_id: group.mesh.untyped(),
                            },
                            (entity, main),
                            InputUniformIndex::default(),
                            BinnedRenderPhaseType::NonMesh,
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct PipelineFor {
    view: RetainedViewEntity,
    view_key: u64,
    mesh: AssetId<Mesh>,
    material: AssetId<TreeWindMaterial>,
}

fn specialize(
    world: &mut World,
    pipelines: &mut HashMap<PipelineFor, CachedRenderPipelineId>,
    view: RetainedViewEntity,
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
    let mut material_bits: MeshPipelineKey = properties.mesh_pipeline_key_bits.downcast();
    material_bits.insert(alpha_mode_pipeline_key(properties.alpha_mode, &msaa));
    let mesh_key =
        view_key | MeshPipelineKey::from_bits_retain(mesh.key_bits.bits()) | material_bits;
    let mut data: <TreeWindMaterial as AsBindGroup>::Data = properties.material_key.to_key();
    data.extension.instanced = true;
    let key = ErasedMaterialPipelineKey {
        type_id: TypeId::of::<TreeWindMaterial>(),
        mesh_key: ErasedMeshPipelineKey::new(mesh_key),
        material_key: ErasedMaterialKey::new(data),
    };
    let base_specialize = properties.base_specialize?;
    match base_specialize(world, key, &layout, &properties) {
        Ok(pipeline) => {
            pipelines.insert(at, pipeline);
            Some((pipeline, phase))
        }
        Err(error) => {
            warn_once!("tree instancing: {error}");
            None
        }
    }
}

type DrawTreeInstances = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    DrawTreeGroup,
);

struct DrawTreeGroup;

impl<P: PhaseItem> RenderCommand<P> for DrawTreeGroup {
    type Param = (
        SRes<TreeInstanceGpu>,
        SRes<ErasedRenderAssets<PreparedMaterial>>,
        SRes<MaterialBindGroupAllocators>,
        SRes<RenderAssets<RenderMesh>>,
        SRes<MeshAllocator>,
        SRes<MeshBindGroups>,
    );
    type ViewQuery = ();
    type ItemQuery = &'static TreeInstanceDraw;

    fn render<'w>(
        _item: &P,
        _view: ROQueryItem<'w, '_, Self::ViewQuery>,
        draw: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        (gpu, materials, allocators, meshes, mesh_allocator, mesh_bind_groups): SystemParamItem<
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
        let Some(group) = gpu.groups.get(draw.0) else {
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
        let phase_groups = match mesh_bind_groups.into_inner() {
            MeshBindGroups::CpuPreprocessing(groups) => Some(groups),
            MeshBindGroups::GpuPreprocessing(groups) => groups.get(&TypeId::of::<P>()),
        };
        let Some(mesh_group) = phase_groups.and_then(|groups| {
            groups.get(None, false, MeshMorphBindGroupKey::NoMorphTargets, false)
        }) else {
            return RenderCommandResult::Skip;
        };
        let mesh_allocator = mesh_allocator.into_inner();
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
