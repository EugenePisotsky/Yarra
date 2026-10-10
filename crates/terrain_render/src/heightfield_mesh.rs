//! The page-local raster mesh for a terrain heightfield.
use bevy::{
    asset::RenderAssetUsages, mesh::Indices, prelude::*, render::render_resource::PrimitiveTopology,
};
use world::TerrainHeightfield;

/// Builds the page-local raster mesh from the same height samples used by CPU surface queries.
///
/// X/Z positions are centred around the page entity. Heights remain absolute world-space Y so the
/// entity only needs render-origin translation in X/Z.
pub fn build_heightfield_mesh(
    heightfield: &TerrainHeightfield,
    cell_size: f32,
) -> Result<Mesh, String> {
    heightfield.validate().map_err(|error| error.to_string())?;
    if !cell_size.is_finite() || cell_size <= 0.0 {
        return Err("terrain cell size must be finite and positive".into());
    }

    let resolution = usize::from(heightfield.resolution);
    let intervals = (resolution - 1) as f32;
    let mut positions = Vec::with_capacity(resolution * resolution);
    let mut normals = Vec::with_capacity(resolution * resolution);
    let mut uvs = Vec::with_capacity(resolution * resolution);
    for z in 0..resolution {
        for x in 0..resolution {
            let u = x as f32 / intervals;
            let v = z as f32 / intervals;
            positions.push([
                u * cell_size - cell_size * 0.5,
                heightfield.height_at(x, z),
                v * cell_size - cell_size * 0.5,
            ]);
            normals.push(heightfield.normal_at(x, z));
            uvs.push([u, v]);
        }
    }

    let mut indices = Vec::with_capacity((resolution - 1) * (resolution - 1) * 6);
    for z in 0..resolution - 1 {
        for x in 0..resolution - 1 {
            let lower_left = (z * resolution + x) as u32;
            let lower_right = lower_left + 1;
            let upper_left = lower_left + resolution as u32;
            let upper_right = upper_left + 1;
            indices.extend_from_slice(&[
                lower_left,
                upper_left,
                upper_right,
                lower_left,
                upper_right,
                lower_right,
            ]);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh.generate_tangents()
        .map_err(|error| format!("could not generate terrain tangents: {error}"))?;
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heightfield_mesh_has_expected_topology_and_tangents() {
        let heightfield = TerrainHeightfield::from_heights(
            3,
            &[0.0, 0.5, 1.0, 0.0, 0.5, 1.0, 0.0, 0.5, 1.0],
            0.0,
            1.0,
            8.0,
        )
        .unwrap();
        let mesh = build_heightfield_mesh(&heightfield, 8.0).unwrap();

        assert_eq!(mesh.count_vertices(), 9);
        assert_eq!(mesh.indices().unwrap().len(), 24);
        assert!(mesh.attribute(Mesh::ATTRIBUTE_TANGENT).is_some());
    }
}
