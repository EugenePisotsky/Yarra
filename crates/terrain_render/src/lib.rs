pub mod composite;
mod environment;
mod heightfield_mesh;
pub mod lod;
mod material;
pub mod near;
mod page_material;
// Submodules share these through `use super::*`.
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{
        AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
        TextureDimension, TextureFormat,
    },
    render::storage::ShaderBuffer,
    shader::ShaderRef,
};
pub use composite::TerrainCompositeMaterial;
pub use heightfield_mesh::build_heightfield_mesh;
#[cfg(test)]
use material::TERRAIN_SHADER;
pub(crate) use material::{TerrainCanopyShading, TerrainMaterialUniform};
pub use material::{
    TerrainMacroVariation, TerrainMaterial, TerrainMaterialKey, TerrainShadingMode,
};
pub(crate) use page_material::make_weight_image;
pub use page_material::{
    PrepareTerrainMaterialContext, PreparedTerrainMaterial, TerrainSurfaceLayer,
    prepare_terrain_material,
};
use world::{CellCoord, TerrainProfile, TerrainSurfaceId, TerrainTextureSet, TerrainWeightPage};

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

/// Installs the terrain materials (near pages and the far composite), their prepared and
/// anti-tiling caches, near detail and the macro-variation switch. A page blends one or two of
/// the world's eight authored surface slots.
pub struct TerrainRenderPlugin;

impl Plugin for TerrainRenderPlugin {
    fn build(&self, app: &mut App) {
        composite::atlas::install(app);
        near::install(app);
        app.init_resource::<TerrainMacroVariation>()
            .add_systems(Startup, atmosphere::environment::init_fallback)
            .add_systems(
                PostUpdate,
                (
                    environment::sync::<TerrainMaterial>,
                    environment::sync::<TerrainCompositeMaterial>,
                ),
            )
            .add_plugins(MaterialPlugin::<TerrainMaterial>::default())
            .add_plugins(MaterialPlugin::<TerrainCompositeMaterial>::default())
            .add_systems(Update, material::apply_macro_variation);
    }
}

#[cfg(test)]
mod gpu_tests;
