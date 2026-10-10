//! Performance roadmap, steps 1, 3 and 4 (docs/REFACTORING.md): the game draws trees from one
//! instance buffer instead of one render entity per mesh (the editor still draws entities).
//! The tree entities stay, so LOD choice and fades still run through them, but they move to render
//! layer 1, which no camera or light sees, so Bevy no longer extracts, culls or queues them.
//! Each frame the meshes visible in the main view, and for every shadow cascade the meshes of
//! the LOD each tree casts from (`TreeInstancing::shadow_lod` steps coarser than the one drawn),
//! are gathered into one buffer, grouped by mesh and material, and each group is drawn with
//! one instanced draw through the tree material's own pipelines, specialized with
//! `TREE_INSTANCED` so they read transforms and fades from the buffer instead of Bevy's mesh
//! uniforms (`shaders/tree_wind.wesl`, `shaders/lighting/material.wesl`).
use super::material::TreeWindMaterial;
use bevy::{
    camera::visibility::VisibilitySystems,
    core_pipeline::{
        core_3d::{AlphaMask3d, Opaque3d},
        prepass::{AlphaMask3dPrepass, Opaque3dPrepass},
    },
    light::SimulationLightSystems,
    pbr::Shadow,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_phase::AddRenderCommand,
        render_resource::{binding_types::storage_buffer_read_only_sized, *},
        view::RetainedViewEntity,
    },
};
use bytemuck::{Pod, Zeroable};
use std::{collections::HashMap, sync::Arc};

mod gather;
mod render;

/// Render layer the instanced trees' entities move to, which no camera or light sees.
const TREE_LAYER: usize = 1;
/// Draw entities, one per group of instances drawn together (form, LOD and primitive) in a
/// view; every view numbers its groups from 0.
const DRAWS: usize = 512;

/// Whether trees are drawn from the instance buffer (the default, as the game does) or as
/// entities (the editor opts out), fixed at startup.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeInstancing {
    pub enabled: bool,
    /// How many LODs coarser than the drawn one a tree casts its shadow from (at most its last
    /// mesh LOD). 0 matches the entities' shadows; 1 saves about a third of the cascades' time
    /// but the coarser crowns let more light through, and the forest reads lighter.
    pub shadow_lod: usize,
}
impl Default for TreeInstancing {
    fn default() -> Self {
        Self {
            enabled: true,
            shadow_lod: 0,
        }
    }
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
            gather::collect
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
        .init_resource::<render::TreeInstanceGpu>()
        .add_render_command::<Opaque3d, render::DrawTreeInstances>()
        .add_render_command::<AlphaMask3d, render::DrawTreeInstances>()
        .add_render_command::<Opaque3dPrepass, render::DrawTreeDepth>()
        .add_render_command::<AlphaMask3dPrepass, render::DrawTreeDepth>()
        .add_render_command::<Shadow, render::DrawTreeDepth>()
        .add_systems(
            Render,
            (
                (render::queue, render::queue_shadows).in_set(RenderSystems::Queue),
                render::prepare.in_set(RenderSystems::PrepareBindGroups),
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

#[cfg(test)]
mod tests;
