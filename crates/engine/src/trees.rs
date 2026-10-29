//! Trees and shrubs: authored foliage weights deform on the GPU using the shared vegetation
//! wind field ([`wind`]) and part where characters push through them ([`touch`]), shaking off
//! their snow (`gpu::shed`), composed with the environment-lit PBR material and identical colour,
//! depth and shadow geometry, with two-layer bark ([`bark`]) and camera-facing cards
//! ([`cards`]). Every representation of them is drawn on the GPU, LOD choice and fades
//! included ([`gpu`], followed on the CPU by [`timed`]); far away, impostors draw them
//! ([`impostor`]) and give the forest shadows ([`forest_shadow`]). The LOD lab ([`lab`])
//! compares one tree's representations.
mod bark;
mod cards;

#[cfg(test)]
mod depth_tests;

pub(crate) mod forest_shadow;
mod gpu;
pub(crate) mod impostor;
mod lab;
mod material;

#[cfg(test)]
mod scene_tests;

mod timed;
mod touch;
mod tuning;
mod wind;

pub(crate) use cards::with_card_attributes;
pub use gpu::TreeInstancing;
pub(crate) use gpu::{DrawnOnGpu, object_key};

pub use lab::{
    LabAsset, LabBand, LabRepresentation, LabTree, LabVariant, LodLabPlugin, spawn_lab_tree,
};

pub use touch::TreeTouch;
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
        gpu::plugin(app);
        touch::plugin(app);

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

    fn finish(&self, app: &mut App) {
        gpu::finish(app);
    }
}
