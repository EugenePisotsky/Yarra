//! A common refinement of two covers, with one shared morph weight. Integer grid
//! collapse reproduces each endpoint's triangles, including stitched boundaries.
use super::{StitchEdges, contains, stitch_indices};
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, morph::MorphAttributes},
    prelude::*,
    render::render_resource::PrimitiveTopology,
};
use std::collections::{BTreeMap, BTreeSet};
use world::{TerrainHeightfield, TerrainNodeKey};

/// The finer partition of two non-overlapping hierarchy covers of the same domain.
/// The caller charges this transient cover against its own patch/triangle budgets.
pub fn common_cover(
    old: &BTreeMap<TerrainNodeKey, StitchEdges>,
    new: &BTreeMap<TerrainNodeKey, StitchEdges>,
) -> Result<Vec<TerrainNodeKey>, String> {
    for &key in old.keys().chain(new.keys()) {
        key.cell_bounds().map_err(|e| e.to_string())?;
    }
    for (a, b) in [(old, new), (new, old)] {
        for &key in a.keys() {
            if !b.keys().any(|&other| contains(other, key)) {
                let covered: u128 = b
                    .keys()
                    .filter(|&&other| contains(key, other))
                    .map(|other| 1_u128 << (2 * other.level))
                    .sum();
                if covered != 1_u128 << (2 * key.level) {
                    return Err("terrain morph covers have different domains".into());
                }
            }
        }
    }
    let keys: BTreeSet<_> = old.keys().chain(new.keys()).copied().collect();
    Ok(keys
        .iter()
        .filter(|&&k| !keys.iter().any(|&other| other != k && contains(k, other)))
        .copied()
        .collect())
}

fn rect(key: TerrainNodeKey) -> [i64; 4] {
    let span = 1_i64 << key.level;
    [
        key.x as i64 * span,
        key.z as i64 * span,
        (key.x as i64 + 1) * span,
        (key.z as i64 + 1) * span,
    ]
}
/// Closed bounds: neighbouring patches contribute the canonical shared edge/corner.
pub fn touches(a: TerrainNodeKey, b: TerrainNodeKey) -> bool {
    let [ax, az, bx, bz] = rect(a);
    let [cx, cz, dx, dz] = rect(b);
    a.space == b.space && ax <= dx && cx <= bx && az <= dz && cz <= bz
}

#[derive(Clone)]
pub struct MorphSource {
    pub key: TerrainNodeKey,
    pub edges: StitchEdges,
    pub field: TerrainHeightfield,
}
pub struct MorphMesh {
    pub mesh: Mesh,
    /// Union of both poses, in patch-local coordinates; base-mesh bounds are insufficient.
    pub bounds: [Vec3; 2],
}
impl MorphMesh {
    pub fn bytes_estimate(resolution: u16) -> u64 {
        // Position + normal + one padded Bevy MorphAttributes + regular indices.
        u64::from(resolution).pow(2) * 72 + u64::from(resolution - 1).pow(2) * 24
    }
}

/// All sources touching this patch in each endpoint cover must be provided. Grids
/// share a world resolution and nested samples. Normals are canonical vertex normals.
pub fn build_morph_mesh(
    key: TerrainNodeKey,
    cell_size: f32,
    old: &[MorphSource],
    new: &[MorphSource],
) -> Result<MorphMesh, String> {
    key.cell_bounds().map_err(|e| e.to_string())?;
    let resolution = old
        .first()
        .ok_or("empty terrain morph source")?
        .field
        .resolution;
    if !cell_size.is_finite() || cell_size <= 0.0 {
        return Err("invalid terrain morph cell size".into());
    }
    let indices = stitch_indices(resolution, StitchEdges::default())?;
    for source in old.iter().chain(new) {
        source.key.cell_bounds().map_err(|e| e.to_string())?;
        source.field.validate().map_err(|e| e.to_string())?;
        if source.key.space != key.space
            || source.field.resolution != resolution
            || source.edges.0 > 15
        {
            return Err("incompatible terrain morph grid".into());
        }
    }
    let intervals = i64::from(resolution - 1);
    let grid_origin = [rect(key)[0] * intervals, rect(key)[1] * intervals];
    let step = 1_i64 << key.level;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut deltas = Vec::new();
    let mut bounds = [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)];
    for z in 0..=intervals {
        for x in 0..=intervals {
            let point = [grid_origin[0] + x * step, grid_origin[1] + z * step];
            let a = endpoint(point, grid_origin, intervals, cell_size, old)?;
            let b = endpoint(point, grid_origin, intervals, cell_size, new)?;
            positions.push(a.0.to_array());
            normals.push(a.1.to_array());
            deltas.push(MorphAttributes::new(b.0 - a.0, b.1 - a.1, Vec3::ZERO));
            bounds[0] = bounds[0].min(a.0).min(b.0);
            bounds[1] = bounds[1].max(a.0).max(b.0);
        }
    }
    let mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_indices(Indices::U32(indices))
    .with_morph_targets(deltas);
    Ok(MorphMesh { mesh, bounds })
}

fn endpoint(
    point: [i64; 2],
    origin: [i64; 2],
    intervals: i64,
    cell_size: f32,
    sources: &[MorphSource],
) -> Result<(Vec3, Vec3), String> {
    // Coarsest closed-boundary owner wins on both sides of an edge. Stable keys
    // break ties at corners; its retained samples are identical in either owner.
    let source = sources
        .iter()
        .filter(|s| {
            let [x, z, end_x, end_z] = rect(s.key);
            point[0] >= x * intervals
                && point[0] <= end_x * intervals
                && point[1] >= z * intervals
                && point[1] <= end_z * intervals
        })
        .max_by_key(|s| (s.key.level, s.key))
        .ok_or("terrain morph coverage hole")?;
    let [min_x, min_z, _, _] = rect(source.key);
    let step = 1_i64 << source.key.level;
    let mut x = (point[0] - min_x * intervals).div_euclid(step);
    let mut z = (point[1] - min_z * intervals).div_euclid(step);
    if z % 2 == 1
        && ((x == 0 && source.edges.0 & StitchEdges::WEST != 0)
            || (x == intervals && source.edges.0 & StitchEdges::EAST != 0))
    {
        z -= 1;
    }
    if x % 2 == 1
        && ((z == 0 && source.edges.0 & StitchEdges::SOUTH != 0)
            || (z == intervals && source.edges.0 & StitchEdges::NORTH != 0))
    {
        x -= 1;
    }
    let position = Vec3::new(
        ((min_x * intervals + x * step - origin[0]) as f64 * f64::from(cell_size)
            / intervals as f64) as f32,
        source.field.height_at(x as usize, z as usize),
        ((min_z * intervals + z * step - origin[1]) as f64 * f64::from(cell_size)
            / intervals as f64) as f32,
    );
    Ok((
        position,
        Vec3::from_array(source.field.normal_at(x as usize, z as usize)),
    ))
}

#[cfg(test)]
mod tests;
