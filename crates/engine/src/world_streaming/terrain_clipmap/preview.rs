//! The editor's live terrain: a two-phase handoff, so the ground and the editor's matching
//! grass and source cells change at the same frame boundary. The editor sends a request; the
//! terrain answers `ready` and, once the editor commits that revision, draws it.
use super::*;
use std::sync::Arc;
use world::TerrainPreviewProducts;

#[derive(Clone)]
pub struct TerrainPreviewRequest {
    pub revision: u64,
    pub generation: String,
    pub space: WorldSpaceId,
    pub products: Arc<TerrainPreviewProducts>,
}

#[derive(Resource, Default)]
pub struct LiveTerrainPreview {
    pub request: Option<TerrainPreviewRequest>,
    pub ready: Option<u64>,
    pub commit: Option<u64>,
    pub applied: Option<u64>,
    pub error: Option<String>,
}

/// Terrain v2 stages nothing: a request is ready once it matches the drawn generation and
/// space, and its commit rewrites the blocks it changes in the frame it arrives.
pub(crate) fn apply(
    mut live: ResMut<LiveTerrainPreview>,
    mut terrain: ResMut<ClipmapTerrain>,
    mut uploads: ResMut<ClipmapUploads>,
) {
    let Some(request) = live.request.clone() else {
        return;
    };

    if terrain.identity.as_ref() != Some(&(request.generation.clone(), request.space))
        || live.applied == Some(request.revision)
        || live.error.is_some()
    {
        return;
    }

    live.ready = Some(request.revision);

    if live.commit != Some(request.revision) {
        return;
    }

    terrain.set_overlay(request.products, &mut uploads);
    live.applied = Some(request.revision);
    live.commit = None;
}

impl ClipmapTerrain {
    /// Replaces the editor's overlay. Resident nodes it changes are rewritten from it now;
    /// those the previous overlay changed and this one leaves to the publication are read
    /// again. Nodes read later take the overlay's parts in `accept`.
    pub(super) fn set_overlay(
        &mut self,
        products: Arc<TerrainPreviewProducts>,
        uploads: &mut ClipmapUploads,
    ) {
        let old = self.overlay.replace(products.clone());
        let Some(layout) = self.layout else {
            return;
        };

        if let Some(old) = &old {
            let reverted = old
                .nodes
                .keys()
                .filter(|k| !products.nodes.contains_key(k))
                .chain(
                    old.controls
                        .keys()
                        .filter(|k| !products.controls.contains_key(k)),
                );
            for &key in reverted {
                if self.resident.bounds.contains_key(&key) {
                    self.stale.insert(key);
                }
            }
        }

        let keys: std::collections::BTreeSet<_> = products
            .nodes
            .keys()
            .chain(products.controls.keys())
            .copied()
            .filter(|k| self.resident.bounds.contains_key(k))
            .collect();

        for key in keys {
            let unchanged = |o: &TerrainPreviewProducts| {
                o.nodes.get(&key) == products.nodes.get(&key)
                    && o.controls.get(&key) == products.controls.get(&key)
            };

            if old.as_deref().is_some_and(unchanged) {
                continue;
            }

            let mut node = match products.nodes.get(&key) {
                Some(n) => match relief(n, layout.intervals) {
                    Ok(node) => node,
                    Err(e) => return self.fail(e),
                },
                None => Decoded {
                    key,
                    bounds: self.resident.bounds[&key],
                    block: NodeBlock::new(key),
                    leaf: None,
                },
            };

            if let Some(control) = products.controls.get(&key) {
                node.block.control = Some(control.to_vec());
            }

            self.place(&layout, uploads, node);
        }
    }
}
