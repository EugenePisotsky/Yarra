//! What a terrain consumer (the clipmap, the valley mist) keeps of its requests: the queries
//! in flight, so a reply finds its question, and those of an earlier publication, whose
//! replies are dropped.
use super::{DatabaseRequest, NotSent, RequestId, TerrainQuery, WorldDatabaseWorker};
use std::collections::{HashMap, HashSet};
use world::TerrainNodeKey;
use world_db::TerrainNodeDescriptor;

#[derive(Default)]
pub(in crate::world_streaming) struct TerrainRequests {
    pending: HashMap<RequestId, TerrainQuery>,
    dropped: HashSet<RequestId>,
}

impl TerrainRequests {
    pub(in crate::world_streaming) fn owns(&self, id: RequestId) -> bool {
        self.pending.contains_key(&id) || self.dropped.contains(&id)
    }

    /// Requests in flight.
    pub(in crate::world_streaming) fn len(&self) -> usize {
        self.pending.len()
    }

    /// Sends `query` against runtime `generation`: `Ok(false)` while the worker's queue is
    /// full, an error once the worker has stopped.
    pub(in crate::world_streaming) fn send(
        &mut self,
        worker: &WorldDatabaseWorker,
        generation: &str,
        query: TerrainQuery,
    ) -> Result<bool, String> {
        match worker.send(DatabaseRequest::Terrain {
            generation: generation.to_owned(),
            query: query.clone(),
        }) {
            Ok(id) => {
                self.pending.insert(id, query);
                Ok(true)
            }
            Err(NotSent::Full) => Ok(false),
            Err(NotSent::Stopped) => Err("the world database worker stopped".into()),
        }
    }

    /// The query a reply answers; `None` for one dropped or not ours.
    pub(in crate::world_streaming) fn answer(&mut self, id: RequestId) -> Option<TerrainQuery> {
        if self.dropped.remove(&id) {
            return None;
        }

        self.pending.remove(&id)
    }

    /// A request in flight, for tests that answer it themselves.
    #[cfg(test)]
    pub(in crate::world_streaming) fn insert(&mut self, id: RequestId, query: TerrainQuery) {
        self.pending.insert(id, query);
    }

    /// Replies to every request in flight will be dropped.
    pub(in crate::world_streaming) fn drop_all(&mut self) {
        self.dropped.extend(self.pending.drain().map(|(id, _)| id));
    }
}

/// Every node of `level` under complete `roots`, or of the lowest root level if that is
/// lower.
pub(in crate::world_streaming) fn nodes_at(
    roots: &[TerrainNodeDescriptor],
    level: u8,
) -> Vec<TerrainNodeKey> {
    let level = roots
        .iter()
        .map(|r| r.key.level)
        .min()
        .unwrap_or(level)
        .min(level);
    roots
        .iter()
        .flat_map(|root| {
            let span = 1_i32 << (root.key.level - level);
            let key = root.key;
            (0..span * span).map(move |i| TerrainNodeKey {
                level,
                x: key.x * span + i % span,
                z: key.z * span + i / span,
                ..key
            })
        })
        .collect()
}
