//! Camera-independent terrain topology. Heights are finalized before entering this hierarchy.
use crate::{CellCoord, TerrainHeightfield, WorldSpaceId};
use serde::{Deserialize, Serialize};

pub const MAX_TERRAIN_NODE_LEVEL: u8 = 30;
pub const MAX_TERRAIN_NODE_BYTES: usize = 1024 * 1024;
pub const TERRAIN_NODE_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TerrainNodeKey {
    pub space: WorldSpaceId,
    pub level: u8,
    pub x: i32,
    pub z: i32,
}

impl TerrainNodeKey {
    pub fn leaf(space: WorldSpaceId, cell: CellCoord) -> Self {
        Self {
            space,
            level: 0,
            x: cell.x,
            z: cell.z,
        }
    }

    /// Inclusive source-cell bounds, checked before any conversion back to i32.
    pub fn cell_bounds(self) -> Result<[CellCoord; 2], TerrainHierarchyError> {
        if self.level > MAX_TERRAIN_NODE_LEVEL {
            return Err(invalid("node level"));
        }
        let span = 1_i64 << self.level;
        let x = i64::from(self.x) * span;
        let z = i64::from(self.z) * span;
        let convert = |x, z| -> Result<CellCoord, TerrainHierarchyError> {
            Ok(CellCoord {
                x: i32::try_from(x).map_err(|_| invalid("node coordinate overflow"))?,
                z: i32::try_from(z).map_err(|_| invalid("node coordinate overflow"))?,
            })
        };
        Ok([convert(x, z)?, convert(x + span - 1, z + span - 1)?])
    }

    pub fn parent(self) -> Result<Option<Self>, TerrainHierarchyError> {
        self.cell_bounds()?;
        if self.level == MAX_TERRAIN_NODE_LEVEL {
            return Ok(None);
        }
        let parent = Self {
            level: self.level + 1,
            x: self.x.div_euclid(2),
            z: self.z.div_euclid(2),
            ..self
        };
        parent.cell_bounds()?;
        Ok(Some(parent))
    }

    /// Child order: lower-left, lower-right, upper-left, upper-right in XZ.
    pub fn children(self) -> Result<Option<[Self; 4]>, TerrainHierarchyError> {
        self.cell_bounds()?;
        if self.level == 0 {
            return Ok(None);
        }
        Ok(Some(std::array::from_fn(|index| Self {
            level: self.level - 1,
            x: self.x * 2 + (index % 2) as i32,
            z: self.z * 2 + (index / 2) as i32,
            ..self
        })))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainNode {
    pub key: TerrainNodeKey,
    pub child_mask: u8,
    /// Includes every descendant, including peaks absent from the coarse samples.
    pub height_bounds: [f32; 2],
    pub geometric_error: f32,
    /// Partial nodes contain traversal metadata only, never invented ground across missing cells.
    pub heightfield: Option<TerrainHeightfield>,
}

impl TerrainNode {
    pub fn leaf(
        key: TerrainNodeKey,
        field: &TerrainHeightfield,
        resolution: u16,
    ) -> Result<Self, TerrainHierarchyError> {
        if key.level != 0 {
            return Err(invalid("leaf level"));
        }
        field.validate().map_err(|_| invalid("leaf samples"))?;
        validate_resolution(resolution)?;
        validate_resolution(field.resolution)?;
        if resolution < field.resolution {
            return Err(invalid("leaf resampling would discard detail"));
        }
        let field = if resolution == field.resolution {
            field.clone()
        } else {
            let n = usize::from(resolution);
            let source_intervals = usize::from(field.resolution - 1);
            let intervals = n - 1;
            let bounds = field.height_bounds();
            let mut heights = Vec::with_capacity(n * n);
            let mut normals = Vec::with_capacity(n * n);
            for z in 0..n {
                for x in 0..n {
                    let sample = field.sample(
                        [x as f32 / intervals as f32, z as f32 / intervals as f32],
                        1.0,
                    );
                    heights.push(sample.height.clamp(bounds[0], bounds[1]));
                    normals.push(sample.normal);
                }
            }
            let mut result = TerrainHeightfield::from_heights_and_normals(
                resolution, &heights, &normals, bounds[0], bounds[1],
            )
            .map_err(|_| invalid("resampled leaf"))?;
            // Preserve original endpoints/normal codes exactly instead of decode/encode drift.
            for z in 0..=source_intervals {
                for x in 0..=source_intervals {
                    let target =
                        z * (intervals / source_intervals) * n + x * (intervals / source_intervals);
                    let source = z * usize::from(field.resolution) + x;
                    result.heights[target] = field.heights[source];
                    result.normals_oct[target] = field.normals_oct[source];
                }
            }
            result
        };
        let node = Self {
            key,
            child_mask: 0,
            height_bounds: field.height_bounds(),
            geometric_error: 0.0,
            heightfield: Some(field),
        };
        node.validate()?;
        Ok(node)
    }

    pub fn parent(
        key: TerrainNodeKey,
        children: [Option<&Self>; 4],
    ) -> Result<Self, TerrainHierarchyError> {
        let keys = key.children()?.ok_or(invalid("parent level"))?;
        let mut mask = 0;
        let mut bounds = [f32::INFINITY, f32::NEG_INFINITY];
        for (i, child) in children.iter().enumerate() {
            if let Some(child) = child {
                child.validate()?;
                if child.key != keys[i] {
                    return Err(invalid("child address or ordering"));
                }
                mask |= 1 << i;
                bounds[0] = bounds[0].min(child.height_bounds[0]);
                bounds[1] = bounds[1].max(child.height_bounds[1]);
            }
        }
        if mask == 0 {
            return Err(invalid("empty parent"));
        }
        let mut node = Self {
            key,
            child_mask: mask,
            height_bounds: bounds,
            geometric_error: 0.0,
            heightfield: None,
        };
        let Some(fields) = children
            .map(|child| child.and_then(|c| c.heightfield.as_ref()))
            .into_iter()
            .collect::<Option<Vec<_>>>()
        else {
            return Ok(node);
        };
        let n = usize::from(fields[0].resolution);
        if fields.iter().any(|f| usize::from(f.resolution) != n) {
            return Err(invalid("inconsistent hierarchy resolution"));
        }
        for (a, b, horizontal) in [(0, 1, true), (2, 3, true), (0, 2, false), (1, 3, false)] {
            for i in 0..n {
                let (left, right) = if horizontal {
                    (i * n + n - 1, i * n)
                } else {
                    ((n - 1) * n + i, i)
                };
                if fields[a].heights[left].to_bits() != fields[b].heights[right].to_bits()
                    || fields[a].normals_oct[left] != fields[b].normals_oct[right]
                {
                    return Err(invalid("child borders disagree"));
                }
            }
        }
        let mut heights = Vec::with_capacity(n * n);
        let mut normals_oct = Vec::with_capacity(n * n);
        for z in 0..n {
            for x in 0..n {
                let cx = usize::from(x * 2 >= n - 1);
                let cz = usize::from(z * 2 >= n - 1);
                let sample = (z * 2 - cz * (n - 1)) * n + x * 2 - cx * (n - 1);
                let field = fields[cz * 2 + cx];
                heights.push(field.heights[sample]);
                // Retain canonical normals for now; all same-level neighbours share their codes.
                normals_oct.push(field.normals_oct[sample]);
            }
        }
        let field = TerrainHeightfield {
            resolution: n as u16,
            heights,
            normals_oct,
        };
        let mut error = 0.0_f64;
        for (i, child) in children.into_iter().enumerate() {
            let child = child.unwrap();
            let mut deviation = 0.0_f64;
            for z in 0..n {
                for x in 0..n {
                    let parent_x = ((i % 2) * (n - 1) + x) as f64 * 0.5;
                    let parent_z = ((i / 2) * (n - 1) + z) as f64 * 0.5;
                    deviation = deviation.max(
                        (f64::from(fields[i].height_at(x, z))
                            - sample_height_f64(&field, parent_x, parent_z))
                        .abs(),
                    );
                }
            }
            error = error.max(f64::from(child.geometric_error) + deviation);
        }
        let rounded = error as f32;
        node.geometric_error = if f64::from(rounded) < error {
            f32::from_bits(rounded.to_bits() + 1)
        } else {
            rounded
        };
        node.heightfield = Some(field);
        node.validate()?;
        Ok(node)
    }

    pub fn validate(&self) -> Result<(), TerrainHierarchyError> {
        self.key.cell_bounds()?;
        if !self.height_bounds.iter().all(|v| v.is_finite())
            || self.height_bounds[0] > self.height_bounds[1]
            || !self.geometric_error.is_finite()
            || self.geometric_error < 0.0
            || self.child_mask > 15
            || (self.key.level == 0) != (self.child_mask == 0)
        {
            return Err(invalid("node descriptor"));
        }
        if let Some(field) = &self.heightfield {
            field.validate().map_err(|_| invalid("node heightfield"))?;
            validate_resolution(field.resolution)?;
            let bounds = field.height_bounds();
            if (self.key.level != 0 && self.child_mask != 15)
                || bounds[0] < self.height_bounds[0]
                || bounds[1] > self.height_bounds[1]
                || (self.key.level == 0
                    && (self.geometric_error != 0.0 || bounds != self.height_bounds))
            {
                return Err(invalid("heightfield disagrees with descriptor"));
            }
        } else if self.key.level == 0 || self.geometric_error != 0.0 {
            return Err(invalid("invalid partial node"));
        }
        Ok(())
    }

    pub fn gpu_bytes_estimate(&self) -> u64 {
        self.heightfield.as_ref().map_or(0, |field| {
            let n = u64::from(field.resolution);
            // position, normal, UV, tangent plus u32 indices; conservative private topology.
            n * n * 48 + (n - 1) * (n - 1) * 6 * 4
        })
    }
}

fn sample_height_f64(field: &TerrainHeightfield, x: f64, z: f64) -> f64 {
    let n = usize::from(field.resolution);
    let x0 = (x.floor() as usize).min(n - 2);
    let z0 = (z.floor() as usize).min(n - 2);
    let x = x - x0 as f64;
    let z = z - z0 as f64;
    let h = |dx, dz| f64::from(field.height_at(x0 + dx, z0 + dz));
    if z >= x {
        h(0, 0) * (1.0 - z) + h(0, 1) * (z - x) + h(1, 1) * x
    } else {
        h(0, 0) * (1.0 - x) + h(1, 0) * (x - z) + h(1, 1) * z
    }
}

fn validate_resolution(n: u16) -> Result<(), TerrainHierarchyError> {
    if !(2..=crate::MAX_TERRAIN_HEIGHTFIELD_RESOLUTION).contains(&n) || !(n - 1).is_power_of_two() {
        return Err(invalid("hierarchy grid must have 2^k+1 samples"));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum TerrainHierarchyError {
    #[error("invalid terrain hierarchy: {0}")]
    Invalid(&'static str),
    #[error("terrain hierarchy codec: {0}")]
    Codec(String),
}
fn invalid(message: &'static str) -> TerrainHierarchyError {
    TerrainHierarchyError::Invalid(message)
}

pub fn encode_terrain_node(node: &TerrainNode) -> Result<Vec<u8>, TerrainHierarchyError> {
    node.validate()?;
    bincode::serde::encode_to_vec(
        (TERRAIN_NODE_VERSION, node),
        bincode::config::standard()
            .with_little_endian()
            .with_fixed_int_encoding(),
    )
    .map_err(|e| TerrainHierarchyError::Codec(e.to_string()))
}
pub fn decode_terrain_node(bytes: &[u8]) -> Result<TerrainNode, TerrainHierarchyError> {
    if bytes.len() > MAX_TERRAIN_NODE_BYTES
        || bytes.len() < 2
        || u16::from_le_bytes([bytes[0], bytes[1]]) != TERRAIN_NODE_VERSION
    {
        return Err(invalid("node size or version"));
    }
    let ((_, node), consumed): ((u16, TerrainNode), usize) = bincode::serde::decode_from_slice(
        bytes,
        bincode::config::standard()
            .with_little_endian()
            .with_fixed_int_encoding()
            .with_limit::<MAX_TERRAIN_NODE_BYTES>(),
    )
    .map_err(|e| TerrainHierarchyError::Codec(e.to_string()))?;
    if consumed != bytes.len() {
        return Err(invalid("node trailing bytes"));
    }
    node.validate()?;
    Ok(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn leaf(x: i32, z: i32, peak: bool) -> TerrainNode {
        let heights: Vec<_> = (0..5)
            .flat_map(|j| (0..5).map(move |i| if peak && i == 1 && j == 1 { 50.0 } else { 0.0 }))
            .collect();
        let field = TerrainHeightfield::from_heights_and_normals(
            5,
            &heights,
            &[[0.0, 1.0, 0.0]; 25],
            -100.0,
            100.0,
        )
        .unwrap();
        TerrainNode::leaf(
            TerrainNodeKey::leaf(WorldSpaceId(1), CellCoord { x, z }),
            &field,
            5,
        )
        .unwrap()
    }
    #[test]
    fn flat_leaf_expansion_preserves_nonbinary_heights_and_original_normals() {
        let heights = [1500.01; 4];
        let field = TerrainHeightfield::from_heights(2, &heights, -500.0, 2500.0, 8.0).unwrap();
        let node = TerrainNode::leaf(
            TerrainNodeKey::leaf(WorldSpaceId(1), CellCoord::ZERO),
            &field,
            33,
        )
        .unwrap();
        let expanded = node.heightfield.unwrap();
        assert!(
            expanded
                .heights
                .iter()
                .all(|&h| h.to_bits() == heights[0].to_bits())
        );
        assert!(
            expanded
                .normals_oct
                .iter()
                .all(|&n| n == field.normals_oct[0])
        );
        assert!(TerrainNode::leaf(node.key, &expanded, 17).is_err());
        assert!(TerrainNode::leaf(node.key, &field, 34).is_err());
    }
    #[test]
    fn addresses_use_euclidean_parents_and_reject_overflow() {
        let key = TerrainNodeKey::leaf(WorldSpaceId(1), CellCoord { x: -3, z: -1 });
        let parent = key.parent().unwrap().unwrap();
        assert_eq!((parent.x, parent.z), (-2, -1));
        assert_eq!(
            parent.cell_bounds().unwrap(),
            [CellCoord { x: -4, z: -2 }, CellCoord { x: -3, z: -1 }]
        );
        assert_eq!(parent.children().unwrap().unwrap()[3], key);
        assert!(TerrainNodeKey { level: 31, ..key }.cell_bounds().is_err());
        assert!(
            TerrainNodeKey {
                level: 2,
                x: i32::MAX,
                ..key
            }
            .cell_bounds()
            .is_err()
        );
        assert!(
            TerrainNodeKey {
                level: 30,
                x: -2,
                z: 1,
                ..key
            }
            .parent()
            .unwrap()
            .is_none()
        );
    }
    #[test]
    fn skipped_peak_stays_in_bounds_and_error() {
        let children = [
            leaf(0, 0, true),
            leaf(1, 0, false),
            leaf(0, 1, false),
            leaf(1, 1, false),
        ];
        let parent = TerrainNode::parent(
            children[0].key.parent().unwrap().unwrap(),
            children.each_ref().map(Some),
        )
        .unwrap();
        assert!(
            parent
                .heightfield
                .as_ref()
                .unwrap()
                .heights
                .iter()
                .all(|&h| h == 0.0)
        );
        assert_eq!(parent.height_bounds, [0.0, 50.0]);
        assert_eq!(parent.geometric_error, 50.0);
        for (i, child) in children.iter().enumerate() {
            for z in 0..21 {
                for x in 0..21 {
                    let local = [x as f32 / 20.0, z as f32 / 20.0];
                    let fine = child
                        .heightfield
                        .as_ref()
                        .unwrap()
                        .sample(local, 1.0)
                        .height;
                    let coarse = parent
                        .heightfield
                        .as_ref()
                        .unwrap()
                        .sample(
                            [
                                (local[0] + (i % 2) as f32) * 0.5,
                                (local[1] + (i / 2) as f32) * 0.5,
                            ],
                            1.0,
                        )
                        .height;
                    assert!((fine - coarse).abs() <= parent.geometric_error);
                }
            }
        }
        let mut grandchildren = [
            parent.clone(),
            parent.clone(),
            parent.clone(),
            parent.clone(),
        ];
        for (i, c) in grandchildren.iter_mut().enumerate() {
            c.key.x = (i % 2) as i32;
            c.key.z = (i / 2) as i32;
        }
        let grandparent = TerrainNode::parent(
            parent.key.parent().unwrap().unwrap(),
            grandchildren.each_ref().map(Some),
        )
        .unwrap();
        assert_eq!(grandparent.geometric_error, 50.0);
    }
    #[test]
    fn sparse_parent_never_invents_ground_and_rejects_bad_children() {
        let child = leaf(-2, 0, false);
        let key = child.key.parent().unwrap().unwrap();
        let partial = TerrainNode::parent(key, [Some(&child), None, None, None]).unwrap();
        assert!(partial.heightfield.is_none());
        assert_eq!(partial.child_mask, 1);
        assert!(TerrainNode::parent(key, [None, Some(&child), None, None]).is_err());
        let mut children = [
            leaf(0, 0, false),
            leaf(1, 0, false),
            leaf(0, 1, false),
            leaf(1, 1, false),
        ];
        children[1].heightfield.as_mut().unwrap().heights[0] = 1.0;
        children[1].height_bounds[1] = 1.0;
        assert!(
            TerrainNode::parent(
                children[0].key.parent().unwrap().unwrap(),
                children.each_ref().map(Some)
            )
            .is_err()
        );
    }
    #[test]
    fn codec_rejects_version_trailing_and_nonfinite_data() {
        let node = leaf(0, 0, true);
        let bytes = encode_terrain_node(&node).unwrap();
        assert_eq!(decode_terrain_node(&bytes).unwrap(), node);
        let mut bad = bytes.clone();
        bad[0] = 0;
        assert!(decode_terrain_node(&bad).is_err());
        let mut bad = bytes;
        bad.push(0);
        assert!(decode_terrain_node(&bad).is_err());
        let mut bad = node;
        bad.geometric_error = f32::NAN;
        assert!(encode_terrain_node(&bad).is_err());
    }
}
