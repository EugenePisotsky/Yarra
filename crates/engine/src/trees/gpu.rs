//! GPU-driven trees (docs/REFACTORING.md "GPU-driven trees", step 2): the game draws every
//! tree and shrub that ends in an impostor without a render entity of its own.
//!
//! Far-object blocks carry each object's whole LOD chain (`world::FarObjectForm`) out to
//! 2 km. Their batches go into one instance pool ([`pool`]); each form's mesh LODs come from
//! one hidden template scene per LOD, converted to tree materials as any tree scene is
//! ([`forms`]). Every frame a compute pass (`shaders/tree/lod.wesl`) picks each instance's
//! representation from its projected height, dissolves changes over time as the CPU used to
//! (`object_lod::timed`), culls it against the main view and each shadow cascade, and appends
//! it to its form and representation's bucket; one indirect draw per form, representation and
//! primitive then draws each bucket through the tree or impostor material's own pipelines,
//! specialized with `TREE_INSTANCED` or `IMPOSTOR_INSTANCED` ([`render`]). Since nothing
//! limits the meshes to the cells' residency, they reach as far as their thresholds say.
//!
//! Cells still spawn an entity per object for gameplay (rain shelter), but no LOD scenes. The
//! editor keeps drawing tree entities and impostor batch meshes.
use bevy::{
    prelude::*,
    render::{
        RenderApp,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_resource::{BindGroupLayoutDescriptor, BindGroupLayoutEntries, ShaderStages},
    },
};

use bytemuck::{Pod, Zeroable};

mod forms;
mod frame;
mod layout;
mod pool;
mod render;

#[cfg(test)]
mod tests;

pub(crate) use forms::TreeForms;
pub(crate) use pool::TreeOverride;

/// Shadow views (cascades of every light that casts) the compute pass culls for.
const MAX_SHADOW_VIEWS: usize = 8;

/// The main view's records and draws, then each shadow view's.
const VIEWS: usize = 1 + MAX_SHADOW_VIEWS;

/// Representations per form: mesh LODs, then the impostor.
const MAX_REPRESENTATIONS: usize = 8;

/// Draw entities, one per draw entry (form, representation and primitive); every view
/// numbers its entries alike.
const DRAWS: usize = 2048;

/// Instances the pool can address: records hold the index in 24 bits.
const MAX_INSTANCES: u32 = 1 << 24;

/// How the game draws trees, fixed at startup except `hidden`.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct TreeInstancing {
    /// From the GPU instance pool (the game), or as entities (the editor opts out).
    pub enabled: bool,
    /// How many LODs coarser than the drawn one a tree casts its shadow from (at most its last
    /// mesh LOD). 0 matches the entities' shadows; 1 saves about a third of the cascades' time
    /// but the coarser crowns let more light through, and the forest reads lighter.
    pub shadow_lod: usize,
    /// Draws no tree at all (F1 Objects off, the Ground and Grass scenes), as hiding their
    /// entities did.
    pub hidden: bool,
}

impl Default for TreeInstancing {
    fn default() -> Self {
        Self {
            enabled: true,
            shadow_lod: 0,
            hidden: false,
        }
    }
}

/// One tree in the pool (`Instance` in `shaders/tree/lod.wesl`), relative to its batch's
/// origin.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct GpuInstance {
    position_yaw: [f32; 4],
    scale: f32,
    form: u32,
    origin: u32,
    flags: u32,
}

/// A live pool slot; free slots draw nothing.
const ALIVE: u32 = 16;

/// The forced representation in an instance's flags: 0 the distance picks, 1 + n
/// representation n, [`FORCED_NOTHING`] none.
const FORCED_MASK: u32 = 15;

const FORCED_NOTHING: u32 = 15;

/// A form's representations at unit scale (`Form` in `shaders/tree/lod.wesl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct GpuForm {
    band_start: [[f32; 4]; 2],
    band_end: [[f32; 4]; 2],
    /// Each representation's buckets: the first and how many (one per primitive).
    first_bucket: [[u32; 4]; 2],
    buckets: [[u32; 4]; 2],
    representations: u32,
    meshes: u32,
    ready: u32,
    height: f32,
}

/// One draw entry's bucket (`Bucket` in `shaders/tree/lod.wesl`): the bounds of what it draws
/// at unit scale in object space, wind included (a box's centre and half extents, or with
/// `centre[3]` 1, a sphere's centre and radius in `extent[0]`), and its records in a view.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct GpuBucket {
    centre: [f32; 4],
    extent: [f32; 4],
    records: u32,
    capacity: u32,
    padding: [u32; 2],
}

/// The compute pass's view of the frame (`Camera` in `shaders/tree/lod.wesl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct GpuCamera {
    eye_dt: [f32; 4],
    planes: [[f32; 4]; 6],
    shadow_planes: [[f32; 4]; 6 * MAX_SHADOW_VIEWS],
    /// Per shadow view, where the main view samples it: the depths that do, as two planes
    /// (near, far: normal and distance), and the direction its light travels (xyz).
    shadow_receivers: [[f32; 4]; 3 * MAX_SHADOW_VIEWS],
    /// Pool length, shadow views, shadow LOD bias, records per view.
    counts: [u32; 4],
    /// Buckets (draw entries) per view, unused, hidden, unused.
    sizes: [u32; 4],
}

/// The instance records, pool, origins and transforms the instanced tree and impostor
/// pipelines read (group 4: `shaders/tree_wind.wesl`, `shaders/tree_impostor.wesl`).
pub(super) fn instance_layout() -> BindGroupLayoutDescriptor {
    use bevy::render::render_resource::binding_types::storage_buffer_read_only_sized;
    BindGroupLayoutDescriptor::new(
        "tree instances",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX,
            (
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
                storage_buffer_read_only_sized(false, None),
            ),
        ),
    )
}

/// The GPU trees draw what object LOD streams (`crate::object_lod`), with its projection.
fn drawing(
    settings: Res<TreeInstancing>,
    lod: Option<Res<crate::object_lod::LodProjection>>,
) -> bool {
    settings.enabled && lod.is_some()
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TreeInstancing>()
        .init_resource::<forms::TreeForms>()
        .init_resource::<pool::TreePool>()
        .init_resource::<layout::TreeDrawLayout>()
        .init_resource::<frame::TreeGpuFrame>()
        .add_observer(pool::release)
        .add_systems(
            PostUpdate,
            (
                forms::register,
                forms::harvest,
                pool::attach,
                pool::follow,
                layout::update,
            )
                .chain()
                .run_if(drawing)
                .after(bevy::transform::TransformSystems::Propagate)
                .after(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
        )
        .add_systems(
            PostUpdate,
            frame::collect
                .run_if(drawing)
                .after(bevy::camera::visibility::VisibilitySystems::CheckVisibility)
                .after(bevy::light::SimulationLightSystems::UpdateLightFrusta)
                .after(bevy::transform::TransformSystems::Propagate),
        )
        .add_systems(Startup, render::spawn_draws);

    if app.get_sub_app(RenderApp).is_none() {
        return;
    }

    let shared = app.world().resource::<pool::TreePool>().shared();
    app.add_plugins((
        ExtractResourcePlugin::<TreeInstancing>::default(),
        ExtractResourcePlugin::<layout::TreeDrawLayout>::default(),
        ExtractResourcePlugin::<frame::TreeGpuFrame>::default(),
        bevy::render::extract_component::ExtractComponentPlugin::<render::TreeDraw>::default(),
    ));
    render::install(app.sub_app_mut(RenderApp), shared);
}

pub(super) fn finish(app: &mut App) {
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render::initialize(render_app.world_mut());
    }
}
