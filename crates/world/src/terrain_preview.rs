//! Immutable, unpublished terrain replacements. Keys absent here still come from
//! the published runtime. A preview is always relative to one runtime generation.
use crate::{CellCoord, TerrainHeightfieldPage, TerrainNode, TerrainNodeKey};
use std::{collections::BTreeMap, sync::Arc};

/// An edited cell as a cook would publish it: its ground page and its control tile.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainPreviewLeaf {
    pub page: TerrainHeightfieldPage,
    pub control: Vec<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct TerrainPreviewProducts {
    pub nodes: BTreeMap<TerrainNodeKey, Arc<TerrainNode>>,
    /// Control tiles (`crate::terrain_control`) of the changed leaves and their ancestors up to
    /// `CONTROL_TOP_LEVEL`, packed samples.
    pub controls: BTreeMap<TerrainNodeKey, Arc<Vec<u32>>>,
    /// Only leaves that differ from the publication; retained after source eviction.
    pub leaves: BTreeMap<CellCoord, Arc<TerrainPreviewLeaf>>,
}

impl TerrainPreviewProducts {
    pub fn bytes(&self) -> usize {
        self.nodes
            .values()
            .map(|n| {
                n.heightfield
                    .as_ref()
                    .map_or(128, |h| 128 + h.heights.len() * 4 + h.normals_oct.len() * 4)
            })
            .sum::<usize>()
            + self
                .controls
                .values()
                .map(|c| 128 + c.len() * 4)
                .sum::<usize>()
            + self
                .leaves
                .values()
                .map(|leaf| {
                    let field = &leaf.page.heightfield;
                    128 + field.heights.len() * 4
                        + field.normals_oct.len() * 4
                        + leaf.control.len() * 4
                })
                .sum::<usize>()
    }
}
