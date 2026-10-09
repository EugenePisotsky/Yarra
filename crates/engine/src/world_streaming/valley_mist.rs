//! The valley mist map (`atmosphere::valley_mist`) and shore map (`atmosphere::shore`) of the
//! active world space, from its terrain at one coarse level. Every node of that level is read once per runtime generation through
//! the database worker, a few at a time beside the terrain LOD's own requests, and the map is
//! built off the main thread. Reteya's 10 km takes 400 nodes and makes a 641² map at 16 m.
use super::{
    ActiveWorldSpace, WorldCatalog,
    database::{DatabaseRequest, TerrainQuery, TerrainReply, WorldDatabaseWorker},
};
use atmosphere::{
    shore::{Shore, ShoreMap},
    valley_mist::{MistMap, ValleyMist},
};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
};
use crossbeam_channel::TrySendError;
use std::{collections::HashMap, sync::Arc};
use world::{TerrainNodeKey, WorldSpaceId};
use world_db::{EncodedTerrainNode, TerrainNodeDescriptor};

/// Terrain level the map samples: 16 m apart with 32 m cells and 33-sample nodes.
const LEVEL: u8 = 4;
/// Node requests in flight at once; the terrain LOD keeps up to 12.
const IN_FLIGHT: usize = 4;
/// Set in this module's request ids, which the reply loop routes here.
const REQUEST_BIT: u64 = 1 << 63;

/// The mist map, and the shore map of a world with a sea.
type Maps = (MistMap, Option<ShoreMap>);

#[derive(Resource, Default)]
pub(super) struct MistTerrain {
    /// Runtime generation and world space the map is for, and that space's cell size.
    identity: Option<(String, WorldSpaceId)>,
    cell_size: f32,
    /// Nodes still to request, and requests in flight by id.
    queue: Vec<TerrainNodeKey>,
    pending: HashMap<u64, TerrainQuery>,
    next_id: u64,
    expected: usize,
    received: Vec<EncodedTerrainNode>,
    build: Option<Task<Result<Maps, String>>>,
    done: bool,
}

impl MistTerrain {
    pub(super) fn owns_request(request_id: u64) -> bool {
        request_id & REQUEST_BIT != 0
    }

    pub(super) fn receive(&mut self, request_id: u64, reply: Result<TerrainReply, String>) {
        let Some(query) = self.pending.remove(&request_id) else {
            return;
        };
        match (query, reply) {
            (TerrainQuery::Roots(_), Ok(TerrainReply::Metadata(roots))) => {
                self.queue = level_keys(&roots);
                self.expected = self.queue.len();
                if self.expected == 0 {
                    self.done = true;
                }
            }
            (TerrainQuery::Node(key), Ok(TerrainReply::Node(node)))
                if node.descriptor.key == key =>
            {
                self.received.push(node);
            }
            (_, Err(e)) => self.fail(&e),
            _ => self.fail("terrain reply does not match its request"),
        }
    }

    fn fail(&mut self, reason: &str) {
        warn!("valley mist: no map for this world: {reason}");
        self.queue.clear();
        self.pending.clear();
        self.received.clear();
        self.done = true;
    }

    fn request(&mut self, worker: &WorldDatabaseWorker, query: TerrainQuery) -> bool {
        let Some((generation, _)) = &self.identity else {
            return false;
        };
        self.next_id = self.next_id.wrapping_add(1);
        let request_id = REQUEST_BIT | self.next_id;
        match worker.try_send(DatabaseRequest::Terrain {
            request_id,
            generation: generation.clone(),
            query: query.clone(),
        }) {
            Ok(()) => {
                self.pending.insert(request_id, query);
                true
            }
            Err(TrySendError::Full(_)) => false,
            Err(TrySendError::Disconnected(_)) => {
                self.fail("the world database worker stopped");
                false
            }
        }
    }
}

/// Every node of [`LEVEL`] under the roots (or of a lower level, for roots below it). Roots
/// are complete, so all of their descendants exist.
fn level_keys(roots: &[TerrainNodeDescriptor]) -> Vec<TerrainNodeKey> {
    let level = roots
        .iter()
        .map(|r| r.key.level)
        .min()
        .unwrap_or(LEVEL)
        .min(LEVEL);
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

pub(super) fn update(
    mut mist: ResMut<MistTerrain>,
    worker: Option<Res<WorldDatabaseWorker>>,
    active_space: Res<ActiveWorldSpace>,
    catalog: Res<WorldCatalog>,
    published: Option<ResMut<ValleyMist>>,
    shore: Option<ResMut<Shore>>,
) {
    let (Some(worker), Some(mut published), Some(mut shore)) = (worker, published, shore) else {
        return;
    };
    let Some(space) = active_space.current() else {
        return;
    };
    let Some(info) = catalog.world_space(space) else {
        return;
    };
    let identity = (catalog.generation_id().to_owned(), space);
    if mist.identity.as_ref() != Some(&identity) {
        *mist = MistTerrain {
            identity: Some(identity),
            cell_size: info.cell_size,
            next_id: mist.next_id,
            ..default()
        };
        published.set_sea_level(info.sea_level);
        published.disable();
        shore.disable();
        if !mist.request(&worker, TerrainQuery::Roots(space)) {
            // Try again next frame.
            mist.identity = None;
        }
        return;
    }
    while mist.pending.len() < IN_FLIGHT {
        let Some(key) = mist.queue.pop() else {
            break;
        };
        if !mist.request(&worker, TerrainQuery::Node(key)) {
            mist.queue.push(key);
            break;
        }
    }
    if !mist.done
        && mist.build.is_none()
        && mist.expected > 0
        && mist.received.len() == mist.expected
    {
        let nodes = std::mem::take(&mut mist.received);
        let (cell_size, sea_level) = (mist.cell_size, info.sea_level);
        mist.build = Some(
            AsyncComputeTaskPool::get().spawn(async move { build(nodes, cell_size, sea_level) }),
        );
    }
    if let Some(task) = &mut mist.build
        && let Some(result) = block_on(poll_once(task))
    {
        mist.build = None;
        mist.done = true;
        match result {
            Ok((map, shore_map)) => {
                info!(
                    "valley mist: {}×{} map at {} m",
                    map.size.x, map.size.y, map.metres_per_texel
                );
                published.publish(Arc::new(map));
                if let Some(shore_map) = shore_map {
                    shore.publish(Arc::new(shore_map));
                }
            }
            Err(e) => mist.fail(&e),
        }
    }
}

/// The maps of one level's nodes: their samples on one grid, sharing the samples along their
/// edges. Only a world with a sea has a shore map.
fn build(
    nodes: Vec<EncodedTerrainNode>,
    cell_size: f32,
    sea_level: Option<f32>,
) -> Result<Maps, String> {
    let nodes = nodes
        .into_iter()
        .map(|n| n.decode().map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let first = nodes.first().ok_or("no terrain nodes")?;
    let level = first.key.level;
    let fields: Vec<_> = nodes
        .iter()
        .map(|n| Some((n.key, n.heightfield.as_ref()?)))
        .collect::<Option<_>>()
        .ok_or("a terrain node holds no heights")?;
    let n = fields[0].1.resolution;
    if fields
        .iter()
        .any(|(key, field)| key.level != level || field.resolution != n)
    {
        return Err("terrain nodes differ in level or resolution".into());
    }
    let intervals = usize::from(n - 1);
    let (min_x, max_x, min_z, max_z) = fields.iter().fold(
        (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
        |(a, b, c, d), (key, _)| (a.min(key.x), b.max(key.x), c.min(key.z), d.max(key.z)),
    );
    let width = (max_x - min_x + 1) as usize * intervals + 1;
    let depth = (max_z - min_z + 1) as usize * intervals + 1;
    let mut heights = vec![None; width * depth];
    for (key, field) in &fields {
        let (x0, z0) = (
            (key.x - min_x) as usize * intervals,
            (key.z - min_z) as usize * intervals,
        );
        for z in 0..=intervals {
            for x in 0..=intervals {
                heights[(z0 + z) * width + x0 + x] = Some(field.height_at(x, z));
            }
        }
    }
    let node_metres = f64::from(cell_size) * f64::from(1_u32 << level);
    let spacing = node_metres / intervals as f64;
    // Samples sit on texel centres, half a spacing in from the map's corner.
    let origin = [
        f64::from(min_x) * node_metres - spacing * 0.5,
        f64::from(min_z) * node_metres - spacing * 0.5,
    ];
    let size = UVec2::new(width as u32, depth as u32);
    let shore = sea_level
        .map(|level| ShoreMap::from_heights(origin, size, spacing as f32, &heights, level));
    Ok((
        MistMap::from_heights(origin, size, spacing as f32, &heights, sea_level),
        shore,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(level: u8, x: i32, z: i32) -> TerrainNodeDescriptor {
        TerrainNodeDescriptor {
            key: TerrainNodeKey {
                space: WorldSpaceId(1),
                level,
                x,
                z,
            },
            child_mask: 15,
            height_bounds: [0., 1.],
            geometric_error: 0.,
            resolution: Some(33),
            decoded_bytes: 0,
            gpu_bytes_estimate: 0,
            checksum: [0; 32],
        }
    }

    #[test]
    fn every_node_of_the_level_under_the_roots_once() {
        let keys = level_keys(&[root(7, -1, 0), root(5, 2, -3)]);
        assert_eq!(keys.len(), 64 + 4);
        assert!(keys.iter().all(|k| k.level == LEVEL));
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
        assert!(keys.contains(&TerrainNodeKey {
            level: 4,
            x: -8,
            z: 0,
            ..keys[0]
        }));
        assert!(keys.contains(&TerrainNodeKey {
            level: 4,
            x: 5,
            z: -5,
            ..keys[0]
        }));
        // A root below the level sets it.
        let keys = level_keys(&[root(2, 0, 0), root(3, 1, 1)]);
        assert_eq!(keys.len(), 1 + 4);
        assert!(keys.iter().all(|k| k.level == 2));
    }
}
