//! Terrain v2 (docs/DECISIONS.md): the ground drawn from GPU height clipmaps with CDLOD patches
//! (`terrain_render::clipmap`).
//!
//! Every level keeps a window of nodes around the camera resident in its clipmap layer. Nodes
//! are read from the world database in batches, decoded on a pool thread and written into their
//! toroidal blocks; a window moving by a node rewrites only the row of blocks that enters it.
//! Each frame selects the patches to draw from the resident nodes and keeps one entity per
//! patch, all sharing one grid mesh and one material; the vertex shader does the rest.
//!
//! The editor's live terrain (`preview`) overlays unpublished nodes and control tiles, and a new
//! publication of the drawn space is read again in place: the old ground stays drawn until each
//! node's new version replaces it.
mod preview;

pub(crate) use preview::apply as apply_preview;
pub use preview::{LiveTerrainPreview, TerrainPreviewRequest};

use super::{
    ActiveWorldSpace, WorldCatalog, WorldOrigin, WorldViewCamera,
    database::{
        RequestId, TerrainQuery, TerrainReply, TerrainRequests, WorldDatabaseWorker, nodes_at,
    },
};

use bevy::{
    camera::{primitives::Aabb, visibility::NoAutoAabb},
    math::DVec3,
    mesh::MeshTag,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
};

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use terrain_render::clipmap::{
    self, ClipmapFrame, ClipmapLayout, ClipmapTerrainMaterial, ClipmapTextures, ClipmapUploads,
    LodRanges, NodeBlock, NodeWindow, Residency,
};

use world::{
    TerrainHeightfield, TerrainNode, TerrainNodeKey, TerrainPreviewProducts, WorldSpaceId,
};

use world_db::{EncodedTerrainNode, TerrainNodeDescriptor};

/// Texels per side of a level: 32 nodes' blocks, of which 31 are a window and one its guard.
const SIDE: u32 = if cfg!(target_os = "ios") { 512 } else { 1024 };

/// The top level: 1 km nodes with 32 m cells, a 31 km window. Roots above it start at it.
const TOP: u8 = 5;

/// Level 0 draws within this many metres of the camera; every coarser level doubles it. At
/// 1 m leaf spacing a triangle is 6–13 px across at 1440p wherever it is drawn. The window
/// must hold a level's patches past its parent's range: (31 − 1) / 2 − HYSTERESIS nodes.
const LEAF_RANGE: f64 = if cfg!(target_os = "ios") { 96.0 } else { 192.0 };

/// Nodes are read ahead out to this multiple of the range that draws them, so the next finer
/// level's normals are resident where the fragment shader asks for them.
const PREFETCH: f64 = 2.0;

/// A window follows the camera once it is more than this many nodes off its centre.
const HYSTERESIS: i32 = 2;

/// Nodes per database request.
const BATCH: usize = 48;

/// Database requests in flight.
const IN_FLIGHT: usize = 2;

/// A failed load starts over after this many seconds.
const RETRY_SECONDS: f64 = 5.0;

/// Samples per node side the clipmap is laid out for.
const NODE_SAMPLES: u16 = 33;

#[derive(Default)]
struct Resident {
    bounds: HashMap<TerrainNodeKey, [f32; 2]>,
    /// Level-0 samples for grounding actors on the drawn surface.
    leaves: HashMap<TerrainNodeKey, TerrainHeightfield>,
}

impl clipmap::NodeSource for Resident {
    fn resident(&self, key: TerrainNodeKey) -> Option<[f32; 2]> {
        self.bounds.get(&key).copied()
    }
}

struct Decoded {
    key: TerrainNodeKey,
    bounds: [f32; 2],
    block: NodeBlock,
    leaf: Option<TerrainHeightfield>,
}

/// Counts for the log and the F1 panel.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClipmapStats {
    pub resident: usize,
    pub patches: usize,
    pub triangles: usize,
    pub demand: usize,
    pub missing: usize,
    pub uploads: u64,
}

#[derive(Resource, Default)]
pub struct ClipmapTerrain {
    identity: Option<(String, WorldSpaceId)>,
    cell_size: f32,
    layout: Option<ClipmapLayout>,
    ranges: Option<LodRanges>,
    space: Option<WorldSpaceId>,
    starts: Vec<TerrainNodeKey>,
    windows: Vec<NodeWindow>,
    resident: Resident,
    residency: Residency,
    /// Requested and not yet resident, and keys the database does not have.
    requested: HashSet<TerrainNodeKey>,
    absent: HashSet<TerrainNodeKey>,
    /// Resident but to be read again: from a new publication, or where the editor's overlay
    /// no longer replaces them.
    stale: HashSet<TerrainNodeKey>,
    requests: TerrainRequests,
    /// The space's roots and surfaces still to be requested.
    wanted_roots: bool,
    wanted_surfaces: bool,
    /// A new publication changed the layout or ground textures: load from scratch.
    rebuild: bool,
    /// The editor's unpublished nodes and control tiles, drawn in place of the published ones.
    overlay: Option<Arc<TerrainPreviewProducts>>,
    /// The space's ground surfaces and textures, once read.
    surfaces: Option<world_db::TerrainRenderResources>,
    received: Vec<Vec<(EncodedTerrainNode, Option<Vec<u8>>)>>,
    decoding: Vec<Task<Result<Vec<Decoded>, String>>>,
    gpu: Option<ClipmapTextures>,
    patches: Patches,
    wear: WearWindow,
    failure: Option<String>,
    /// When the failure was seen, for the retry.
    failed_at: Option<f64>,
    started: Option<f64>,
    ready_logged: bool,
    pub stats: ClipmapStats,
}

/// The drawn patches: one entity per selected node, all sharing one grid mesh.
#[derive(Default)]
struct Patches {
    mesh: Option<Handle<Mesh>>,
    /// Each drawn patch and the height bounds its box was made from.
    drawn: HashMap<TerrainNodeKey, (Entity, [f32; 2])>,
    /// The render origin cell the patches are placed for.
    placed: Option<world::CellCoord>,
}

/// Road wear around the camera ([`follow_wear`]).
#[derive(Default)]
struct WearWindow {
    /// The cell and page whose road wear each wear atlas slot holds.
    slots: [Option<(world::CellCoord, Entity)>; clipmap::WEAR_SLOTS],
    /// Layers newly found worn bare to, for the surface table.
    bare: Vec<u16>,
}

impl ClipmapTerrain {
    pub(super) fn owns_request(&self, id: RequestId) -> bool {
        self.requests.owns(id)
    }

    pub(super) fn receive(&mut self, id: RequestId, reply: Result<TerrainReply, String>) {
        let Some(query) = self.requests.answer(id) else {
            return;
        };

        match (query, reply) {
            (TerrainQuery::Roots(_), Ok(TerrainReply::Metadata(roots))) => self.adopt_roots(&roots),
            (TerrainQuery::Surfaces(_), Ok(TerrainReply::Surfaces(resources))) => {
                if self.gpu.is_some() && self.surfaces.as_ref() != Some(&*resources) {
                    self.rebuild = true;
                }
                self.surfaces = Some(*resources);
            }
            (TerrainQuery::Nodes(keys), Ok(TerrainReply::Nodes(nodes))) => {
                let found: HashSet<_> = nodes.iter().map(|(n, _)| n.descriptor.key).collect();

                for key in keys.into_iter().filter(|k| !found.contains(k)) {
                    self.requested.remove(&key);
                    self.absent.insert(key);

                    // A new publication without it.
                    if self.stale.remove(&key) {
                        self.evict(key);
                    }
                }

                self.received.push(nodes);
            }
            (_, Err(e)) => self.fail(e),
            _ => self.fail("terrain reply does not match its request".into()),
        }
    }

    fn fail(&mut self, reason: String) {
        if self.failure.is_none() {
            warn!("TERRAIN_V2_FAILED {reason}");
        }
        self.failure = Some(reason);
    }

    fn adopt_roots(&mut self, roots: &[TerrainNodeDescriptor]) {
        let Some(top) = roots.iter().map(|r| r.key.level).min().map(|l| l.min(TOP)) else {
            self.fail("the world space has no terrain".into());
            return;
        };

        if roots
            .iter()
            .any(|r| r.resolution.is_some_and(|n| n != NODE_SAMPLES))
        {
            self.fail(format!("terrain v2 needs {NODE_SAMPLES}-sample nodes"));
            return;
        }

        let layout = ClipmapLayout::new(
            SIDE,
            top + 1,
            f64::from(self.cell_size),
            u32::from(NODE_SAMPLES - 1),
        );
        let starts = nodes_at(roots, top);

        if self.layout.is_some() {
            // A new publication: kept as it is, unless its extent changed.
            self.rebuild |= self.layout != Some(layout) || self.starts != starts;
            return;
        }

        self.starts = starts;
        self.space = roots.first().map(|r| r.key.space);
        self.residency = Residency::new(layout.levels);
        self.layout = Some(layout);
        self.ranges = Some(LodRanges { leaf: LEAF_RANGE });
    }

    fn request(&mut self, worker: &WorldDatabaseWorker, query: TerrainQuery) -> bool {
        let Some((generation, _)) = &self.identity else {
            return false;
        };
        let sent = self.requests.send(worker, generation, query);
        sent.unwrap_or_else(|e| {
            self.fail(e);
            false
        })
    }

    fn window(&self, level: u8) -> Option<&NodeWindow> {
        self.windows.get(usize::from(level))
    }

    fn in_window(&self, key: TerrainNodeKey) -> bool {
        self.window(key.level)
            .is_some_and(|w| w.contains(key.x, key.z))
    }

    /// Follows the eye; nodes leaving a window leave the clipmap.
    fn move_windows(&mut self, layout: &ClipmapLayout, xz: [f64; 2]) {
        let mut moved = false;

        for level in 0..layout.levels {
            let at = NodeWindow::node_at(layout, level, xz);
            let keep = self.window(level).is_some_and(|w| {
                let c = w.centre();
                (at[0] - c[0]).abs() <= HYSTERESIS && (at[1] - c[1]).abs() <= HYSTERESIS
            });

            if keep {
                continue;
            }

            let window = NodeWindow::around(layout, level, at);

            match self.windows.get_mut(usize::from(level)) {
                Some(w) => *w = window,
                None => self.windows.push(window),
            }

            moved = true;
        }

        if !moved {
            return;
        }

        let gone: Vec<_> = self
            .resident
            .bounds
            .keys()
            .copied()
            .filter(|&k| !self.in_window(k))
            .collect();

        for key in gone {
            self.evict(key);
        }

        let windows = self.windows.clone();
        let inside = |k: &TerrainNodeKey| {
            windows
                .get(usize::from(k.level))
                .is_some_and(|w| w.contains(k.x, k.z))
        };
        self.requested.retain(inside);
        self.stale.retain(inside);
    }

    fn evict(&mut self, key: TerrainNodeKey) {
        self.resident.bounds.remove(&key);
        self.resident.leaves.remove(&key);

        if let Some(layout) = &self.layout {
            self.residency.set(layout, key, false);
        }
    }

    /// Nodes the view needs or soon will, parents first: those inside their window whose
    /// parent is within `PREFETCH` × the parent's split range. Unloaded nodes take their
    /// parent's height bounds, which hold theirs.
    fn demand(
        &self,
        layout: &ClipmapLayout,
        ranges: &LodRanges,
        eye: DVec3,
    ) -> Vec<TerrainNodeKey> {
        let mut out: Vec<(f64, TerrainNodeKey)> = Vec::new();
        let mut level_nodes: Vec<(TerrainNodeKey, [f32; 2])> = self
            .starts
            .iter()
            .filter(|&&k| self.in_window(k))
            .map(|&k| {
                (
                    k,
                    self.resident
                        .bounds
                        .get(&k)
                        .copied()
                        .unwrap_or([-1.0e4, 1.0e4]),
                )
            })
            .collect();

        while !level_nodes.is_empty() {
            let mut next = Vec::new();

            for &(key, bounds) in &level_nodes {
                let bounds = self.resident.bounds.get(&key).copied().unwrap_or(bounds);
                let d = clipmap::distance(layout, key, bounds, eye);
                out.push((d / layout.node_metres(key.level), key));

                if key.level == 0 || d >= PREFETCH * ranges.range(key.level - 1) {
                    continue;
                }

                let Ok(Some(children)) = key.children() else {
                    continue;
                };
                next.extend(
                    children
                        .into_iter()
                        .filter(|&c| self.in_window(c) && !self.absent.contains(&c))
                        .map(|c| (c, bounds)),
                );
            }

            level_nodes = next;
        }

        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.into_iter().map(|(_, k)| k).collect()
    }

    /// A node read from the database, with the parts the editor's overlay replaces.
    fn accept(&mut self, layout: &ClipmapLayout, uploads: &mut ClipmapUploads, mut node: Decoded) {
        self.requested.remove(&node.key);
        self.stale.remove(&node.key);

        if !self.in_window(node.key) {
            return;
        }

        if let Some(overlay) = self.overlay.clone() {
            if let Some(n) = overlay.nodes.get(&node.key) {
                match relief(n, layout.intervals) {
                    Ok(r) => {
                        node = Decoded {
                            block: NodeBlock {
                                control: node.block.control,
                                ..r.block
                            },
                            ..r
                        }
                    }
                    Err(e) => return self.fail(e),
                }
            }
            if let Some(control) = overlay.controls.get(&node.key) {
                node.block.control = Some(control.to_vec());
            }
        }

        self.place(layout, uploads, node);
    }

    fn place(&mut self, layout: &ClipmapLayout, uploads: &mut ClipmapUploads, node: Decoded) {
        self.resident.bounds.insert(node.key, node.bounds);

        if let Some(leaf) = node.leaf {
            self.resident.leaves.insert(node.key, leaf);
        }

        self.residency.set(layout, node.key, true);

        if let Some(gpu) = &self.gpu {
            uploads.push(gpu, node.block);
        }
    }

    fn clear(&mut self, commands: &mut Commands, uploads: &mut ClipmapUploads) {
        for (_, (entity, _)) in self.patches.drawn.drain() {
            commands.entity(entity).despawn();
        }

        if let Some(gpu) = &self.gpu {
            uploads.forget(gpu);
        }

        *self = Self::default();
    }

    /// A new publication of the drawn space: what is resident stays drawn until its node is
    /// read again from it. The editor's overlay belonged to the previous one.
    fn republish(&mut self) {
        self.requests.drop_all();
        self.stale.extend(self.resident.bounds.keys().copied());
        self.requested.clear();
        self.absent.clear();
        self.received.clear();
        self.decoding.clear();
        self.overlay = None;
        self.ready_logged = false;
    }

    /// Whether nodes the view needs are still being read or decoded.
    pub fn pending(&self) -> bool {
        self.stats.missing > 0 || !self.decoding.is_empty()
    }

    /// Whether any leaf is resident to stand on.
    pub fn has_ground(&self) -> bool {
        !self.resident.leaves.is_empty()
    }

    /// The drawn leaf surface's height under a render-space position, once its node is
    /// resident: the same triangles the patch draws within the leaf range.
    pub fn ground_height(&self, render: Vec3, origin: &WorldOrigin) -> Option<f32> {
        let (key, local) = self.leaf_under(render, origin)?;
        let field = self.resident.leaves.get(&key)?;
        Some(field.sample_height(local, self.cell_size))
    }

    /// Whether the leaf under a render-space position is resident, so that within the leaf
    /// range the ground there is drawn from its full-detail samples.
    pub fn leaf_resident(&self, render: Vec3, origin: &WorldOrigin) -> bool {
        self.leaf_under(render, origin)
            .is_some_and(|(key, _)| self.resident.leaves.contains_key(&key))
    }

    /// The leaf node under a render-space position and the position within it.
    fn leaf_under(&self, render: Vec3, origin: &WorldOrigin) -> Option<(TerrainNodeKey, [f32; 2])> {
        let space = self.space?;
        let cell = f64::from(self.cell_size);
        let [ox, oz] = origin.cell().origin(self.cell_size);
        let (x, z) = (f64::from(render.x) + ox, f64::from(render.z) + oz);
        let key = TerrainNodeKey {
            space,
            level: 0,
            x: (x / cell).floor() as i32,
            z: (z / cell).floor() as i32,
        };
        let local = [
            (x - f64::from(key.x) * cell) as f32,
            (z - f64::from(key.z) * cell) as f32,
        ];
        Some((key, local))
    }
}

/// Decodes a batch and lays each node out as its clipmap block. A node without a control tile
/// shows its world's first surface.
fn decode(
    nodes: Vec<(EncodedTerrainNode, Option<Vec<u8>>)>,
    intervals: u32,
) -> Result<Vec<Decoded>, String> {
    nodes
        .into_iter()
        .map(|(encoded, control)| {
            let node = encoded.decode().map_err(|e| e.to_string())?;
            let control = match control {
                Some(payload) => {
                    world_db::decode_terrain_control(&payload).map_err(|e| e.to_string())?
                }
                None => vec![
                    world::pack_control([(0, 1.0), (0, 0.0), (0, 0.0)]);
                    world::CONTROL_SAMPLES * world::CONTROL_SAMPLES
                ],
            };
            let mut decoded = relief(&node, intervals)?;
            decoded.block.control = Some(control);
            Ok(decoded)
        })
        .collect()
}

/// A node's block with its heights and normals, its height bounds and, at level 0, its
/// samples for grounding.
fn relief(node: &TerrainNode, intervals: u32) -> Result<Decoded, String> {
    let block = NodeBlock::relief_of(node, intervals)?;
    Ok(Decoded {
        key: node.key,
        bounds: node.height_bounds,
        block,
        leaf: (node.key.level == 0)
            .then(|| node.heightfield.clone())
            .flatten(),
    })
}

/// Road wear around the camera: each cell of the wear window whose page has a wear field holds
/// it in its toroidal atlas slot, uploaded when the slot's page changes.
fn follow_wear(
    wear_window: &mut WearWindow,
    textures: &ClipmapTextures,
    uploads: &mut ClipmapUploads,
    sources: &Query<(Entity, Ref<super::StreamedTerrainWear>)>,
    space: WorldSpaceId,
    eye: DVec3,
    cell_size: f32,
) {
    let size = f64::from(cell_size);
    let at = world::CellCoord {
        x: (eye.x / size).floor() as i32,
        z: (eye.z / size).floor() as i32,
    };
    let half = clipmap::WEAR_WINDOW / 2;
    let window = |c: world::CellCoord| (c.x - at.x).abs() <= half && (c.z - at.z).abs() <= half;
    let mut pages: HashMap<
        world::CellCoord,
        (
            Entity,
            bool,
            &world::TerrainWear,
            &super::StreamedTerrainWear,
        ),
    > = HashMap::new();

    for (entity, source) in sources {
        let changed = source.is_changed();
        let source = Ref::into_inner(source);

        if source.key.space != space || !window(source.key.cell) {
            continue;
        }

        if let Some(wear) = source
            .wear
            .as_ref()
            .filter(|w| w.resolution == world::WEAR_RESOLUTION)
        {
            pages.insert(source.key.cell, (entity, changed, wear, source));
        }
    }

    for dz in -half..=half {
        for dx in -half..=half {
            let cell = world::CellCoord {
                x: at.x + dx,
                z: at.z + dz,
            };
            let slot = clipmap::wear_slot(cell);

            match pages.get(&cell) {
                Some(&(entity, changed, wear, source)) => {
                    if changed || wear_window.slots[slot] != Some((cell, entity)) {
                        uploads.push_wear(textures, slot, wear);
                        wear_window.slots[slot] = Some((cell, entity));

                        // The page's surfaces its roads wear bare to.
                        wear_window
                            .bare
                            .extend(wear.worn.iter().flatten().filter_map(|&(to, share)| {
                                (to != world::NO_WORN_SLOT && share > 0)
                                    .then(|| source.layers.get(usize::from(to)).copied())
                                    .flatten()
                            }));
                    }
                }
                None => wear_window.slots[slot] = None,
            }
        }
    }
}

/// The world's surfaces by texture layer, with the grass each carries, and its textures.
fn ground(
    asset_server: &AssetServer,
    resources: &world_db::TerrainRenderResources,
    grass: Option<&vegetation::VegetationCatalog>,
) -> clipmap::ClipmapGround {
    let layers: Vec<_> = resources
        .surfaces
        .iter()
        .map(|s| terrain_render::TerrainSurfaceLayer {
            surface: s.surface.clone(),
            layer: s.layer,
            grass: grass.map_or([0.0; 4], |g| g.cover(s.surface.id.0).rgba()),
        })
        .collect();

    for l in &layers {
        info!(
            "terrain v2 surface {:>2} {:<18} grass cover {:.2}{}",
            l.layer,
            l.surface.key,
            l.grass[3],
            if l.grass[3] >= clipmap::COVERED_GROUND {
                " (field colour, no textures)"
            } else {
                ""
            }
        );
    }

    clipmap::ClipmapGround::new(
        asset_server,
        &resources.profile,
        &resources.texture_set,
        &layers,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update(
    mut commands: Commands,
    mut terrain: ResMut<ClipmapTerrain>,
    mut uploads: ResMut<ClipmapUploads>,
    worker: Option<Res<WorldDatabaseWorker>>,
    catalog: Res<WorldCatalog>,
    active_space: Res<ActiveWorldSpace>,
    origin: Res<WorldOrigin>,
    time: Res<Time>,
    camera: Query<(&Camera, &GlobalTransform), With<WorldViewCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ClipmapTerrainMaterial>>,
    (mut buffers, asset_server): (
        ResMut<Assets<bevy::render::storage::ShaderBuffer>>,
        Res<AssetServer>,
    ),
    sources: Query<(Entity, Ref<super::StreamedTerrainWear>)>,
) {
    let (Some(worker), Some(space)) = (worker, active_space.current()) else {
        return;
    };
    let Some(info) = catalog.world_space(space) else {
        return;
    };
    let terrain = terrain.as_mut();
    let now = time.elapsed_secs_f64();
    let identity = (catalog.generation_id().to_owned(), space);
    terrain.follow_publication(identity, info.cell_size, now, &mut commands, &mut uploads);
    terrain.request_metadata(&worker, space);

    if terrain.failure.is_some() {
        terrain.failed_at.get_or_insert(now);
        return;
    }

    let (Some(layout), Some(ranges)) = (terrain.layout, terrain.ranges) else {
        return;
    };

    if terrain.gpu.is_none() {
        let Some(resources) = &terrain.surfaces else {
            return;
        };
        let ground = ground(&asset_server, resources, catalog.vegetation());
        let textures = ClipmapTextures::new(
            layout,
            ranges,
            ground,
            &mut images,
            &mut buffers,
            &mut materials,
        );
        uploads.target(&textures);
        terrain.gpu = Some(textures);
        terrain.patches.mesh = Some(meshes.add(clipmap::patch_mesh(&layout)));
    }

    let Some((_, camera)) = camera.iter().find(|(c, _)| c.is_active) else {
        return;
    };
    let render_eye = camera.translation();
    let offset = origin.cell().origin(info.cell_size);
    let eye = DVec3::new(
        f64::from(render_eye.x) + offset[0],
        f64::from(render_eye.y),
        f64::from(render_eye.z) + offset[1],
    );
    terrain.move_windows(&layout, [eye.x, eye.z]);
    terrain.take_decoded(&layout, &mut uploads);
    terrain.request_nodes(&worker, &layout, &ranges, eye);
    terrain.draw_patches(&mut commands, &layout, &ranges, eye, origin.cell(), offset);
    terrain.stats.uploads = uploads.uploaded_blocks;
    let Some(textures) = terrain.gpu.as_ref() else {
        return;
    };
    follow_wear(
        &mut terrain.wear,
        textures,
        &mut uploads,
        &sources,
        space,
        eye,
        info.cell_size,
    );
    let bare = std::mem::take(&mut terrain.wear.bare);

    if let Some(gpu) = terrain.gpu.as_mut() {
        gpu.mark_bare_ground(bare, &mut buffers);
    }

    let cells = origin.cell();
    let samples = |c: i32| i64::from(c) * i64::from(layout.intervals);
    let wear = terrain.wear.slots.map(|slot| slot.map(|(cell, _)| cell));
    uploads.set_frame(
        terrain.gpu.as_ref().unwrap(),
        &ClipmapFrame {
            eye: render_eye,
            origin_samples: [samples(cells.x), samples(cells.z)],
            windows: &terrain.windows,
            residency: &terrain.residency,
            wear: &wear,
        },
    );

    if !terrain.ready_logged && !terrain.pending() {
        terrain.ready_logged = true;
        let seconds = terrain.started.map_or(0.0, |s| now - s);
        info!(
            "TERRAIN_V2_READY {seconds:.2} s: {} nodes resident, {} patches, {} triangles",
            terrain.stats.resident, terrain.stats.patches, terrain.stats.triangles
        );
    }
}

impl ClipmapTerrain {
    /// Another space or cell size, a new layout or a failure some seconds old start over; a
    /// new publication of the same space is read again in place.
    fn follow_publication(
        &mut self,
        identity: (String, WorldSpaceId),
        cell_size: f32,
        now: f64,
        commands: &mut Commands,
        uploads: &mut ClipmapUploads,
    ) {
        let changed = self.identity.as_ref() != Some(&identity);
        let retry = self.failed_at.is_some_and(|t| now - t >= RETRY_SECONDS);

        if self.rebuild
            || retry
            || changed
                && (self
                    .identity
                    .as_ref()
                    .is_some_and(|(_, s)| *s != identity.1)
                    || self.cell_size != cell_size
                    || self.failure.is_some()
                    || self.gpu.is_none())
        {
            self.clear(commands, uploads);
        }

        if self.identity.as_ref() != Some(&identity) {
            if self.identity.is_some() {
                self.republish();
            }

            self.identity = Some(identity);
            self.cell_size = cell_size;
            self.started = Some(now);
            self.wanted_roots = true;
            self.wanted_surfaces = true;
        }
    }

    fn request_metadata(&mut self, worker: &WorldDatabaseWorker, space: WorldSpaceId) {
        if self.wanted_roots && self.request(worker, TerrainQuery::Roots(space)) {
            self.wanted_roots = false;
        }

        if self.wanted_surfaces && self.request(worker, TerrainQuery::Surfaces(space)) {
            self.wanted_surfaces = false;
        }
    }

    /// Decoded nodes join the clipmap; received batches start decoding.
    fn take_decoded(&mut self, layout: &ClipmapLayout, uploads: &mut ClipmapUploads) {
        let mut finished = Vec::new();
        self.decoding
            .retain_mut(|task| match block_on(poll_once(task)) {
                Some(result) => {
                    finished.push(result);
                    false
                }
                None => true,
            });

        for result in finished {
            match result {
                Ok(nodes) => {
                    for node in nodes {
                        self.accept(layout, uploads, node);
                    }
                }
                Err(e) => self.fail(e),
            }
        }

        let intervals = layout.intervals;

        for batch in std::mem::take(&mut self.received) {
            self.decoding
                .push(AsyncComputeTaskPool::get().spawn(async move { decode(batch, intervals) }));
        }
    }

    /// Requests what the view needs, nearest (relative to node size) first, then stale nodes
    /// it does not need but may still draw or sample.
    fn request_nodes(
        &mut self,
        worker: &WorldDatabaseWorker,
        layout: &ClipmapLayout,
        ranges: &LodRanges,
        eye: DVec3,
    ) {
        let demand = self.demand(layout, ranges, eye);
        let mut missing: Vec<_> = demand
            .iter()
            .copied()
            .filter(|k| !self.resident.bounds.contains_key(k) || self.stale.contains(k))
            .collect();

        if !self.stale.is_empty() {
            let demanded: HashSet<_> = demand.iter().collect();
            missing.extend(self.stale.iter().filter(|k| !demanded.contains(k)));
        }

        self.stats.demand = demand.len();
        self.stats.missing = missing.len();
        let wanted: Vec<_> = missing
            .into_iter()
            .filter(|k| !self.requested.contains(k))
            .collect();

        for batch in wanted.chunks(BATCH) {
            if self.requests.len() >= IN_FLIGHT
                || !self.request(worker, TerrainQuery::Nodes(batch.to_vec()))
            {
                break;
            }
            self.requested.extend(batch.iter().copied());
        }
    }

    /// One patch entity per selected node, placed for the render origin.
    fn draw_patches(
        &mut self,
        commands: &mut Commands,
        layout: &ClipmapLayout,
        ranges: &LodRanges,
        eye: DVec3,
        origin: world::CellCoord,
        offset: [f64; 2],
    ) {
        let patches = clipmap::select(
            layout,
            ranges,
            &self.windows,
            &self.starts,
            &self.resident,
            eye,
        );
        let selected: HashSet<_> = patches.iter().map(|p| p.key).collect();
        let drawn = &mut self.patches.drawn;

        drawn.retain(|key, (entity, _)| {
            let keep = selected.contains(key);

            if !keep {
                commands.entity(*entity).despawn();
            }

            keep
        });

        if self.patches.placed != Some(origin) {
            self.patches.placed = Some(origin);

            for (&key, &(entity, _)) in drawn.iter() {
                let transform = clipmap::patch_transform(layout, key, offset);
                commands
                    .entity(entity)
                    .insert((transform, GlobalTransform::from(transform)));
            }
        }

        let (mesh, material) = (
            self.patches.mesh.clone().unwrap(),
            self.gpu.as_ref().unwrap().material.clone(),
        );
        let n = layout.intervals as f32;
        let aabb = |bounds: [f32; 2]| {
            Aabb::from_min_max(Vec3::new(0.0, bounds[0], 0.0), Vec3::new(n, bounds[1], n))
        };

        for patch in &patches {
            // An edit or a new publication may have moved a drawn patch's bounds.
            if let Some((entity, bounds)) = drawn.get_mut(&patch.key) {
                if *bounds != patch.bounds {
                    *bounds = patch.bounds;
                    commands.entity(*entity).insert(aabb(patch.bounds));
                }
                continue;
            }

            let transform = clipmap::patch_transform(layout, patch.key, offset);
            let entity = commands
                .spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    MeshTag::new(clipmap::patch_tag(patch.key)),
                    transform,
                    GlobalTransform::from(transform),
                    aabb(patch.bounds),
                    NoAutoAabb,
                ))
                .id();
            drawn.insert(patch.key, (entity, patch.bounds));
        }

        self.stats.resident = self.resident.bounds.len();
        self.stats.patches = patches.len();
        self.stats.triangles = patches.len() * (layout.intervals * layout.intervals * 2) as usize;
    }
}

#[cfg(test)]
impl ClipmapTerrain {
    /// One resident flat leaf at `height`, for grounding tests.
    pub(crate) fn test_flat(
        space: WorldSpaceId,
        cell: world::CellCoord,
        cell_size: f32,
        height: f32,
    ) -> Self {
        let field =
            TerrainHeightfield::from_heights(2, &[height; 4], height, height, cell_size).unwrap();
        let key = TerrainNodeKey::leaf(space, cell);
        let mut terrain = Self {
            space: Some(space),
            cell_size,
            ..default()
        };
        terrain.resident.bounds.insert(key, [height, height]);
        terrain.resident.leaves.insert(key, field);
        terrain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(space: WorldSpaceId, x: i32) -> TerrainNodeDescriptor {
        TerrainNodeDescriptor {
            key: TerrainNodeKey {
                space,
                level: 5,
                x,
                z: 0,
            },
            child_mask: 0xf,
            height_bounds: [0.0, 10.0],
            resolution: Some(NODE_SAMPLES),
            decoded_bytes: 0,
            checksum: [0; 32],
        }
    }

    fn flat(key: TerrainNodeKey, height: f32) -> TerrainNode {
        let n = usize::from(NODE_SAMPLES);
        let field = TerrainHeightfield::from_heights(
            NODE_SAMPLES,
            &vec![height; n * n],
            height - 1.0,
            height + 1.0,
            32.0,
        )
        .unwrap();
        TerrainNode::leaf(key, &field, NODE_SAMPLES).unwrap()
    }

    /// An overlay node the clipmap cannot draw refuses the overlay and leaves the ground and
    /// the stream as they were.
    #[test]
    fn an_undrawable_overlay_is_refused_whole() {
        let space = WorldSpaceId(1);
        let mut terrain = ClipmapTerrain {
            cell_size: 32.0,
            identity: Some(("a".into(), space)),
            ..default()
        };
        terrain.adopt_roots(&[root(space, 0)]);
        let layout = terrain.layout.unwrap();
        terrain.move_windows(&layout, [16.0, 16.0]);
        let mut uploads = ClipmapUploads::default();
        let key = TerrainNodeKey::leaf(space, world::CellCoord::ZERO);
        terrain.accept(
            &layout,
            &mut uploads,
            relief(&flat(key, 1.0), layout.intervals).unwrap(),
        );
        let coarse = TerrainHeightfield::from_heights(17, &[3.0; 17 * 17], 2.0, 4.0, 32.0).unwrap();
        let products = Arc::new(TerrainPreviewProducts {
            nodes: [(key, Arc::new(TerrainNode::leaf(key, &coarse, 17).unwrap()))].into(),
            ..default()
        });
        assert!(terrain.set_overlay(products, &mut uploads).is_err());
        assert!(terrain.overlay.is_none() && terrain.failure.is_none());
        assert_eq!(
            terrain.ground_height(Vec3::new(16.0, 0.0, 16.0), &WorldOrigin::default()),
            Some(1.0)
        );
    }

    /// GPU textures for `layout`, as assets only: what uploads are queued for.
    fn textures(layout: ClipmapLayout, uploads: &mut ClipmapUploads) -> ClipmapTextures {
        let ground = clipmap::ClipmapGround {
            surfaces: Vec::new(),
            base_color: default(),
            normal_material: default(),
            macro_variation: default(),
            macro_scales: [1.0; 3],
            macro_contrast: 1.0,
            macro_albedo_strength: 0.0,
        };
        let textures = ClipmapTextures::new(
            layout,
            LodRanges { leaf: LEAF_RANGE },
            ground,
            &mut Assets::default(),
            &mut Assets::default(),
            &mut Assets::default(),
        );
        uploads.target(&textures);
        textures
    }

    /// The editor's overlay rewrites resident blocks at once, nodes read later take its parts,
    /// and a node it stops replacing is read again. A new publication keeps the ground drawn.
    #[test]
    fn overlay_and_republish_keep_resident_ground() {
        let space = WorldSpaceId(1);
        let mut terrain = ClipmapTerrain {
            cell_size: 32.0,
            identity: Some(("a".into(), space)),
            ..default()
        };
        terrain.adopt_roots(&[root(space, 0)]);
        let layout = terrain.layout.unwrap();
        terrain.move_windows(&layout, [16.0, 16.0]);
        let mut uploads = ClipmapUploads::default();
        terrain.gpu = Some(textures(layout, &mut uploads));
        let key = TerrainNodeKey::leaf(space, world::CellCoord::ZERO);
        let published = || relief(&flat(key, 1.0), layout.intervals).unwrap();
        let ground = |t: &ClipmapTerrain| {
            t.ground_height(Vec3::new(16.0, 0.0, 16.0), &WorldOrigin::default())
        };
        terrain.accept(&layout, &mut uploads, published());
        assert_eq!(ground(&terrain), Some(1.0));

        let raised = Arc::new(TerrainPreviewProducts {
            nodes: [(key, Arc::new(flat(key, 3.0)))].into(),
            controls: [(
                key,
                Arc::new(vec![7; world::CONTROL_SAMPLES * world::CONTROL_SAMPLES]),
            )]
            .into(),
            ..default()
        });
        let before = uploads.uploaded_blocks;
        terrain.set_overlay(raised.clone(), &mut uploads).unwrap();
        assert_eq!(uploads.uploaded_blocks, before + 1);
        assert_eq!(ground(&terrain), Some(3.0));
        assert_eq!(
            terrain.resident.bounds[&key],
            raised.nodes[&key].height_bounds
        );
        terrain.set_overlay(raised.clone(), &mut uploads).unwrap();
        assert_eq!(uploads.uploaded_blocks, before + 1, "unchanged overlay");
        terrain.accept(&layout, &mut uploads, published());
        assert_eq!(
            ground(&terrain),
            Some(3.0),
            "a later read takes the overlay"
        );

        // Left to the publication without an explicit baseline: read again.
        terrain.set_overlay(Arc::default(), &mut uploads).unwrap();
        assert!(terrain.stale.contains(&key));
        terrain.accept(&layout, &mut uploads, published());
        assert!(terrain.stale.is_empty());
        assert_eq!(ground(&terrain), Some(1.0));

        // A new publication: drawn until read again, the old one's replies dropped quietly.
        terrain.requests.insert(9, TerrainQuery::Roots(space));
        terrain.overlay = Some(raised);
        terrain.republish();
        assert!(terrain.stale.contains(&key) && terrain.overlay.is_none());
        assert!(terrain.owns_request(9));
        terrain.receive(
            9,
            Err("terrain request belongs to a stale generation".into()),
        );
        assert!(terrain.failure.is_none() && !terrain.owns_request(9));
        assert_eq!(ground(&terrain), Some(1.0));
        terrain.adopt_roots(&[root(space, 0)]);
        assert!(!terrain.rebuild, "the same extent keeps the clipmap");

        // A node the new publication lacks leaves the clipmap.
        terrain.requests.insert(10, TerrainQuery::Nodes(vec![key]));
        terrain.receive(10, Ok(TerrainReply::Nodes(vec![])));
        assert_eq!(ground(&terrain), None);
        terrain.adopt_roots(&[root(space, 0), root(space, 1)]);
        assert!(terrain.rebuild, "a new extent loads from scratch");
    }
}
