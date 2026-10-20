//! The valley mist map (`atmosphere::valley_mist`), shore map (`atmosphere::shore`) and terrain
//! relief for distant terrain shadows (`atmosphere::terrain_shadow`) of the active world space,
//! from its terrain at one coarse level. Every node of that level is read once per runtime
//! generation through the database worker, in batches beside the clipmap's own requests, and
//! the maps are built off the main thread. Reteya's 10 km takes 1,600 nodes: an 8 m relief, and
//! every other sample of it, 16 m, for the mist and shore maps (coarser levels decimate finer
//! ones exactly).
use super::{
    ActiveWorldSpace, WorldCatalog,
    database::{
        RequestId, TerrainQuery, TerrainReply, TerrainRequests, WorldDatabaseWorker, nodes_at,
    },
};

use atmosphere::{
    shore::{Shore, ShoreMap},
    terrain_shadow::{TerrainRelief, TerrainShadow},
    valley_mist::{MistMap, ValleyMist},
};

use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
};

use std::sync::Arc;
use world::{TerrainNodeKey, WorldSpaceId};
use world_db::EncodedTerrainNode;

/// Terrain level the maps sample: 8 m apart with 32 m cells and 33-sample nodes. The mist and
/// shore maps take every other sample.
const LEVEL: u8 = 3;

/// Nodes per request, and requests in flight at once.
const BATCH: usize = 64;

const IN_FLIGHT: usize = 2;

/// The mist map, the shore map of a world with a sea, and the relief.
type Maps = (MistMap, Option<ShoreMap>, TerrainRelief);

#[derive(Resource, Default)]
pub(super) struct MistTerrain {
    /// Runtime generation and world space the map is for, and that space's cell size.
    identity: Option<(String, WorldSpaceId)>,
    cell_size: f32,
    /// Nodes still to request, and requests in flight.
    queue: Vec<TerrainNodeKey>,
    requests: TerrainRequests,
    expected: usize,
    received: Vec<EncodedTerrainNode>,
    build: Option<Task<Result<Maps, String>>>,
    done: bool,
}

impl MistTerrain {
    pub(super) fn owns_request(&self, request_id: RequestId) -> bool {
        self.requests.owns(request_id)
    }

    pub(super) fn receive(&mut self, request_id: RequestId, reply: Result<TerrainReply, String>) {
        let Some(query) = self.requests.answer(request_id) else {
            return;
        };
        match (query, reply) {
            (TerrainQuery::Roots(_), Ok(TerrainReply::Metadata(roots))) => {
                self.queue = nodes_at(&roots, LEVEL);
                self.expected = self.queue.len();

                if self.expected == 0 {
                    self.done = true;
                }
            }
            (TerrainQuery::Heights(keys), Ok(TerrainReply::Heights(nodes)))
                if nodes.len() == keys.len() =>
            {
                self.received.extend(nodes);
            }
            (_, Err(e)) => self.fail(&e),
            _ => self.fail("terrain reply does not match its request"),
        }
    }

    fn fail(&mut self, reason: &str) {
        warn!("valley mist: no map for this world: {reason}");
        self.queue.clear();
        self.requests.drop_all();
        self.received.clear();
        self.done = true;
    }

    fn request(&mut self, worker: &WorldDatabaseWorker, query: TerrainQuery) -> bool {
        let Some((generation, _)) = &self.identity else {
            return false;
        };
        let sent = self.requests.send(worker, generation, query);
        sent.unwrap_or_else(|e| {
            self.fail(&e);
            false
        })
    }
}

pub(super) fn update(
    mut mist: ResMut<MistTerrain>,
    worker: Option<Res<WorldDatabaseWorker>>,
    active_space: Res<ActiveWorldSpace>,
    catalog: Res<WorldCatalog>,
    published: Option<ResMut<ValleyMist>>,
    shore: Option<ResMut<Shore>>,
    terrain: Option<ResMut<TerrainShadow>>,
) {
    let (Some(worker), Some(mut published), Some(mut shore), Some(mut terrain)) =
        (worker, published, shore, terrain)
    else {
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
            ..default()
        };
        published.set_sea_level(info.sea_level);
        published.disable();
        shore.disable();
        terrain.disable();

        if !mist.request(&worker, TerrainQuery::Roots(space)) {
            // Try again next frame.
            mist.identity = None;
        }

        return;
    }

    while mist.requests.len() < IN_FLIGHT && !mist.queue.is_empty() {
        let at = mist.queue.len().saturating_sub(BATCH);
        let batch = mist.queue.split_off(at);

        if !mist.request(&worker, TerrainQuery::Heights(batch.clone())) {
            mist.queue.extend(batch);
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
            Ok((map, shore_map, relief)) => {
                info!(
                    "valley mist: {}×{} map at {} m; terrain relief {}×{} at {} m",
                    map.size.x,
                    map.size.y,
                    map.metres_per_texel,
                    relief.size.x,
                    relief.size.y,
                    relief.metres_per_texel
                );
                published.publish(Arc::new(map));

                if let Some(shore_map) = shore_map {
                    shore.publish(Arc::new(shore_map));
                }

                terrain.set_relief(Arc::new(relief));
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
    let corner = |spacing: f64| {
        [
            f64::from(min_x) * node_metres - spacing * 0.5,
            f64::from(min_z) * node_metres - spacing * 0.5,
        ]
    };
    let size = UVec2::new(width as u32, depth as u32);
    let relief =
        TerrainRelief::from_heights(corner(spacing), size, spacing as f32, &heights, sea_level);

    // Mist and shore at twice the spacing: every other sample, the next level's own samples.
    let (mist_width, mist_depth) = (width.div_ceil(2), depth.div_ceil(2));
    let coarse: Vec<_> = (0..mist_depth)
        .flat_map(|z| (0..mist_width).map(move |x| (x, z)))
        .map(|(x, z)| heights[2 * z * width + 2 * x])
        .collect();
    let (origin, size, spacing) = (
        corner(spacing * 2.0),
        UVec2::new(mist_width as u32, mist_depth as u32),
        spacing * 2.0,
    );
    let shore =
        sea_level.map(|level| ShoreMap::from_heights(origin, size, spacing as f32, &coarse, level));
    Ok((
        MistMap::from_heights(origin, size, spacing as f32, &coarse, sea_level),
        shore,
        relief,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use world_db::TerrainNodeDescriptor;

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
            resolution: Some(33),
            decoded_bytes: 0,
            checksum: [0; 32],
        }
    }

    #[test]
    fn every_node_of_the_level_under_the_roots_once() {
        let keys = nodes_at(&[root(7, -1, 0), root(5, 2, -3)], LEVEL);
        assert_eq!(keys.len(), 256 + 16);
        assert!(keys.iter().all(|k| k.level == LEVEL));
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
        assert!(keys.contains(&TerrainNodeKey {
            level: 3,
            x: -16,
            z: 0,
            ..keys[0]
        }));
        assert!(keys.contains(&TerrainNodeKey {
            level: 3,
            x: 11,
            z: -9,
            ..keys[0]
        }));

        // A root below the level sets it.
        let keys = nodes_at(&[root(2, 0, 0), root(3, 1, 1)], LEVEL);
        assert_eq!(keys.len(), 1 + 4);
        assert!(keys.iter().all(|k| k.level == 2));
    }
}
