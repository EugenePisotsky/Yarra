//! Snow characters shake off trees (`shaders/tree/shed.wesl`). Each snowy form has a table of its
//! primary limbs from its nearest LOD: root, direction and reach, and where along it its snow lies
//! ([`limb_table`]). Each snowy batch in the pool gets a state per limb of each of its trees (when
//! the limb shed, and how fast its bough moved) for as long as it stays in the pool. Each frame a
//! compute pass (`shaders/tree/shed_shake.wesl`) reads the touch field (`super::super::touch`)
//! along every limb of the snowy trees near it: a limb whose bough characters push aside sheds,
//! its snow sliding off (`shaders/tree/material.wesl`) and falling from where it lay
//! ([`SnowFalls`]), flung as the bough moved.
use super::super::cards::{WIND_AXIS, WIND_PIVOT};
use super::super::impostor::ImpostorBatch;
use super::super::material::SnowHolder;
use super::super::touch::{self, TreeTouch};
use super::forms::TreeForms;
use super::pool::TreePool;
use super::render::{Growing, TreeGpu};

use atmosphere::ambient_particles::{SNOW_FALL_CAPACITY, SnowFalls, snow_falls_handle};

use bevy::{
    mesh::VertexAttributeValues,
    prelude::*,
    render::{
        Render, RenderSystems,
        extract_resource::ExtractResource,
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue},
        storage::GpuShaderBuffer,
        texture::GpuImage,
    },
};

use bytemuck::{Pod, Zeroable};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};
use vegetation_render::VegetationLodFocus;

const SHADER: &str = "shaders/tree/shed_shake.wesl";

/// The most limbs a form may have (`MAX_LIMBS` in `shaders/tree/shed_shake.wesl`): the compute
/// pass runs this many invocations per tree.
const MAX_LIMBS: u32 = 128;

const WORKGROUP: u32 = 64;

/// Trees whose trunk lies this far from the player, horizontally, may reach into the touch
/// field: its half width (8 m) and a limb's longest reach (`TOUCH_LIMB`, 6 m).
const FIELD_REACH: f32 = 14.0;

/// A limb that holds no snow load sheds its needles' snow, this share of a full load's.
const NEEDLE_SHARE: f32 = 0.25;

/// A batch origin slot whose trees hold no snow (`SHED_NONE`).
const NONE: [u32; 4] = [u32::MAX, 0, 0, 0];

/// A limb still holding its snow: shed at a negative time.
const HELD: [f32; 4] = [-1.0, 0.0, 0.0, 0.0];

/// A primary limb of a snowy form at unit scale (`Limb` in `shaders/tree/shed.wesl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct Limb {
    /// Its root, and how far it reaches along its axis (0: no such limb).
    root: [f32; 4],
    /// Its direction, and its share of the form's snow.
    axis: [f32; 4],
    /// Where its snow starts along it, and how far across the snow lies.
    snow: [f32; 4],
    /// From there to where the snow ends, and how far its foliage spreads from its axis.
    along: [f32; 4],
}

/// A limb's vertices as a form's nearest LOD holds them.
#[derive(Default)]
struct Gathered {
    root: Vec3,
    axis: Vec3,
    reach: f32,
    /// How far its snowy foliage lies from its axis.
    spread: f32,
    loads: Vec<Vec3>,
    needles: Vec<Vec3>,
}

impl Gathered {
    /// The limb, its snow lying where its loads do (its needles without any), as a line along
    /// the limb through their middle.
    fn limb(&self, most_loads: usize) -> Limb {
        let (snow, share) = if self.loads.is_empty() {
            (&self.needles, NEEDLE_SHARE)
        } else {
            (&self.loads, self.loads.len() as f32 / most_loads as f32)
        };
        let tip = self.root + self.axis * self.reach;
        let (start, along, across) = if snow.is_empty() {
            let middle = self.root.lerp(tip, 0.5);
            (middle, tip - middle, 0.0)
        } else {
            let centre = snow.iter().sum::<Vec3>() / snow.len() as f32;
            let (low, high) = snow.iter().fold((f32::MAX, f32::MIN), |(low, high), &p| {
                let t = (p - centre).dot(self.axis);
                (low.min(t), high.max(t))
            });
            let lateral = snow
                .iter()
                .map(|&p| {
                    (p - centre)
                        .reject_from_normalized(self.axis)
                        .length_squared()
                })
                .sum::<f32>()
                / snow.len() as f32;
            (
                centre + self.axis * low,
                self.axis * (high - low),
                lateral.sqrt(),
            )
        };
        Limb {
            root: self.root.extend(self.reach.max(0.01)).to_array(),
            axis: self.axis.extend(share).to_array(),
            snow: start.extend(across).to_array(),
            along: along.extend(self.spread).to_array(),
        }
    }
}

/// The primary limbs of the snowy parts of a form's nearest LOD, by the number each vertex
/// carries in its second UV's y (the kit's `wind_binding`, -1 the trunk); none without any.
pub(super) fn limb_table<'a>(
    parts: impl IntoIterator<Item = (&'a Mesh, SnowHolder)>,
) -> Option<Vec<Limb>> {
    let mut limbs: Vec<Option<Gathered>> = Vec::new();

    for (mesh, holder) in parts {
        let (
            Some(VertexAttributeValues::Float32x3(positions)),
            Some(VertexAttributeValues::Float32x2(numbers)),
            Some(VertexAttributeValues::Float32x4(pivots)),
            Some(VertexAttributeValues::Float32x4(axes)),
        ) = (
            mesh.attribute(Mesh::ATTRIBUTE_POSITION),
            mesh.attribute(Mesh::ATTRIBUTE_UV_1),
            mesh.attribute(WIND_PIVOT),
            mesh.attribute(WIND_AXIS),
        )
        else {
            continue;
        };

        for (((p, number), pivot), axis) in positions.iter().zip(numbers).zip(pivots).zip(axes) {
            let number = number[1].round();

            if axis[3] == 0.0 || !(0.0..MAX_LIMBS as f32).contains(&number) {
                continue;
            }

            let number = number as usize;

            if limbs.len() <= number {
                limbs.resize_with(number + 1, || None);
            }

            let limb = limbs[number].get_or_insert_with(|| Gathered {
                root: Vec3::new(pivot[0], pivot[1], pivot[2]),
                axis: Vec3::new(axis[0], axis[1], axis[2]).normalize_or_zero(),
                ..default()
            });
            let p = Vec3::from_array(*p);
            let offset = p - limb.root;
            limb.reach = limb.reach.max(offset.dot(limb.axis));

            if holder != SnowHolder::Bark {
                let spread = offset.reject_from_normalized(limb.axis).length();
                limb.spread = limb.spread.max(spread);
            }

            match holder {
                SnowHolder::Load => limb.loads.push(p),
                SnowHolder::Needles => limb.needles.push(p),
                SnowHolder::Bark => {}
            }
        }
    }

    let most_loads = limbs.iter().flatten().map(|l| l.loads.len()).max()?;
    let table: Vec<Limb> = limbs
        .iter()
        .map(|limb| {
            limb.as_ref()
                .map_or_else(Limb::default, |l| l.limb(most_loads))
        })
        .collect();
    (!table.is_empty()).then_some(table)
}

/// What the GPU reads the states by: per batch origin slot its first state, its first pool
/// instance and limbs per tree (`NONE`: no snow); per form its first limb in `limbs` and how
/// many; and how many states there are.
#[derive(Default)]
pub(super) struct ShedTables {
    revision: u64,
    states: u32,
    batches: Vec<[u32; 4]>,
    forms: Vec<[u32; 4]>,
    limbs: Vec<Limb>,
}

/// The snowy batches' states.
#[derive(Resource, Default)]
pub(super) struct SnowShed {
    /// Each snowy batch's states, and its origin slot.
    snowy: HashMap<Entity, (Range<u32>, u32)>,
    /// Batches whose form holds no snow.
    plain: HashSet<Entity>,
    /// Free state ranges, sorted.
    free: Vec<Range<u32>>,
    tables: ShedTables,
    /// State ranges handed out since the frame took them, which start holding their snow.
    fresh: Vec<Range<u32>>,
    /// The pool and forms last seen, and whether the tables changed since the frame took them.
    pool_revision: u64,
    forms_revision: u64,
    changed: bool,
}

impl SnowShed {
    fn allocate(&mut self, count: u32) -> u32 {
        if let Some(i) = self.free.iter().position(|r| r.len() >= count as usize) {
            let start = self.free[i].start;
            self.free[i].start += count;

            if self.free[i].is_empty() {
                self.free.remove(i);
            }

            return start;
        }

        let start = self.tables.states;
        self.tables.states += count;
        start
    }

    fn release(&mut self, range: Range<u32>) {
        let at = self.free.partition_point(|r| r.start < range.start);
        self.free.insert(at, range);
        let mut joined: Vec<Range<u32>> = Vec::with_capacity(self.free.len());

        for range in self.free.drain(..) {
            match joined.last_mut() {
                Some(last) if last.end == range.start => last.end = range.end,
                _ => joined.push(range),
            }
        }

        self.free = joined;
    }

    /// Every snowy form's limbs, after forms became ready.
    fn take_forms(&mut self, forms: &TreeForms) {
        let tables = &mut self.tables;
        tables.forms.clear();
        tables.limbs.clear();

        for index in 0..forms.len() {
            let limbs = forms.limbs(index as u32).map_or(&[][..], |l| &l[..]);
            tables
                .forms
                .push([tables.limbs.len() as u32, limbs.len() as u32, 0, 0]);
            tables.limbs.extend_from_slice(limbs);
        }

        self.changed = true;
    }
}

/// Gives every new batch of a snowy form its states, once its form is ready.
pub(super) fn assign(pool: Res<TreePool>, forms: Res<TreeForms>, mut shed: ResMut<SnowShed>) {
    let shed = shed.as_mut();
    let forms_changed = forms.revision != shed.forms_revision;

    if forms_changed {
        shed.forms_revision = forms.revision;
        shed.take_forms(&forms);
    }

    if pool.revision == shed.pool_revision && !forms_changed {
        return;
    }

    shed.pool_revision = pool.revision;
    let slots = pool.origin_slots() as usize;

    if shed.tables.batches.len() < slots {
        shed.tables.batches.resize(slots, NONE);
        shed.changed = true;
    }

    for (entity, batch) in pool.placed() {
        if shed.snowy.contains_key(&entity) || shed.plain.contains(&entity) {
            continue;
        }

        if !forms.is_ready(batch.form) {
            continue;
        }

        let Some(limbs) = forms.limbs(batch.form).map(|l| l.len() as u32) else {
            shed.plain.insert(entity);
            continue;
        };
        let count = batch.range.len() as u32 * limbs;
        let first = shed.allocate(count);
        shed.tables.batches[batch.origin as usize] = [first, batch.range.start, limbs, 0];
        shed.snowy
            .insert(entity, (first..first + count, batch.origin));
        shed.fresh.push(first..first + count);
        shed.changed = true;
    }
}

/// A batch leaving frees its states; its origin slot reads none.
pub(super) fn release(removed: On<Remove<ImpostorBatch>>, mut shed: Option<ResMut<SnowShed>>) {
    let Some(shed) = shed.as_deref_mut() else {
        return;
    };
    shed.plain.remove(&removed.entity);

    if let Some((range, origin)) = shed.snowy.remove(&removed.entity) {
        shed.tables.batches[origin as usize] = NONE;
        shed.release(range);
        shed.changed = true;
    }
}

/// This frame's shake: the snowy trees near the touch field, and the tables when they changed.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub(super) struct ShedFrame {
    now: f32,
    /// The trees' pool instances.
    trees: Vec<u32>,
    tables: Arc<ShedTables>,
    fresh: Vec<Range<u32>>,
}

/// Lists the snowy trees that may reach into the touch field round the player.
#[allow(clippy::too_many_arguments)] // The clock, the field, the pool and what it writes.
pub(super) fn list(
    time: Res<Time>,
    touch: Res<TreeTouch>,
    focus: Option<Res<VegetationLodFocus>>,
    pool: Res<TreePool>,
    batches: Query<&ImpostorBatch>,
    mut shed: ResMut<SnowShed>,
    mut frame: ResMut<ShedFrame>,
    mut falls: Option<ResMut<SnowFalls>>,
) {
    let now = time.elapsed_secs();
    let mut trees = Vec::new();

    if let Some(feet) = focus.and_then(|f| f.position).filter(|_| touch.enabled) {
        for (entity, batch) in pool.placed() {
            let (true, Ok(placed)) = (shed.snowy.contains_key(&entity), batches.get(entity)) else {
                continue;
            };
            let origin = Vec3::new(batch.position[0], batch.position[1], batch.position[2]);

            for (index, tree) in batch.range.clone().zip(&placed.instances) {
                if (origin + tree.translation).xz().distance(feet.xz()) <= FIELD_REACH {
                    trees.push(index);
                }
            }
        }
    }

    if let (false, Some(falls)) = (trees.is_empty(), falls.as_deref_mut()) {
        falls.shaking(now);
    }

    let shed = shed.as_mut();

    if shed.tables.revision != frame.tables.revision || trees.len() != frame.trees.len() {
        debug!(
            "TREE_SNOW_SHED snowy_batches={} trees={} states={}",
            shed.snowy.len(),
            trees.len(),
            shed.tables.states
        );
    }

    if std::mem::take(&mut shed.changed) {
        shed.tables.revision += 1;
        frame.tables = Arc::new(ShedTables {
            revision: shed.tables.revision,
            states: shed.tables.states,
            batches: shed.tables.batches.clone(),
            forms: shed.tables.forms.clone(),
            limbs: shed.tables.limbs.clone(),
        });
    }

    frame.now = now;
    frame.trees = trees;
    frame.fresh = std::mem::take(&mut shed.fresh);
}

/// The states and tables on the GPU, and the compute pass that sheds limbs.
#[derive(Resource)]
pub(super) struct ShedGpu {
    states: Growing,
    /// The clock, then the batches' table (`ShedTrees`).
    trees: Growing,
    forms: Growing,
    limbs: Growing,
    listed: Growing,
    uniform: Buffer,
    /// The next burst in the ambient particles' ring.
    next_fall: Buffer,
    revision: Option<u64>,
    /// Counts the states and trees buffers replaced, which the draws' bind group holds.
    generation: u64,
    count: u32,
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
}

impl ShedGpu {
    pub(super) fn states(&self) -> &Buffer {
        self.states.get()
    }

    pub(super) fn trees(&self) -> &Buffer {
        self.trees.get()
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }
}

impl FromWorld for ShedGpu {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>();
        let queue = world.resource::<RenderQueue>();
        let growing = |label: &'static str, usage: BufferUsages| {
            let mut buffer = Growing::default();
            buffer.reserve(device, label, usage, 0);
            buffer
        };
        let storage = BufferUsages::STORAGE;

        // The states keep their contents as they grow (`Growing::reserve_keeping`).
        let states = growing("tree shed states", storage | BufferUsages::COPY_SRC);
        let trees = growing("tree shed batches", storage);
        let forms = growing("tree shed forms", storage);
        let limbs = growing("tree shed limbs", storage);
        let listed = growing("tree shed trees", storage);

        // No batch holds states until the first tables arrive.
        queue.write_buffer(trees.get(), 16, bytemuck::cast_slice(&[NONE; 15]));
        let uniform = device.create_buffer(&BufferDescriptor {
            label: Some("tree shed"),
            size: 16,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let next_fall = device.create_buffer(&BufferDescriptor {
            label: Some("tree shed next fall"),
            size: 16,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let layout = BindGroupLayoutDescriptor::new(
            "tree shed",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                    texture_3d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                ),
            ),
        );
        let shader = world.resource::<AssetServer>().load(SHADER);
        let pipeline =
            world
                .resource::<PipelineCache>()
                .queue_compute_pipeline(ComputePipelineDescriptor {
                    label: Some("tree shed".into()),
                    layout: vec![layout.clone()],
                    shader,
                    entry_point: Some("shake_limbs".into()),
                    ..default()
                });
        Self {
            states,
            trees,
            forms,
            limbs,
            listed,
            uniform,
            next_fall,
            revision: None,
            generation: 0,
            count: 0,
            layout,
            pipeline,
        }
    }
}

pub(super) fn install(render_app: &mut SubApp) {
    render_app.add_systems(Render, prepare.in_set(RenderSystems::PrepareResources));
}

pub(super) fn initialize(world: &mut World) {
    world.init_resource::<ShedGpu>();
}

/// Bytes of a state, a table row and a limb.
const ROW_BYTES: u64 = size_of::<[f32; 4]>() as u64;

/// The frame's tables, fresh states, clock and trees.
fn prepare(
    frame: Res<ShedFrame>,
    mut gpu: ResMut<ShedGpu>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let gpu = gpu.as_mut();
    let tables = &frame.tables;
    let storage = BufferUsages::STORAGE;

    if gpu.revision != Some(tables.revision) {
        gpu.revision = Some(tables.revision);
        let states = u64::from(tables.states) * ROW_BYTES;
        let batches = (1 + tables.batches.len() as u64) * ROW_BYTES;
        let grown =
            gpu.states
                .reserve_keeping(&device, &queue, "tree shed states", storage, states)
                | gpu
                    .trees
                    .reserve(&device, "tree shed batches", storage, batches);
        gpu.forms.reserve(
            &device,
            "tree shed forms",
            storage,
            tables.forms.len() as u64 * ROW_BYTES,
        );
        gpu.limbs.reserve(
            &device,
            "tree shed limbs",
            storage,
            size_of_val(tables.limbs.as_slice()) as u64,
        );
        queue.write_buffer(
            gpu.trees.get(),
            ROW_BYTES,
            bytemuck::cast_slice(&tables.batches),
        );
        queue.write_buffer(gpu.forms.get(), 0, bytemuck::cast_slice(&tables.forms));
        queue.write_buffer(gpu.limbs.get(), 0, bytemuck::cast_slice(&tables.limbs));
        gpu.generation += u64::from(grown);
    }

    for range in &frame.fresh {
        let held = vec![HELD; range.len()];
        queue.write_buffer(
            gpu.states.get(),
            u64::from(range.start) * ROW_BYTES,
            bytemuck::cast_slice(&held),
        );
    }

    queue.write_buffer(gpu.trees.get(), 0, bytemuck::bytes_of(&[frame.now; 4]));
    gpu.listed.reserve(
        &device,
        "tree shed trees",
        storage,
        size_of_val(frame.trees.as_slice()) as u64,
    );
    queue.write_buffer(gpu.listed.get(), 0, bytemuck::cast_slice(&frame.trees));
    gpu.count = frame.trees.len() as u32;
    let shake = [frame.now, gpu.count as f32, SNOW_FALL_CAPACITY as f32, 0.0];
    queue.write_buffer(&gpu.uniform, 0, bytemuck::bytes_of(&shake));
}

/// Sheds the limbs of the listed trees their boughs' pushes reach, after the LOD pass placed
/// them this frame.
pub(super) fn shake(
    mut render_context: RenderContext,
    shed: Res<ShedGpu>,
    trees: Res<TreeGpu>,
    images: Res<RenderAssets<GpuImage>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    cache: Res<PipelineCache>,
) {
    if shed.count == 0 {
        return;
    }

    let (Some(pipeline), Some((instances, transforms)), Some(field), Some(window), Some(falls)) = (
        cache.get_compute_pipeline(shed.pipeline),
        trees.pool_buffers(),
        images.get(&touch::field_handle()),
        buffers.get(&touch::window_handle()),
        buffers.get(&snow_falls_handle()),
    ) else {
        return;
    };
    let group = render_context.render_device().create_bind_group(
        "tree shed",
        &cache.get_bind_group_layout(&shed.layout),
        &BindGroupEntries::sequential((
            shed.uniform.as_entire_binding(),
            shed.listed.get().as_entire_binding(),
            instances.as_entire_binding(),
            transforms.as_entire_binding(),
            shed.trees.get().as_entire_binding(),
            shed.forms.get().as_entire_binding(),
            shed.limbs.get().as_entire_binding(),
            shed.states.get().as_entire_binding(),
            &field.texture_view,
            &field.sampler,
            window.buffer.as_entire_binding(),
            falls.buffer.as_entire_binding(),
            shed.next_fall.as_entire_binding(),
        )),
    );
    let mut pass = render_context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("tree shed"),
            ..default()
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.dispatch_workgroups((shed.count * MAX_LIMBS).div_ceil(WORKGROUP), 1, 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology};

    /// A limb along +x from (0, 1, 0) numbered `number`, its vertices at `xs`, as `holder`.
    fn part(number: f32, xs: &[f32], lift: f32) -> Mesh {
        let n = xs.len();
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD,
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            xs.iter()
                .map(|&x| [x, 1.0 + lift, 0.1 * x])
                .collect::<Vec<_>>(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, vec![[1.0, number]; n])
        .with_inserted_attribute(WIND_PIVOT, vec![[0.0, 1.0, 0.0, 14.0]; n])
        .with_inserted_attribute(WIND_AXIS, vec![[1.0, 0.0, 0.0, 0.8]; n])
    }

    #[test]
    fn a_limb_reaches_its_tip_and_its_snow_lies_where_its_loads_do() {
        let bark = part(1.0, &[0.0, 1.0, 3.0], 0.0);
        let needles = part(1.0, &[0.5, 2.5], 0.1);
        let loads = part(1.0, &[1.0, 2.0], 0.2);
        let bare = part(0.0, &[0.0, 1.0], 0.0);
        let table = limb_table([
            (&bark, SnowHolder::Bark),
            (&needles, SnowHolder::Needles),
            (&loads, SnowHolder::Load),
            (&bare, SnowHolder::Bark),
        ])
        .unwrap();
        assert_eq!(table.len(), 2);
        let limb = table[1];
        assert_eq!(
            limb.root,
            [0.0, 1.0, 0.0, 3.0],
            "it reaches its furthest vertex"
        );
        assert_eq!(
            limb.axis[3], 1.0,
            "the most loaded limb holds a whole share"
        );
        assert!((limb.snow[0] - 1.0).abs() < 1e-5 && (limb.snow[1] - 1.2).abs() < 1e-5);
        assert!((limb.along[0] - 1.0).abs() < 1e-5, "{:?}", limb.along);

        // Its furthest snowy vertex, the load at x 2, lies 0.2 up and 0.2 across from its axis.
        assert!(
            (limb.along[3] - 0.2f32.hypot(0.2)).abs() < 1e-5,
            "{:?}",
            limb.along
        );
        assert_eq!(table[0].axis[3], NEEDLE_SHARE, "no load: its needles' snow");
    }

    #[test]
    fn the_trunk_and_unnumbered_vertices_make_no_limb() {
        let mut trunk = part(0.0, &[0.0, 1.0], 0.0);
        trunk.insert_attribute(WIND_AXIS, vec![[0.0; 4]; 2]);
        let unnumbered = part(-1.0, &[0.0, 1.0], 0.0);
        assert!(
            limb_table([(&trunk, SnowHolder::Bark), (&unnumbered, SnowHolder::Bark)]).is_none()
        );
    }

    #[test]
    fn freed_states_are_reused_and_joined() {
        let mut shed = SnowShed::default();
        let a = shed.allocate(10);
        let b = shed.allocate(5);
        let c = shed.allocate(5);
        assert_eq!((a, b, c), (0, 10, 15));
        shed.release(a..a + 10);
        shed.release(b..b + 5);
        assert_eq!(shed.free, std::iter::once(0..15).collect::<Vec<_>>());
        assert_eq!(shed.allocate(12), 0);
        assert_eq!(shed.allocate(4), 20, "the 3 left are too few");
        assert_eq!(shed.tables.states, 24);
    }
}
