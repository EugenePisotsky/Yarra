//! The glTF loader Yarra's applications install in place of Bevy's default.

use bevy::gltf::GltfPlugin;

/// Bevy's glTF loader with the vertex attributes Yarra's models carry registered: the trees'
/// branch cards and the characters' source vertices.
pub fn gltf_plugin() -> GltfPlugin {
    crate::trees::with_card_attributes(GltfPlugin::default())
        .add_custom_vertex_attribute("SOURCE_VERTEX", crate::character_kit::SOURCE_VERTEX)
}
