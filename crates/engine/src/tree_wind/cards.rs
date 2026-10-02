//! Selected foliage clusters rotate about authored pivots. `_CARD_FACING` opts into
//! centered camera facing with adjustable elevation follow; older assets keep their
//! branch-axis constraint. Zero axes mark fixed structural cards. All passes share
//! the main camera pose and temporal motion uses its previous pose. Lighting normals
//! remain stable while geometry turns, preserving the current experimental shading.
use bevy::{
    gltf::GltfPlugin,
    mesh::{MeshVertexAttribute, MeshVertexBufferLayoutRef, VertexFormat},
    render::render_resource::{RenderPipelineDescriptor, SpecializedMeshPipelineError},
};

pub const CARD_PIVOT: MeshVertexAttribute =
    MeshVertexAttribute::new("Card_Pivot", 0x5952_5241_4341_0001, VertexFormat::Float32x3);
pub const CARD_AXIS: MeshVertexAttribute =
    MeshVertexAttribute::new("Card_Axis", 0x5952_5241_4341_0002, VertexFormat::Float32x3);
pub const CARD_NORMAL: MeshVertexAttribute = MeshVertexAttribute::new(
    "Card_Normal",
    0x5952_5241_4341_0003,
    VertexFormat::Float32x3,
);
pub const CARD_FACING: MeshVertexAttribute = MeshVertexAttribute::new(
    "Card_Facing",
    0x5952_5241_4341_0004,
    VertexFormat::Float32x2,
);

/// Shader locations after Bevy's own vertex inputs (0–7 in every pass).
const FIRST_LOCATION: u32 = 10;

/// Conservative local-space displacement for culling, including a full half-turn.
pub(super) fn max_card_swing(mesh: &bevy::mesh::Mesh) -> f32 {
    use bevy::{math::Vec3, mesh::VertexAttributeValues};
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(bevy::mesh::Mesh::ATTRIBUTE_POSITION)
    else {
        return 0.;
    };
    let Some(VertexAttributeValues::Float32x3(pivots)) = mesh.attribute(CARD_PIVOT) else {
        return 0.;
    };
    let Some(VertexAttributeValues::Float32x3(axes)) = mesh.attribute(CARD_AXIS) else {
        return 0.;
    };
    let settings = match mesh.attribute(CARD_FACING) {
        Some(VertexAttributeValues::Float32x2(v)) => Some(v),
        _ => None,
    };
    positions
        .iter()
        .zip(pivots)
        .zip(axes)
        .enumerate()
        .map(|(i, ((p, pivot), axis))| {
            let axis = Vec3::from_array(*axis);
            if axis.length_squared() < 0.25 {
                return 0.;
            }
            let d = Vec3::from_array(*p) - Vec3::from_array(*pivot);
            let radius = if settings.is_some_and(|s| s[i][0] > 0.5) {
                d.length()
            } else {
                let axis = axis.normalize();
                (d - axis * d.dot(axis)).length()
            };
            radius * 2.
        })
        .fold(0., f32::max)
}

/// Bevy's glTF loader with the branch card attributes registered. Applications install it
/// in place of the default `GltfPlugin`. The files name them `_CARD_PIVOT` and so on;
/// gltf-json 1.4 strips the underscore before Bevy looks the name up.
pub fn tree_gltf_plugin() -> GltfPlugin {
    GltfPlugin::default()
        .add_custom_vertex_attribute("CARD_PIVOT", CARD_PIVOT)
        .add_custom_vertex_attribute("CARD_AXIS", CARD_AXIS)
        .add_custom_vertex_attribute("CARD_NORMAL", CARD_NORMAL)
        .add_custom_vertex_attribute("CARD_FACING", CARD_FACING)
}

/// Adds the card attributes to the pipeline's vertex buffer when the mesh has them.
pub(super) fn specialize(
    descriptor: &mut RenderPipelineDescriptor,
    layout: &MeshVertexBufferLayoutRef,
) -> Result<(), SpecializedMeshPipelineError> {
    let mesh = &layout.0;
    if !(mesh.contains(CARD_PIVOT) && mesh.contains(CARD_AXIS) && mesh.contains(CARD_NORMAL)) {
        return Ok(());
    }
    let cards = mesh.get_layout(&[
        CARD_PIVOT.at_shader_location(FIRST_LOCATION),
        CARD_AXIS.at_shader_location(FIRST_LOCATION + 1),
        CARD_NORMAL.at_shader_location(FIRST_LOCATION + 2),
    ])?;
    let Some(buffer) = descriptor.vertex.buffers.first_mut() else {
        return Ok(());
    };
    buffer.attributes.extend(cards.attributes);
    if mesh.contains(CARD_FACING) {
        buffer.attributes.extend(
            mesh.get_layout(&[CARD_FACING.at_shader_location(FIRST_LOCATION + 3)])?
                .attributes,
        );
        descriptor
            .vertex
            .shader_defs
            .push("TREE_CARD_FACING".into());
    }
    descriptor
        .vertex
        .shader_defs
        .push("TREE_BRANCH_CARDS".into());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::RenderAssetUsages,
        mesh::{Mesh, MeshVertexBufferLayouts, PrimitiveTopology},
        render::render_resource::{VertexState, VertexStepMode},
    };

    fn descriptor(mesh: &Mesh) -> (RenderPipelineDescriptor, MeshVertexBufferLayoutRef) {
        let layout = mesh.get_mesh_vertex_buffer_layout(&mut MeshVertexBufferLayouts::default());
        let base = layout
            .0
            .get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])
            .unwrap();
        let descriptor = RenderPipelineDescriptor {
            vertex: VertexState {
                buffers: vec![base],
                ..Default::default()
            },
            ..Default::default()
        };
        (descriptor, layout)
    }

    fn mesh(cards: bool) -> Mesh {
        let mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 3]);
        if !cards {
            return mesh;
        }
        mesh.with_inserted_attribute(CARD_PIVOT, vec![[0.0f32; 3]; 3])
            .with_inserted_attribute(CARD_AXIS, vec![[0.0f32, 1.0, 0.0]; 3])
            .with_inserted_attribute(CARD_NORMAL, vec![[0.0f32, 0.0, 1.0]; 3])
    }

    #[test]
    fn card_attributes_join_the_vertex_buffer_only_when_the_mesh_has_them() {
        let (mut plain, layout) = descriptor(&mesh(false));
        specialize(&mut plain, &layout).unwrap();
        assert_eq!(plain.vertex.buffers[0].attributes.len(), 1);
        assert!(plain.vertex.shader_defs.is_empty());

        let (mut cards, layout) = descriptor(&mesh(true));
        specialize(&mut cards, &layout).unwrap();
        let buffer = &cards.vertex.buffers[0];
        assert_eq!(buffer.step_mode, VertexStepMode::Vertex);
        let locations: Vec<_> = buffer
            .attributes
            .iter()
            .map(|a| a.shader_location)
            .collect();
        assert_eq!(locations, [0, 10, 11, 12]);
        // Offsets index the mesh's interleaved vertex, so every attribute is distinct.
        let mut offsets: Vec<_> = buffer.attributes.iter().map(|a| a.offset).collect();
        offsets.sort();
        offsets.dedup();
        assert_eq!(offsets.len(), 4);
        assert_eq!(cards.vertex.shader_defs.len(), 1);

        let centered = mesh(true).with_inserted_attribute(CARD_FACING, vec![[1.0f32, 1.0]; 3]);
        let (mut desc, layout) = descriptor(&centered);
        specialize(&mut desc, &layout).unwrap();
        assert_eq!(
            desc.vertex.buffers[0]
                .attributes
                .last()
                .unwrap()
                .shader_location,
            13
        );
        assert_eq!(desc.vertex.shader_defs.len(), 2);
    }

    #[test]
    fn bounds_cover_centered_pitch_and_legacy_axis_sweep() {
        let m =
            mesh(true).with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[1.0f32, 2., 0.]; 3]);
        assert!((max_card_swing(&m) - 2.).abs() < 1e-5);
        let m = m.with_inserted_attribute(CARD_FACING, vec![[1.0f32, 1.0]; 3]);
        assert!((max_card_swing(&m) - 2. * 5.0f32.sqrt()).abs() < 1e-5);
        let m = m.with_inserted_attribute(CARD_AXIS, vec![[0.0f32; 3]; 3]);
        assert_eq!(max_card_swing(&m), 0.);
    }
}
