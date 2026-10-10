//! Trees and shrubs: authored foliage weights deform on the GPU using the shared vegetation
//! wind field ([`wind`]), composed with the environment-lit PBR material and identical colour,
//! depth and shadow geometry, with two-layer bark ([`bark`]) and camera-facing cards
//! ([`cards`]). The game draws them from one instance buffer ([`instancing`]); far away,
//! impostors draw them ([`impostor`]) and give the forest shadows ([`forest_shadow`]). The LOD
//! lab ([`lab`]) compares one tree's representations.
mod bark;
mod cards;
#[cfg(test)]
mod depth_tests;
pub(crate) mod forest_shadow;
pub(crate) mod impostor;
mod instancing;
mod lab;
mod material;
#[cfg(test)]
mod scene_tests;
mod tuning;
mod wind;
pub use cards::tree_gltf_plugin;
pub use instancing::TreeInstancing;
pub use lab::{
    LabAsset, LabBand, LabRepresentation, LabTree, LabVariant, LodLabPlugin, spawn_lab_tree,
};
pub use tuning::TreeWindTuning;
pub use wind::TreeWindResponse;

use bevy::{
    prelude::*,
    render::{Render, RenderApp, RenderSystems, extract_resource::ExtractResourcePlugin},
};
use wind::{WindBuffer, WindPose};

/// Requires the atmosphere environment material pipeline and VegetationRenderPlugin's clock.
/// Installed explicitly by game/editor composition, independently of world streaming.
pub struct TreeWindPlugin;
/// Wind producers that run in PostUpdate must finish before this snapshot is taken.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TreeWindSystems;
impl Plugin for TreeWindPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TreeWindTuning>()
            .init_resource::<tuning::WindTuningState>()
            .add_systems(PreUpdate, tuning::restore_automatic)
            .init_resource::<TreeWindResponse>()
            .init_resource::<WindPose>()
            .add_systems(
                PostUpdate,
                wind::sample_wind
                    .in_set(TreeWindSystems)
                    .after(bevy::transform::TransformSystems::Propagate),
            );
        instancing::plugin(app);
        if app.get_sub_app(RenderApp).is_none() {
            return;
        }
        app.add_plugins((
            ExtractResourcePlugin::<WindPose>::default(),
            ExtractResourcePlugin::<WindBuffer>::default(),
            material::TreeWindMaterialPlugin,
            bark::TreeBarkPlugin,
        ))
        .add_systems(Startup, wind::setup_buffer);
        app.sub_app_mut(RenderApp).add_systems(
            Render,
            wind::upload_wind.in_set(RenderSystems::PrepareResources),
        );
    }
}
