//! Immutable, unpublished terrain replacements. Keys absent here still come from
//! the published runtime. A preview is always relative to one runtime generation.
use crate::{
    CellCoord, TerrainComposite, TerrainHeightfieldPage, TerrainMaterialKey, TerrainNode,
    TerrainNodeKey,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, Default)]
pub struct TerrainPreviewProducts {
    pub nodes: BTreeMap<TerrainNodeKey, Arc<TerrainNode>>,
    pub composites: BTreeMap<TerrainMaterialKey, Arc<TerrainComposite>>,
    /// Only leaves that differ from the publication; retained after source eviction.
    pub leaves: BTreeMap<CellCoord, Arc<TerrainHeightfieldPage>>,
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
            + self.composites.len() * (TerrainComposite::gpu_bytes() + 128)
            + self
                .leaves
                .values()
                .map(|p| {
                    128 + p.heightfield.heights.len() * 4
                        + p.heightfield.normals_oct.len() * 4
                        + p.weight_pages.iter().map(|w| w.rgba.len()).sum::<usize>()
                })
                .sum::<usize>()
    }
}
