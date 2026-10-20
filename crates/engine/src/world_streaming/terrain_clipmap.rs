//! Terrain v2 (the terrain rendering roadmap in docs/REFACTORING.md): the ground drawn from GPU
//! height clipmaps with CDLOD patches (`terrain_render::clipmap`).
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
        DatabaseRequest, NotSent, RequestId, TerrainQuery, TerrainReply, WorldDatabaseWorker,
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
/// Nodes per database request, and requests in flight.
const BATCH: usize = 48;
const IN_FLIGHT: usize = 2;
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

/// Counts for the log and for comparing against the terrain LOD.
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
    pending: HashMap<RequestId, TerrainQuery>,
    /// Requests of a previous publication, whose replies are dropped.
    dropped: HashSet<RequestId>,
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
    mesh: Option<Handle<Mesh>>,
    /// Each drawn patch and the height bounds its box was made from.
    patches: HashMap<TerrainNodeKey, (Entity, [f32; 2])>,
    /// The cell and page whose road wear each wear atlas slot holds.
    wear: [Option<(world::CellCoord, Entity)>; clipmap::WEAR_SLOTS],
    /// Layers newly found worn bare to, for the surface table.
    bare: Vec<u16>,
    placed: Option<world::CellCoord>,
    failure: Option<String>,
    started: Option<f64>,
    ready_logged: bool,
    pub stats: ClipmapStats,
}

impl ClipmapTerrain {
    pub(super) fn owns_request(&self, id: RequestId) -> bool {
        self.pending.contains_key(&id) || self.dropped.contains(&id)
    }

    pub(super) fn receive(&mut self, id: RequestId, reply: Result<TerrainReply, String>) {
        if self.dropped.remove(&id) {
            return;
        }
        let Some(query) = self.pending.remove(&id) else {
            return;
        };
        match (query, reply) {
            (TerrainQuery::Roots(_), Ok(TerrainReply::Metadata(roots))) => self.adopt_roots(&roots),
            (TerrainQuery::Surfaces(_), Ok(TerrainReply::Surfaces(resources))) => {
                if self.gpu.is_some() && self.surfaces.as_ref() != Some(&resources) {
                    self.rebuild = true;
                }
                self.surfaces = Some(resources);
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
        let starts: Vec<_> = roots
            .iter()
            .flat_map(|root| {
                let span = 1_i32 << (root.key.level - top);
                let key = root.key;
                (0..span * span).map(move |i| TerrainNodeKey {
                    level: top,
                    x: key.x * span + i % span,
                    z: key.z * span + i / span,
                    ..key
                })
            })
            .collect();
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
        match worker.send(DatabaseRequest::Terrain {
            generation: generation.clone(),
            query: query.clone(),
        }) {
            Ok(id) => {
                self.pending.insert(id, query);
                true
            }
            Err(NotSent::Full) => false,
            Err(NotSent::Stopped) => {
                self.fail("the world database worker stopped".into());
                false
            }
        }
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
        uploads.push(node.block);
    }

    fn clear(&mut self, commands: &mut Commands, uploads: &mut ClipmapUploads) {
        for (_, (entity, _)) in self.patches.drain() {
            commands.entity(entity).despawn();
        }
        uploads.clear();
        *self = Self::default();
    }

    /// A new publication of the drawn space: what is resident stays drawn until its node is
    /// read again from it. The editor's overlay belonged to the previous one.
    fn republish(&mut self) {
        self.dropped.extend(self.pending.drain().map(|(id, _)| id));
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
    let field = node
        .heightfield
        .as_ref()
        .ok_or_else(|| format!("terrain node {:?} holds no heights", node.key))?;
    if u32::from(field.resolution) != intervals + 1 {
        return Err(format!(
            "terrain node {:?} is not {NODE_SAMPLES}²",
            node.key
        ));
    }
    let n = usize::from(field.resolution);
    let normals: Vec<_> = (0..n * n).map(|i| field.normal_at(i % n, i / n)).collect();
    Ok(Decoded {
        key: node.key,
        bounds: node.height_bounds,
        block: NodeBlock::new(node.key).with_relief(field.heights.clone(), &normals),
        leaf: (node.key.level == 0).then(|| field.clone()),
    })
}

/// Road wear around the camera: each cell of the wear window whose page has a wear field holds
/// it in its toroidal atlas slot, uploaded when the slot's page changes.
fn follow_wear(
    terrain: &mut ClipmapTerrain,
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
        if source.key.space != space || source.key.lod != 0 || !window(source.key.cell) {
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
    let n = clipmap::WEAR_WINDOW;
    for dz in -half..=half {
        for dx in -half..=half {
            let cell = world::CellCoord {
                x: at.x + dx,
                z: at.z + dz,
            };
            let slot = (cell.x.rem_euclid(n) + n * cell.z.rem_euclid(n)) as usize;
            match pages.get(&cell) {
                Some(&(entity, changed, wear, source)) => {
                    if changed || terrain.wear[slot] != Some((cell, entity)) {
                        uploads.push_wear(slot, wear);
                        terrain.wear[slot] = Some((cell, entity));
                        // The page's surfaces its roads wear bare to.
                        terrain.bare.extend(wear.worn.iter().flatten().filter_map(
                            |&(to, share)| {
                                (to != world::NO_WORN_SLOT && share > 0)
                                    .then(|| source.layers.get(usize::from(to)).copied())
                                    .flatten()
                            },
                        ));
                    }
                }
                None => terrain.wear[slot] = None,
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

/// Mesh tag of a patch: its level and node, as `terrain_clipmap_vertex.wesl` unpacks them.
fn patch_tag(key: TerrainNodeKey) -> u32 {
    debug_assert!((-8192..8192).contains(&key.x) && (-8192..8192).contains(&key.z));
    (u32::from(key.level) << 28) | ((key.x as u32 & 0x3fff) << 14) | (key.z as u32 & 0x3fff)
}

fn patch_transform(layout: &ClipmapLayout, key: TerrainNodeKey, origin: [f64; 2]) -> Transform {
    let size = layout.node_metres(key.level);
    let spacing = layout.spacing(key.level) as f32;
    Transform::from_xyz(
        (f64::from(key.x) * size - origin[0]) as f32,
        0.0,
        (f64::from(key.z) * size - origin[1]) as f32,
    )
    .with_scale(Vec3::new(spacing, 1.0, spacing))
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
    let identity = (catalog.generation_id().to_owned(), space);
    let changed = terrain.identity.as_ref() != Some(&identity);
    // Another space or cell size, a failed load or a new layout start over; a new publication
    // of the same space is read again in place.
    if terrain.rebuild
        || changed
            && (terrain.identity.as_ref().is_some_and(|(_, s)| *s != space)
                || terrain.cell_size != info.cell_size
                || terrain.failure.is_some()
                || terrain.gpu.is_none())
    {
        terrain.clear(&mut commands, &mut uploads);
    }
    if terrain.identity.as_ref() != Some(&identity) {
        if terrain.identity.is_some() {
            terrain.republish();
        }
        terrain.identity = Some(identity);
        terrain.cell_size = info.cell_size;
        terrain.started = Some(time.elapsed_secs_f64());
        terrain.wanted_roots = true;
        terrain.wanted_surfaces = true;
    }
    if terrain.wanted_roots && terrain.request(&worker, TerrainQuery::Roots(space)) {
        terrain.wanted_roots = false;
    }
    if terrain.wanted_surfaces && terrain.request(&worker, TerrainQuery::Surfaces(space)) {
        terrain.wanted_surfaces = false;
    }
    if terrain.failure.is_some() {
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
        terrain.mesh = Some(meshes.add(clipmap::patch_mesh(&layout)));
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

    // Decoded nodes join the clipmap; received batches start decoding.
    let mut finished = Vec::new();
    terrain
        .decoding
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
                    terrain.accept(&layout, &mut uploads, node);
                }
            }
            Err(e) => terrain.fail(e),
        }
    }
    let intervals = layout.intervals;
    for batch in std::mem::take(&mut terrain.received) {
        terrain
            .decoding
            .push(AsyncComputeTaskPool::get().spawn(async move { decode(batch, intervals) }));
    }

    // Request what the view needs, nearest (relative to node size) first, then stale nodes it
    // does not need but may still draw or sample.
    let demand = terrain.demand(&layout, &ranges, eye);
    let mut missing: Vec<_> = demand
        .iter()
        .copied()
        .filter(|k| !terrain.resident.bounds.contains_key(k) || terrain.stale.contains(k))
        .collect();
    if !terrain.stale.is_empty() {
        let demanded: HashSet<_> = demand.iter().collect();
        missing.extend(terrain.stale.iter().filter(|k| !demanded.contains(k)));
    }
    terrain.stats.demand = demand.len();
    terrain.stats.missing = missing.len();
    let wanted: Vec<_> = missing
        .into_iter()
        .filter(|k| !terrain.requested.contains(k))
        .collect();
    for batch in wanted.chunks(BATCH) {
        if terrain.pending.len() >= IN_FLIGHT
            || !terrain.request(&worker, TerrainQuery::Nodes(batch.to_vec()))
        {
            break;
        }
        terrain.requested.extend(batch.iter().copied());
    }

    // Patches: one entity per selected node.
    let patches = clipmap::select(
        &layout,
        &ranges,
        &terrain.windows,
        &terrain.starts,
        &terrain.resident,
        eye,
    );
    let selected: HashMap<_, _> = patches.iter().map(|p| (p.key, p.bounds)).collect();
    let mut unselected = Vec::new();
    for (key, (entity, _)) in &terrain.patches {
        if !selected.contains_key(key) {
            unselected.push(*key);
            commands.entity(*entity).despawn();
        }
    }
    for key in unselected {
        terrain.patches.remove(&key);
    }
    let replace = terrain.placed != Some(origin.cell());
    if replace {
        terrain.placed = Some(origin.cell());
        for (&key, &(entity, _)) in &terrain.patches {
            let transform = patch_transform(&layout, key, offset);
            commands
                .entity(entity)
                .insert((transform, GlobalTransform::from(transform)));
        }
    }
    let (mesh, material) = (
        terrain.mesh.clone().unwrap(),
        terrain.gpu.as_ref().unwrap().material.clone(),
    );
    let n = layout.intervals as f32;
    let aabb = |bounds: [f32; 2]| {
        Aabb::from_min_max(Vec3::new(0.0, bounds[0], 0.0), Vec3::new(n, bounds[1], n))
    };
    for patch in &patches {
        // An edit or a new publication may have moved a drawn patch's bounds.
        if let Some((entity, bounds)) = terrain.patches.get_mut(&patch.key) {
            if *bounds != patch.bounds {
                *bounds = patch.bounds;
                commands.entity(*entity).insert(aabb(patch.bounds));
            }
            continue;
        }
        let transform = patch_transform(&layout, patch.key, offset);
        let entity = commands
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                MeshTag::new(patch_tag(patch.key)),
                transform,
                GlobalTransform::from(transform),
                aabb(patch.bounds),
                NoAutoAabb,
            ))
            .id();
        terrain.patches.insert(patch.key, (entity, patch.bounds));
    }

    let resident = terrain.resident.bounds.len();
    terrain.stats.resident = resident;
    terrain.stats.patches = patches.len();
    terrain.stats.triangles = patches.len() * (layout.intervals * layout.intervals * 2) as usize;
    terrain.stats.uploads = uploads.uploaded_blocks;
    follow_wear(
        &mut terrain,
        &mut uploads,
        &sources,
        space,
        eye,
        info.cell_size,
    );
    let bare = std::mem::take(&mut terrain.bare);
    if let Some(gpu) = terrain.gpu.as_mut() {
        gpu.mark_bare_ground(bare, &mut buffers);
    }
    let cells = origin.cell();
    let samples = |c: i32| i64::from(c) * i64::from(layout.intervals);
    let wear = terrain.wear.map(|slot| slot.map(|(cell, _)| cell));
    uploads.set_frame(&ClipmapFrame {
        eye: render_eye,
        origin_samples: [samples(cells.x), samples(cells.z)],
        windows: &terrain.windows,
        residency: &terrain.residency,
        wear: &wear,
    });
    if !terrain.ready_logged && terrain.stats.missing == 0 && terrain.decoding.is_empty() {
        terrain.ready_logged = true;
        let seconds = terrain.started.map_or(0.0, |s| time.elapsed_secs_f64() - s);
        info!(
            "TERRAIN_V2_READY {seconds:.2} s: {} nodes resident, {} patches, {} triangles",
            resident, terrain.stats.patches, terrain.stats.triangles
        );
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

    #[test]
    fn patch_tags_carry_negative_nodes() {
        let key = TerrainNodeKey {
            space: WorldSpaceId(1),
            level: 5,
            x: -5,
            z: 4,
        };
        let tag = patch_tag(key);
        assert_eq!(tag >> 28, 5);
        assert_eq!(((tag << 4) as i32) >> 18, -5);
        assert_eq!(((tag << 18) as i32) >> 18, 4);
    }

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
            geometric_error: 0.0,
            resolution: Some(NODE_SAMPLES),
            decoded_bytes: 0,
            gpu_bytes_estimate: 0,
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
        terrain.set_overlay(raised.clone(), &mut uploads);
        assert_eq!(uploads.uploaded_blocks, before + 1);
        assert_eq!(ground(&terrain), Some(3.0));
        assert_eq!(
            terrain.resident.bounds[&key],
            raised.nodes[&key].height_bounds
        );
        terrain.set_overlay(raised.clone(), &mut uploads);
        assert_eq!(uploads.uploaded_blocks, before + 1, "unchanged overlay");
        terrain.accept(&layout, &mut uploads, published());
        assert_eq!(
            ground(&terrain),
            Some(3.0),
            "a later read takes the overlay"
        );

        // Left to the publication without an explicit baseline: read again.
        terrain.set_overlay(Arc::default(), &mut uploads);
        assert!(terrain.stale.contains(&key));
        terrain.accept(&layout, &mut uploads, published());
        assert!(terrain.stale.is_empty());
        assert_eq!(ground(&terrain), Some(1.0));

        // A new publication: drawn until read again, the old one's replies dropped quietly.
        terrain.pending.insert(9, TerrainQuery::Roots(space));
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
        terrain.pending.insert(10, TerrainQuery::Nodes(vec![key]));
        terrain.receive(10, Ok(TerrainReply::Nodes(vec![])));
        assert_eq!(ground(&terrain), None);
        terrain.adopt_roots(&[root(space, 0), root(space, 1)]);
        assert!(terrain.rebuild, "a new extent loads from scratch");
    }
}
