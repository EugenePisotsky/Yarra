//! Performance roadmap, steps 1 and 3 (docs/REFACTORING.md): trees drawn from one instance
//! buffer instead of one render entity per mesh, opted into with `--tree-instancing`. The tree
//! entities stay, so LOD choice and fades still run through them, but they move to render
//! layer 1, which no camera or light sees, so Bevy no longer extracts, culls or queues them.
//! Each frame the meshes visible in the main view, and for every shadow cascade the meshes of
//! the LOD each tree casts from (`TreeInstancing::shadow_lod` steps coarser than the one drawn),
//! are gathered into one buffer, grouped by mesh and material, and each group is drawn with
//! one instanced draw through the tree material's own pipelines, specialized with
//! `TREE_INSTANCED` so they read transforms and fades from the buffer instead of Bevy's mesh
//! uniforms (`shaders/tree_wind.wesl`, `shaders/clouds/material.wesl`).
use super::material::TreeWindMaterial;
use crate::object_lod::{LodScene, ScreenSpaceLod, TAG_BIAS, TIMED_RANGE};
use bevy::{
    camera::{
        primitives::{Aabb, CascadesFrusta, Frustum},
        visibility::{RenderLayers, VisibilityRange, VisibilitySystems},
    },
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
    light::SimulationLightSystems,
    material::{
        AlphaMode, RenderPhaseType,
        key::{ErasedMaterialKey, ErasedMaterialPipelineKey, ErasedMeshPipelineKey},
    },
    math::Vec3A,
    mesh::{Mesh, MeshTag},
    pbr::{
        LightEntity, MATERIAL_BIND_GROUP_INDEX, MeshBindGroups, MeshMorphBindGroupKey,
        MeshPipelineKey, PreparedMaterial, SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup,
        SetPrepassViewBindGroup, SetPrepassViewEmptyBindGroup, Shadow, ShadowBatchSetKey,
        ShadowBinKey, ViewKeyCache, ViewKeyPrepassCache, alpha_mode_pipeline_key,
    },
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        erased_render_asset::ErasedRenderAssets,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
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
        render_resource::{binding_types::storage_buffer_read_only_sized, *},
        renderer::{RenderDevice, RenderQueue},
        sync_world::MainEntity,
        view::{ExtractedView, RetainedViewEntity},
    },
    shape::ViewFrustum,
};
use bytemuck::{Pod, Zeroable};
use std::{any::TypeId, collections::HashMap, sync::Arc};

/// Render layer the instanced trees' entities move to, which no camera or light sees.
const TREE_LAYER: usize = 1;
/// Draw entities, one per group of instances drawn together (form, LOD and primitive) in a
/// view; every view numbers its groups from 0.
const DRAWS: usize = 512;

/// Whether trees are drawn from the instance buffer (the prototype) or as entities.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeInstancing {
    pub enabled: bool,
    /// How many LODs coarser than the drawn one a tree casts its shadow from (at most its last
    /// mesh LOD). 0 matches the entities' shadows; 1 saves about a third of the cascades' time
    /// but the coarser crowns let more light through, and the forest reads lighter.
    pub shadow_lod: usize,
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

impl TreeInstance {
    fn new(transform: &GlobalTransform, tag: u32) -> Self {
        let affine = transform.affine();
        let (m, t) = (affine.matrix3, affine.translation);
        Self {
            rows: [
                [m.x_axis.x, m.y_axis.x, m.z_axis.x, t.x],
                [m.x_axis.y, m.y_axis.y, m.z_axis.y, t.y],
                [m.x_axis.z, m.y_axis.z, m.z_axis.z, t.z],
            ],
            tag,
            ..default()
        }
    }
}

#[derive(Clone, Debug)]
struct Group {
    mesh: AssetId<Mesh>,
    material: AssetId<TreeWindMaterial>,
    first: u32,
    count: u32,
}

/// This frame's instances, by view and group.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
struct TreeInstanceFrame {
    instances: Arc<Vec<TreeInstance>>,
    /// The main view's groups.
    groups: Arc<Vec<Group>>,
    /// Each shadow cascade's groups, by its view.
    cascades: Arc<HashMap<RetainedViewEntity, Vec<Group>>>,
}

/// The entity a group's phase item is drawn for; its index is the group's in its view.
#[derive(Component, ExtractComponent, Clone, Copy, Debug)]
#[extract_app(bevy::render::RenderApp)]
struct TreeInstanceDraw(usize);

/// A tree mesh entity moved out of every view.
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
                .after(SimulationLightSystems::UpdateLightFrusta)
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
        .add_render_command::<Opaque3dPrepass, DrawTreeDepth>()
        .add_render_command::<AlphaMask3dPrepass, DrawTreeDepth>()
        .add_render_command::<Shadow, DrawTreeDepth>()
        .add_systems(
            Render,
            (
                (queue, queue_shadows).in_set(RenderSystems::Queue),
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

/// What a group draws: a mesh with a material.
type Form = (AssetId<Mesh>, AssetId<TreeWindMaterial>);
/// Instances of one view, by mesh and material.
type Groups = HashMap<Form, Vec<TreeInstance>>;

/// Appends a view's instances to `instances`, group after group, and empties `groups` for the
/// next frame (keeping their allocations).
fn flatten(groups: &mut Groups, instances: &mut Vec<TreeInstance>) -> Vec<Group> {
    groups.retain(|_, list| !list.is_empty());
    let mut drawn = Vec::with_capacity(groups.len().min(DRAWS));
    for (&(mesh, material), list) in groups.iter().take(DRAWS) {
        drawn.push(Group {
            mesh,
            material,
            first: instances.len() as u32,
            count: list.len() as u32,
        });
        instances.extend_from_slice(list);
    }
    for list in groups.values_mut() {
        list.clear();
    }
    drawn
}

/// The LOD scenes (by variant index) an object casts its shadow from, with their dither
/// levels: each drawn mesh LOD casts from the one `bias` steps coarser, at most the last mesh
/// LOD. Two drawn LODs casting from the same scene, in the middle of a fade between them, cast
/// it whole, so the shadow does not change while the tree dissolves.
fn shadow_scenes(lod: &ScreenSpaceLod, bias: usize, casts: &mut Vec<(usize, i32)>) {
    casts.clear();
    let variants = lod.variants();
    let Some(last) = variants.iter().rposition(|v| v.scene.is_some()) else {
        return;
    };
    for index in 0..=last {
        let Some(level) = lod.timed_level(index) else {
            continue;
        };
        let cast = (index + bias).min(last);
        match casts.iter_mut().find(|(scene, _)| *scene == cast) {
            Some(both) => both.1 = 0,
            None => casts.push((cast, level)),
        }
    }
}

/// Whether a sphere touches a shadow cascade, whose near plane does not cull: a caster
/// between the light and the cascade still shades it.
fn in_cascade(frustum: &Frustum, centre: Vec3A, radius: f32) -> bool {
    let centre = centre.extend(1.0);
    frustum.half_spaces.iter().enumerate().all(|(i, half)| {
        i == ViewFrustum::NEAR_PLANE_IDX || half.normal_d().dot(centre) + radius > 0.0
    })
}

/// One shadow cascade being gathered.
struct Cascade {
    view: RetainedViewEntity,
    frustum: Frustum,
    groups: Groups,
}

#[derive(Default)]
struct Gathering {
    main: Groups,
    cascades: Vec<Cascade>,
    /// Tree mesh entities under each LOD scene, found once its meshes exist.
    scene_meshes: HashMap<Entity, Vec<Entity>>,
    casts: Vec<(usize, i32)>,
    log_in: u32,
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)] // Tree meshes, LODs, lights.
fn collect(
    mut commands: Commands,
    settings: Res<TreeInstancing>,
    mut frame: ResMut<TreeInstanceFrame>,
    camera: Query<(Entity, &Frustum, &Camera), With<crate::WorldViewCamera>>,
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
    objects: Query<(
        &ScreenSpaceLod,
        &Children,
        &GlobalTransform,
        &InheritedVisibility,
    )>,
    scenes: Query<&LodScene>,
    descendants: Query<&Children>,
    lights: Query<(Entity, &DirectionalLight, &CascadesFrusta, &ViewVisibility)>,
    mut gathering: Local<Gathering>,
) {
    if !settings.enabled {
        for (entity, .., instanced, _) in &trees {
            if instanced {
                commands
                    .entity(entity)
                    .remove::<(Instanced, RenderLayers)>();
            }
        }
        if !frame.groups.is_empty() || !frame.cascades.is_empty() {
            *frame = TreeInstanceFrame::default();
        }
        return;
    }
    let gathering = &mut *gathering;
    let view = camera
        .iter()
        .find_map(|(entity, frustum, camera)| camera.is_active.then_some((entity, frustum)));

    // The main view: every visible mesh of an object that fades over time (others keep Bevy's
    // per-view distance crossfades, so they stay entities).
    for (entity, mesh, material, transform, visible, aabb, tag, instanced, range) in &trees {
        if range != Some(&TIMED_RANGE) {
            continue;
        }
        if !instanced {
            commands
                .entity(entity)
                .insert((Instanced, RenderLayers::layer(TREE_LAYER)));
        }
        let Some((_, frustum)) = view else {
            continue;
        };
        if !visible.get()
            || aabb
                .is_some_and(|aabb| !frustum.intersects_obb(aabb, &transform.affine(), true, false))
        {
            continue;
        }
        gathering
            .main
            .entry((mesh.id(), material.id()))
            .or_default()
            .push(TreeInstance::new(transform, tag.map_or(0, |t| t.value)));
    }

    // Shadows: the world view's cascades of every light that casts.
    gathering.cascades.retain(|_| false);
    if let Some((camera, _)) = view {
        for (light, directional, frusta, light_visible) in &lights {
            if !directional.shadow_maps_enabled || !light_visible.get() {
                continue;
            }
            for (index, frustum) in frusta.frusta.get(&camera).into_iter().flatten().enumerate() {
                gathering.cascades.push(Cascade {
                    view: RetainedViewEntity::new(
                        MainEntity::from(light),
                        Some(MainEntity::from(camera)),
                        index as u32,
                    ),
                    frustum: *frustum,
                    groups: Groups::default(),
                });
            }
        }
    }
    let mut casting = 0;
    if !gathering.cascades.is_empty() {
        for (lod, children, transform, visible) in &objects {
            if !visible.get() {
                continue;
            }
            // A sphere round the whole tree, as wide as it is tall, picks the cascades it
            // may shade; each of its meshes is then tested as Bevy tests casters.
            let (scale, _, translation) = transform.to_scale_rotation_translation();
            let height = lod.height(scale);
            let centre = Vec3A::from(translation + Vec3::Y * height * 0.5);
            let mut touched = 0u32;
            for (index, cascade) in gathering.cascades.iter().enumerate() {
                if in_cascade(&cascade.frustum, centre, height) {
                    touched |= 1 << index;
                }
            }
            if touched == 0 {
                continue;
            }
            shadow_scenes(lod, settings.shadow_lod, &mut gathering.casts);
            if !gathering.casts.is_empty() {
                casting += 1;
            }
            for &(index, level) in &gathering.casts {
                let Some(scene) = children
                    .iter()
                    .find(|child| scenes.get(*child).is_ok_and(|scene| scene.0 == index))
                else {
                    continue;
                };
                let meshes = gathering.scene_meshes.entry(scene).or_default();
                if meshes.is_empty() {
                    meshes.extend(
                        descendants
                            .iter_descendants(scene)
                            .filter(|entity| trees.contains(*entity)),
                    );
                }
                let tag = (TAG_BIAS + level) as u32;
                for &entity in meshes.iter() {
                    let Ok((_, mesh, material, transform, _, aabb, ..)) = trees.get(entity) else {
                        continue;
                    };
                    let affine = transform.affine();
                    let instance = TreeInstance::new(transform, tag);
                    for (index, cascade) in gathering.cascades.iter_mut().enumerate() {
                        if touched & (1 << index) == 0
                            || aabb.is_some_and(|aabb| {
                                !cascade.frustum.intersects_obb(aabb, &affine, false, true)
                            })
                        {
                            continue;
                        }
                        cascade
                            .groups
                            .entry((mesh.id(), material.id()))
                            .or_default()
                            .push(instance);
                    }
                }
            }
        }
    }

    let mut instances = Vec::new();
    let groups = flatten(&mut gathering.main, &mut instances);
    let main_instances = instances.len();
    let mut cascades = HashMap::with_capacity(gathering.cascades.len());
    let mut per_cascade = Vec::with_capacity(gathering.cascades.len());
    for cascade in &mut gathering.cascades {
        let first = instances.len();
        cascades.insert(cascade.view, flatten(&mut cascade.groups, &mut instances));
        per_cascade.push(instances.len() - first);
    }
    gathering.log_in = gathering.log_in.saturating_sub(1);
    if gathering.log_in == 0 {
        gathering.log_in = 600;
        // Scenes despawned with their cells leave the cache here.
        gathering
            .scene_meshes
            .retain(|scene, _| scenes.contains(*scene));
        info!(
            "TREE_INSTANCING groups={} instances={} entities={} casting={} cascades={:?} shadow_lod={}",
            groups.len(),
            main_instances,
            trees.iter().len(),
            casting,
            per_cascade,
            settings.shadow_lod,
        );
    }
    *frame = TreeInstanceFrame {
        instances: Arc::new(instances),
        groups: Arc::new(groups),
        cascades: Arc::new(cascades),
    };
}

#[derive(Resource, Default)]
struct TreeInstanceGpu {
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

fn prepare(
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
fn queue(world: &mut World, mut pipelines: Local<HashMap<PipelineFor, CachedRenderPipelineId>>) {
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
struct PipelineFor {
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
fn queue_shadows(world: &mut World, mut pipelines: Local<HashMap<Form, CachedRenderPipelineId>>) {
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

type DrawTreeInstances = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    DrawTreeGroup,
);

/// Shadow cascades and the depth and motion prepass.
type DrawTreeDepth = (
    SetItemPipeline,
    SetPrepassViewBindGroup<0>,
    SetPrepassViewEmptyBindGroup<1>,
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
