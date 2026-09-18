use bevy::{
    asset::RenderAssetUsages, mesh::Indices, prelude::*, render::render_resource::PrimitiveTopology,
};
use world::TerrainHeightfield;

/// Fine edges adjacent to a patch exactly one level coarser.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct StitchEdges(pub u8);
impl StitchEdges {
    pub const WEST: u8 = 1;
    pub const EAST: u8 = 2;
    pub const SOUTH: u8 = 4;
    pub const NORTH: u8 = 8;
    pub fn insert(&mut self, edge: u8) {
        self.0 |= edge;
    }
}
/// Collapse odd boundary indices onto canonical even vertices. Degenerate triangles
/// are removed; the retained edge is exactly the neighbour's linear segment.
pub fn stitch_indices(resolution: u16, edges: StitchEdges) -> Result<Vec<u32>, String> {
    if !(3..=257).contains(&resolution) || !(resolution - 1).is_power_of_two() || edges.0 > 15 {
        return Err("invalid stitched terrain grid".into());
    }
    let n = u32::from(resolution);
    let end = n - 1;
    let index = |mut x: u32, mut z: u32| {
        if z % 2 == 1
            && ((x == 0 && edges.0 & StitchEdges::WEST != 0)
                || (x == end && edges.0 & StitchEdges::EAST != 0))
        {
            z -= 1;
        }
        if x % 2 == 1
            && ((z == 0 && edges.0 & StitchEdges::SOUTH != 0)
                || (z == end && edges.0 & StitchEdges::NORTH != 0))
        {
            x -= 1;
        }
        z * n + x
    };
    let mut indices = Vec::with_capacity((end * end * 6) as usize);
    for z in 0..end {
        for x in 0..end {
            let [a, b, c, d] = [
                index(x, z),
                index(x, z + 1),
                index(x + 1, z + 1),
                index(x + 1, z),
            ];
            for triangle in [[a, b, c], [a, c, d]] {
                if triangle[0] != triangle[1]
                    && triangle[1] != triangle[2]
                    && triangle[0] != triangle[2]
                {
                    indices.extend(triangle);
                }
            }
        }
    }
    Ok(indices)
}
/// Simple geometry-pass mesh. No detailed material tangents or skirts are required.
pub fn build_patch_mesh(
    field: &TerrainHeightfield,
    extent: f32,
    edges: StitchEdges,
) -> Result<Mesh, String> {
    field.validate().map_err(|e| e.to_string())?;
    if !extent.is_finite() || extent <= 0.0 {
        return Err("invalid terrain patch extent".into());
    }
    let indices = stitch_indices(field.resolution, edges)?;
    let n = usize::from(field.resolution);
    let mut positions = Vec::with_capacity(n * n);
    let mut normals = Vec::with_capacity(n * n);
    for z in 0..n {
        for x in 0..n {
            positions.push([
                x as f32 / (n - 1) as f32 * extent,
                field.height_at(x, z),
                z as f32 / (n - 1) as f32 * extent,
            ]);
            normals.push(field.normal_at(x, z));
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    Ok(mesh)
}
