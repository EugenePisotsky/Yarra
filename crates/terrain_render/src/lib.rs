//! The ground: terrain v2's clipmap material ([`clipmap`]), its surfaces and textures
//! ([`ground`]), the prints feet press into it ([`GroundPresses`]), the environment it is lit
//! by, and its albedo for the light it reflects onto everything above it ([`GroundAlbedoSource`]).
pub mod clipmap;
mod environment;
mod ground;
mod ground_albedo;
mod prints;

use bevy::prelude::*;

pub use ground::{TerrainMacroVariation, TerrainSurfaceLayer};

pub use ground_albedo::GroundAlbedoSource;

pub use prints::{GroundPress, GroundPressSystems, GroundPresses, PrintGround};

/// A value's bytes in WGSL storage-buffer layout, as `ShaderBuffer::from` encoded it before
/// Bevy 0.20 (whose `ShaderBuffer` now takes plain-old-data vectors).
pub(crate) fn storage_bytes<T>(value: &T) -> Vec<u8>
where
    T: bevy::render::render_resource::ShaderType
        + bevy::render::render_resource::encase::internal::WriteInto,
{
    let mut buffer = bevy::render::render_resource::encase::StorageBuffer::new(Vec::new());
    buffer.write(value).expect("table fits its storage layout");
    buffer.into_inner()
}

/// Installs terrain v2's clipmap material, ground prints, the ground albedo map's pass and the
/// macro-variation switch.
pub struct TerrainRenderPlugin;

impl Plugin for TerrainRenderPlugin {
    fn build(&self, app: &mut App) {
        clipmap::install(app);
        prints::install(app);
        ground_albedo::install(app);
        app.init_resource::<TerrainMacroVariation>()
            .add_systems(Startup, atmosphere::environment::init_fallback)
            .add_systems(
                PostUpdate,
                environment::sync::<clipmap::ClipmapTerrainMaterial>,
            );
    }
}

#[cfg(test)]
mod gpu_tests;
