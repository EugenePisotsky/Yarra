pub mod clipmap;
mod environment;
mod heightfield_mesh;
mod material;
mod page_material;
// Submodules share these through `use super::*`.
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    pbr::{Material, MaterialPlugin},
    prelude::*,
    reflect::TypePath,
    render::storage::ShaderBuffer,
    shader::ShaderRef,
};
pub use heightfield_mesh::build_heightfield_mesh;
#[cfg(test)]
use material::TERRAIN_SHADER;
pub(crate) use material::TerrainMaterialUniform;
pub use material::{
    TerrainMacroVariation, TerrainMaterial, TerrainMaterialKey, TerrainShadingMode,
};
pub use page_material::{
    PrepareTerrainMaterialContext, PreparedTerrainMaterial, TerrainSurfaceLayer,
    prepare_terrain_material,
};

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

/// Schedule terrain material edits before this set to prepare their current inputs.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerrainMaterialPreparation;

/// Installs terrain v2's clipmap material, the page material the editor previews presets with
/// and the macro-variation switch.
pub struct TerrainRenderPlugin;

impl Plugin for TerrainRenderPlugin {
    fn build(&self, app: &mut App) {
        clipmap::install(app);
        app.init_resource::<TerrainMacroVariation>()
            .add_systems(Startup, atmosphere::environment::init_fallback)
            .add_systems(
                PostUpdate,
                (
                    environment::sync::<TerrainMaterial>,
                    environment::sync::<clipmap::ClipmapTerrainMaterial>,
                ),
            )
            .add_plugins(MaterialPlugin::<TerrainMaterial>::default())
            .add_systems(Update, material::apply_macro_variation);
    }
}

#[cfg(test)]
mod gpu_tests;
